use builder_project::{MANIFEST_NAME, ManifestRevision, PROJECT_SCHEMA_VERSION, ValidatedManifest};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ExportError, GENERATION_NAME, OutputFiles, publication, template};

const VERSION: u32 = 1;
const PROTOCOL_VERSIONS: &[&str] = &["2026-07-28", "2025-11-25"];
const LOCKFILE_NAME: &str = "Cargo.lock";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileOwner {
    Generator,
    Project,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeneratedFile {
    pub path: String,
    pub owner: FileOwner,
    pub digest: ManifestRevision,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationRecord {
    pub version: u32,
    pub project_id: Uuid,
    pub project_schema_version: u32,
    pub snapshot_digest: ManifestRevision,
    pub generator_version: String,
    pub runtime_version: String,
    pub template_version: String,
    pub protocol_versions: Vec<String>,
    pub cargo_lock_digest: ManifestRevision,
    pub files: Vec<GeneratedFile>,
}

impl GenerationRecord {
    pub(crate) fn new(
        manifest: &ValidatedManifest,
        files: &OutputFiles,
    ) -> Result<Self, ExportError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Snapshot<'a> {
            version: u32,
            project: &'a builder_project::ProjectDocument,
            asset_digests: [ManifestRevision; 0],
        }
        let document = manifest.document();
        let mut snapshot = serde_json::to_value(Snapshot {
            version: VERSION,
            project: document,
            // Schema 1 maps native roots through operator config. It declares no assets.
            asset_digests: [],
        })
        .map_err(|_| ExportError::Internal)?;
        snapshot.sort_all_objects();
        let snapshot = serde_json::to_vec(&snapshot).map_err(|_| ExportError::Internal)?;
        let lock = files.get(LOCKFILE_NAME).ok_or(ExportError::Internal)?;
        Ok(Self {
            version: VERSION,
            project_id: document.project_id,
            project_schema_version: document.schema_version,
            snapshot_digest: ManifestRevision::of(&snapshot),
            generator_version: env!("CARGO_PKG_VERSION").into(),
            runtime_version: template::RUNTIME_VERSION.into(),
            template_version: template::TEMPLATE_VERSION.into(),
            protocol_versions: PROTOCOL_VERSIONS
                .iter()
                .map(|value| (*value).into())
                .collect(),
            cargo_lock_digest: ManifestRevision::of(&lock.bytes),
            files: files
                .iter()
                .map(|(path, file)| GeneratedFile {
                    path: path.clone(),
                    owner: file.owner,
                    digest: ManifestRevision::of(&file.bytes),
                })
                .collect(),
        })
    }

    pub(crate) fn parse(bytes: &[u8], project_id: Uuid) -> Result<Self, ExportError> {
        let invalid = || ExportError::InvalidPreviousExport;
        let record: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        if record.version != VERSION
            || record.project_schema_version != PROJECT_SCHEMA_VERSION
            || record.project_id != project_id
            || record.files.len() > 4096
            || record.protocol_versions.is_empty()
            || record
                .files
                .windows(2)
                .any(|pair| pair[0].path >= pair[1].path)
        {
            return Err(invalid());
        }
        publication::validate_paths(
            record
                .files
                .iter()
                .map(|file| file.path.as_str())
                .chain([GENERATION_NAME]),
        )
        .map_err(|_| invalid())?;
        if record
            .files
            .iter()
            .find(|file| file.path == LOCKFILE_NAME)
            .map(|file| file.digest)
            != Some(record.cargo_lock_digest)
            || !record.files.iter().any(|file| file.path == MANIFEST_NAME)
        {
            return Err(invalid());
        }
        Ok(record)
    }
}
