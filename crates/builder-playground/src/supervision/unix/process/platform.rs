use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null_mut;

use super::*;
use crate::supervision::unix::{c_string, invalid_data};

pub(super) fn spawn(
    executable: Executable,
    cwd: Directory,
    arguments: ExecStrings,
    environment: ExecStrings,
    pipes: ChildPipes,
) -> io::Result<ProcessGroup> {
    let path = c_string(executable.path().as_os_str())?;
    let mut actions = FileActions::new()?;
    let mut attributes = Attributes::new()?;
    // SAFETY: Actions and attributes are initialized, and all source
    // descriptors remain open until posix_spawn has applied the actions.
    unsafe {
        status(libc::posix_spawn_file_actions_addfchdir_np(
            &mut actions.0,
            cwd.fd(),
        ))?;
        for (source, destination) in [
            (pipes.stdin.as_raw_fd(), libc::STDIN_FILENO),
            (pipes.stdout.as_raw_fd(), libc::STDOUT_FILENO),
            (pipes.stderr.as_raw_fd(), libc::STDERR_FILENO),
        ] {
            status(libc::posix_spawn_file_actions_adddup2(
                &mut actions.0,
                source,
                destination,
            ))?;
        }

        let mut mask = 0;
        let mut defaults = 0;
        status(libc::sigemptyset(&mut mask))?;
        status(libc::sigemptyset(&mut defaults))?;
        status(libc::sigaddset(&mut defaults, libc::SIGPIPE))?;
        status(libc::posix_spawnattr_setsigmask(&mut attributes.0, &mask))?;
        status(libc::posix_spawnattr_setsigdefault(
            &mut attributes.0,
            &defaults,
        ))?;
        status(libc::posix_spawnattr_setpgroup(&mut attributes.0, 0))?;
        let flags = libc::POSIX_SPAWN_CLOEXEC_DEFAULT
            | libc::POSIX_SPAWN_START_SUSPENDED
            | libc::POSIX_SPAWN_SETPGROUP
            | libc::POSIX_SPAWN_SETSIGMASK
            | libc::POSIX_SPAWN_SETSIGDEF;
        status(libc::posix_spawnattr_setflags(
            &mut attributes.0,
            flags as libc::c_short,
        ))?;
    }

    // Darwin has no descriptor-based exec. Check the physical, trusted route
    // before spawning and the kernel's recorded image while it is suspended.
    // Execute the original artifact so Gatekeeper and signature checks apply.
    executable.confirm_path(executable.path())?;
    let mut pid = 0;
    // SAFETY: Image path and vectors are NUL-terminated; initialized spawn
    // handles and all referenced strings/descriptors remain live for the call.
    status(unsafe {
        libc::posix_spawn(
            &mut pid,
            path.as_ptr(),
            &actions.0,
            &attributes.0,
            arguments.as_ptr(),
            environment.as_ptr(),
        )
    })?;
    if pid <= 0 {
        return Err(invalid_data());
    }
    let group = ProcessGroup::new(pid);
    let mut image = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: This is our suspended child and the buffer is writable.
    let count = unsafe { libc::proc_pidpath(pid, image.as_mut_ptr().cast(), image.len() as u32) };
    if count <= 0 {
        return Err(io::Error::last_os_error());
    }
    let end = image
        .iter()
        .position(|&byte| byte == 0)
        .ok_or_else(invalid_data)?;
    executable.confirm_path(Path::new(OsStr::from_bytes(&image[..end])))?;
    // SAFETY: The exact suspended child remains unreaped and is ready to enter user code.
    retry(|| unsafe { libc::kill(pid, libc::SIGCONT) })?;
    Ok(group)
}

// libproc includes zombies when listing a process group. Capacity for a second
// member makes a count of one proof the observed exited leader is alone.
pub(super) fn group_contains_only(pid: libc::pid_t) -> bool {
    let mut members: [libc::pid_t; 2] = [0; 2];
    // SAFETY: The array is writable for the supplied byte size.
    let count = unsafe {
        libc::proc_listpgrppids(
            pid,
            members.as_mut_ptr().cast(),
            size_of_val(&members) as libc::c_int,
        )
    };
    count == 1 && members[0] == pid
}

struct FileActions(libc::posix_spawn_file_actions_t);

impl FileActions {
    fn new() -> io::Result<Self> {
        let mut actions = null_mut();
        // SAFETY: posix_spawn initializes this writable output handle.
        status(unsafe { libc::posix_spawn_file_actions_init(&mut actions) })?;
        Ok(Self(actions))
    }
}

impl Drop for FileActions {
    fn drop(&mut self) {
        // SAFETY: This value owns the initialized handle.
        unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
    }
}

struct Attributes(libc::posix_spawnattr_t);

impl Attributes {
    fn new() -> io::Result<Self> {
        let mut attributes = null_mut();
        // SAFETY: posix_spawn initializes this writable output handle.
        status(unsafe { libc::posix_spawnattr_init(&mut attributes) })?;
        Ok(Self(attributes))
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: This value owns the initialized handle.
        unsafe { libc::posix_spawnattr_destroy(&mut self.0) };
    }
}

fn status(result: libc::c_int) -> io::Result<()> {
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    Ok(())
}
