use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::{
    ffi::OsStringExt,
    fs::{MetadataExt, OpenOptionsExt},
    io::{AsRawHandle, FromRawHandle},
};
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr::{null, null_mut};

use windows_sys::Wdk::{
    Foundation::OBJECT_ATTRIBUTES,
    Storage::FileSystem::{
        FILE_OPEN, FILE_OPEN_NO_RECALL, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
        FILE_SYNCHRONOUS_IO_NONALERT, FileFsDeviceInformation, FileRenameInformation, NtCreateFile,
        NtQueryVolumeInformationFile, NtSetInformationFile,
    },
    System::SystemServices::{
        FILE_FS_DEVICE_INFORMATION, FILE_REMOTE_DEVICE, FILE_REMOTE_DEVICE_VSMB,
    },
};
use windows_sys::Win32::{
    Foundation::{
        INVALID_HANDLE_VALUE, OBJ_DONT_REPARSE, RtlNtStatusToDosError,
        STATUS_REPARSE_POINT_ENCOUNTERED, STATUS_SUCCESS, UNICODE_STRING,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_OFFLINE,
        FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_VIRTUAL, FILE_DEVICE_DISK,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_LIST_DIRECTORY, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_TYPE_DISK, GetFileInformationByHandle, GetFileType,
        GetFinalPathNameByHandleW, GetVolumeInformationByHandleW,
        GetVolumeNameForVolumeMountPointW, SYNCHRONIZE, VOLUME_NAME_GUID,
    },
    System::IO::IO_STATUS_BLOCK,
};

use super::{ExportError, read_bounded, storage};

const DIRECTORY_ACCESS: u32 =
    FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const VOLUME_SHARING: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
const UNSUPPORTED_ATTRIBUTES: u32 = FILE_ATTRIBUTE_DEVICE
    | FILE_ATTRIBUTE_OFFLINE
    | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
    | FILE_ATTRIBUTE_RECALL_ON_OPEN
    | FILE_ATTRIBUTE_REPARSE_POINT
    | FILE_ATTRIBUTE_VIRTUAL;

pub(super) struct Directory {
    file: File,
    // The real volume GUID path is independent of a subsequent DOS drive remap.
    access_path: PathBuf,
    _ancestors: Vec<File>,
}

impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self, ExportError> {
        let mut components = path.components();
        let volume = match components.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => volume_name(drive)?,
                Prefix::Verbatim(name) if is_volume_name(name) => {
                    let mut root = prefix.as_os_str().to_os_string();
                    root.push(r"\");
                    PathBuf::from(root)
                }
                _ => return Err(ExportError::UnsafePath),
            },
            _ => return Err(ExportError::UnsafePath),
        };
        if !matches!(components.next(), Some(Component::RootDir)) {
            return Err(ExportError::UnsafePath);
        }
        let file = OpenOptions::new()
            .access_mode(DIRECTORY_ACCESS)
            .share_mode(VOLUME_SHARING)
            .custom_flags(
                FILE_FLAG_BACKUP_SEMANTICS
                    | FILE_FLAG_OPEN_REPARSE_POINT
                    | FILE_FLAG_OPEN_NO_RECALL,
            )
            .open(&volume)
            .map_err(storage)?;
        check_local_ntfs(&file)?;
        let metadata = checked_metadata(&file)?;
        let access_path = normalized_path(&file)?;
        if !metadata.is_dir() || !volume_eq(&access_path, &volume) {
            return Err(ExportError::UnsafePath);
        }
        let mut current = Self {
            file,
            access_path,
            _ancestors: Vec::new(),
        };
        for component in components {
            let Component::Normal(name) = component else {
                return Err(ExportError::UnsafePath);
            };
            let Node::Directory(mut child) =
                current.open_child(name.to_str().ok_or(ExportError::UnsafePath)?)?
            else {
                return Err(ExportError::UnsafePath);
            };
            child._ancestors = current._ancestors;
            child._ancestors.push(current.file);
            current = child;
        }
        Ok(current)
    }

    pub(super) fn path(&self) -> &Path {
        &self.access_path
    }

    pub(super) fn access_path(&self) -> &Path {
        &self.access_path
    }

    pub(super) fn verify(&self) -> Result<(), ExportError> {
        if checked_metadata(&self.file)?.is_dir()
            && normalized_path(&self.file)? == self.access_path
        {
            Ok(())
        } else {
            Err(ExportError::UnsafePath)
        }
    }

    pub(super) fn check_absent(&self, name: &str) -> Result<(), ExportError> {
        self.verify()?;
        check_absent(&self.access_path.join(name))
    }

    pub(super) fn open_stage(&self, name: &str) -> Result<Self, ExportError> {
        let file = open_relative(&self.file, name, DIRECTORY_ACCESS | DELETE, FILE_SHARE_READ)?;
        let Node::Directory(stage) = self.checked_child(name, file)? else {
            return Err(ExportError::UnsafePath);
        };
        Ok(stage)
    }

    pub(super) fn open_child(&self, name: &str) -> Result<Node, ExportError> {
        let file = open_relative(&self.file, name, DIRECTORY_ACCESS, FILE_SHARE_READ)?;
        self.checked_child(name, file)
    }

    fn checked_child(&self, name: &str, file: File) -> Result<Node, ExportError> {
        let metadata = checked_metadata(&file)?;
        let access_path = self.access_path.join(name);
        if normalized_path(&file)?.as_os_str() != access_path.as_os_str() {
            return Err(ExportError::UnsafePath);
        }
        if metadata.is_dir() {
            Ok(Node::Directory(Self {
                file,
                access_path,
                _ancestors: Vec::new(),
            }))
        } else if metadata.is_file() {
            check_single_link(&file)?;
            Ok(Node::File(RegularFile { file, access_path }))
        } else {
            Err(ExportError::UnsafePath)
        }
    }

    pub(super) fn publish(
        &self,
        stage: &Directory,
        stage_name: &str,
        name: &str,
    ) -> Result<(), ExportError> {
        self.verify()?;
        stage.verify()?;
        let destination = self.access_path.join(name);
        if stage.access_path != self.access_path.join(stage_name) {
            return Err(ExportError::UnsafePath);
        }
        // Use a handle-based rename so the verified staging directory stays pinned.
        rename(&stage.file, name).map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => ExportError::DestinationExists,
            _ => check_absent(&destination)
                .err()
                .unwrap_or_else(|| storage(error)),
        })
    }
}

pub(super) enum Node {
    Directory(Directory),
    File(RegularFile),
}

pub(super) struct RegularFile {
    file: File,
    access_path: PathBuf,
}

impl RegularFile {
    pub(super) fn read(&self, max_bytes: usize) -> Result<Vec<u8>, ExportError> {
        self.verify()?;
        let bytes = read_bounded(&self.file, max_bytes)?;
        self.verify()?;
        Ok(bytes)
    }

    fn verify(&self) -> Result<(), ExportError> {
        check_single_link(&self.file)?;
        if checked_metadata(&self.file)?.is_file()
            && normalized_path(&self.file)? == self.access_path
        {
            Ok(())
        } else {
            Err(ExportError::UnsafePath)
        }
    }
}

fn open_relative(
    parent: &File,
    name: &str,
    access: u32,
    sharing: u32,
) -> Result<File, ExportError> {
    let mut name: Vec<u16> = name.encode_utf16().collect();
    let length =
        u16::try_from(size_of_val(name.as_slice())).map_err(|_| ExportError::UnsafePath)?;
    let object_name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &object_name,
        Attributes: OBJ_DONT_REPARSE,
        ..Default::default()
    };
    let mut handle = null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: The parent is an open directory. This validated component and all
    // output structures remain live for the synchronous native open.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status,
            null(),
            0,
            sharing,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT | FILE_OPEN_NO_RECALL | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    check_status(result)?;
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(ExportError::Storage);
    }
    // SAFETY: NtCreateFile returned a new owned handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn volume_name(drive: u8) -> Result<PathBuf, ExportError> {
    let drive = [u16::from(drive), b':' as u16, b'\\' as u16, 0];
    let mut volume = [0u16; 64];
    // SAFETY: The input is terminated and the buffer is writable.
    if unsafe { GetVolumeNameForVolumeMountPointW(drive.as_ptr(), volume.as_mut_ptr(), 64) } == 0 {
        return Err(storage(io::Error::last_os_error()));
    }
    let length = volume
        .iter()
        .position(|&unit| unit == 0)
        .ok_or(ExportError::Storage)?;
    Ok(PathBuf::from(OsString::from_wide(&volume[..length])))
}

pub(super) fn is_volume_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let bytes = name.as_bytes();
    if bytes.len() != 44 || !bytes[..7].eq_ignore_ascii_case(b"Volume{") || bytes[43] != b'}' {
        return false;
    }
    bytes[7..43].iter().enumerate().all(|(index, &byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

fn volume_eq(left: &Path, right: &Path) -> bool {
    left.to_str()
        .zip(right.to_str())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
}

fn check_local_ntfs(file: &File) -> Result<(), ExportError> {
    let mut status = IO_STATUS_BLOCK::default();
    let mut device = FILE_FS_DEVICE_INFORMATION::default();
    // SAFETY: The borrowed handle and both writable outputs are live.
    let result = unsafe {
        NtQueryVolumeInformationFile(
            file.as_raw_handle(),
            &mut status,
            (&mut device as *mut FILE_FS_DEVICE_INFORMATION).cast(),
            size_of::<FILE_FS_DEVICE_INFORMATION>() as u32,
            FileFsDeviceInformation,
        )
    };
    check_status(result)?;
    if status.Information != size_of::<FILE_FS_DEVICE_INFORMATION>()
        || device.DeviceType != FILE_DEVICE_DISK
        || device.Characteristics & (FILE_REMOTE_DEVICE | FILE_REMOTE_DEVICE_VSMB) != 0
    {
        return Err(ExportError::Storage);
    }
    let mut filesystem = [0u16; 32];
    // SAFETY: The handle is live and the only requested output is the writable name buffer.
    if unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    } == 0
    {
        return Err(storage(io::Error::last_os_error()));
    }
    if filesystem[..5] != [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0] {
        return Err(ExportError::Storage);
    }
    Ok(())
}

fn checked_metadata(file: &File) -> Result<fs::Metadata, ExportError> {
    // SAFETY: The handle is borrowed.
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
        return Err(ExportError::UnsafePath);
    }
    let metadata = file.metadata().map_err(storage)?;
    if metadata.file_attributes() & UNSUPPORTED_ATTRIBUTES != 0 {
        return Err(ExportError::UnsafePath);
    }
    Ok(metadata)
}

fn check_single_link(file: &File) -> Result<(), ExportError> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: The borrowed handle and writable output are live through the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(storage(io::Error::last_os_error()));
    }
    if info.nNumberOfLinks == 1 {
        Ok(())
    } else {
        Err(ExportError::UnsafePath)
    }
}

fn normalized_path(file: &File) -> Result<PathBuf, ExportError> {
    let mut name = vec![0u16; 260];
    loop {
        // SAFETY: The borrowed handle and writable buffer are live.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                name.as_mut_ptr(),
                name.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_GUID,
            )
        } as usize;
        if length == 0 {
            return Err(storage(io::Error::last_os_error()));
        }
        if length < name.len() {
            return Ok(PathBuf::from(OsString::from_wide(&name[..length])));
        }
        if length == name.len() || length > 32_768 {
            return Err(ExportError::UnsafePath);
        }
        name.resize(length, 0);
    }
}

fn check_status(status: i32) -> Result<(), ExportError> {
    match status {
        STATUS_SUCCESS => Ok(()),
        STATUS_REPARSE_POINT_ENCOUNTERED => Err(ExportError::UnsafePath),
        // SAFETY: Translating an NTSTATUS has no pointer or handle arguments.
        _ => Err(storage(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ))),
    }
}

fn check_absent(path: &Path) -> Result<(), ExportError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(ExportError::DestinationExists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(storage(error)),
    }
}

fn rename(file: &File, name: &str) -> io::Result<()> {
    // The fixed buffer has the native variable-length structure's layout, plus
    // enough trailing storage for one validated NTFS component and its NUL.
    #[repr(C)]
    struct RenameInfo {
        flags: u32,
        root_directory: windows_sys::Win32::Foundation::HANDLE,
        name_length: u32,
        name: [u16; 256],
    }
    const _: () = {
        assert!(
            std::mem::offset_of!(RenameInfo, flags)
                == std::mem::offset_of!(FILE_RENAME_INFORMATION, Anonymous)
        );
        assert!(
            std::mem::offset_of!(RenameInfo, root_directory)
                == std::mem::offset_of!(FILE_RENAME_INFORMATION, RootDirectory)
        );
        assert!(
            std::mem::offset_of!(RenameInfo, name_length)
                == std::mem::offset_of!(FILE_RENAME_INFORMATION, FileNameLength)
        );
        assert!(
            std::mem::offset_of!(RenameInfo, name)
                == std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName)
        );
    };
    let mut info = RenameInfo {
        flags: 0,
        root_directory: null_mut(),
        name_length: 0,
        name: [0; 256],
    };
    for (slot, unit) in info.name.iter_mut().zip(name.encode_utf16()) {
        *slot = unit;
        info.name_length += size_of::<u16>() as u32;
    }
    // A simple name with no root handle renames within the pinned parent.
    // A zero flag requires the destination to be absent.
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: The DELETE handle and writable status are live, and info matches
    // the structure required by FileRenameInformation.
    let result = unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status,
            (&raw const info).cast(),
            size_of::<RenameInfo>() as u32,
            FileRenameInformation,
        )
    };
    if result != STATUS_SUCCESS {
        // SAFETY: Translating an NTSTATUS has no pointer or handle arguments.
        Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ))
    } else {
        Ok(())
    }
}
