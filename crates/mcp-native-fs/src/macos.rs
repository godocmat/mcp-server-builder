use std::ffi::{CStr, OsStr, OsString};
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use rustix::fs::{self as native, AtFlags, CWD, Dir, FileType, Mode, OFlags, Stat};
use rustix::io::Errno;

const NO_LINKS: OFlags = OFlags::NOFOLLOW_ANY;
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC)
    .union(NO_LINKS);
const FILE_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NONBLOCK)
    .union(OFlags::NOCTTY)
    .union(OFlags::CLOEXEC)
    .union(NO_LINKS);

/// A pinned, named directory on a local APFS volume.
pub struct Directory(Pinned);

impl Directory {
    pub fn open(path: &Path) -> io::Result<Self> {
        let path = physical_path(path)?;
        // The kernel applies no-follow to every component of the absolute
        // path. Ancestors only need search permission, including the protected
        // /private/var/folders hierarchy used by macOS temp directories.
        let current = File::from(open_directory(CWD, &path)?);
        let filesystem = native::fstatfs(&current)?;
        let filesystem_name = filesystem.f_fstypename.map(|byte| byte as u8);
        if filesystem.f_flags & libc::MNT_LOCAL as u32 == 0
            || !filesystem_name.starts_with(b"apfs\0")
        {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let directory = Self(Pinned::new(current, path)?);
        directory.verify()?;
        Ok(directory)
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }

    pub fn verify(&self) -> io::Result<()> {
        self.0.verify_spelling()?;
        let current = open_directory(CWD, self.path())?;
        if same_file(&native::fstat(&self.0.file)?, &native::fstat(&current)?) {
            Ok(())
        } else {
            Err(denied())
        }
    }

    pub fn identity(&self) -> io::Result<[u8; 24]> {
        self.verify()?;
        let metadata = native::fstat(self)?;
        let mut result = [0; 24];
        result[..8].copy_from_slice(&(metadata.st_dev as u64).to_le_bytes());
        result[8..16].copy_from_slice(&metadata.st_ino.to_le_bytes());
        Ok(result)
    }

    pub fn names(&self) -> io::Result<impl Iterator<Item = io::Result<OsString>>> {
        self.verify()?;
        Ok(Dir::read_from(self)?.filter_map(|entry| match entry {
            Ok(entry) if matches!(entry.file_name().to_bytes(), b"." | b"..") => None,
            Ok(entry) => Some(Ok(OsString::from_vec(
                entry.file_name().to_bytes().to_vec(),
            ))),
            Err(error) => Some(Err(error.into())),
        }))
    }

    pub fn open_child(&self, name: &OsStr) -> io::Result<Entry> {
        self.verify()?;
        let selected = named_entry(&self.0.file, name)?;
        if selected.st_dev != native::fstat(self)?.st_dev {
            return Err(denied());
        }
        let directory = FileType::from_raw_mode(selected.st_mode) == FileType::Directory;
        let flags = if directory {
            DIRECTORY_FLAGS
        } else if regular(&selected) {
            FILE_FLAGS
        } else {
            return Err(denied());
        };
        // The no-follow open is nonblocking for a file. A concurrent replacement
        // must still match the checked identity before any bytes are read.
        let file =
            File::from(native::openat(self, name, flags, Mode::empty()).map_err(path_error)?);
        let actual = native::fstat(&file)?;
        if !same_file(&actual, &selected)
            || !same_file(&actual, &named_entry(&self.0.file, name)?)
            || (!directory && !regular(&actual))
        {
            return Err(denied());
        }
        let pin = Pinned::new(file, self.path().join(name))?;
        if directory {
            let child = Self(pin);
            child.verify()?;
            Ok(Entry::Directory(child))
        } else {
            let file = RegularFile(pin);
            file.verify()?;
            Ok(Entry::File(file))
        }
    }
}

impl AsFd for Directory {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.file.as_fd()
    }
}

pub enum Entry {
    Directory(Directory),
    File(RegularFile),
}

pub struct RegularFile(Pinned);

impl RegularFile {
    pub fn metadata(&self) -> io::Result<Metadata> {
        self.verify()?;
        self.0.file.metadata()
    }

    fn verify(&self) -> io::Result<()> {
        self.0.verify_spelling()?;
        let parent = self.0.path.parent().ok_or_else(denied)?;
        let name = self.0.path.file_name().ok_or_else(denied)?;
        let parent = open_directory(CWD, parent)?;
        let named = native::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(path_error)?;
        let opened = native::fstat(&self.0.file)?;
        if regular(&named) && regular(&opened) && same_file(&named, &opened) {
            Ok(())
        } else {
            Err(denied())
        }
    }
}

impl Read for &RegularFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.verify()?;
        let count = (&self.0.file).read(buffer)?;
        self.verify()?;
        Ok(count)
    }
}

impl Read for RegularFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buffer)
    }
}

struct Pinned {
    file: File,
    path: PathBuf,
    native_path: PathBuf,
}

impl Pinned {
    fn new(file: File, path: PathBuf) -> io::Result<Self> {
        let native_path = descriptor_path(&file)?;
        if data_volume_spelling(&native_path) != data_volume_spelling(&path) {
            return Err(denied());
        }
        Ok(Self {
            file,
            path,
            native_path,
        })
    }

    fn verify_spelling(&self) -> io::Result<()> {
        if descriptor_path(&self.file)?.as_os_str() == self.native_path.as_os_str() {
            Ok(())
        } else {
            Err(denied())
        }
    }
}

fn open_directory(
    parent: impl AsFd,
    name: impl rustix::path::Arg,
) -> io::Result<std::os::fd::OwnedFd> {
    native::openat(parent, name, DIRECTORY_FLAGS, Mode::empty()).map_err(path_error)
}

fn named_entry(parent: &File, name: &OsStr) -> io::Result<Stat> {
    let requested = name.as_bytes();
    if matches!(requested, b"" | b"." | b"..") || requested.contains(&b'/') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let entry = native::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(path_error)?;
    // APFS can resolve case and Unicode normalization aliases. Read the entry
    // name from the pinned parent rather than accepting lookup success as proof.
    for candidate in Dir::read_from(parent)? {
        if candidate?.file_name().to_bytes() == requested {
            return Ok(entry);
        }
    }
    Err(denied())
}

fn physical_path(path: &Path) -> io::Result<PathBuf> {
    let raw = path.as_os_str().as_bytes();
    if raw == b"/" {
        return Ok(path.to_path_buf());
    }
    let relative = raw.strip_prefix(b"/").ok_or(io::ErrorKind::InvalidInput)?;
    let relative = relative.strip_suffix(b"/").unwrap_or(relative);
    let mut components = relative.split(|&byte| byte == b'/');
    let head = components.next().ok_or(io::ErrorKind::InvalidInput)?;
    if std::iter::once(head)
        .chain(components)
        .any(|component| matches!(component, b"" | b"." | b".."))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let relative = Path::new(OsStr::from_bytes(relative));
    // Recognize only Darwin's two fixed boot-volume aliases. They are owned by
    // the operating system; the operator-selected descendant is still opened
    // without following links.
    if matches!(head, b"tmp" | b"var") {
        let alias = Path::new("/").join(OsStr::from_bytes(head));
        let expected = Path::new("/private").join(OsStr::from_bytes(head));
        if fs::read_link(alias).is_ok_and(|target| {
            target == expected || target == Path::new("private").join(OsStr::from_bytes(head))
        }) {
            return Ok(Path::new("/private").join(relative));
        }
    }
    Ok(Path::new("/").join(relative))
}

fn data_volume_spelling(path: &Path) -> &[u8] {
    // XNU may give a descriptor's backing Data-volume spelling for a system
    // firmlink. The no-follow identity checks still use the exact caller path.
    let raw = path.as_os_str().as_bytes();
    raw.strip_prefix(b"/System/Volumes/Data")
        .filter(|suffix| suffix.starts_with(b"/"))
        .unwrap_or(raw)
}

fn descriptor_path(file: &File) -> io::Result<PathBuf> {
    let mut buffer = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most MAXPATHLEN bytes, the size of buffer on
    // macOS. The descriptor is live and the buffer stays writable for the call.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let path = CStr::from_bytes_until_nul(&buffer).map_err(|_| io::ErrorKind::InvalidData)?;
    if !path.to_bytes().starts_with(b"/") {
        return Err(denied());
    }
    Ok(PathBuf::from(OsStr::from_bytes(path.to_bytes())))
}

fn regular(entry: &Stat) -> bool {
    FileType::from_raw_mode(entry.st_mode) == FileType::RegularFile && entry.st_nlink == 1
}

fn same_file(left: &Stat, right: &Stat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino
}

fn denied() -> io::Error {
    io::ErrorKind::PermissionDenied.into()
}

fn path_error(error: Errno) -> io::Error {
    match error {
        Errno::LOOP | Errno::NOTDIR | Errno::XDEV => denied(),
        error => error.into(),
    }
}
