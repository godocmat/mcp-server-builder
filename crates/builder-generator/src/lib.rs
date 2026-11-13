//! Standalone export planning, rendering and regeneration.

mod publication;
mod record;
mod regenerate;
mod template;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use builder_project::{
    MANIFEST_NAME, MAX_MANIFEST_BYTES, ManifestError, ManifestRevision, ValidatedManifest,
    parse_manifest,
};

pub use record::{FileOwner, GeneratedFile, GenerationRecord};
pub use regenerate::{ConflictDecision, ConflictResolution, ExportConflict};
use serde::Serialize;

const GENERATION_NAME: &str = "generation.json";
const MAX_RESOLUTIONS_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub struct ExportRequest {
    pub project_root: PathBuf,
    pub destination: PathBuf,
    pub previous_export: Option<PathBuf>,
    pub resolutions: Vec<ConflictResolution>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPlan {
    pub generation: Option<GenerationRecord>,
    pub conflicts: Vec<ExportConflict>,
}

#[derive(Debug)]
pub enum ExportError {
    InvalidProject(ManifestError),
    InvalidPreviousExport,
    InvalidResolution,
    Conflict(Vec<ExportConflict>),
    InputChanged,
    ProjectMissing,
    DestinationExists,
    UnsafePath,
    InputTooLarge,
    Storage,
    Internal,
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidProject(_) => "The project manifest is invalid.",
            Self::InvalidPreviousExport => "The previous export record is missing or invalid.",
            Self::InvalidResolution => "An export resolution is invalid or stale.",
            Self::Conflict(_) => "The previous export has changes that require review.",
            Self::InputChanged => "An input changed while preparing the export.",
            Self::ProjectMissing => "The project folder or manifest is missing.",
            Self::DestinationExists => "The export destination already exists.",
            Self::UnsafePath => "An export path or file is outside the supported profile.",
            Self::InputTooLarge => "An export input exceeds the supported size or entry limit.",
            Self::Storage => "The export files could not be read or written.",
            Self::Internal => "The standalone template could not be rendered.",
        })
    }
}

impl std::error::Error for ExportError {}

struct OutputFile {
    owner: FileOwner,
    bytes: Vec<u8>,
}

type OutputFiles = BTreeMap<String, OutputFile>;

/// Read versioned, digest-bound conflict choices without executing any project content.
pub fn read_resolutions(path: &Path) -> Result<Vec<ConflictResolution>, ExportError> {
    let bytes = publication::read_file(path, MAX_RESOLUTIONS_BYTES)?;
    regenerate::parse_resolutions(&bytes)
}

/// Inspect a deterministic export and any edits requiring operator review.
pub fn plan_export(request: &ExportRequest) -> Result<ExportPlan, ExportError> {
    let prepared = prepare(request)?;
    Ok(ExportPlan {
        generation: prepared.generation,
        conflicts: prepared.conflicts,
    })
}

/// Stage a complete export beside a new destination and recheck the source before publishing.
pub fn publish_export(request: &ExportRequest) -> Result<GenerationRecord, ExportError> {
    let prepared = prepare(request)?;
    let generation = prepared
        .generation
        .ok_or(ExportError::Conflict(prepared.conflicts))?;
    let mut files: BTreeMap<_, _> = prepared
        .files
        .into_iter()
        .map(|(path, file)| (path, file.bytes))
        .collect();
    files.insert(GENERATION_NAME.into(), pretty_json(&generation)?);
    publication::publish(&prepared.destination, &files, || {
        let source =
            read_manifest(&prepared.project_root).map_err(|_| ExportError::InputChanged)?;
        if ManifestRevision::of(&source) != prepared.source_revision {
            return Err(ExportError::InputChanged);
        }
        if let Some(root) = &prepared.previous_export {
            let current = publication::read_tree(root).map_err(|_| ExportError::InputChanged)?;
            if regenerate::tree_digests(&current) != prepared.previous_digests {
                return Err(ExportError::InputChanged);
            }
        }
        Ok(())
    })?;
    Ok(generation)
}

struct PreparedExport {
    destination: PathBuf,
    project_root: PathBuf,
    previous_export: Option<PathBuf>,
    source_revision: ManifestRevision,
    previous_digests: BTreeMap<String, ManifestRevision>,
    files: OutputFiles,
    generation: Option<GenerationRecord>,
    conflicts: Vec<ExportConflict>,
}

fn prepare(request: &ExportRequest) -> Result<PreparedExport, ExportError> {
    let destination = publication::check_destination(&request.destination)?;
    let project_root =
        publication::checked_root(&request.project_root).map_err(|error| match error {
            ExportError::Storage if missing(&request.project_root) => ExportError::ProjectMissing,
            error => error,
        })?;
    let source = read_manifest(&project_root)?;
    let manifest = parse_manifest(&source).map_err(ExportError::InvalidProject)?;
    let source_revision = manifest.revision();
    let mut canonical =
        serde_json::to_value(manifest.document()).map_err(|_| ExportError::Internal)?;
    canonical.sort_all_objects();
    let canonical_bytes = pretty_json(&canonical)?;
    let normalized = parse_manifest(&canonical_bytes).map_err(ExportError::InvalidProject)?;
    let mut files = render(&normalized)?;
    let previous_export = request
        .previous_export
        .as_deref()
        .map(publication::checked_root)
        .transpose()?;
    let (previous_digests, conflicts) = if let Some(root) = &previous_export {
        if destination.starts_with(root) {
            return Err(ExportError::UnsafePath);
        }
        regenerate::apply(root, &normalized, &mut files, &request.resolutions)?
    } else {
        if !request.resolutions.is_empty() {
            return Err(ExportError::InvalidResolution);
        }
        (BTreeMap::new(), Vec::new())
    };
    publication::validate_paths(files.keys().map(String::as_str).chain([GENERATION_NAME]))?;
    let generation = if conflicts.is_empty() {
        Some(GenerationRecord::new(&normalized, &files)?)
    } else {
        None
    };
    Ok(PreparedExport {
        destination,
        project_root,
        previous_export,
        source_revision,
        previous_digests,
        files,
        generation,
        conflicts,
    })
}

fn render(manifest: &ValidatedManifest) -> Result<OutputFiles, ExportError> {
    let mut files = BTreeMap::from([(
        MANIFEST_NAME.into(),
        OutputFile {
            owner: FileOwner::Generator,
            bytes: manifest.bytes().to_vec(),
        },
    )]);
    for file in template::render(manifest) {
        if files
            .insert(
                file.path.into(),
                OutputFile {
                    owner: file.owner,
                    bytes: file.bytes,
                },
            )
            .is_some()
        {
            return Err(ExportError::Internal);
        }
    }
    Ok(files)
}

fn read_manifest(root: &Path) -> Result<Vec<u8>, ExportError> {
    let path = root.join(MANIFEST_NAME);
    match publication::read_file(&path, MAX_MANIFEST_BYTES) {
        Err(ExportError::Storage) if missing(&path) => Err(ExportError::ProjectMissing),
        result => result,
    }
}

fn missing(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn pretty_json(value: &impl Serialize) -> Result<Vec<u8>, ExportError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|_| ExportError::Internal)?;
    bytes.push(b'\n');
    Ok(bytes)
}
