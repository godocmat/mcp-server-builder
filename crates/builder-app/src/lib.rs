//! Shared application services and DTOs for the CLI and desktop shells.

mod catalog;
mod diagnostic;
mod generation;
mod local_fs;
pub mod playground;
mod project;
mod project_files;
mod reply;
mod smoke;

pub use builder_project::{ManifestRevision, ProjectDocument, ScenarioMode, SchemaVersion};
pub use diagnostic::{Diagnostic, DiagnosticCode, DiagnosticDetails, Severity};
pub use generation::{
    ConflictDecision, ConflictResolution, ExportConflict, ExportPlan, FileOwner, GeneratedFile,
    GenerationRecord, GenerationRequest, GenerationResult, ResolutionSource, ReviewedGeneration,
    generate,
};
pub use project::{
    ProjectLocation, ProjectRecovery, ProjectService, ProjectSnapshot, ProjectStatus,
    ProjectSummary, ProjectValidation, project_read_draft, project_read_draft_file, project_validate,
    project_validate_document, project_validate_root,
};
pub use project_files::{RecoveryCandidate, RecoveryKind};
pub use reply::Reply;
pub use smoke::{SMOKE_CONTRACT_VERSION, SmokeRequest, SmokeStatus, workspace_smoke};
