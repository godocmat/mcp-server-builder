use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rustix::fs::{
    AtFlags, FsWord, IFlags, Mode, OFlags, PROC_SUPER_MAGIC, RenameFlags, ResolveFlags, openat,
    openat2, renameat_with, statat,
};
use rustix::io::Errno;

use super::{ExportError, read_bounded, storage};

const NO_LINKS: ResolveFlags = ResolveFlags::NO_SYMLINKS;
const UNDER_DIRECTORY: ResolveFlags = NO_LINKS
    .union(ResolveFlags::BENEATH)
    .union(ResolveFlags::NO_XDEV);
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC);
const ENTRY_FLAGS: OFlags = OFlags::PATH.union(OFlags::CLOEXEC);
const EXT4: FsWord = 0xef53;
const XFS: FsWord = 0x58465342;
const BTRFS: FsWord = 0x9123683e_u32 as FsWord;
const TMPFS: FsWord = 0x01021994;
const OVERLAYFS: FsWord = 0x794c7630;
const CASEFOLD_OR_ENCRYPT: IFlags = IFlags::from_bits_retain(0x4000_0800);

pub(super) struct Directory {
    file: File,
    path: PathBuf,
    access_path: PathBuf,
}

impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self, ExportError> {
        let file = openat2(
            rustix::fs::CWD,
            path,
            DIRECTORY_FLAGS,
            Mode::empty(),
            NO_LINKS,
        )
        .map_err(open_error)?;
        match rustix::fs::fstatfs(&file).map_err(storage_errno)?.f_type {
            EXT4 | XFS | BTRFS | TMPFS | OVERLAYFS => {}
            _ => return Err(ExportError::Storage),
        }
        let proc = openat2(
            rustix::fs::CWD,
            "/proc",
            ENTRY_FLAGS | OFlags::DIRECTORY,
            Mode::empty(),
            NO_LINKS,
        )
        .map_err(open_error)?;
        if rustix::fs::fstatfs(&proc).map_err(storage_errno)?.f_type != PROC_SUPER_MAGIC {
            return Err(ExportError::Storage);
        }
        let root = Self::from_file(file.into(), path.to_path_buf());
        root.verify()?;
        Ok(root)
    }

    fn from_file(file: File, path: PathBuf) -> Self {
        let access_path = descriptor_path(&file);
        Self {
            file,
            path,
            access_path,
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn access_path(&self) -> &Path {
        &self.access_path
    }

    pub(super) fn verify(&self) -> Result<(), ExportError> {
        verify_path(&self.access_path, &self.path)?;
        match rustix::fs::ioctl_getflags(&self.file) {
            Ok(flags) if flags.intersects(CASEFOLD_OR_ENCRYPT) => Err(ExportError::UnsafePath),
            Ok(_) => Ok(()),
            // Older tmpfs and overlayfs kernels do not expose inode flags.
            Err(Errno::NOTTY | Errno::OPNOTSUPP)
                if matches!(
                    rustix::fs::fstatfs(&self.file)
                        .map_err(storage_errno)?
                        .f_type,
                    TMPFS | OVERLAYFS
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(storage_errno(error)),
        }
    }

    pub(super) fn check_absent(&self, name: &str) -> Result<(), ExportError> {
        self.verify()?;
        match statat(&self.file, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Err(ExportError::DestinationExists),
            Err(Errno::NOENT) => Ok(()),
            Err(error) => Err(storage_errno(error)),
        }
    }

    pub(super) fn open_stage(&self, name: &str) -> Result<Self, ExportError> {
        let Node::Directory(stage) = self.open_child(name)? else {
            return Err(ExportError::UnsafePath);
        };
        Ok(stage)
    }

    pub(super) fn open_child(&self, name: &str) -> Result<Node, ExportError> {
        let selected = openat2(
            &self.file,
            name,
            ENTRY_FLAGS,
            Mode::empty(),
            UNDER_DIRECTORY,
        )
        .map_err(open_error)?;
        let path = self.path.join(name);
        let selected = File::from(selected);
        let selected_path = descriptor_path(&selected);
        verify_path(&selected_path, &path)?;
        let metadata = selected.metadata().map_err(storage)?;
        let flags = if metadata.is_dir() {
            DIRECTORY_FLAGS
        } else if metadata.is_file() && metadata.nlink() == 1 {
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK
        } else {
            return Err(ExportError::UnsafePath);
        };
        // Reopen the checked inode through its live O_PATH descriptor. That avoids
        // opening a FIFO or device for data if a different entry replaces it.
        let file =
            openat(rustix::fs::CWD, selected_path, flags, Mode::empty()).map_err(storage_errno)?;
        let file = File::from(file);
        if metadata.is_dir() {
            let child = Self::from_file(file, path);
            child.verify()?;
            Ok(Node::Directory(child))
        } else {
            Ok(Node::File(RegularFile { file, path }))
        }
    }

    pub(super) fn sync(&self) -> Result<(), ExportError> {
        self.file.sync_all().map_err(storage)
    }

    pub(super) fn publish(
        &self,
        stage: &Directory,
        stage_name: &str,
        name: &str,
    ) -> Result<(), ExportError> {
        self.verify()?;
        stage.verify()?;
        renameat_with(
            &self.file,
            stage_name,
            &self.file,
            name,
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| match error {
            Errno::EXIST => ExportError::DestinationExists,
            _ => storage_errno(error),
        })?;
        // The complete tree is already visible after the rename. A flush failure
        // here cannot be reported as a failed publication.
        let _ = self.sync();
        Ok(())
    }
}

pub(super) enum Node {
    Directory(Directory),
    File(RegularFile),
}

pub(super) struct RegularFile {
    file: File,
    path: PathBuf,
}

impl RegularFile {
    pub(super) fn read(&self, max_bytes: usize) -> Result<Vec<u8>, ExportError> {
        self.verify()?;
        let before = self.file.metadata().map_err(storage)?;
        let bytes = read_bounded(&self.file, max_bytes)?;
        let after = self.file.metadata().map_err(storage)?;
        if before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(ExportError::Storage);
        }
        self.verify()?;
        Ok(bytes)
    }

    fn verify(&self) -> Result<(), ExportError> {
        let metadata = self.file.metadata().map_err(storage)?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(ExportError::UnsafePath);
        }
        verify_path(&descriptor_path(&self.file), &self.path)
    }
}

fn descriptor_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

fn verify_path(descriptor: &Path, expected: &Path) -> Result<(), ExportError> {
    // Compare the native spelling byte for byte, including the path to the parent.
    if fs::read_link(descriptor).map_err(storage)?.as_os_str() == expected.as_os_str() {
        Ok(())
    } else {
        Err(ExportError::UnsafePath)
    }
}

fn storage_errno(error: Errno) -> ExportError {
    storage(io::Error::from(error))
}

fn open_error(error: Errno) -> ExportError {
    match error {
        Errno::LOOP | Errno::XDEV | Errno::NOTDIR | Errno::AGAIN => ExportError::UnsafePath,
        _ => storage_errno(error),
    }
}
