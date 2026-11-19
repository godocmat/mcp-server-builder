use std::ffi::OsStr;
use std::io;
use std::path::Path;

use mcp_native_fs::macos::{Directory, Entry, RegularFile};

use super::denied;

pub(super) struct Root(Directory);

impl Root {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        Directory::open(path).map(Self)
    }

    pub(super) fn identity(&self) -> io::Result<[u8; 24]> {
        self.0.identity()
    }

    pub(super) fn open_file(&self, relative: &str) -> io::Result<RegularFile> {
        let mut opened_parent = None;
        let mut parts = relative.split('/').peekable();
        while let Some(component) = parts.next() {
            let parent = opened_parent.as_ref().unwrap_or(&self.0);
            match parent.open_child(OsStr::new(component))? {
                Entry::Directory(directory) if parts.peek().is_some() => {
                    opened_parent = Some(directory);
                }
                Entry::File(file) if parts.peek().is_none() => return Ok(file),
                _ => return Err(denied()),
            }
        }
        Err(io::ErrorKind::InvalidInput.into())
    }
}
