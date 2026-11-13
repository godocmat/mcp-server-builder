use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::AsRawHandle};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use sha2::{Digest, Sha256};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_VIRTUAL, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_TYPE_DISK, FileIdInfo,
    GetFileInformationByHandle, GetFileInformationByHandleEx, GetFileType,
    GetFinalPathNameByHandleW, GetVolumeInformationByHandleW, GetVolumeNameForVolumeMountPointW,
    SYNCHRONIZE, VOLUME_NAME_DOS, VOLUME_NAME_GUID, VOLUME_NAME_NT,
};

use super::{MAX_WINDOWS_UNITS, compare_names, invalid_data, wide};

const BACKSLASH: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;
const COLON: u16 = b':' as u16;
const DOT: u16 = b'.' as u16;
const VERBATIM_PREFIX: [u16; 4] = [BACKSLASH, BACKSLASH, b'?' as u16, BACKSLASH];
const UNSUPPORTED_ATTRIBUTES: u32 = FILE_ATTRIBUTE_DEVICE
    | FILE_ATTRIBUTE_OFFLINE
    | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
    | FILE_ATTRIBUTE_RECALL_ON_OPEN
    | FILE_ATTRIBUTE_REPARSE_POINT
    | FILE_ATTRIBUTE_VIRTUAL;

pub(super) struct LockedPath {
    file: File,
    // Retain every directory below the volume root until CreateProcessW has opened both paths.
    _ancestors: Vec<File>,
    pub(super) application_path: Vec<u16>,
    pub(super) display_path: PathBuf,
    pub(super) identity: [u8; 32],
}

impl LockedPath {
    pub(super) fn open(path: &Path, directory: bool) -> io::Result<Self> {
        let selected = wide(path.as_os_str())?;
        let (volume, components) = components(&selected)?;
        let mut full_path = volume.clone();
        let mut file = open_component(&full_path, true, true)?;
        verify_ntfs(&file)?;
        if directory && components.is_empty() {
            // Pin the selected volume root with the same sharing rules as other cwd targets.
            file = open_component(&full_path, true, false)?;
        }
        let normalized_volume = final_name(&file, VOLUME_NAME_GUID)?;
        if !compare_names(&normalized_volume, &volume)?.is_eq() {
            return Err(invalid_data());
        }
        full_path = normalized_volume;

        let mut ancestors = Vec::new();
        let mut components = components.into_iter().peekable();
        if !directory && components.peek().is_none() {
            return Err(invalid_data());
        }
        while let Some(component) = components.next() {
            if full_path.last() != Some(&BACKSLASH) {
                full_path.push(BACKSLASH);
            }
            full_path.extend_from_slice(component);
            ancestors.push(file);
            file = open_component(&full_path, directory || components.peek().is_some(), false)?;
            let normalized = final_name(&file, VOLUME_NAME_GUID)?;
            if !compare_names(&normalized, &full_path)?.is_eq() {
                return Err(invalid_data());
            }
            full_path = normalized;
        }

        let display_path = PathBuf::from(OsString::from_wide(&final_name(&file, VOLUME_NAME_DOS)?));
        let identity = file_identity(&file)?;
        Ok(Self {
            file,
            _ancestors: ancestors,
            application_path: full_path,
            display_path,
            identity,
        })
    }

    pub(super) fn image_name(&self) -> io::Result<Vec<u16>> {
        final_name(&self.file, VOLUME_NAME_NT)
    }

    pub(super) fn has_current_identity(&self, directory: bool) -> io::Result<bool> {
        let current = open_component(&self.application_path, directory, false)?;
        Ok(file_identity(&current)? == self.identity)
    }

    pub(super) fn executable_digest(&mut self) -> io::Result<[u8; 32]> {
        if !is_exe(&self.display_path) {
            return Err(invalid_data());
        }
        let mut dos_header = [0; 64];
        self.file.read_exact(&mut dos_header)?;
        if &dos_header[..2] != b"MZ" {
            return Err(invalid_data());
        }
        let pe_offset = u32::from_le_bytes(dos_header[60..64].try_into().unwrap());
        self.file.seek(SeekFrom::Start(u64::from(pe_offset)))?;
        let mut pe_signature = [0; 4];
        self.file.read_exact(&mut pe_signature)?;
        if &pe_signature != b"PE\0\0" {
            return Err(invalid_data());
        }

        self.file.rewind()?;
        let mut digest = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let count = self.file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        Ok(digest.finalize().into())
    }
}

pub(super) fn is_exe(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

fn components(path: &[u16]) -> io::Result<(Vec<u16>, Vec<&[u16]>)> {
    const VOLUME_PREFIX: [u16; 7] = [
        b'V' as u16,
        b'o' as u16,
        b'l' as u16,
        b'u' as u16,
        b'm' as u16,
        b'e' as u16,
        b'{' as u16,
    ];
    const GUID_UNITS: usize = 36;
    const GUID_END: usize = VOLUME_PREFIX.len() + GUID_UNITS;
    const VOLUME_ROOT_LEN: usize = GUID_END + 2;
    let verbatim = path.starts_with(&VERBATIM_PREFIX);
    let is_separator = |unit: u16| unit == BACKSLASH || (!verbatim && unit == SLASH);
    let selected = path.strip_prefix(&VERBATIM_PREFIX).unwrap_or(path);
    let (volume, tail) = if let [drive, COLON, separator, tail @ ..] = selected
        && matches!(*drive, 65..=90 | 97..=122)
        && is_separator(*separator)
    {
        (
            volume_path(u16::from((*drive as u8).to_ascii_uppercase()))?,
            tail,
        )
    } else if verbatim
        && selected.len() >= VOLUME_ROOT_LEN
        && compare_names(&selected[..VOLUME_PREFIX.len()], &VOLUME_PREFIX)?.is_eq()
        && selected[GUID_END..VOLUME_ROOT_LEN] == [b'}' as u16, BACKSLASH]
        && is_volume_guid(&selected[VOLUME_PREFIX.len()..GUID_END])
    {
        (
            path[..VERBATIM_PREFIX.len() + VOLUME_ROOT_LEN].to_vec(),
            &selected[VOLUME_ROOT_LEN..],
        )
    } else {
        return Err(invalid_data());
    };
    if tail.is_empty() {
        return Ok((volume, Vec::new()));
    }
    let tail = if is_separator(*tail.last().unwrap()) {
        &tail[..tail.len() - 1]
    } else {
        tail
    };
    let components: Vec<_> = tail.split(|unit| is_separator(*unit)).collect();
    if components
        .iter()
        .any(|component| !valid_component(component))
    {
        return Err(invalid_data());
    }
    Ok((volume, components))
}

fn is_volume_guid(units: &[u16]) -> bool {
    units.iter().enumerate().all(|(index, &unit)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            unit == b'-' as u16
        } else {
            matches!(unit, 48..=57 | 65..=70 | 97..=102)
        }
    })
}

fn valid_component(component: &[u16]) -> bool {
    !component.is_empty()
        && component.len() <= 255
        && component
            .last()
            .is_some_and(|&unit| unit != DOT && unit != 32)
        && component
            .iter()
            .all(|&unit| !matches!(unit, 0..=31 | 34 | 42 | SLASH | COLON | 60 | 62 | 63 | 124))
}

fn volume_path(drive: u16) -> io::Result<Vec<u16>> {
    let drive = [drive, COLON, BACKSLASH, 0];
    let mut volume = [0; 64];
    // SAFETY: The drive root is terminated and the output is writable.
    if unsafe {
        GetVolumeNameForVolumeMountPointW(drive.as_ptr(), volume.as_mut_ptr(), volume.len() as u32)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let length = volume
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(invalid_data)?;
    Ok(volume[..length].to_vec())
}

fn open_component(path: &[u16], directory: bool, volume: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.custom_flags(
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
    );
    // Exclude writing and deletion while the handles are held. The volume root is
    // reached by GUID and is a stable namespace anchor.
    options.share_mode(if volume {
        FILE_SHARE_READ | FILE_SHARE_WRITE
    } else {
        FILE_SHARE_READ
    });
    if directory {
        options.access_mode(FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE);
    } else {
        options.read(true);
    }
    let file = options.open(OsString::from_wide(path))?;
    let info = file_info(&file)?;
    if info.dwFileAttributes & UNSUPPORTED_ATTRIBUTES != 0
        || directory != (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0)
        || (!directory && info.nNumberOfLinks != 1)
    {
        return Err(invalid_data());
    }
    Ok(file)
}

fn verify_ntfs(file: &File) -> io::Result<()> {
    let mut name = [0; 16];
    // SAFETY: The volume directory handle is live and the only requested output is
    // the writable filesystem name buffer.
    if unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if OsString::from_wide(&name[..4]) != OsStr::new("NTFS") || name[4] != 0 {
        return Err(invalid_data());
    }
    Ok(())
}

fn file_info(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: The file handle is live and the output is writable.
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
        return Err(invalid_data());
    }
    // SAFETY: The file handle is live and the output is writable.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}

fn file_identity(file: &File) -> io::Result<[u8; 32]> {
    let info = file_info(file)?;
    let mut id = FILE_ID_INFO::default();
    // SAFETY: The live handle and correctly sized FileIdInfo output remain valid.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut identity = [0; 32];
    identity[..8].copy_from_slice(&id.VolumeSerialNumber.to_le_bytes());
    identity[8..24].copy_from_slice(&id.FileId.Identifier);
    identity[24..28].copy_from_slice(&info.ftCreationTime.dwLowDateTime.to_le_bytes());
    identity[28..].copy_from_slice(&info.ftCreationTime.dwHighDateTime.to_le_bytes());
    Ok(identity)
}

fn final_name(file: &File, volume_format: u32) -> io::Result<Vec<u16>> {
    let mut name = vec![0; 260];
    loop {
        // SAFETY: The file handle is live and the output is writable.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                name.as_mut_ptr(),
                name.len() as u32,
                volume_format,
            )
        } as usize;
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length < name.len() {
            name.truncate(length);
            return Ok(name);
        }
        if length == name.len() || length > MAX_WINDOWS_UNITS {
            return Err(invalid_data());
        }
        name.resize(length, 0);
    }
}
