use std::path::PathBuf;

pub use builder_generator::{
    ConflictDecision, ConflictResolution, ExportConflict, FileOwner, GeneratedFile,
    GenerationRecord,
};
use builder_generator::{
    ExportError, ExportPlan, ExportRequest, plan_export, publish_export, read_resolutions,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{Diagnostic, DiagnosticCode, DiagnosticDetails, Reply, Severity, project};

#[derive(Debug)]
pub enum ResolutionSource {
    Choices(Vec<ConflictResolution>),
    File(PathBuf),
}

#[derive(Debug)]
pub struct GenerationRequest {
    pub project_root: PathBuf,
    pub destination: PathBuf,
    pub previous_export: Option<PathBuf>,
    pub resolutions: Option<ResolutionSource>,
    pub preview: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationResult {
    pub published: bool,
    pub generation: Option<GenerationRecord>,
    pub conflicts: Vec<ExportConflict>,
}

/// Plan or publish source without building it or opening the local project catalog.
pub fn generate(request: GenerationRequest) -> Reply<GenerationResult> {
    match run(request) {
        Ok(data) => Reply::Success {
            data,
            diagnostics: Vec::new(),
        },
        Err(ExportError::InvalidProject(error)) => project::invalid_manifest_reply(error),
        Err(error) => Reply::Failure {
            error: diagnostic(error),
            diagnostics: Vec::new(),
        },
    }
}

fn run(request: GenerationRequest) -> Result<GenerationResult, ExportError> {
    let resolutions = match request.resolutions {
        None => Vec::new(),
        Some(ResolutionSource::Choices(choices)) => choices,
        Some(ResolutionSource::File(path)) => read_resolutions(&path)?,
    };
    let export = ExportRequest {
        project_root: request.project_root,
        destination: request.destination,
        previous_export: request.previous_export,
        resolutions,
    };
    if request.preview {
        let ExportPlan {
            generation,
            conflicts,
        } = plan_export(&export)?;
        Ok(GenerationResult {
            published: false,
            generation,
            conflicts,
        })
    } else {
        let generation = publish_export(&export)?;
        Ok(GenerationResult {
            published: true,
            generation: Some(generation),
            conflicts: Vec::new(),
        })
    }
}

fn diagnostic(error: ExportError) -> Diagnostic {
    use DiagnosticCode as Code;
    let message = error.to_string();
    let (code, action, details) = match error {
        ExportError::Conflict(conflicts) => (
            Code::ExportConflict,
            "Preview the export and review each changed file before retrying.",
            Some(DiagnosticDetails::ExportConflicts { conflicts }),
        ),
        ExportError::InputChanged => (
            Code::ExportInputChanged,
            "Review the latest inputs and retry.",
            None,
        ),
        ExportError::ProjectMissing => (
            Code::ProjectMissing,
            "Select a project folder containing mcp-project.json.",
            None,
        ),
        ExportError::DestinationExists => (
            Code::ExportDestinationExists,
            "Choose a new export folder.",
            None,
        ),
        ExportError::UnsafePath => (
            Code::ExportUnsafePath,
            "Select a local directory with regular files and portable names.",
            None,
        ),
        ExportError::InvalidPreviousExport => (
            Code::ExportInvalidPrevious,
            "Select a previous export from this project with a valid generation.json.",
            None,
        ),
        ExportError::InvalidResolution => (
            Code::ExportInvalidResolution,
            "Preview the current conflicts and use their observed and replacement digests.",
            None,
        ),
        ExportError::InputTooLarge => (
            Code::ExportLimit,
            "Remove large or unrelated files from the inputs and retry.",
            None,
        ),
        ExportError::Storage => (
            Code::StorageUnavailable,
            "Check access to the source and destination on a supported local filesystem.",
            None,
        ),
        ExportError::Internal | ExportError::InvalidProject(_) => (
            Code::InternalError,
            "Check the installed builder version and retry.",
            None,
        ),
    };
    Diagnostic {
        code,
        message,
        severity: Severity::Error,
        correlation_id: Uuid::new_v4(),
        action: Some(action.into()),
        field: None,
        capability_id: None,
        details: details.map(Box::new),
    }
}
