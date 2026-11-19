use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::ptr::null_mut;

use builder_project::ManifestRevision;
use tokio::net::unix::pipe;
use zeroize::Zeroizing;

use super::{EnvironmentSource, LaunchError, LaunchReview, LaunchSpec, SupervisedChild};

mod paths;
mod process;

pub(super) use process::NativeChild;
pub(super) type Input = pipe::Sender;
pub(super) type Output = pipe::Receiver;

const MAX_ARGUMENT_BYTES: usize = 32_767;
const MAX_ENVIRONMENT_ENTRIES: usize = 256;
const MAX_ENVIRONMENT_BYTES: usize = 32_767;

pub(super) struct PreparedLaunch {
    pub(super) review: LaunchReview,
    executable: paths::Executable,
    cwd: paths::Directory,
    environment: BTreeMap<OsString, EnvironmentSource>,
}

impl PreparedLaunch {
    pub(super) fn new(spec: &LaunchSpec, binding: ManifestRevision) -> Result<Self, LaunchError> {
        Self::inspect(spec, binding).map_err(|_| LaunchError::InvalidSpec)
    }

    fn inspect(spec: &LaunchSpec, binding: ManifestRevision) -> io::Result<Self> {
        let operator = paths::Operator::current()?;
        let executable = paths::Executable::open(&spec.executable, operator)?;
        let cwd = paths::Directory::open(&spec.cwd, operator)?;
        let environment = review_environment(&spec.environment)?;

        let mut bytes = b"mcp-builder-playground/unix-native-stdio/v1\0".to_vec();
        put_bytes(&mut bytes, std::env::consts::OS.as_bytes());
        put_bytes(&mut bytes, binding.to_string().as_bytes());
        put_bytes(&mut bytes, &operator.0.to_le_bytes());
        put_bytes(&mut bytes, spec.executable.as_os_str().as_bytes());
        put_bytes(&mut bytes, executable.path().as_os_str().as_bytes());
        executable.write_identity(&mut bytes);
        put_bytes(&mut bytes, spec.cwd.as_os_str().as_bytes());
        put_bytes(&mut bytes, cwd.path().as_os_str().as_bytes());
        cwd.write_identity(&mut bytes);
        put_count(&mut bytes, spec.args.len());
        let mut total = executable.path().as_os_str().as_bytes().len() + 1;
        for argument in &spec.args {
            let argument = argument.as_bytes();
            total = total.saturating_add(argument.len() + 1);
            if argument.contains(&0) || total > MAX_ARGUMENT_BYTES {
                return Err(invalid_data());
            }
            put_bytes(&mut bytes, argument);
        }
        put_count(&mut bytes, environment.len());
        for (name, source) in &environment {
            put_bytes(&mut bytes, name.as_bytes());
            match source {
                EnvironmentSource::Literal(value) => {
                    bytes.push(0);
                    put_bytes(&mut bytes, value.as_bytes());
                }
                EnvironmentSource::SecretReference(reference) => {
                    bytes.push(1);
                    put_bytes(&mut bytes, reference.as_bytes());
                }
            }
        }

        Ok(Self {
            review: LaunchReview {
                digest: ManifestRevision::of(&bytes),
                executable: executable.path().to_path_buf(),
                args: spec.args.clone(),
                cwd: cwd.path().to_path_buf(),
                environment: spec.environment.clone(),
            },
            executable,
            cwd,
            environment,
        })
    }

    pub(super) fn spawn(
        self,
        resolve_secret: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<SupervisedChild, LaunchError> {
        let environment = resolve_environment(self.environment, resolve_secret)?;
        let arguments = ExecStrings::new(
            std::iter::once(self.executable.path().as_os_str())
                .chain(self.review.args.iter().map(OsString::as_os_str))
                .map(|arg| {
                    let mut bytes = arg.as_bytes().to_vec();
                    bytes.push(0);
                    Zeroizing::new(bytes)
                })
                .collect(),
        );
        let inner = process::launch(self.executable, self.cwd, arguments, environment)
            .map_err(|_| LaunchError::NativeFailure)?;
        Ok(SupervisedChild { inner })
    }
}

fn review_environment(
    environment: &[super::LaunchEnvironment],
) -> io::Result<BTreeMap<OsString, EnvironmentSource>> {
    if environment.len() > MAX_ENVIRONMENT_ENTRIES {
        return Err(invalid_data());
    }
    let mut output = BTreeMap::new();
    let mut minimum = 0usize;
    for entry in environment {
        let name = entry.name.as_bytes();
        if name.is_empty() || name.contains(&b'=') || name.contains(&0) {
            return Err(invalid_data());
        }
        minimum = minimum.saturating_add(name.len() + 2);
        match &entry.source {
            EnvironmentSource::Literal(value) => {
                let value = value.as_bytes();
                if value.contains(&0) {
                    return Err(invalid_data());
                }
                minimum = minimum.saturating_add(value.len());
            }
            EnvironmentSource::SecretReference(reference) => {
                if reference.is_empty()
                    || reference.len() > MAX_ENVIRONMENT_BYTES
                    || reference.contains('\0')
                {
                    return Err(invalid_data());
                }
            }
        }
        if minimum > MAX_ENVIRONMENT_BYTES
            || output
                .insert(entry.name.clone(), entry.source.clone())
                .is_some()
        {
            return Err(invalid_data());
        }
    }
    Ok(output)
}

fn resolve_environment(
    environment: BTreeMap<OsString, EnvironmentSource>,
    mut resolve_secret: impl FnMut(&str) -> Option<OsString>,
) -> Result<ExecStrings, LaunchError> {
    let mut entries = Vec::with_capacity(environment.len());
    let mut total = 0usize;
    for (name, source) in environment {
        let value = match source {
            EnvironmentSource::Literal(value) => value,
            EnvironmentSource::SecretReference(reference) => {
                resolve_secret(&reference).ok_or(LaunchError::CredentialUnavailable)?
            }
        };
        let value = Zeroizing::new(value.into_vec());
        let size = name.as_bytes().len().saturating_add(value.len() + 2);
        total = total.saturating_add(size);
        if value.contains(&0) || total > MAX_ENVIRONMENT_BYTES {
            return Err(LaunchError::CredentialUnavailable);
        }
        // One allocation keeps the resolved value out of abandoned buffers.
        let mut entry = Zeroizing::new(Vec::with_capacity(size));
        entry.extend_from_slice(name.as_bytes());
        entry.push(b'=');
        entry.extend_from_slice(&value);
        entry.push(0);
        entries.push(entry);
    }
    Ok(ExecStrings::new(entries))
}

struct ExecStrings {
    _storage: Vec<Zeroizing<Vec<u8>>>,
    pointers: Vec<*mut libc::c_char>,
}

impl ExecStrings {
    fn new(storage: Vec<Zeroizing<Vec<u8>>>) -> Self {
        let pointers = storage
            .iter()
            .map(|value| value.as_ptr().cast::<libc::c_char>().cast_mut())
            .chain(std::iter::once(null_mut()))
            .collect();
        Self {
            _storage: storage,
            pointers,
        }
    }

    fn as_ptr(&self) -> *const *mut libc::c_char {
        self.pointers.as_ptr()
    }
}

// SAFETY: The pointers address immutable heap buffers owned by this value. They
// remain valid across moves and the exec APIs read the vectors without mutating them.
#[cfg(target_os = "linux")]
unsafe impl Send for ExecStrings {}
// SAFETY: Reading the same frozen exec arguments concurrently cannot mutate them.
#[cfg(target_os = "linux")]
unsafe impl Sync for ExecStrings {}

fn put_count(bytes: &mut Vec<u8>, size: usize) {
    bytes.extend_from_slice(&(size as u64).to_le_bytes());
}

fn put_bytes(bytes: &mut Vec<u8>, value: &[u8]) {
    put_count(bytes, value.len());
    bytes.extend_from_slice(value);
}

fn invalid_data() -> io::Error {
    io::ErrorKind::InvalidData.into()
}

fn c_string(value: &OsStr) -> io::Result<std::ffi::CString> {
    std::ffi::CString::new(value.as_bytes()).map_err(|_| invalid_data())
}
