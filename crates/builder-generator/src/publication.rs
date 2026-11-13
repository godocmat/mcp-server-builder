//! Local I/O for export plans. The operator selects a trusted local parent.
//! Keep generated paths portable and publish only through a no-replace rename.

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use builder_project::is_portable_path;
use unicode_normalization::UnicodeNormalization;

use crate::ExportError;

#[cfg(target_os = "linux")]
#[path = "publication/linux.rs"]
mod platform;
#[cfg(windows)]
#[path = "publication/windows.rs"]
mod platform;
#[cfg(not(any(target_os = "linux", windows)))]
#[path = "publication/unsupported.rs"]
mod platform;

use platform::{Directory, Node};

const STAGE_PREFIX: &str = ".mcp-export-stage-";
const MAX_ENTRIES: usize = 4096;
const MAX_DEPTH: usize = 64;
const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TREE_BYTES: usize = 128 * 1024 * 1024;

pub(crate) fn validate_paths<'a>(
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<(), ExportError> {
    let mut entries = BTreeMap::new();
    for path in paths {
        validate_relative(path)?;
        if is_reserved_path(path) {
            return Err(ExportError::UnsafePath);
        }
        let mut end = 0;
        for name in path.split('/') {
            end += name.len();
            let prefix = &path[..end];
            let directory = end < path.len();
            match entries.entry(portable_key(prefix)) {
                Entry::Vacant(entry) => {
                    entry.insert((prefix, directory));
                }
                Entry::Occupied(entry) if directory && *entry.get() == (prefix, true) => {}
                Entry::Occupied(_) => return Err(ExportError::UnsafePath),
            }
            if entries.len() > MAX_ENTRIES {
                return Err(ExportError::InputTooLarge);
            }
            end += 1;
        }
    }
    Ok(())
}

pub(crate) fn is_reserved_path(path: &str) -> bool {
    let first = path.split('/').next().unwrap_or_default();
    let key = portable_key(first);
    matches!(
        key.as_str(),
        "TARGET" | ".GIT" | "RUNTIME-CONFIG.JSON" | "RUNTIME-CONFIG.LOCAL.JSON" | ".ENV"
    ) || key.starts_with(".ENV.")
        || is_stage_name(first)
}

pub(crate) fn check_destination(path: &Path) -> Result<PathBuf, ExportError> {
    let destination = Destination::open(path)?;
    Ok(destination.parent.path().join(destination.name))
}

pub(crate) fn checked_root(path: &Path) -> Result<PathBuf, ExportError> {
    Ok(Directory::open(&absolute_path(path)?)?.path().to_path_buf())
}

pub(crate) fn publish(
    destination: &Path,
    files: &BTreeMap<String, Vec<u8>>,
    recheck: impl FnOnce() -> Result<(), ExportError>,
) -> Result<(), ExportError> {
    validate_paths(files.keys().map(String::as_str))?;
    let mut bytes_left = MAX_TREE_BYTES;
    for bytes in files.values() {
        check_size(bytes.len(), &mut bytes_left)?;
    }

    let destination = Destination::open(destination)?;
    let mut stage_builder = tempfile::Builder::new();
    stage_builder.prefix(STAGE_PREFIX).rand_bytes(16);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        stage_builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let stage = stage_builder
        .tempdir_in(destination.parent.access_path())
        .map_err(storage)?;
    let result = (|| {
        let stage_name = leaf_name(stage.path())?;
        let directory = destination.parent.open_stage(stage_name)?;
        write_stage(&directory, files)?;
        recheck()?;
        destination
            .parent
            .publish(&directory, stage_name, &destination.name)
    })();
    match result {
        Ok(()) => {
            let _ = stage.keep();
            Ok(())
        }
        Err(error) => {
            stage.close().map_err(storage)?;
            Err(error)
        }
    }
}

struct Destination {
    parent: Directory,
    name: String,
}

impl Destination {
    fn open(path: &Path) -> Result<Self, ExportError> {
        leaf_name(path)?;
        let absolute = absolute_path(path)?;
        let name = leaf_name(&absolute)?.to_owned();
        if is_stage_name(&name) {
            return Err(ExportError::UnsafePath);
        }
        let parent = Directory::open(absolute.parent().ok_or(ExportError::UnsafePath)?)?;
        parent.check_absent(&name)?;
        Ok(Self { parent, name })
    }
}

fn write_stage(
    directory: &Directory,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ExportError> {
    let root = directory.access_path();
    let mut directories = BTreeSet::new();
    for (relative, bytes) in files {
        for (end, _) in relative.match_indices('/') {
            let prefix = &relative[..end];
            if directories.insert(prefix) {
                fs::create_dir(root.join(prefix)).map_err(storage)?;
            }
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(relative))
            .map_err(storage)?;
        file.write_all(bytes).map_err(storage)?;
        file.sync_all().map_err(storage)?;
    }
    #[cfg(target_os = "linux")]
    {
        // Flush directory entries bottom-up after all contents have been written.
        for relative in directories.into_iter().rev() {
            File::open(root.join(relative))
                .and_then(|directory| directory.sync_all())
                .map_err(storage)?;
        }
        directory.sync()?;
    }
    Ok(())
}

pub(crate) fn read_file(path: &Path, max_bytes: usize) -> Result<Vec<u8>, ExportError> {
    leaf_name(path)?;
    let absolute = absolute_path(path)?;
    let parent = Directory::open(absolute.parent().ok_or(ExportError::UnsafePath)?)?;
    let Node::File(file) = parent.open_child(leaf_name(&absolute)?)? else {
        return Err(ExportError::UnsafePath);
    };
    file.read(max_bytes)
}

pub(crate) fn read_tree(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, ExportError> {
    let root = Directory::open(&absolute_path(root)?)?;
    let mut tree = Tree {
        files: BTreeMap::new(),
        entries_left: MAX_ENTRIES,
        bytes_left: MAX_TREE_BYTES,
    };
    tree.scan(&root, "")?;
    Ok(tree.files)
}

struct Tree {
    files: BTreeMap<String, Vec<u8>>,
    entries_left: usize,
    bytes_left: usize,
}

impl Tree {
    fn scan(&mut self, directory: &Directory, prefix: &str) -> Result<(), ExportError> {
        let mut names = BTreeSet::new();
        for entry in fs::read_dir(directory.access_path()).map_err(storage)? {
            self.entries_left = self
                .entries_left
                .checked_sub(1)
                .ok_or(ExportError::InputTooLarge)?;
            let entry = entry.map_err(storage)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ExportError::UnsafePath)?;
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            validate_relative(&relative)?;
            if !names.insert(portable_key(&name)) {
                return Err(ExportError::UnsafePath);
            }
            let node = directory.open_child(&name)?;
            if is_reserved_path(&relative) {
                if is_stage_name(&name)
                    || (name.eq_ignore_ascii_case("target") && matches!(node, Node::File(_)))
                {
                    return Err(ExportError::UnsafePath);
                }
                continue;
            }
            match node {
                Node::Directory(child) => self.scan(&child, &relative)?,
                Node::File(file) => {
                    let bytes = file.read(MAX_FILE_BYTES.min(self.bytes_left))?;
                    check_size(bytes.len(), &mut self.bytes_left)?;
                    self.files.insert(relative, bytes);
                }
            }
        }
        directory.verify()
    }
}

fn validate_relative(path: &str) -> Result<(), ExportError> {
    if is_portable_path(path) && path.split('/').count() <= MAX_DEPTH {
        Ok(())
    } else {
        Err(ExportError::UnsafePath)
    }
}

fn portable_key(path: &str) -> String {
    path.nfc().flat_map(char::to_uppercase).nfc().collect()
}

fn is_stage_name(name: &str) -> bool {
    name.get(..STAGE_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(STAGE_PREFIX))
}

fn portable_component(name: &str) -> bool {
    !name.contains('/') && is_portable_path(name)
}

fn leaf_name(path: &Path) -> Result<&str, ExportError> {
    let text = path.to_str().ok_or(ExportError::UnsafePath)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(ExportError::UnsafePath)?;
    #[cfg(windows)]
    let text = text.trim_end_matches(['/', '\\']);
    #[cfg(not(windows))]
    let text = text.trim_end_matches('/');
    if text.ends_with(name) && portable_component(name) {
        Ok(name)
    } else {
        Err(ExportError::UnsafePath)
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, ExportError> {
    if path.as_os_str().is_empty() {
        return Err(ExportError::UnsafePath);
    }
    let absolute = path.is_absolute();
    let mut resolved = if absolute {
        PathBuf::new()
    } else {
        std::env::current_dir().map_err(storage)?
    };
    let mut can_go_up = !absolute;
    for component in path.components() {
        match component {
            #[cfg(windows)]
            Component::Prefix(prefix)
                if absolute
                    && matches!(
                        prefix.kind(),
                        std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                    ) =>
            {
                resolved.push(prefix.as_os_str());
            }
            Component::RootDir if absolute => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir if can_go_up && resolved.pop() => {}
            Component::Normal(name) if name.to_str().is_some_and(portable_component) => {
                can_go_up = false;
                resolved.push(name);
            }
            _ => return Err(ExportError::UnsafePath),
        }
    }
    Ok(resolved)
}

fn read_bounded(file: &File, max_bytes: usize) -> Result<Vec<u8>, ExportError> {
    if file.metadata().map_err(storage)?.len() > max_bytes as u64 {
        return Err(ExportError::InputTooLarge);
    }
    let mut bytes = Vec::new();
    file.take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(storage)?;
    if bytes.len() > max_bytes {
        return Err(ExportError::InputTooLarge);
    }
    Ok(bytes)
}

fn check_size(size: usize, bytes_left: &mut usize) -> Result<(), ExportError> {
    if size > MAX_FILE_BYTES || size > *bytes_left {
        return Err(ExportError::InputTooLarge);
    }
    *bytes_left -= size;
    Ok(())
}

fn storage(_: io::Error) -> ExportError {
    ExportError::Storage
}

#[cfg(all(test, any(target_os = "linux", windows)))]
mod tests;
