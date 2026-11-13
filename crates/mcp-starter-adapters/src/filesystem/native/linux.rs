use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Read};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::Path;

use super::denied;

const NO_LINKS: u64 = libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;
const BENEATH_ROOT: u64 = NO_LINKS | libc::RESOLVE_BENEATH | libc::RESOLVE_NO_XDEV;
const DIRECTORY_FLAGS: i32 = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
const PROC_DIRECTORY_FLAGS: i32 = DIRECTORY_FLAGS | libc::O_PATH;
const FS_CASEFOLD_FL: libc::c_uint = 0x40000000;
const FS_ENCRYPT_FL: libc::c_uint = 0x00000800;

pub(super) struct Root {
    directory: File,
    name: Vec<u8>,
    proc_fds: File,
    filesystem: libc::c_long,
}

impl Root {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let raw_path = path.as_os_str().as_bytes();
        let Some(relative) = raw_path.strip_prefix(b"/") else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        let relative = relative.strip_suffix(b"/").unwrap_or(relative);
        if raw_path != b"/"
            && relative
                .split(|&byte| byte == b'/')
                .any(|segment| matches!(segment, b"" | b"." | b".."))
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let native_path = CString::new(raw_path).map_err(|_| io::ErrorKind::InvalidInput)?;
        let directory = open_at2(libc::AT_FDCWD, &native_path, DIRECTORY_FLAGS, NO_LINKS)?;
        let filesystem = filesystem_type(&directory)?;
        if !matches!(filesystem, libc::EXT4_SUPER_MAGIC | libc::TMPFS_MAGIC) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        check_directory_flags(&directory, filesystem)?;

        let proc = open_at2(libc::AT_FDCWD, c"/proc", PROC_DIRECTORY_FLAGS, NO_LINKS)?;
        if filesystem_type(&proc)? != libc::PROC_SUPER_MAGIC {
            return Err(denied());
        }
        // "self" is a kernel-generated relative link to this process in the proc mount's
        // PID namespace. No magic links or additional mounts are allowed in this lookup.
        let proc_fds = open_at2(
            proc.as_raw_fd(),
            c"self/fd",
            PROC_DIRECTORY_FLAGS,
            libc::RESOLVE_BENEATH | libc::RESOLVE_NO_MAGICLINKS | libc::RESOLVE_NO_XDEV,
        )?;
        let name = descriptor_path(&proc_fds, &descriptor_name(&directory))?;
        let expected = if raw_path == b"/" {
            raw_path
        } else {
            raw_path.strip_suffix(b"/").unwrap_or(raw_path)
        };
        if name != expected {
            return Err(denied());
        }
        Ok(Self {
            directory,
            name,
            proc_fds,
            filesystem,
        })
    }

    pub(super) fn open_file(&self, relative: &str) -> io::Result<OpenedFile<'_>> {
        // Check each actual parent directory. A casefolded directory may look up an alias
        // even when the returned dentry name happens to match the spelling in the request.
        let mut parent = None::<File>;
        let mut components = relative.split('/').peekable();
        let selected = loop {
            let component = components.next().ok_or(io::ErrorKind::InvalidInput)?;
            let name = CString::new(component).map_err(|_| io::ErrorKind::InvalidInput)?;
            let directory = parent.as_ref().unwrap_or(&self.directory);
            let is_leaf = components.peek().is_none();
            let flags = if is_leaf {
                // O_PATH resolves the name without opening a device, FIFO or socket for data.
                libc::O_PATH | libc::O_CLOEXEC
            } else {
                DIRECTORY_FLAGS
            };
            check_directory_flags(directory, self.filesystem)?;
            let child = open_at2(directory.as_raw_fd(), &name, flags, BENEATH_ROOT)?;
            check_directory_flags(directory, self.filesystem)?;
            if is_leaf {
                break child;
            }
            parent = Some(child);
        };
        let selected_metadata = selected.metadata()?;
        if !selected_metadata.is_file() || selected_metadata.nlink() != 1 {
            return Err(denied());
        }

        let mut expected = self.name.clone();
        if expected != b"/" {
            expected.push(b'/');
        }
        expected.extend_from_slice(relative.as_bytes());
        let name = descriptor_name(&selected);
        if descriptor_path(&self.proc_fds, &name)? != expected {
            return Err(denied());
        }
        // Verify the complete relative path is still beneath the approved root and resolves
        // to the same file selected by the single-component walk.
        let path = CString::new(relative).map_err(|_| io::ErrorKind::InvalidInput)?;
        let resolved = open_at2(
            self.directory.as_raw_fd(),
            &path,
            libc::O_PATH | libc::O_CLOEXEC,
            BENEATH_ROOT,
        )?;
        if !same_file(&selected_metadata, &resolved.metadata()?) {
            return Err(denied());
        }
        // SAFETY: This opens the numeric descriptor entry of the live O_PATH file above,
        // relative to the verified procfs descriptor. The procfs magic link refers to the
        // already-opened path, and no caller-supplied path is used in this reopen.
        let fd = unsafe {
            libc::openat(
                self.proc_fds.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK
                    | libc::O_NOCTTY
                    | libc::O_LARGEFILE,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new owned descriptor on success.
        let file = unsafe { File::from_raw_fd(fd) };
        if !same_file(&selected_metadata, &file.metadata()?) {
            return Err(denied());
        }
        let opened = OpenedFile {
            name: descriptor_name(&file),
            file,
            proc_fds: &self.proc_fds,
            expected,
        };
        opened.check()?;
        Ok(opened)
    }
}

pub(super) struct OpenedFile<'a> {
    file: File,
    proc_fds: &'a File,
    name: CString,
    expected: Vec<u8>,
}

impl OpenedFile<'_> {
    fn check(&self) -> io::Result<()> {
        let metadata = self.file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || descriptor_path(self.proc_fds, &self.name)? != self.expected
        {
            return Err(denied());
        }
        Ok(())
    }
}

impl Read for OpenedFile<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        self.file.read(buffer)
    }
}

fn check_directory_flags(file: &File, filesystem: libc::c_long) -> io::Result<()> {
    let mut flags: libc::c_uint = 0;
    // SAFETY: Linux FS_IOC_GETFLAGS writes a 32-bit flag value to the live output.
    if unsafe { libc::ioctl(file.as_raw_fd(), libc::FS_IOC_GETFLAGS, &mut flags) } != 0 {
        let error = io::Error::last_os_error();
        // Older tmpfs kernels lack the flag API and cannot enable casefolded directories.
        if filesystem == libc::TMPFS_MAGIC && error.raw_os_error() == Some(libc::ENOTTY) {
            return Ok(());
        }
        return Err(error);
    }
    if flags & (FS_CASEFOLD_FL | FS_ENCRYPT_FL) != 0 {
        return Err(denied());
    }
    Ok(())
}

fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(test)]
#[test]
fn refuses_casefolded_directories_when_the_test_mount_supports_them() {
    const FS_IOC_SETFLAGS: libc::Ioctl = libc::_IOW::<libc::c_long>('f' as u32, 2);

    let temp = tempfile::tempdir().unwrap();
    let casefolded = temp.path().join("casefolded");
    std::fs::create_dir(&casefolded).unwrap();
    let directory = File::open(&casefolded).unwrap();
    let flags = FS_CASEFOLD_FL;
    // SAFETY: The flag value is valid and the descriptor is a unique, empty test directory.
    if unsafe { libc::ioctl(directory.as_raw_fd(), FS_IOC_SETFLAGS, &flags) } != 0 {
        let error = io::Error::last_os_error();
        assert!(
            matches!(
                error.raw_os_error(),
                Some(libc::EOPNOTSUPP | libc::ENOTTY | libc::EINVAL)
            ),
            "{error:?}"
        );
        eprintln!("Test mount cannot set the casefold flag: {error}");
        return;
    }
    std::fs::write(casefolded.join("Exact.txt"), b"synthetic fixture").unwrap();
    assert_eq!(
        std::fs::read(casefolded.join("exact.txt")).unwrap(),
        b"synthetic fixture"
    );

    assert!(matches!(
        Root::open(&casefolded),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied
    ));
    let root = Root::open(temp.path()).unwrap();
    for name in ["casefolded/Exact.txt", "casefolded/exact.txt"] {
        assert!(matches!(
            root.open_file(name),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied
        ));
    }
}

fn open_at2(directory: i32, path: &CStr, flags: i32, resolve: u64) -> io::Result<File> {
    // SAFETY: open_how contains only integer flags and fields.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = flags as u64;
    how.resolve = resolve;
    // SAFETY: The NUL-terminated path and correctly sized structure are live for the syscall.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory,
            path.as_ptr(),
            &how as *const libc::open_how,
            size_of::<libc::open_how>(),
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(libc::ELOOP | libc::EXDEV | libc::EAGAIN) => denied(),
            _ => error,
        });
    }
    // SAFETY: A successful openat2 returns a new owned file descriptor.
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}

fn filesystem_type(file: &File) -> io::Result<libc::c_long> {
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: The descriptor is borrowed and the output is valid for writing.
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: A successful fstatfs initializes the output.
    Ok(unsafe { stat.assume_init() }.f_type as libc::c_long)
}

fn descriptor_name(file: &File) -> CString {
    CString::new(file.as_raw_fd().to_string()).expect("descriptor number contains no NUL")
}

fn descriptor_path(proc_fds: &File, name: &CStr) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: The borrowed fd, terminated input name and writable output are live for this call.
    let length = unsafe {
        libc::readlinkat(
            proc_fds.as_raw_fd(),
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize == buffer.len() {
        return Err(denied());
    }
    buffer.truncate(length as usize);
    Ok(buffer)
}
