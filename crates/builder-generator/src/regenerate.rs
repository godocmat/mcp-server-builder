use std::collections::BTreeMap;
use std::path::Path;

use builder_project::{ManifestRevision, ValidatedManifest};
use serde::{Deserialize, Serialize};

use crate::{
    ExportError, FileOwner, GENERATION_NAME, GenerationRecord, OutputFile, OutputFiles, publication,
};

const MAX_GENERATION_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictDecision {
    Keep,
    Regenerate,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictResolution {
    pub path: String,
    pub observed_digest: Option<ManifestRevision>,
    pub replacement_digest: Option<ManifestRevision>,
    pub decision: ConflictDecision,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportConflict {
    pub path: String,
    pub previous_digest: Option<ManifestRevision>,
    pub observed_digest: Option<ManifestRevision>,
    pub replacement_digest: Option<ManifestRevision>,
    pub can_keep: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolutionFile {
    version: u32,
    resolutions: Vec<ConflictResolution>,
}

pub(crate) fn parse_resolutions(bytes: &[u8]) -> Result<Vec<ConflictResolution>, ExportError> {
    let file: ResolutionFile =
        serde_json::from_slice(bytes).map_err(|_| ExportError::InvalidResolution)?;
    if file.version != 1 {
        return Err(ExportError::InvalidResolution);
    }
    Ok(file.resolutions)
}

pub(crate) type TreeDigests = BTreeMap<String, ManifestRevision>;

pub(crate) fn tree_digests(tree: &BTreeMap<String, Vec<u8>>) -> TreeDigests {
    tree.iter()
        .map(|(path, bytes)| (path.clone(), ManifestRevision::of(bytes)))
        .collect()
}

pub(crate) fn apply(
    root: &Path,
    manifest: &ValidatedManifest,
    output: &mut OutputFiles,
    resolutions: &[ConflictResolution],
) -> Result<(TreeDigests, Vec<ExportConflict>), ExportError> {
    let mut tree = publication::read_tree(root)?;
    let observed = tree_digests(&tree);
    let record = tree
        .remove(GENERATION_NAME)
        .ok_or(ExportError::InvalidPreviousExport)?;
    if record.len() > MAX_GENERATION_BYTES {
        return Err(ExportError::InputTooLarge);
    }
    let record = GenerationRecord::parse(&record, manifest.document().project_id)?;
    let mut resolutions_by_path = BTreeMap::new();
    for resolution in resolutions {
        if resolutions_by_path
            .insert(resolution.path.as_str(), resolution)
            .is_some()
        {
            return Err(ExportError::InvalidResolution);
        }
    }

    let mut conflicts = Vec::new();
    for prior in record.files {
        let current = tree.remove(&prior.path);
        if prior.owner == FileOwner::Project {
            if let Some(bytes) = current {
                keep(output, prior.path, bytes);
            } else if output.contains_key(&prior.path) {
                review(
                    output,
                    &mut conflicts,
                    &mut resolutions_by_path,
                    prior.path,
                    Some(prior.digest),
                    None,
                )?;
            }
        } else if observed.get(&prior.path) != Some(&prior.digest) {
            review(
                output,
                &mut conflicts,
                &mut resolutions_by_path,
                prior.path,
                Some(prior.digest),
                current,
            )?;
        }
    }
    // Untracked content may include local configuration. The user chooses individual files.
    for (path, bytes) in tree {
        review(
            output,
            &mut conflicts,
            &mut resolutions_by_path,
            path,
            None,
            Some(bytes),
        )?;
    }
    if !resolutions_by_path.is_empty() {
        return Err(ExportError::InvalidResolution);
    }
    conflicts.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    Ok((observed, conflicts))
}

fn review(
    output: &mut OutputFiles,
    conflicts: &mut Vec<ExportConflict>,
    resolutions: &mut BTreeMap<&str, &ConflictResolution>,
    path: String,
    previous_digest: Option<ManifestRevision>,
    current: Option<Vec<u8>>,
) -> Result<(), ExportError> {
    let observed_digest = current.as_deref().map(ManifestRevision::of);
    let replacement_digest = output
        .get(&path)
        .map(|file| ManifestRevision::of(&file.bytes));
    let can_keep = current.as_ref().is_some_and(|bytes| {
        path != builder_project::MANIFEST_NAME || builder_project::parse_manifest(bytes).is_ok()
    });
    let Some(choice) = resolutions.remove(path.as_str()) else {
        conflicts.push(ExportConflict {
            path,
            previous_digest,
            observed_digest,
            replacement_digest,
            can_keep,
        });
        return Ok(());
    };
    if observed_digest != choice.observed_digest || replacement_digest != choice.replacement_digest
    {
        return Err(ExportError::InvalidResolution);
    }
    if choice.decision == ConflictDecision::Keep {
        if !can_keep {
            return Err(ExportError::InvalidResolution);
        }
        keep(output, path, current.ok_or(ExportError::InvalidResolution)?);
    }
    Ok(())
}

fn keep(output: &mut OutputFiles, path: String, bytes: Vec<u8>) {
    output.insert(
        path,
        OutputFile {
            owner: FileOwner::Project,
            bytes,
        },
    );
}
