use std::path::Path;

use super::ExportError;

pub(super) struct Directory;

impl Directory {
    pub(super) fn open(_: &Path) -> Result<Self, ExportError> {
        Err(ExportError::Storage)
    }

    pub(super) fn path(&self) -> &Path {
        unreachable!()
    }

    pub(super) fn access_path(&self) -> &Path {
        unreachable!()
    }

    pub(super) fn verify(&self) -> Result<(), ExportError> {
        unreachable!()
    }

    pub(super) fn check_absent(&self, _: &str) -> Result<(), ExportError> {
        unreachable!()
    }

    pub(super) fn open_stage(&self, _: &str) -> Result<Self, ExportError> {
        unreachable!()
    }

    pub(super) fn open_child(&self, _: &str) -> Result<Node, ExportError> {
        unreachable!()
    }

    pub(super) fn publish(&self, _: &Directory, _: &str, _: &str) -> Result<(), ExportError> {
        unreachable!()
    }
}

#[allow(dead_code)]
pub(super) enum Node {
    Directory(Directory),
    File(RegularFile),
}

pub(super) struct RegularFile;

impl RegularFile {
    pub(super) fn read(&self, _: usize) -> Result<Vec<u8>, ExportError> {
        unreachable!()
    }
}
