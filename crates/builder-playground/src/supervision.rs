//! Inert native launch review and ownership of a supervised stdio process.
//!
//! Callers own the approval and must supply a binding derived from the current trusted
//! registration, configuration, grants, target bindings, and operation. A digest returned
//! by `review_launch` describes a proposed operation. It grants no permission on its own.
//! The Windows backend currently supports native `.exe` files on local NTFS and assigns
//! the process to a kill-on-close job at creation. It contains normal child processes
//! created within that job. The program still runs with the operator's OS privileges,
//! and the installation, its dependencies and the working directory must be trusted.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use builder_project::ManifestRevision;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(windows)]
mod windows;

/// A selected native command. Arguments and literal environment values are public
/// configuration. Sensitive values must be represented by a reference.
#[derive(Clone)]
pub struct LaunchSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub environment: Vec<LaunchEnvironment>,
}

#[derive(Clone)]
pub struct LaunchEnvironment {
    pub name: OsString,
    pub source: EnvironmentSource,
}

#[derive(Clone)]
pub enum EnvironmentSource {
    Literal(OsString),
    SecretReference(String),
}

/// A launch summary for a trusted caller to present during approval.
pub struct LaunchReview {
    digest: ManifestRevision,
    executable: PathBuf,
    args: Vec<OsString>,
    cwd: PathBuf,
    environment: Vec<LaunchEnvironment>,
}

impl LaunchReview {
    pub fn digest(&self) -> ManifestRevision {
        self.digest
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn environment(&self) -> &[LaunchEnvironment] {
        &self.environment
    }
}

/// Failures carry static diagnostics. Native paths, arguments and environment values
/// stay out of error formatting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchError {
    InvalidSpec,
    ReviewMismatch,
    CredentialUnavailable,
    UnsupportedPlatform,
    NativeFailure,
    StopFailed,
}

impl LaunchError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidSpec => "playground.invalid_launch",
            Self::ReviewMismatch => "policy.approval_required",
            Self::CredentialUnavailable => "credential.unavailable",
            Self::UnsupportedPlatform => "playground.launch_unsupported",
            Self::NativeFailure => "playground.launch_failed",
            Self::StopFailed => "playground.stop_failed",
        }
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSpec => "The executable, working directory or launch inputs are invalid.",
            Self::ReviewMismatch => "Review and approve the current launch inputs.",
            Self::CredentialUnavailable => "A reviewed launch credential is unavailable.",
            Self::UnsupportedPlatform => {
                "Native playground launch is unavailable on this platform."
            }
            Self::NativeFailure => "Could not start the supervised playground process.",
            Self::StopFailed => "Could not verify bounded playground process shutdown.",
        })
    }
}

impl std::error::Error for LaunchError {}

/// Inspect an executable and working directory without starting a process or resolving
/// credentials. File inspection is synchronous.
pub fn review_launch(
    spec: &LaunchSpec,
    binding: ManifestRevision,
) -> Result<LaunchReview, LaunchError> {
    #[cfg(windows)]
    {
        Ok(windows::PreparedLaunch::new(spec, binding)?.review)
    }
    #[cfg(not(windows))]
    {
        let _ = (spec, binding);
        Err(LaunchError::UnsupportedPlatform)
    }
}

/// Start the exact launch described by an already admitted approval. The trusted caller
/// must consume one-use approval before calling. Windows requires an entered Tokio runtime
/// with I/O enabled. Reinspection and process creation are synchronous. The returned pipes
/// are asynchronous, with framing and message limits enforced by the stdio client.
pub fn spawn_reviewed(
    spec: &LaunchSpec,
    binding: ManifestRevision,
    reviewed_digest: ManifestRevision,
    resolve_secret: impl FnMut(&str) -> Option<OsString>,
) -> Result<SupervisedChild, LaunchError> {
    #[cfg(windows)]
    {
        let prepared = windows::PreparedLaunch::new(spec, binding).map_err(|error| {
            if error == LaunchError::InvalidSpec {
                LaunchError::ReviewMismatch
            } else {
                error
            }
        })?;
        if prepared.review.digest != reviewed_digest {
            return Err(LaunchError::ReviewMismatch);
        }
        prepared.spawn(resolve_secret)
    }
    #[cfg(not(windows))]
    {
        let _ = (spec, binding, reviewed_digest, resolve_secret);
        Err(LaunchError::UnsupportedPlatform)
    }
}

/// A supervised process and its available stdio endpoints. Dropping the owner closes the
/// input and enforces containment, including when a stop future is canceled.
pub struct SupervisedChild {
    #[cfg(windows)]
    inner: windows::NativeChild,
}

impl SupervisedChild {
    pub fn id(&self) -> u32 {
        #[cfg(windows)]
        return self.inner.id();

        #[cfg(not(windows))]
        unreachable!("native process construction is unsupported");
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        #[cfg(windows)]
        return self.inner.take_stdin().map(ChildStdin);

        #[cfg(not(windows))]
        None
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        #[cfg(windows)]
        return self.inner.take_stdout().map(ChildStdout);

        #[cfg(not(windows))]
        None
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        #[cfg(windows)]
        return self.inner.take_stderr().map(ChildStderr);

        #[cfg(not(windows))]
        None
    }

    /// Close the child's input, allow up to two seconds for graceful exit, then
    /// terminate and check that the supervised processes exited within five seconds total.
    pub async fn stop(self) -> Result<(), LaunchError> {
        #[cfg(windows)]
        return self.inner.stop().await;

        #[cfg(not(windows))]
        Err(LaunchError::UnsupportedPlatform)
    }
}

pub struct ChildStdin(#[cfg(windows)] windows::Input);
pub struct ChildStdout(#[cfg(windows)] tokio::net::windows::named_pipe::NamedPipeServer);
pub struct ChildStderr(#[cfg(windows)] tokio::net::windows::named_pipe::NamedPipeServer);

impl AsyncWrite for ChildStdin {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        #[cfg(windows)]
        return Pin::new(&mut self.get_mut().0).poll_write(context, bytes);

        #[cfg(not(windows))]
        {
            let _ = (self, context, bytes);
            Poll::Ready(Err(io::ErrorKind::Unsupported.into()))
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        #[cfg(windows)]
        return Pin::new(&mut self.get_mut().0).poll_flush(context);

        #[cfg(not(windows))]
        {
            let _ = (self, context);
            Poll::Ready(Err(io::ErrorKind::Unsupported.into()))
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        #[cfg(windows)]
        return Pin::new(&mut self.get_mut().0).poll_shutdown(context);

        #[cfg(not(windows))]
        {
            let _ = (self, context);
            Poll::Ready(Err(io::ErrorKind::Unsupported.into()))
        }
    }
}

impl AsyncRead for ChildStdout {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        #[cfg(windows)]
        return Pin::new(&mut self.get_mut().0).poll_read(context, buffer);

        #[cfg(not(windows))]
        {
            let _ = (self, context, buffer);
            Poll::Ready(Err(io::ErrorKind::Unsupported.into()))
        }
    }
}

impl AsyncRead for ChildStderr {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        #[cfg(windows)]
        return Pin::new(&mut self.get_mut().0).poll_read(context, buffer);

        #[cfg(not(windows))]
        {
            let _ = (self, context, buffer);
            Poll::Ready(Err(io::ErrorKind::Unsupported.into()))
        }
    }
}
