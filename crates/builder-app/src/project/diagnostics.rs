use std::path::Path;

use builder_project::{
    IssueCode, ManifestError, ManifestRevision, PROJECT_FORMAT, PROJECT_SCHEMA_VERSION,
    ValidationIssue,
};
use uuid::Uuid;

use crate::project_files::{FileError, recovery_candidates};
use crate::{Diagnostic, DiagnosticCode, DiagnosticDetails, Reply, Severity};

pub(super) struct Operation {
    id: Uuid,
    warnings: Vec<Diagnostic>,
}

impl Operation {
    pub(super) fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            warnings: Vec::new(),
        }
    }

    pub(super) fn warn(
        &mut self,
        code: DiagnosticCode,
        message: &'static str,
        action: &'static str,
        details: DiagnosticDetails,
    ) {
        self.warnings.push(
            Message::new(code, message, action)
                .details(details)
                .diagnostic(self.id, Severity::Warning),
        );
    }

    pub(super) fn finish<T>(mut self, result: Result<T, Failure>) -> Reply<T> {
        match result {
            Ok(data) => Reply::Success {
                data,
                diagnostics: self.warnings,
            },
            Err(failure) => {
                self.warnings.extend(
                    failure
                        .issues
                        .into_iter()
                        .map(|issue| issue.diagnostic(self.id, Severity::Error)),
                );
                if let Some(recovery) = failure.recovery {
                    self.warnings
                        .push(recovery.diagnostic(self.id, Severity::Warning));
                }
                Reply::Failure {
                    error: failure.error.diagnostic(self.id, Severity::Error),
                    diagnostics: self.warnings,
                }
            }
        }
    }
}

pub(super) struct Failure {
    error: Box<Message>,
    issues: Vec<Message>,
    recovery: Option<Box<Message>>,
}

impl Failure {
    pub(super) fn internal() -> Self {
        Self::new(Message::new(
            DiagnosticCode::InternalError,
            "The local project operation was interrupted.",
            "Reopen the project folder and check its current revision.",
        ))
    }

    pub(super) fn catalog() -> Self {
        Self::new(Message::new(
            DiagnosticCode::StorageUnavailable,
            "The local project catalog is unavailable.",
            "Check the application-data directory or open the project by folder.",
        ))
    }

    pub(super) fn registration() -> Self {
        Self::new(Message::new(
            DiagnosticCode::ProjectMissing,
            "The local registration no longer identifies this project.",
            "Open the project by its folder to reconcile the catalog.",
        ))
    }

    pub(super) fn field(field: &str, message: &'static str) -> Self {
        Self::manifest(ManifestError::Invalid(vec![ValidationIssue {
            code: IssueCode::Field,
            field: field.into(),
            capability_id: None,
            message,
        }]))
    }

    pub(super) fn manifest(error: ManifestError) -> Self {
        match error {
            ManifestError::UnsupportedSchema { version } => Self::new(
                Message::new(
                    DiagnosticCode::ProjectUnsupportedSchema,
                    "This project uses an unsupported manifest schema.",
                    "Use a builder that supports the reported schema version.",
                )
                .at("/schemaVersion")
                .details(DiagnosticDetails::UnsupportedProjectSchema {
                    format: PROJECT_FORMAT,
                    schema_version: version,
                    supported_versions: vec![PROJECT_SCHEMA_VERSION],
                }),
            ),
            ManifestError::Invalid(issues) => Self {
                error: Box::new(Message::new(
                    DiagnosticCode::ProjectInvalid,
                    "The project manifest is invalid.",
                    "Correct the reported fields and validate again.",
                )),
                issues: issues.into_iter().map(Message::validation).collect(),
                recovery: None,
            },
        }
    }

    pub(super) fn file(error: FileError) -> Self {
        use DiagnosticCode as Code;
        let message = match error {
            FileError::Missing => Message::new(
                Code::ProjectMissing,
                "The project folder or manifest is missing.",
                "Select a folder containing mcp-project.json.",
            ),
            FileError::Unsafe => Message::new(
                Code::ProjectUnsafePath,
                "The project path contains an unsupported storage object.",
                "Select a local directory with regular project files.",
            ),
            FileError::Unavailable => Message::new(
                Code::StorageUnavailable,
                "The project files could not be read or written.",
                "Check folder access and retry with the current revision.",
            ),
            FileError::Locked => Message::new(
                Code::ProjectLocked,
                "Another builder operation holds the project lock.",
                "Retry after the other operation finishes.",
            ),
            FileError::Exists => Message::new(
                Code::ProjectExists,
                "The destination already contains a project manifest.",
                "Open that project or choose a different folder.",
            ),
            FileError::Conflict(current_revision) => Message::new(
                Code::ProjectRevisionConflict,
                "The project manifest changed since it was read.",
                "Reopen the current revision or save your draft as a copy.",
            )
            .details(DiagnosticDetails::CurrentRevision { current_revision }),
            FileError::RecoveryUnavailable => Message::new(
                Code::ProjectRecoveryUnavailable,
                "The selected backup is unavailable or belongs to a different project.",
                "Review the available recovery files and select a valid revision.",
            ),
            FileError::TooLarge => {
                return Self::manifest(ManifestError::Invalid(vec![ValidationIssue {
                    code: IssueCode::Limit,
                    field: String::new(),
                    capability_id: None,
                    message: "The manifest exceeds 4 MiB.",
                }]));
            }
        };
        Self::new(message)
    }

    pub(super) fn recoverable(
        mut self,
        root: &Path,
        current: Option<ManifestRevision>,
        project_id: Option<Uuid>,
    ) -> Self {
        let current_revision = match &self.error.details {
            Some(DiagnosticDetails::CurrentRevision { current_revision }) => *current_revision,
            _ => current,
        };
        let candidates = recovery_candidates(root, project_id);
        if !candidates.is_empty() {
            self.recovery = Some(Box::new(
                Message::new(
                    DiagnosticCode::ProjectRecoveryAvailable,
                    "A valid local recovery file is available.",
                    "Review its revision before restoring a backup or copying a staged draft.",
                )
                .details(DiagnosticDetails::Recovery {
                    current_revision,
                    candidates,
                }),
            ));
        }
        self
    }

    fn new(error: Message) -> Self {
        Self {
            error: Box::new(error),
            issues: Vec::new(),
            recovery: None,
        }
    }
}

impl From<FileError> for Failure {
    fn from(error: FileError) -> Self {
        Self::file(error)
    }
}

struct Message {
    code: DiagnosticCode,
    message: &'static str,
    action: &'static str,
    field: Option<String>,
    capability_id: Option<String>,
    details: Option<DiagnosticDetails>,
}

impl Message {
    fn new(code: DiagnosticCode, message: &'static str, action: &'static str) -> Self {
        Self {
            code,
            message,
            action,
            field: None,
            capability_id: None,
            details: None,
        }
    }

    fn at(mut self, field: &str) -> Self {
        self.field = Some(field.into());
        self
    }

    fn details(mut self, details: DiagnosticDetails) -> Self {
        self.details = Some(details);
        self
    }

    fn validation(issue: ValidationIssue) -> Self {
        use DiagnosticCode as Code;
        let code = match issue.code {
            IssueCode::Json => Code::ProjectJson,
            IssueCode::DuplicateKey => Code::ProjectDuplicateKey,
            IssueCode::Field => Code::ProjectInvalidField,
            IssueCode::UnsupportedSetting => Code::ProjectUnsupportedSetting,
            IssueCode::DuplicateId => Code::ProjectDuplicateId,
            IssueCode::Reference => Code::ProjectInvalidReference,
            IssueCode::Schema => Code::ProjectInvalidSchema,
            IssueCode::AmbiguousResource => Code::ProjectAmbiguousResource,
            IssueCode::Limit => Code::ProjectLimit,
        };
        Self {
            code,
            message: issue.message,
            action: "Correct this field in the project manifest.",
            field: Some(issue.field),
            capability_id: issue.capability_id,
            details: None,
        }
    }

    fn diagnostic(self, correlation_id: Uuid, severity: Severity) -> Diagnostic {
        Diagnostic {
            code: self.code,
            message: self.message.into(),
            severity,
            correlation_id,
            action: Some(self.action.into()),
            field: self.field,
            capability_id: self.capability_id,
            details: self.details,
        }
    }
}
