use std::fs::{self, File, Metadata, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

pub(crate) fn file_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !is_link(&metadata) => Ok(true),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Expected a regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub(crate) fn read_file(path: &Path) -> io::Result<File> {
    open_file(path, false)
}

fn open_file(path: &Path, create: bool) -> io::Result<File> {
    if !file_exists(path)? && !create {
        return Err(io::ErrorKind::NotFound.into());
    }
    let mut options = OpenOptions::new();
    options.read(true).write(create).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        if create {
            // Preserve the lock-file identity while any builder holds the file open.
            options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        }
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Expected a regular file.",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Expected a single-link file.",
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // The borrowed handle and the output structure live through the call.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.nNumberOfLinks != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Expected a single-link file.",
            ));
        }
    }
    Ok(file)
}

pub(crate) fn lock(path: &Path) -> io::Result<File> {
    let file = open_file(path, true)?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if started.elapsed() < LOCK_TIMEOUT => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(TryLockError::WouldBlock) => return Err(io::ErrorKind::TimedOut.into()),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => check_private_dir(path),
        Err(error) => Err(error),
    }
}

fn check_private_dir(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Expected a real directory.",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        // getuid/geteuid have no pointer or memory arguments.
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Expected an owner-only directory.",
            ));
        }
    }
    Ok(())
}

pub(crate) fn flush_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path; // Windows publishes with a same-volume write-through move.
    Ok(())
}

pub(crate) fn publish_file(
    staged: tempfile::NamedTempFile,
    destination: &Path,
    replace: bool,
) -> io::Result<()> {
    #[cfg(unix)]
    {
        let result = if replace {
            staged.persist(destination)
        } else {
            staged.persist_noclobber(destination)
        };
        result.map_err(|error| error.error)?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_NORMAL, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            SetFileAttributesW,
        };

        let from: Vec<u16> = staged.path().as_os_str().encode_wide().chain([0]).collect();
        let to: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
        // Both paths are NUL-terminated native names from the checked project directory.
        // Remove the temp-file attribute before a write-through rename.
        if unsafe { SetFileAttributesW(from.as_ptr(), FILE_ATTRIBUTE_NORMAL) } == 0 {
            return Err(io::Error::last_os_error());
        }
        staged.as_file().sync_all()?;
        let mut flags = MOVEFILE_WRITE_THROUGH;
        if replace {
            flags |= MOVEFILE_REPLACE_EXISTING;
        }
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
