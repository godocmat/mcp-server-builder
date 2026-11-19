use std::fs::{self, File};
use std::io::{self, Read, Seek};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{c_string, invalid_data};

#[cfg(target_os = "macos")]
mod acl;

#[cfg(target_os = "linux")]
const DIRECTORY_SEARCH: libc::c_int = libc::O_PATH | libc::O_DIRECTORY;
#[cfg(target_os = "macos")]
const DIRECTORY_SEARCH: libc::c_int = libc::O_SEARCH;

#[derive(Clone, Copy)]
pub(super) struct Operator(pub(super) libc::uid_t);

impl Operator {
    pub(super) fn current() -> io::Result<Self> {
        // SAFETY: These calls read the invoking process credentials.
        let (uid, effective_uid, gid, effective_gid) = unsafe {
            (
                libc::getuid(),
                libc::geteuid(),
                libc::getgid(),
                libc::getegid(),
            )
        };
        if uid != effective_uid || gid != effective_gid {
            return Err(invalid_data());
        }
        Ok(Self(uid))
    }
}

pub(super) struct Directory {
    path: PathBuf,
    file: File,
    identity: Identity,
}

impl Directory {
    pub(super) fn open(path: &Path, operator: Operator) -> io::Result<Self> {
        let (path, file) = open_trusted(path, true, operator)?;
        let identity = Identity::of(&file.metadata()?);
        Ok(Self {
            path,
            file,
            identity,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    pub(super) fn write_identity(&self, output: &mut Vec<u8>) {
        self.identity.write(output);
    }
}

pub(super) struct Executable {
    path: PathBuf,
    file: File,
    version: FileVersion,
    digest: [u8; 32],
}

impl Executable {
    pub(super) fn open(path: &Path, operator: Operator) -> io::Result<Self> {
        let (path, mut file) = open_trusted(path, false, operator)?;
        let version = FileVersion::of(&file.metadata()?);
        let mut header = [0u8; 20];
        file.read_exact(&mut header)?;
        if !is_native_image(&header) {
            return Err(invalid_data());
        }
        file.rewind()?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        if version != FileVersion::of(&file.metadata()?) {
            return Err(invalid_data());
        }
        Ok(Self {
            path,
            file,
            version,
            digest: hasher.finalize().into(),
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(target_os = "linux")]
    pub(super) fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    pub(super) fn confirm_descriptor(&self) -> io::Result<()> {
        if self.version != FileVersion::of(&self.file.metadata()?) {
            return Err(invalid_data());
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub(super) fn confirm_path(&self, image: &Path) -> io::Result<()> {
        let (path, file) = open_trusted(image, false, Operator::current()?)?;
        if self.path != path || self.version != FileVersion::of(&file.metadata()?) {
            return Err(invalid_data());
        }
        self.confirm_descriptor()
    }

    pub(super) fn write_identity(&self, output: &mut Vec<u8>) {
        self.version.identity.write(output);
        output.extend(self.version.length.to_le_bytes());
        for stat in self.version.times {
            output.extend(stat.to_le_bytes());
        }
        output.extend(self.digest);
    }
}

#[derive(PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
    owner: libc::uid_t,
    group: libc::gid_t,
    mode: u32,
}

impl Identity {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner: metadata.uid(),
            group: metadata.gid(),
            mode: metadata.mode(),
        }
    }

    fn write(&self, output: &mut Vec<u8>) {
        output.extend(self.device.to_le_bytes());
        output.extend(self.inode.to_le_bytes());
        output.extend(self.owner.to_le_bytes());
        output.extend(self.group.to_le_bytes());
        output.extend(self.mode.to_le_bytes());
    }
}

#[derive(PartialEq, Eq)]
struct FileVersion {
    identity: Identity,
    length: u64,
    times: [i64; 4],
}

impl FileVersion {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            identity: Identity::of(metadata),
            length: metadata.len(),
            times: [
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ],
        }
    }
}

fn open_trusted(path: &Path, directory: bool, operator: Operator) -> io::Result<(PathBuf, File)> {
    if !path.is_absolute() {
        return Err(invalid_data());
    }
    // Resolve deliberate platform aliases such as macOS /var and review the
    // resulting physical path. Open every physical component without following links.
    let path = fs::canonicalize(path)?;
    let mut components = path.components().peekable();
    if components.next() != Some(Component::RootDir) {
        return Err(invalid_data());
    }
    let mut file = File::open("/")?;
    validate(&file, true, operator)?;
    if !directory && components.peek().is_none() {
        return Err(invalid_data());
    }
    while let Some(Component::Normal(component)) = components.next() {
        let wants_directory = directory || components.peek().is_some();
        let component = c_string(component)?;
        let access = if wants_directory {
            DIRECTORY_SEARCH
        } else {
            libc::O_RDONLY | libc::O_NONBLOCK
        };
        let flags = access | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: This is a single NUL-terminated component relative to a held directory.
        let child = unsafe { libc::openat(file.as_raw_fd(), component.as_ptr(), flags) };
        if child < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: Openat returned a new owned descriptor.
        file = unsafe { File::from_raw_fd(child) };
        validate(&file, wants_directory, operator)?;
    }
    Ok((path, File::from(super::process::above_stdio(file.into())?)))
}

fn validate(file: &File, directory: bool, operator: Operator) -> io::Result<()> {
    let metadata = file.metadata()?;
    let owner = metadata.uid();
    let mode = metadata.mode();
    let owned_by_trusted_user = owner == 0 || owner == operator.0;
    let private = mode & 0o022 == 0;
    // A sticky directory such as /tmp permits creating entries, while only the
    // entry owner or the directory owner can replace an existing reviewed entry.
    let sticky_directory = directory && mode & 0o1000 != 0;
    // macOS can grant write access in /Applications to its built-in admin group.
    let system_admin_directory = cfg!(target_os = "macos")
        && directory
        && owner == 0
        && metadata.gid() == 80
        && mode & 0o002 == 0;
    let usable_kind = if directory {
        metadata.is_dir()
    } else {
        metadata.is_file() && mode & 0o111 != 0 && mode & 0o6000 == 0
    };
    if !usable_kind
        || !owned_by_trusted_user
        || !(private || sticky_directory || system_admin_directory)
    {
        return Err(invalid_data());
    }
    #[cfg(target_os = "macos")]
    acl::validate(file.as_raw_fd())?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn is_native_image(header: &[u8; 20]) -> bool {
    if &header[..4] != b"\x7fELF" || !matches!(header[4], 1 | 2) || header[6] != 1 {
        return false;
    }
    let image_type = [header[16], header[17]];
    match header[5] {
        1 => matches!(u16::from_le_bytes(image_type), 2 | 3),
        2 => matches!(u16::from_be_bytes(image_type), 2 | 3),
        _ => false,
    }
}

#[cfg(target_os = "macos")]
fn is_native_image(header: &[u8; 20]) -> bool {
    matches!(
        &header[..4],
        [0xcf, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xca, 0xfe, 0xba, 0xbf]
    )
}
