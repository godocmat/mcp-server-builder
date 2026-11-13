//! Native application interface to the managed local playground.

use std::path::PathBuf;

pub use builder_playground::CancellationToken;
pub use builder_playground::client::{
    Discovery, ProtocolPreference, ProtocolRevision, RequestOptions,
};
pub use builder_playground::managed::{
    ApprovedPreview, CredentialSource, ManagedSession, PreparedPreview, PreviewAccess,
    PreviewError, PreviewMode, PreviewOptions, PreviewRequest, PreviewReview, PreviewRun,
    RunDiagnostic, RunResultV1, RunTarget, WORKER_ARG, prepare_preview, serve_worker,
};
pub use builder_playground::scenario::RunOutcome;
pub use builder_playground::sdk_diagnostic_filter;
pub use builder_playground::trace::TraceSnapshot;
pub use builder_project::Invocation;

use crate::{Diagnostic, DiagnosticCode, ManifestRevision, Reply, Severity, project_files};

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaygroundRunDetails {
    pub run_id: uuid::Uuid,
    pub run: RunResultV1,
    pub trace: TraceSnapshot,
}

pub struct PreparedProjectPreview {
    preview: PreparedPreview,
    root: PathBuf,
}

impl PreparedProjectPreview {
    pub(crate) fn new(preview: PreparedPreview, root: PathBuf) -> Self {
        Self { preview, root }
    }

    pub fn review(&self) -> PreviewReview<'_> {
        self.preview.review()
    }

    pub fn approve(self, digest: ManifestRevision) -> Result<ApprovedProjectPreview, PreviewError> {
        let revision = self.preview.review().manifest_revision;
        Ok(ApprovedProjectPreview {
            preview: self.preview.approve(digest)?,
            root: self.root,
            revision,
        })
    }
}

pub struct ApprovedProjectPreview {
    preview: ApprovedPreview,
    root: PathBuf,
    revision: ManifestRevision,
}

impl ApprovedProjectPreview {
    pub async fn start(
        self,
        options: PreviewOptions,
        cancellation: &CancellationToken,
    ) -> Result<ManagedSession, PreviewError> {
        if cancellation.is_cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let Self {
            preview,
            root,
            revision,
        } = self;
        let unchanged = tokio::task::spawn_blocking(move || {
            project_files::canonical_root(&root).is_ok_and(|resolved| resolved == root)
                && project_files::read(&root.join(builder_project::MANIFEST_NAME))
                    .is_ok_and(|current| current.revision == revision)
        })
        .await
        .map_err(|_| PreviewError::WorkerTask)?;
        if !unchanged {
            return Err(PreviewError::ManifestChanged);
        }
        preview.start(options, cancellation).await
    }
}

pub fn failure_reply<T>(error: PreviewError) -> Reply<T> {
    Reply::Failure {
        error: Diagnostic {
            code: DiagnosticCode::Playground(error.code()),
            message: error.message().into(),
            severity: Severity::Error,
            correlation_id: error.correlation_id().unwrap_or_else(uuid::Uuid::new_v4),
            action: None,
            field: None,
            capability_id: None,
            details: None,
        },
        diagnostics: Vec::new(),
    }
}
