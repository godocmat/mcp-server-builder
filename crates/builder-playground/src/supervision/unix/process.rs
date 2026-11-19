use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

use tokio::net::unix::pipe;
use tokio::time::{Instant, sleep_until};

use super::paths::{Directory, Executable};
use super::{ExecStrings, Input, Output};
use crate::supervision::LaunchError;

const GRACE: Duration = Duration::from_secs(2);
const LIMIT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

pub(in crate::supervision) struct NativeChild {
    process: ProcessGroup,
    stdin: Option<Input>,
    stdout: Option<Output>,
    stderr: Option<Output>,
}

impl NativeChild {
    pub(crate) fn id(&self) -> u32 {
        self.process.pid.expect("admitted worker").cast_unsigned()
    }

    pub(crate) fn take_stdin(&mut self) -> Option<Input> {
        self.stdin.take()
    }

    pub(crate) fn take_stdout(&mut self) -> Option<Output> {
        self.stdout.take()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<Output> {
        self.stderr.take()
    }

    pub(crate) async fn stop(mut self) -> Result<(), LaunchError> {
        self.stdin.take();
        let started = Instant::now();
        let grace = started + GRACE;
        let deadline = started + LIMIT;
        while !self.process.exited().map_err(stop_failed)? && Instant::now() < grace {
            sleep_until(grace.min(Instant::now() + POLL_INTERVAL)).await;
        }

        // Signal while the worker PID is still reserved, even if it closed stdin
        // and exited. Descendants still in its process group receive the signal.
        self.process.kill().map_err(stop_failed)?;
        loop {
            if self.process.reap().map_err(stop_failed)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(LaunchError::StopFailed);
            }
            sleep_until(deadline.min(Instant::now() + POLL_INTERVAL)).await;
        }
    }
}

fn stop_failed(_: io::Error) -> LaunchError {
    LaunchError::StopFailed
}

struct ChildPipes {
    stdin: OwnedFd,
    stdout: OwnedFd,
    stderr: OwnedFd,
}

pub(super) fn launch(
    executable: Executable,
    cwd: Directory,
    arguments: ExecStrings,
    environment: ExecStrings,
) -> io::Result<NativeChild> {
    let (child_stdin, stdin) = io::pipe()?;
    let (stdout, child_stdout) = io::pipe()?;
    let (stderr, child_stderr) = io::pipe()?;
    let pipes = ChildPipes {
        stdin: above_stdio(child_stdin.into())?,
        stdout: above_stdio(child_stdout.into())?,
        stderr: above_stdio(child_stderr.into())?,
    };
    // Register only the parent endpoints as asynchronous. The child can use
    // ordinary blocking stdin/stdout, with no inheritance of other host FDs.
    let stdin = pipe::Sender::from_owned_fd(stdin.into())?;
    let stdout = pipe::Receiver::from_owned_fd(stdout.into())?;
    let stderr = pipe::Receiver::from_owned_fd(stderr.into())?;
    executable.confirm_descriptor()?;
    let process = platform::spawn(executable, cwd, arguments, environment, pipes)?;
    Ok(NativeChild {
        process,
        stdin: Some(stdin),
        stdout: Some(stdout),
        stderr: Some(stderr),
    })
}

pub(super) fn above_stdio(fd: OwnedFd) -> io::Result<OwnedFd> {
    if fd.as_raw_fd() > libc::STDERR_FILENO {
        return Ok(fd);
    }
    // SAFETY: This duplicates the live descriptor to a fresh number outside stdio.
    let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: F_DUPFD_CLOEXEC returned a new uniquely owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

// Keep the group leader unreaped until sending any final group signal. If a
// child exits first, its reserved PID still protects us from group ID reuse.
struct ProcessGroup {
    pid: Option<libc::pid_t>,
}

impl ProcessGroup {
    fn new(pid: libc::pid_t) -> Self {
        debug_assert!(pid > 0);
        Self { pid: Some(pid) }
    }

    fn exited(&mut self) -> io::Result<bool> {
        let Some(pid) = self.pid else { return Ok(true) };
        // SAFETY: Zero is the required empty result for waitid with WNOHANG.
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        let result = retry(|| {
            // SAFETY: We retain this unreaped child PID and supply writable storage.
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid.cast_unsigned(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            }
        });
        self.disown_if_reaped(&result);
        result?;
        // SAFETY: An exit record or the zero initialized empty result is present.
        Ok(unsafe { info.si_pid() } == pid)
    }

    fn kill(&self) -> io::Result<()> {
        let Some(pid) = self.pid else { return Ok(()) };
        // SAFETY: Spawn created a private group and its leader is still ours to reap.
        if unsafe { libc::killpg(pid, libc::SIGKILL) } != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }

    fn reap(&mut self) -> io::Result<bool> {
        let Some(pid) = self.pid else { return Ok(true) };
        // SAFETY: This waits only for our child and discards the status.
        let result = retry(|| unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) });
        self.disown_if_reaped(&result);
        if result? == pid {
            self.pid = None;
        }
        Ok(self.pid.is_none())
    }

    fn disown_if_reaped(&mut self, result: &io::Result<libc::c_int>) {
        if result
            .as_ref()
            .is_err_and(|error| error.raw_os_error() == Some(libc::ECHILD))
        {
            self.pid = None;
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // Check ownership before signaling even on task cancellation.
        let _ = self.exited();
        let _ = self.kill();
        let _ = self.reap();
        if let Some(pid) = self.pid.take() {
            // Keep blocking waits out of GUI/async threads. This thread never
            // signals after ownership has been released, so it cannot kill reuse.
            let _ = std::thread::Builder::new()
                .name("mcp-preview-reap".into())
                .spawn(move || {
                    // SAFETY: Sole remaining owner waits for this particular child.
                    let _ = retry(|| unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) });
                });
        }
    }
}

fn retry(mut call: impl FnMut() -> libc::c_int) -> io::Result<libc::c_int> {
    loop {
        let result = call();
        if result >= 0 {
            return Ok(result);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use super::*;

    pub(super) fn spawn(
        executable: Executable,
        cwd: Directory,
        arguments: ExecStrings,
        environment: ExecStrings,
        pipes: ChildPipes,
    ) -> io::Result<ProcessGroup> {
        let mut child = Command::new(executable.path());
        child
            .process_group(0)
            .env_clear()
            .stdin(Stdio::from(pipes.stdin))
            .stdout(Stdio::from(pipes.stdout))
            .stderr(Stdio::from(pipes.stderr));
        // SAFETY: The closure runs after fork. It only accesses preallocated
        // data and calls async-signal-safe system operations before exec.
        unsafe {
            child.pre_exec(move || {
                if libc::fchdir(cwd.fd()) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::syscall(
                    libc::SYS_close_range,
                    3u32,
                    u32::MAX,
                    libc::CLOSE_RANGE_CLOEXEC,
                ) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                // Execute the reviewed open descriptor itself. CLOEXEC leaves
                // this descriptor usable during execveat but closes it on entry.
                libc::syscall(
                    libc::SYS_execveat,
                    executable.fd(),
                    c"".as_ptr(),
                    arguments.as_ptr(),
                    environment.as_ptr(),
                    libc::AT_EMPTY_PATH,
                );
                Err(io::Error::last_os_error())
            });
        }
        // std::process::Child does not implicitly wait on Drop. The supervisor
        // owns the PID until it sends the final signal and explicitly reaps it.
        let pid = child.spawn()?.id().cast_signed();
        Ok(ProcessGroup::new(pid))
    }
}

#[cfg(target_os = "macos")]
mod platform;
