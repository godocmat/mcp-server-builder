use builder_project::{ManifestRevision, SchemaVersion};
use serde::Serialize;
use uuid::Uuid;

use crate::RecoveryCandidate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DiagnosticCode {
    #[serde(rename = "cli.usage")]
    CliUsage,
    #[serde(rename = "input.invalid")]
    InvalidInput,
    #[serde(rename = "project.invalid")]
    ProjectInvalid,
    #[serde(rename = "project.unsupported_schema")]
    ProjectUnsupportedSchema,
    #[serde(rename = "project.json")]
    ProjectJson,
    #[serde(rename = "project.duplicate_key")]
    ProjectDuplicateKey,
    #[serde(rename = "project.invalid_field")]
    ProjectInvalidField,
    #[serde(rename = "project.unsupported_setting")]
    ProjectUnsupportedSetting,
    #[serde(rename = "project.duplicate_id")]
    ProjectDuplicateId,
    #[serde(rename = "project.invalid_reference")]
    ProjectInvalidReference,
    #[serde(rename = "project.invalid_schema")]
    ProjectInvalidSchema,
    #[serde(rename = "project.ambiguous_resource")]
    ProjectAmbiguousResource,
    #[serde(rename = "project.limit")]
    ProjectLimit,
    #[serde(rename = "project.missing")]
    ProjectMissing,
    #[serde(rename = "project.unsafe_path")]
    ProjectUnsafePath,
    #[serde(rename = "project.exists")]
    ProjectExists,
    #[serde(rename = "project.locked")]
    ProjectLocked,
    #[serde(rename = "project.revision_conflict")]
    ProjectRevisionConflict,
    #[serde(rename = "project.recovery_available")]
    ProjectRecoveryAvailable,
    #[serde(rename = "project.recovery_unavailable")]
    ProjectRecoveryUnavailable,
    #[serde(rename = "project.index_repair_required")]
    ProjectIndexRepairRequired,
    #[serde(rename = "export.conflict")]
    ExportConflict,
    #[serde(rename = "export.input_changed")]
    ExportInputChanged,
    #[serde(rename = "export.destination_exists")]
    ExportDestinationExists,
    #[serde(rename = "export.unsafe_path")]
    ExportUnsafePath,
    #[serde(rename = "export.invalid_previous")]
    ExportInvalidPrevious,
    #[serde(rename = "export.invalid_resolution")]
    ExportInvalidResolution,
    #[serde(rename = "export.limit")]
    ExportLimit,
    #[serde(rename = "storage.unavailable")]
    StorageUnavailable,
    #[serde(rename = "storage.durability_unconfirmed")]
    StorageDurabilityUnconfirmed,
    #[serde(rename = "internal.error")]
    InternalError,
    #[serde(untagged)]
    Playground(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum DiagnosticDetails {
    SupportedVersions {
        supported_versions: Vec<u32>,
    },
    UnsupportedProjectSchema {
        format: &'static str,
        schema_version: SchemaVersion,
        supported_versions: Vec<u32>,
    },
    CurrentRevision {
        current_revision: Option<ManifestRevision>,
    },
    CommittedRevision {
        committed_revision: ManifestRevision,
    },
    Recovery {
        current_revision: Option<ManifestRevision>,
        candidates: Vec<RecoveryCandidate>,
    },
    ExportConflicts {
        conflicts: Vec<builder_generator::ExportConflict>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub message: String,
    pub severity: Severity,
    pub correlation_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<DiagnosticDetails>,
}
