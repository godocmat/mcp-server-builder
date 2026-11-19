use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use mcp_native_fs::macos::{Directory as NativeDirectory, Entry, RegularFile as NativeFile};
use rustix::fs::{self as native, AtFlags, Mode, OFlags, RenameFlags};
use rustix::io::Errno;
use uuid::Uuid;

use super::{ExportError, STAGE_PREFIX, read_bounded, storage};

pub(super) struct Directory(NativeDirectory);

impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self, ExportError> {
        NativeDirectory::open(path).map(Self).map_err(open_error)
    }

    pub(super) fn path(&self) -> &Path {
        self.0.path()
    }

    pub(super) fn verify(&self) -> Result<(), ExportError> {
        self.0.verify().map_err(open_error)
    }

    pub(super) fn check_absent(&self, name: &str) -> Result<(), ExportError> {
        self.verify()?;
        match native::statat(&self.0, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Err(ExportError::DestinationExists),
            Err(Errno::NOENT) => Ok(()),
            Err(error) => Err(storage(error.into())),
        }
    }

    pub(super) fn names(
        &self,
    ) -> Result<impl Iterator<Item = Result<OsString, ExportError>>, ExportError> {
        Ok(self
            .0
            .names()
            .map_err(open_error)?
            .map(|name| name.map_err(open_error)))
    }

    pub(super) fn open_child(&self, name: &str) -> Result<Node, ExportError> {
        match self.0.open_child(OsStr::new(name)).map_err(open_error)? {
            Entry::Directory(directory) => Ok(Node::Directory(Self(directory))),
            Entry::File(file) => Ok(Node::File(RegularFile(file))),
        }
    }

    pub(super) fn write_and_publish(
        &self,
        destination: &str,
        files: &BTreeMap<String, Vec<u8>>,
        recheck: impl FnOnce() -> Result<(), ExportError>,
    ) -> Result<(), ExportError> {
        let stage_name = format!("{STAGE_PREFIX}{}", Uuid::new_v4().simple());
        let stage = create_directory(&self.0, &stage_name)?;
        let result = (|| {
            write_stage(&stage, files)?;
            recheck()?;
            self.verify()?;
            stage.verify().map_err(open_error)?;
            native::renameat_with(
                &self.0,
                stage_name.as_str(),
                &self.0,
                destination,
                RenameFlags::NOREPLACE,
            )
            .map_err(|error| match error {
                Errno::EXIST => ExportError::DestinationExists,
                _ => storage(error.into()),
            })?;
            // Publication succeeded once the no-replace rename was visible.
            let _ = flush_directory(&self.0);
            Ok(())
        })();
        if result.is_err() {
            clear_stage(&stage)?;
            stage.verify().map_err(open_error)?;
            native::unlinkat(&self.0, stage_name.as_str(), AtFlags::REMOVEDIR)
                .map_err(|error| storage(error.into()))?;
        }
        result
    }
}

pub(super) enum Node {
    Directory(Directory),
    File(RegularFile),
}

pub(super) struct RegularFile(NativeFile);

impl RegularFile {
    pub(super) fn read(&self, max_bytes: usize) -> Result<Vec<u8>, ExportError> {
        let before = self.0.metadata().map_err(open_error)?;
        let bytes = read_bounded(&self.0, before.len(), max_bytes)?;
        let after = self.0.metadata().map_err(open_error)?;
        if before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(ExportError::Storage);
        }
        Ok(bytes)
    }
}

fn write_stage(
    stage: &NativeDirectory,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ExportError> {
    let mut directories = BTreeSet::new();
    for (relative, bytes) in files {
        let mut parent = None;
        for (end, _) in relative.match_indices('/') {
            let prefix = &relative[..end];
            let name = prefix.rsplit('/').next().ok_or(ExportError::UnsafePath)?;
            let current = parent.as_ref().unwrap_or(stage);
            parent = Some(if directories.insert(prefix) {
                create_directory(current, name)?
            } else {
                open_directory(current, name)?
            });
        }
        let parent = parent.as_ref().unwrap_or(stage);
        parent.verify().map_err(open_error)?;
        let name = relative.rsplit('/').next().ok_or(ExportError::UnsafePath)?;
        let file = native::openat(
            parent,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| storage(error.into()))?;
        let mut file = File::from(file);
        file.write_all(bytes).map_err(storage)?;
        file.sync_all().map_err(storage)?;
    }
    for relative in directories.into_iter().rev() {
        let mut parent = None;
        for name in relative.split('/') {
            parent = Some(open_directory(parent.as_ref().unwrap_or(stage), name)?);
        }
        flush_directory(parent.as_ref().unwrap_or(stage))?;
    }
    flush_directory(stage)
}

fn create_directory(parent: &NativeDirectory, name: &str) -> Result<NativeDirectory, ExportError> {
    parent.verify().map_err(open_error)?;
    native::mkdirat(parent, name, Mode::RWXU).map_err(|error| storage(error.into()))?;
    open_directory(parent, name)
}

fn open_directory(parent: &NativeDirectory, name: &str) -> Result<NativeDirectory, ExportError> {
    let Entry::Directory(directory) = parent.open_child(OsStr::new(name)).map_err(open_error)?
    else {
        return Err(ExportError::UnsafePath);
    };
    Ok(directory)
}

fn flush_directory(directory: &NativeDirectory) -> Result<(), ExportError> {
    directory.verify().map_err(open_error)?;
    match native::fsync(directory) {
        Ok(()) | Err(Errno::INVAL | Errno::NOTSUP) => Ok(()),
        Err(error) => Err(storage(error.into())),
    }
}

fn clear_stage(directory: &NativeDirectory) -> Result<(), ExportError> {
    for name in directory.names().map_err(open_error)? {
        let name = name.map_err(open_error)?;
        let flags = match directory.open_child(&name).map_err(open_error)? {
            Entry::Directory(child) => {
                clear_stage(&child)?;
                child.verify().map_err(open_error)?;
                AtFlags::REMOVEDIR
            }
            Entry::File(_) => AtFlags::empty(),
        };
        native::unlinkat(directory, name.as_os_str(), flags)
            .map_err(|error| storage(error.into()))?;
    }
    Ok(())
}

fn open_error(error: io::Error) -> ExportError {
    match error.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::InvalidInput => ExportError::UnsafePath,
        _ => storage(error),
    }
}
