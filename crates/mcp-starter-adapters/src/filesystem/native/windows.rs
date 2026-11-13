use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::{
    ffi::OsStringExt,
    fs::OpenOptionsExt,
    io::{AsRawHandle, FromRawHandle},
};
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Wdk::{
    Foundation::OBJECT_ATTRIBUTES,
    Storage::FileSystem::{
        FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_NO_RECALL, FILE_OPEN_REPARSE_POINT,
        FILE_STANDARD_LINK_INFORMATION, FILE_SYNCHRONOUS_IO_NONALERT, FileFsDeviceInformation,
        FileStandardLinkInformation, NtCreateFile, NtQueryInformationFile,
        NtQueryVolumeInformationFile,
    },
    System::SystemServices::{
        FILE_FS_DEVICE_INFORMATION, FILE_REMOTE_DEVICE, FILE_REMOTE_DEVICE_VSMB,
    },
};
use windows_sys::Win32::{
    Foundation::{
        INVALID_HANDLE_VALUE, OBJ_DONT_REPARSE, RtlNtStatusToDosError, STATUS_SUCCESS,
        UNICODE_STRING,
    },
    Storage::FileSystem::{
        FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE,
        FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_ATTRIBUTE_VIRTUAL,
        FILE_DEVICE_DISK, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_TYPE_DISK, FileAttributeTagInfo,
        GetFileInformationByHandleEx, GetFileType, GetFinalPathNameByHandleW,
        GetVolumeInformationByHandleW, GetVolumeNameForVolumeMountPointW, SYNCHRONIZE,
        VOLUME_NAME_GUID,
    },
    System::IO::IO_STATUS_BLOCK,
};

use super::{denied, valid_component};

const DIRECTORY_ACCESS: u32 = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const VOLUME_SHARING: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
const UNSUPPORTED_ATTRIBUTES: u32 = FILE_ATTRIBUTE_DEVICE
    | FILE_ATTRIBUTE_OFFLINE
    | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
    | FILE_ATTRIBUTE_RECALL_ON_OPEN
    | FILE_ATTRIBUTE_REPARSE_POINT
    | FILE_ATTRIBUTE_VIRTUAL;

pub(super) struct Root {
    directory: File,
    name: Vec<u16>,
}

impl Root {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let (drive, names) = root_components(path)?;
        let volume = volume_name(drive)?;
        let directory = OpenOptions::new()
            .access_mode(DIRECTORY_ACCESS)
            .share_mode(VOLUME_SHARING)
            .custom_flags(
                FILE_FLAG_BACKUP_SEMANTICS
                    | FILE_FLAG_OPEN_REPARSE_POINT
                    | FILE_FLAG_OPEN_NO_RECALL,
            )
            .open(OsString::from_wide(&volume))?;
        check_local_ntfs(&directory)?;
        check_type(&directory, true)?;
        let name = normalized_name(&directory)?;
        let opened_volume = String::from_utf16(&name).map_err(|_| denied())?;
        let mapped_volume = String::from_utf16(&volume).map_err(|_| denied())?;
        if !opened_volume.eq_ignore_ascii_case(&mapped_volume) {
            return Err(denied());
        }

        // Keep the chain stable until the approved directory has been opened and checked.
        let mut ancestors = Vec::new();
        let mut root = Self { directory, name };
        for name in names {
            let (directory, name) = open_child(&root.directory, &root.name, name, true)?;
            ancestors.push(root.directory);
            root = Self { directory, name };
        }
        Ok(root)
    }

    pub(super) fn open_file(&self, relative: &str) -> io::Result<OpenedFile<'_>> {
        let mut names = relative.split('/').peekable();
        let mut ancestors = Vec::new();
        let mut parent_name = self.name.clone();
        while let Some(name) = names.next() {
            let parent = ancestors.last().unwrap_or(&self.directory);
            let directory = names.peek().is_some();
            let (file, full_name) = open_child(parent, &parent_name, name, directory)?;
            if !directory {
                return Ok(OpenedFile {
                    file,
                    name: full_name,
                    _ancestors: ancestors,
                    _root: &self.directory,
                });
            }
            ancestors.push(file);
            parent_name = full_name;
        }
        Err(io::ErrorKind::InvalidInput.into())
    }
}

pub(super) struct OpenedFile<'a> {
    file: File,
    name: Vec<u16>,
    _ancestors: Vec<File>,
    _root: &'a File,
}

impl Read for OpenedFile<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        check_type(&self.file, false)?;
        check_single_link(&self.file)?;
        // Higher directories outside the mapped root need not stay open for its lifetime.
        if normalized_name(&self.file)? != self.name {
            return Err(denied());
        }
        self.file.read(buffer)
    }
}

fn root_components(path: &Path) -> io::Result<(u16, Vec<&str>)> {
    let path = path.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    let (path, separators): (_, &[_]) = match path.strip_prefix(r"\\?\") {
        Some(path) => (path, &['\\']),
        None => (path, &['\\', '/']),
    };
    let [drive, b':', separator, ..] = path.as_bytes() else {
        return Err(io::ErrorKind::InvalidInput.into());
    };
    if !drive.is_ascii_alphabetic() || !separators.contains(&char::from(*separator)) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let path = &path[3..];
    let names: Vec<_> = path.split_terminator(separators).collect();
    if names.iter().any(|name| !valid_component(name)) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok((u16::from(drive.to_ascii_uppercase()), names))
}

fn volume_name(drive: u16) -> io::Result<Vec<u16>> {
    let drive = [drive, b':' as u16, b'\\' as u16, 0];
    let mut volume = [0u16; 64];
    // SAFETY: The drive name is terminated and the output buffer is writable.
    if unsafe {
        GetVolumeNameForVolumeMountPointW(drive.as_ptr(), volume.as_mut_ptr(), volume.len() as u32)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let length = volume
        .iter()
        .position(|&unit| unit == 0)
        .ok_or_else(denied)?;
    Ok(volume[..length].to_vec())
}

fn check_local_ntfs(file: &File) -> io::Result<()> {
    let mut status = IO_STATUS_BLOCK::default();
    let mut device = FILE_FS_DEVICE_INFORMATION::default();
    // SAFETY: The handle is borrowed and both outputs are valid for this synchronous call.
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
        return Err(denied());
    }
    let mut filesystem = [0u16; 32];
    // SAFETY: The handle is borrowed and the only requested output is the writable name buffer.
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
        return Err(io::Error::last_os_error());
    }
    if filesystem[..5] != [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0] {
        return Err(denied());
    }
    Ok(())
}

fn open_child(
    parent: &File,
    parent_name: &[u16],
    name: &str,
    directory: bool,
) -> io::Result<(File, Vec<u16>)> {
    let mut name: Vec<u16> = name.encode_utf16().collect();
    let length =
        u16::try_from(size_of_val(name.as_slice())).map_err(|_| io::ErrorKind::InvalidInput)?;
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
    // Deny write and delete sharing while this descendant handle remains open.
    let (access, options) = if directory {
        // The native directory-only option conflicts with no-reparse options. Verify the
        // directory attribute on the same handle before using it as a parent.
        (DIRECTORY_ACCESS, 0)
    } else {
        (
            FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_NON_DIRECTORY_FILE,
        )
    };
    let mut handle = null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    // Open a single component with FILE_OPEN_REPARSE_POINT so the filesystem does not follow
    // a link at that component. OBJ_DONT_REPARSE also refuses any reparse requested by a filter.
    // SAFETY: The parent is a checked directory. The single component, attributes and outputs
    // remain live for this synchronous open. No creation or mutation is requested.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status,
            null(),
            0,
            FILE_SHARE_READ,
            FILE_OPEN,
            options | FILE_OPEN_REPARSE_POINT | FILE_OPEN_NO_RECALL | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    check_status(result)?;
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(denied());
    }
    // SAFETY: A successful NtCreateFile produced a new owned, synchronous file handle.
    let file = unsafe { File::from_raw_handle(handle) };
    check_type(&file, directory)?;
    if !directory {
        check_single_link(&file)?;
    }

    // The normalized handle name expands any 8.3 alias and preserves the spelling on disk.
    let normalized = normalized_name(&file)?;
    let mut expected = parent_name.to_vec();
    if expected.last() != Some(&(b'\\' as u16)) {
        expected.push(b'\\' as u16);
    }
    expected.extend(name);
    if normalized != expected {
        return Err(denied());
    }
    Ok((file, normalized))
}

fn check_type(file: &File, directory: bool) -> io::Result<()> {
    // SAFETY: The handle is live and borrowed.
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
        return Err(denied());
    }
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: The live handle and correctly sized FileAttributeTagInfo output remain valid.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if info.FileAttributes & UNSUPPORTED_ATTRIBUTES != 0
        || info.ReparseTag != 0
        || directory != (info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0)
    {
        return Err(denied());
    }
    Ok(())
}

fn check_single_link(file: &File) -> io::Result<()> {
    // NtQueryInformationFile requires 8-byte alignment for FileStandardLinkInformation.
    #[repr(align(8))]
    #[derive(Default)]
    struct AlignedLinks(FILE_STANDARD_LINK_INFORMATION);

    let mut status = IO_STATUS_BLOCK::default();
    let mut links = AlignedLinks::default();
    // SAFETY: The handle is borrowed and both outputs are valid for this synchronous call.
    let result = unsafe {
        NtQueryInformationFile(
            file.as_raw_handle(),
            &mut status,
            (&mut links.0 as *mut FILE_STANDARD_LINK_INFORMATION).cast(),
            size_of::<FILE_STANDARD_LINK_INFORMATION>() as u32,
            FileStandardLinkInformation,
        )
    };
    check_status(result)?;
    let info = links.0;
    if status.Information != size_of::<FILE_STANDARD_LINK_INFORMATION>()
        || info.Directory
        || info.DeletePending
        || info.NumberOfAccessibleLinks != 1
        || info.TotalNumberOfLinks != 1
    {
        return Err(denied());
    }
    Ok(())
}

fn normalized_name(file: &File) -> io::Result<Vec<u16>> {
    let mut name = vec![0u16; 260];
    loop {
        // SAFETY: The handle is borrowed and the buffer holds the advertised number of WCHARs.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                name.as_mut_ptr(),
                name.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_GUID,
            )
        } as usize;
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length < name.len() {
            name.truncate(length);
            return Ok(name);
        }
        if length == name.len() || length > 32_768 {
            return Err(denied());
        }
        name.resize(length, 0);
    }
}

fn check_status(status: i32) -> io::Result<()> {
    if status == STATUS_SUCCESS {
        return Ok(());
    }
    // SAFETY: Translating an NTSTATUS has no pointer or handle arguments.
    Err(io::Error::from_raw_os_error(
        unsafe { RtlNtStatusToDosError(status) } as i32,
    ))
}
