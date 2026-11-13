// Temporary fail-closed stub for building the REST integration before native code arrives.
use std::io;
use std::path::Path;

pub(super) struct Root;

impl Root {
    pub(super) fn open(_: &Path) -> io::Result<Self> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub(super) fn open_file(&self, _: &str) -> io::Result<std::fs::File> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
