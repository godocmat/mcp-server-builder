use std::io;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use tokio::net::windows::named_pipe::NamedPipeServer;
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        JobObjects::{
            CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
            InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION,
            PROCESS_NAME_NATIVE, QueryFullProcessImageNameW, ResumeThread, STARTF_USESTDHANDLES,
            STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

use super::paths::LockedPath;
use super::pipes::{Input, InputControl, PipePair};
use super::{LaunchError, MAX_WINDOWS_UNITS, compare_names, invalid_data, owned, terminated};

const GRACE_PERIOD: Duration = Duration::from_secs(2);
const STOP_DEADLINE: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(in crate::supervision) struct NativeChild {
    job: OwnedHandle,
    process: OwnedHandle,
    id: u32,
    input_control: InputControl,
    stdin: Option<Input>,
    stdout: Option<NamedPipeServer>,
    stderr: Option<NamedPipeServer>,
}

impl NativeChild {
    pub(in crate::supervision) fn id(&self) -> u32 {
        self.id
    }

    pub(in crate::supervision) fn take_stdin(&mut self) -> Option<Input> {
        self.stdin.take()
    }

    pub(in crate::supervision) fn take_stdout(&mut self) -> Option<NamedPipeServer> {
        self.stdout.take()
    }

    pub(in crate::supervision) fn take_stderr(&mut self) -> Option<NamedPipeServer> {
        self.stderr.take()
    }

    pub(in crate::supervision) async fn stop(self) -> Result<(), LaunchError> {
        self.input_control.close();
        let started = Instant::now();
        let grace = self.wait_for_exit(started + GRACE_PERIOD).await;
        if matches!(grace, Ok(true)) {
            return Ok(());
        }

        // SAFETY: The job is owned by this child and the only process scope affected is its job.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            return Err(LaunchError::StopFailed);
        }
        match self.wait_for_exit(started + STOP_DEADLINE).await {
            Ok(true) => Ok(()),
            _ => Err(LaunchError::StopFailed),
        }
    }

    async fn wait_for_exit(&self, deadline: Instant) -> io::Result<bool> {
        loop {
            if process_exited(&self.process)? && active_processes(&self.job)? == 0 {
                return Ok(true);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            tokio::time::sleep(remaining.min(POLL_INTERVAL)).await;
        }
    }
}

impl Drop for NativeChild {
    fn drop(&mut self) {
        self.input_control.close();
        // The owned job handle is the only handle to the unnamed kill-on-close job.
        // Field drop closes it even if a caller canceled stop or the parent exits.
    }
}

pub(super) fn launch(
    executable: &LockedPath,
    cwd: &LockedPath,
    mut command_line: Vec<u16>,
    environment: &[u16],
) -> io::Result<NativeChild> {
    let job = new_job()?;
    launch_in_job(executable, cwd, &mut command_line, environment, job)
}

fn launch_in_job(
    executable: &LockedPath,
    cwd: &LockedPath,
    command_line: &mut [u16],
    environment: &[u16],
    job: OwnedHandle,
) -> io::Result<NativeChild> {
    let stdin = PipePair::new(true)?;
    let stdout = PipePair::new(false)?;
    let stderr = PipePair::new(false)?;
    let application = terminated(&executable.application_path);
    let directory = terminated(&cwd.application_path);

    let job_handles = [job.as_raw_handle()];
    let stdio = [
        stdin.allow_inheritance()?,
        stdout.allow_inheritance()?,
        stderr.allow_inheritance()?,
    ];
    // These arrays outlive the attribute list. They are borrowed by Windows until deletion.
    let mut attributes = AttributeList::new(2)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &job_handles)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &stdio)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    [
        startup.StartupInfo.hStdInput,
        startup.StartupInfo.hStdOutput,
        startup.StartupInfo.hStdError,
    ] = stdio;
    startup.lpAttributeList = attributes.as_ptr();
    let mut information = PROCESS_INFORMATION::default();
    // SAFETY: All terminated strings and handles live through the call. The command line
    // buffer is writable, the Unicode environment is double-terminated, and startup
    // contains the live extended attributes. Only the three listed handles are inherited.
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_SUSPENDED
                | CREATE_NO_WINDOW
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast(),
            directory.as_ptr(),
            &startup.StartupInfo,
            &mut information,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // The job list assigned the child at creation. Keep the primary thread suspended
    // until both job membership and the loaded native image path are checked.
    let mut process = SuspendedProcess(Some(owned(information.hProcess)?));
    let thread = owned(information.hThread)?;
    drop(attributes);
    drop(stdin.child);
    drop(stdout.child);
    drop(stderr.child);
    if !is_member(process.handle(), &job)? || !same_image(process.handle(), executable)? {
        return Err(invalid_data());
    }
    // SAFETY: The primary thread belongs to this newly created, checked process.
    if unsafe { ResumeThread(thread.as_raw_handle()) } != 1 {
        return Err(io::Error::last_os_error());
    }
    let (stdin, input_control) = Input::new(stdin.parent);
    let process = process.0.take().expect("the process handle is still owned");
    Ok(NativeChild {
        job,
        process,
        id: information.dwProcessId,
        input_control,
        stdin: Some(stdin),
        stdout: Some(stdout.parent),
        stderr: Some(stderr.parent),
    })
}

fn new_job() -> io::Result<OwnedHandle> {
    // SAFETY: Null security attributes and name create an unnamed, non-inheritable job.
    let job = owned(unsafe { CreateJobObjectW(null(), null()) })?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: The job and correctly sized, initialized limits structure live for this call.
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

fn is_member(process: &OwnedHandle, job: &OwnedHandle) -> io::Result<bool> {
    let mut member = 0;
    // SAFETY: Both handles are live and the output is writable.
    if unsafe { IsProcessInJob(process.as_raw_handle(), job.as_raw_handle(), &mut member) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(member != 0)
}

fn same_image(process: &OwnedHandle, executable: &LockedPath) -> io::Result<bool> {
    let mut path = vec![0; MAX_WINDOWS_UNITS];
    let mut length = path.len() as u32;
    // SAFETY: The process handle is live and the path and length outputs are writable.
    if unsafe {
        QueryFullProcessImageNameW(
            process.as_raw_handle(),
            PROCESS_NAME_NATIVE,
            path.as_mut_ptr(),
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let path = &path[..length as usize];
    Ok(compare_names(path, &executable.image_name()?)?.is_eq())
}

fn process_exited(process: &OwnedHandle) -> io::Result<bool> {
    // SAFETY: The process handle is live and the wait is nonblocking.
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

fn active_processes(job: &OwnedHandle) -> io::Result<u32> {
    let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    // SAFETY: The job handle and correctly sized writable output live through the call.
    if unsafe {
        QueryInformationJobObject(
            job.as_raw_handle(),
            JobObjectBasicAccountingInformation,
            (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(accounting.ActiveProcesses)
}

struct SuspendedProcess(Option<OwnedHandle>);

impl SuspendedProcess {
    fn handle(&self) -> &OwnedHandle {
        self.0.as_ref().expect("the process handle is still owned")
    }
}

impl Drop for SuspendedProcess {
    fn drop(&mut self) {
        if let Some(process) = &self.0 {
            // SAFETY: The handle owns this exact process, which never passed admission.
            unsafe {
                TerminateProcess(process.as_raw_handle(), 1);
                WaitForSingleObject(process.as_raw_handle(), 1_000);
            }
        }
    }
}

struct AttributeList(Vec<usize>);

impl AttributeList {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: A null list queries the required storage size.
        if unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut size) } != 0
            || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
            || size == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
        // SAFETY: The aligned buffer is writable and provides at least the requested size.
        if unsafe {
            InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), count, 0, &mut size)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(buffer))
    }

    fn as_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }

    fn set(&mut self, attribute: u32, handles: &[HANDLE]) -> io::Result<()> {
        // SAFETY: The list is initialized. The handle array remains live through list
        // deletion and contains the kind of handle required by the selected attribute.
        if unsafe {
            UpdateProcThreadAttribute(
                self.as_ptr(),
                0,
                attribute as usize,
                handles.as_ptr().cast(),
                size_of_val(handles),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: The list was initialized successfully and its backing storage is live.
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) };
    }
}
