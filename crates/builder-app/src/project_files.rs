use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use builder_project::{
    MANIFEST_NAME, MAX_MANIFEST_BYTES, ManifestError, ManifestRevision, ValidatedManifest,
    parse_manifest,
};

use crate::local_fs;

const LOCK_NAME: &str = ".mcp-project.lock";
const BACKUP_NAME: &str = ".mcp-project.last-valid.json";
const STAGE_PREFIX: &str = ".mcp-project-stage-";
const MAX_RECOVERY_ENTRIES: usize = 32;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryCandidate {
    pub path: String,
    pub revision: ManifestRevision,
    pub kind: RecoveryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryKind {
    LastValid,
    InterruptedWrite,
}

pub(crate) struct CurrentFile {
    bytes: Vec<u8>,
    pub revision: ManifestRevision,
}

impl CurrentFile {
    pub fn validate(&self) -> Result<ValidatedManifest, ManifestError> {
        parse_manifest(&self.bytes)
    }
}

#[derive(Debug)]
pub(crate) enum FileError {
    Missing,
    Unsafe,
    Unavailable,
    Locked,
    TooLarge,
    Exists,
    Conflict(Option<ManifestRevision>),
    RecoveryUnavailable,
}

impl From<io::Error> for FileError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::Missing,
            io::ErrorKind::InvalidData => Self::Unsafe,
            io::ErrorKind::TimedOut => Self::Locked,
            _ => Self::Unavailable,
        }
    }
}

pub(crate) struct LockedRoot {
    pub path: PathBuf,
    // This guard stays live through the catalog transaction. The lock file is persistent.
    _lock: File,
    #[cfg(test)]
    before_publish: Option<Box<dyn FnOnce() -> io::Result<()> + Send>>,
}

pub(crate) fn canonical_root(path: &Path) -> Result<PathBuf, FileError> {
    // Use the native canonical spelling, including Windows' extended path prefix.
    let root = fs::canonicalize(path)?;
    let metadata = fs::symlink_metadata(&root)?;
    if !metadata.is_dir() || local_fs::is_link(&metadata) || root.to_str().is_none() {
        return Err(FileError::Unsafe);
    }
    #[cfg(windows)]
    if !matches!(
        root.components().next(),
        Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::VerbatimDisk(_) | std::path::Prefix::Disk(_))
    ) {
        return Err(FileError::Unsafe);
    }
    Ok(root)
}

impl LockedRoot {
    pub fn open(path: &Path) -> Result<Self, FileError> {
        let path = canonical_root(path)?;
        let lock = local_fs::lock(&path.join(LOCK_NAME))?;
        Ok(Self {
            path,
            _lock: lock,
            #[cfg(test)]
            before_publish: None,
        })
    }

    pub fn read(&self) -> Result<CurrentFile, FileError> {
        read(&self.path.join(MANIFEST_NAME))
    }

    pub fn create(&mut self, manifest: &ValidatedManifest) -> Result<bool, FileError> {
        let path = self.path.join(MANIFEST_NAME);
        if local_fs::file_exists(&path)? {
            return Err(FileError::Exists);
        }
        let stage = self.stage(manifest.bytes(), STAGE_PREFIX)?;
        local_fs::publish_file(stage, &path, false).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                FileError::Exists
            } else {
                error.into()
            }
        })?;
        Ok(local_fs::flush_directory(&self.path).is_ok())
    }

    pub fn replace(
        &mut self,
        current: &CurrentFile,
        prior_valid: Option<&ValidatedManifest>,
        manifest: &ValidatedManifest,
    ) -> Result<bool, FileError> {
        let manifest_path = self.path.join(MANIFEST_NAME);
        if manifest.revision() == current.revision {
            self.compare(current.revision)?;
            return Ok(true);
        }
        let staged = self.stage(manifest.bytes(), STAGE_PREFIX)?;
        self.compare(current.revision)?;
        if let Some(prior_valid) = prior_valid {
            let backup = self.stage(prior_valid.bytes(), ".mcp-project-backup-")?;
            let backup_path = self.path.join(BACKUP_NAME);
            if local_fs::file_exists(&backup_path)? {
                local_fs::read_file(&backup_path)?;
            }
            local_fs::publish_file(backup, &backup_path, true)?;
            local_fs::flush_directory(&self.path)?;
        }
        #[cfg(test)]
        if let Some(before_publish) = self.before_publish.take() {
            before_publish()?;
        }
        self.compare(current.revision)?;
        local_fs::publish_file(staged, &manifest_path, true)?;
        Ok(local_fs::flush_directory(&self.path).is_ok())
    }

    fn compare(&self, expected: ManifestRevision) -> Result<(), FileError> {
        match self.read() {
            Ok(current) if current.revision == expected => Ok(()),
            Ok(current) => Err(FileError::Conflict(Some(current.revision))),
            Err(FileError::Missing) => Err(FileError::Conflict(None)),
            Err(error) => Err(error),
        }
    }

    fn stage(&self, bytes: &[u8], prefix: &str) -> Result<tempfile::NamedTempFile, FileError> {
        let mut file = tempfile::Builder::new()
            .prefix(prefix)
            .rand_bytes(16)
            .tempfile_in(&self.path)?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        Ok(file)
    }
}

pub(crate) fn read(path: &Path) -> Result<CurrentFile, FileError> {
    let file = local_fs::read_file(path)?;
    let mut bytes = Vec::new();
    file.take((MAX_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(FileError::TooLarge);
    }
    let revision = ManifestRevision::of(&bytes);
    Ok(CurrentFile { bytes, revision })
}

pub(crate) fn read_backup(
    root: &Path,
    revision: ManifestRevision,
) -> Result<ValidatedManifest, FileError> {
    let backup = read(&root.join(BACKUP_NAME))
        .and_then(|file| file.validate().map_err(|_| FileError::RecoveryUnavailable))
        .map_err(|_| FileError::RecoveryUnavailable)?;
    if backup.revision() != revision {
        return Err(FileError::RecoveryUnavailable);
    }
    Ok(backup)
}

pub(crate) fn recovery_candidates(
    root: &Path,
    project_id: Option<uuid::Uuid>,
) -> Vec<RecoveryCandidate> {
    let mut candidates = Vec::new();
    let mut add = |path: &Path, kind| {
        if let Ok(file) = read(path)
            && let Ok(manifest) = file.validate()
            && project_id.is_none_or(|id| manifest.document().project_id == id)
        {
            candidates.push(RecoveryCandidate {
                path: dunce::simplified(path).to_string_lossy().into_owned(),
                revision: file.revision,
                kind,
            });
        }
    };
    add(&root.join(BACKUP_NAME), RecoveryKind::LastValid);
    if let Ok(entries) = fs::read_dir(root) {
        let mut staged = entries
            .take(4096)
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(STAGE_PREFIX))
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        staged.sort_unstable();
        for path in staged.iter().take(MAX_RECOVERY_ENTRIES) {
            add(path, RecoveryKind::InterruptedWrite);
        }
    }
    candidates
}

#[cfg(test)]
mod tests;
