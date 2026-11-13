use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::{
    ffi::OsStrExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use std::ptr::null_mut;
use std::slice;

use builder_project::ManifestRevision;
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, INVALID_HANDLE_VALUE},
    Globalization::{CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN, CompareStringOrdinal},
    Security::{
        GetLengthSid, GetTokenInformation, SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};
use zeroize::Zeroizing;

use super::{EnvironmentSource, LaunchError, LaunchReview, LaunchSpec, SupervisedChild};

mod paths;
mod pipes;
mod process;

pub(super) use pipes::Input;
pub(super) use process::NativeChild;

const MAX_WINDOWS_UNITS: usize = 32_767;
const MAX_ENVIRONMENT_ENTRIES: usize = 256;
const MAX_ENVIRONMENT_UNITS: usize = 32_767;
const EQUALS: u16 = b'=' as u16;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const SPACE: u16 = b' ' as u16;

pub(super) struct PreparedLaunch {
    pub(super) review: LaunchReview,
    executable: paths::LockedPath,
    cwd: paths::LockedPath,
    command_line: Vec<u16>,
    environment: Vec<EnvironmentEntry>,
}

struct EnvironmentEntry {
    name: Vec<u16>,
    source: EncodedSource,
}

enum EncodedSource {
    Literal(Vec<u16>),
    SecretReference(String),
}

impl PreparedLaunch {
    pub(super) fn new(spec: &LaunchSpec, binding: ManifestRevision) -> Result<Self, LaunchError> {
        Self::inspect(spec, binding).map_err(|_| LaunchError::InvalidSpec)
    }

    fn inspect(spec: &LaunchSpec, binding: ManifestRevision) -> io::Result<Self> {
        if !spec.executable.is_absolute()
            || !spec.cwd.is_absolute()
            || !paths::is_exe(&spec.executable)
        {
            return Err(invalid_data());
        }
        let environment = prepare_environment(spec)?;
        let mut executable = paths::LockedPath::open(&spec.executable, false)?;
        let cwd = paths::LockedPath::open(&spec.cwd, true)?;
        let operator = operator_identity()?;
        let command_line = command_line(executable.display_path.as_os_str(), &spec.args)?;
        let artifact = executable.executable_digest()?;

        let mut bytes = b"mcp-builder-playground/windows-native-stdio/v1\0".to_vec();
        put_bytes(&mut bytes, binding.to_string().as_bytes());
        put_bytes(&mut bytes, &operator);
        put_wide(&mut bytes, &wide(spec.executable.as_os_str())?);
        put_wide(&mut bytes, &executable.application_path);
        put_wide(&mut bytes, &wide(executable.display_path.as_os_str())?);
        bytes.extend_from_slice(&executable.identity);
        bytes.extend_from_slice(&artifact);
        put_wide(&mut bytes, &wide(spec.cwd.as_os_str())?);
        put_wide(&mut bytes, &cwd.application_path);
        put_wide(&mut bytes, &wide(cwd.display_path.as_os_str())?);
        bytes.extend_from_slice(&cwd.identity);
        put_count(&mut bytes, spec.args.len());
        for arg in &spec.args {
            put_wide(&mut bytes, &wide(arg)?);
        }
        put_count(&mut bytes, environment.len());
        for entry in &environment {
            put_wide(&mut bytes, &entry.name);
            match &entry.source {
                EncodedSource::Literal(value) => {
                    bytes.push(0);
                    put_wide(&mut bytes, value);
                }
                EncodedSource::SecretReference(reference) => {
                    bytes.push(1);
                    put_bytes(&mut bytes, reference.as_bytes());
                }
            }
        }

        let review = LaunchReview {
            digest: ManifestRevision::of(&bytes),
            executable: executable.display_path.clone(),
            args: spec.args.clone(),
            cwd: cwd.display_path.clone(),
            environment: spec.environment.clone(),
        };
        Ok(Self {
            review,
            executable,
            cwd,
            command_line,
            environment,
        })
    }

    pub(super) fn spawn(
        self,
        resolve_secret: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<SupervisedChild, LaunchError> {
        let environment = environment_block(self.environment, resolve_secret)?;
        let inner = process::launch(&self.executable, &self.cwd, self.command_line, &environment)
            .map_err(|_| LaunchError::NativeFailure)?;
        Ok(SupervisedChild { inner })
    }
}

fn prepare_environment(spec: &LaunchSpec) -> io::Result<Vec<EnvironmentEntry>> {
    if spec.environment.len() > MAX_ENVIRONMENT_ENTRIES {
        return Err(invalid_data());
    }
    let mut minimum_block_size = 2;
    let mut entries: Vec<EnvironmentEntry> = Vec::with_capacity(spec.environment.len());
    for entry in &spec.environment {
        let name = wide(&entry.name)?;
        if name.is_empty() || name.contains(&EQUALS) {
            return Err(invalid_data());
        }
        minimum_block_size += name.len() + 2;
        let source = match &entry.source {
            EnvironmentSource::Literal(value) => {
                let value = wide(value)?;
                minimum_block_size += value.len();
                EncodedSource::Literal(value)
            }
            EnvironmentSource::SecretReference(reference) => {
                if reference.is_empty()
                    || reference.len() > MAX_ENVIRONMENT_UNITS
                    || reference.contains('\0')
                {
                    return Err(invalid_data());
                }
                EncodedSource::SecretReference(reference.clone())
            }
        };
        if minimum_block_size > MAX_ENVIRONMENT_UNITS {
            return Err(invalid_data());
        }
        let mut index = entries.len();
        while index > 0 {
            match compare_names(&entries[index - 1].name, &name)? {
                Ordering::Less => break,
                Ordering::Equal => return Err(invalid_data()),
                Ordering::Greater => index -= 1,
            }
        }
        entries.insert(index, EnvironmentEntry { name, source });
    }
    Ok(entries)
}

fn environment_block(
    entries: Vec<EnvironmentEntry>,
    mut resolve_secret: impl FnMut(&str) -> Option<OsString>,
) -> Result<Zeroizing<Vec<u16>>, LaunchError> {
    // Reserve a fixed capacity so growing the block never copies resolved secrets into
    // an allocation that would be freed without wiping it.
    let mut block = Zeroizing::new(Vec::with_capacity(MAX_ENVIRONMENT_UNITS));
    for entry in entries {
        if block.len() + entry.name.len() + 3 > MAX_ENVIRONMENT_UNITS {
            return Err(LaunchError::CredentialUnavailable);
        }
        block.extend(entry.name);
        block.push(EQUALS);
        let value = match entry.source {
            EncodedSource::Literal(value) => Zeroizing::new(value),
            EncodedSource::SecretReference(reference) => {
                let value = resolve_secret(&reference).ok_or(LaunchError::CredentialUnavailable)?;
                let value = Zeroizing::new(value.into_encoded_bytes());
                // SAFETY: The value is the unmodified encoding from an OsString on Windows.
                let os_value = unsafe { OsStr::from_encoded_bytes_unchecked(&value) };
                let mut value = Zeroizing::new(Vec::with_capacity(MAX_WINDOWS_UNITS));
                value.extend(os_value.encode_wide().take(MAX_WINDOWS_UNITS));
                if value.len() == MAX_WINDOWS_UNITS || value.contains(&0) {
                    return Err(LaunchError::CredentialUnavailable);
                }
                value
            }
        };
        if block.len() + value.len() + 2 > MAX_ENVIRONMENT_UNITS {
            return Err(LaunchError::CredentialUnavailable);
        }
        block.extend(value.iter().copied());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

fn command_line(executable: &OsStr, args: &[OsString]) -> io::Result<Vec<u16>> {
    let mut line = Vec::new();
    for value in std::iter::once(executable).chain(args.iter().map(OsString::as_os_str)) {
        let value = wide(value)?;
        if !line.is_empty() {
            line.push(SPACE);
        }
        quote_argument(&mut line, &value);
        if line.len() >= MAX_WINDOWS_UNITS {
            return Err(invalid_data());
        }
    }
    line.push(0);
    Ok(line)
}

fn quote_argument(line: &mut Vec<u16>, argument: &[u16]) {
    // The native Win32 command line is a single string. Use the backslash/quote
    // rules of the Microsoft C runtime and Rust's native Windows argv parser.
    line.push(QUOTE);
    let mut backslashes = 0;
    for &unit in argument {
        if unit == BACKSLASH {
            backslashes += 1;
            continue;
        }
        if unit == QUOTE {
            line.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2 + 1));
        } else {
            line.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        }
        backslashes = 0;
        line.push(unit);
    }
    line.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2));
    line.push(QUOTE);
}

fn operator_identity() -> io::Result<Vec<u8>> {
    let mut token = null_mut();
    // SAFETY: GetCurrentProcess supplies the process pseudohandle and the output is writable.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = owned(token)?;
    let mut size = 0;
    // SAFETY: The live token is borrowed. The null buffer queries the required size.
    if unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut size) }
        == 0
        && io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
    {
        return Err(io::Error::last_os_error());
    }
    let size = size as usize;
    if size < size_of::<TOKEN_USER>() {
        return Err(invalid_data());
    }
    let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
    let mut used = 0;
    // SAFETY: The buffer is aligned for TOKEN_USER and holds at least the requested bytes.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size as u32,
            &mut used,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: A successful TokenUser query wrote TOKEN_USER and its SID in the live buffer.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let sid = user.User.Sid;
    if sid.is_null() {
        return Err(invalid_data());
    }
    // SAFETY: The SID is the result of a successful TokenUser query and its buffer is live.
    let length = unsafe { GetLengthSid(sid) };
    if length == 0 || length > SECURITY_MAX_SID_SIZE {
        return Err(invalid_data());
    }
    // SAFETY: TokenUser provides a valid SID spanning this many bytes in buffer.
    Ok(unsafe { slice::from_raw_parts(sid.cast::<u8>(), length as usize) }.to_vec())
}

fn put_count(bytes: &mut Vec<u8>, count: usize) {
    bytes.extend_from_slice(&(count as u64).to_le_bytes());
}

fn put_bytes(bytes: &mut Vec<u8>, value: &[u8]) {
    put_count(bytes, value.len());
    bytes.extend_from_slice(value);
}

fn put_wide(bytes: &mut Vec<u8>, value: &[u16]) {
    put_count(bytes, value.len());
    for unit in value {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let value: Vec<_> = value.encode_wide().take(MAX_WINDOWS_UNITS).collect();
    if value.len() == MAX_WINDOWS_UNITS || value.contains(&0) {
        return Err(invalid_data());
    }
    Ok(value)
}

fn compare_names(left: &[u16], right: &[u16]) -> io::Result<Ordering> {
    // SAFETY: The slices remain live for the call and their lengths fit i32.
    let result = unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len() as i32,
            right.as_ptr(),
            right.len() as i32,
            1,
        )
    };
    match result {
        CSTR_LESS_THAN => Ok(Ordering::Less),
        CSTR_EQUAL => Ok(Ordering::Equal),
        CSTR_GREATER_THAN => Ok(Ordering::Greater),
        _ => Err(io::Error::last_os_error()),
    }
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: Callers pass a newly created Win32 handle and transfer sole ownership here.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn terminated(path: &[u16]) -> Vec<u16> {
    let mut path = path.to_vec();
    path.push(0);
    path
}

fn invalid_data() -> io::Error {
    io::ErrorKind::InvalidData.into()
}
