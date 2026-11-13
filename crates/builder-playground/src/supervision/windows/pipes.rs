use std::io;
use std::os::windows::io::{AsRawHandle, IntoRawHandle, OwnedHandle};
use std::pin::Pin;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use tokio::io::AsyncWrite;
use tokio::net::windows::named_pipe::NamedPipeServer;
use uuid::Uuid;
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation},
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    },
    System::{
        IO::CancelIoEx,
        Pipes::{CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_REJECT_REMOTE_CLIENTS},
        Threading::GetCurrentProcessId,
    },
};

use super::{invalid_data, owned};

const PIPE_CAPACITY: u32 = 16 * 1024;

pub(super) struct PipePair {
    pub(super) parent: NamedPipeServer,
    pub(super) child: OwnedHandle,
}

impl PipePair {
    pub(super) fn new(child_reads: bool) -> io::Result<Self> {
        let name = format!(r"\\.\pipe\mcp-builder-playground-{}", Uuid::new_v4());
        let name: Vec<_> = name.encode_utf16().chain([0]).collect();
        let (server_access, client_access) = if child_reads {
            (PIPE_ACCESS_OUTBOUND, GENERIC_READ)
        } else {
            (PIPE_ACCESS_INBOUND, GENERIC_WRITE)
        };
        // SAFETY: The path is terminated. The parent handle is overlapped and not inheritable.
        let parent = owned(unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                server_access | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_REJECT_REMOTE_CLIENTS,
                1,
                PIPE_CAPACITY,
                PIPE_CAPACITY,
                0,
                null(),
            )
        })?;
        // SAFETY: The path is terminated. The child handle is synchronous and has only
        // the access needed for its stdio stream.
        let child = owned(unsafe {
            CreateFileW(
                name.as_ptr(),
                client_access,
                0,
                null(),
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                null_mut(),
            )
        })?;
        // The sole client instance was just opened in this process. Confirm the peer before
        // using either end. An unexpected local client can only make setup fail.
        let mut client_pid = 0;
        // SAFETY: The parent handle is live and the output is writable.
        if unsafe { GetNamedPipeClientProcessId(parent.as_raw_handle(), &mut client_pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: This function has no arguments.
        if client_pid != unsafe { GetCurrentProcessId() } {
            return Err(invalid_data());
        }
        // SAFETY: Transfer sole ownership of a connected overlapped server end to Tokio.
        let parent = unsafe { NamedPipeServer::from_raw_handle(parent.into_raw_handle())? };
        Ok(Self { parent, child })
    }

    pub(super) fn allow_inheritance(&self) -> io::Result<HANDLE> {
        let child = self.child.as_raw_handle();
        // SAFETY: The child handle is live. CreateProcessW will use an explicit handle list.
        if unsafe { SetHandleInformation(child, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(child)
    }
}

// A caller can take stdin for the MCP writer while the process owner retains the ability
// to close it during stop or Drop. The lock is held only across a nonblocking poll.
pub(in crate::supervision) struct Input {
    control: Arc<Mutex<InputState>>,
}

#[derive(Clone)]
pub(super) struct InputControl(Arc<Mutex<InputState>>);

struct InputState {
    pipe: Option<NamedPipeServer>,
    waker: Option<Waker>,
}

impl Input {
    pub(super) fn new(pipe: NamedPipeServer) -> (Self, InputControl) {
        let control = Arc::new(Mutex::new(InputState {
            pipe: Some(pipe),
            waker: None,
        }));
        (
            Self {
                control: control.clone(),
            },
            InputControl(control),
        )
    }

    fn state(&self) -> MutexGuard<'_, InputState> {
        self.control
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

impl InputControl {
    pub(super) fn close(&self) {
        close_input(&self.0);
    }
}

fn close_input(control: &Mutex<InputState>) {
    let (pipe, waker) = {
        let mut state = control.lock().unwrap_or_else(|error| error.into_inner());
        (state.pipe.take(), state.waker.take())
    };
    if let Some(pipe) = pipe {
        // Mio otherwise lets an in-flight write keep the handle open after drop. Cancel
        // its overlapped I/O so the handle can close even when the child quit reading.
        // SAFETY: This is the live server end and null selects all operations on that end.
        unsafe { CancelIoEx(pipe.as_raw_handle(), null()) };
        drop(pipe);
    }
    if let Some(waker) = waker {
        waker.wake();
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        close_input(&self.control);
    }
}

impl AsyncWrite for Input {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = self.state();
        let Some(pipe) = state.pipe.as_mut() else {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        };
        // Tokio's Windows pipe implementation may queue a copy of an entire write.
        let bytes = &bytes[..bytes.len().min(PIPE_CAPACITY as usize)];
        let result = Pin::new(pipe).poll_write(context, bytes);
        if result.is_pending() {
            state.waker = Some(context.waker().clone());
        }
        result
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut state = self.state();
        match state.pipe.as_mut() {
            Some(pipe) => Pin::new(pipe).poll_flush(context),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        close_input(&self.control);
        Poll::Ready(Ok(()))
    }
}
