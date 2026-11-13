//! Reviewed, first-party stdio preview sessions for the application services.

mod access;
mod run;
mod worker;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use builder_project::{ManifestRevision, ScenarioMode, ScenarioV1, ValidatedManifest};
use mcp_policy::{Grant, LimitOverrides, Redactor};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::client::{
    ClientError, ClientOptions, Discovery, ProtocolPreference, RequestOptions, StdioClient,
};
use crate::scenario::ScenarioError;
use crate::supervision::{
    LaunchError, LaunchReview, LaunchSpec, SupervisedChild, review_launch, spawn_reviewed,
};
use crate::trace::TraceSnapshot;

pub use access::{CredentialSource, PreviewAccess};
pub use run::{PreviewRun, RunDiagnostic, RunResultV1, RunTarget};
pub use worker::{WORKER_ARG, serve_worker};

use access::{Access, CredentialSources, SourceReference};

const OPERATOR: &str = "managed-preview";
const APPROVAL_WINDOW: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PreviewMode {
    Fixture {
        scenario_id: String,
    },
    Live {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scenario_id: Option<String>,
    },
}

impl PreviewMode {
    fn is_fixture(&self) -> bool {
        matches!(self, Self::Fixture { .. })
    }

    fn scenario<'a>(
        &self,
        manifest: &'a ValidatedManifest,
    ) -> Result<Option<&'a ScenarioV1>, PreviewError> {
        let (id, expected) = match self {
            Self::Fixture { scenario_id } => (scenario_id, ScenarioMode::Fixture),
            Self::Live {
                scenario_id: Some(id),
            } => (id, ScenarioMode::Live),
            Self::Live { scenario_id: None } => return Ok(None),
        };
        let scenario = manifest
            .document()
            .scenarios
            .iter()
            .find(|item| &item.id == id)
            .ok_or(PreviewError::Scenario(ScenarioError::NotFound))?;
        if scenario.mode != expected {
            return Err(PreviewError::ScenarioMode);
        }
        Ok(Some(scenario))
    }
}

/// The host supplies an absolute path to its installed first-party worker.
/// A project cannot select the worker, arguments or working directory.
pub struct PreviewRequest {
    pub worker: PathBuf,
    pub registration_id: Uuid,
    pub manifest: ValidatedManifest,
    pub mode: PreviewMode,
    pub access: PreviewAccess,
}

#[derive(Clone, Copy, Default)]
pub struct PreviewOptions {
    pub protocol: ProtocolPreference,
    pub capture_payloads: bool,
}

/// An inert plan. Approving or starting takes ownership of the respective plan.
pub struct PreparedPreview {
    manifest: ValidatedManifest,
    registration_id: Uuid,
    mode: PreviewMode,
    access: Access,
    credentials: CredentialSources,
    spec: LaunchSpec,
    binding: ManifestRevision,
    review: LaunchReview,
    prepared_at: Instant,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewReview<'a> {
    pub digest: ManifestRevision,
    pub registration_id: Uuid,
    pub project_id: Uuid,
    pub manifest_revision: ManifestRevision,
    pub mode: &'a PreviewMode,
    pub executable: &'a Path,
    pub arguments: Vec<&'static str>,
    pub working_directory: &'a Path,
    pub environment: Vec<&'static str>,
    pub approved_grants: Vec<&'a Grant>,
    pub approved_prompt_ids: &'a BTreeSet<String>,
    pub file_roots: BTreeMap<&'a str, &'a Path>,
    pub credential_sources: BTreeMap<&'a str, CredentialReference<'a>>,
    pub limits: LimitOverrides,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CredentialReference<'a> {
    Environment { name: &'a str },
    Session,
}

pub fn prepare_preview(request: PreviewRequest) -> Result<PreparedPreview, PreviewError> {
    let PreviewRequest {
        worker,
        registration_id,
        manifest,
        mode,
        access,
    } = request;
    if !worker.is_absolute() {
        return Err(PreviewError::InvalidAccess);
    }
    mode.scenario(&manifest)?;
    let (access, credentials) = Access::review(access, &mode, &manifest)?;
    // The worker's install directory supplies cwd, never an imported project directory.
    let cwd = worker
        .parent()
        .ok_or(PreviewError::InvalidAccess)?
        .to_path_buf();
    let spec = LaunchSpec {
        executable: worker,
        cwd,
        args: vec![WORKER_ARG.into()],
        environment: Vec::new(),
    };
    let binding = serde_json::to_vec(&(
        worker::HANDOFF_VERSION,
        registration_id,
        manifest.revision(),
        &mode,
        &access,
    ))
    .map_err(|_| PreviewError::InvalidAccess)?;
    let binding = ManifestRevision::of(&binding);
    let review = review_launch(&spec, binding)?;
    Ok(PreparedPreview {
        manifest,
        registration_id,
        mode,
        access,
        credentials,
        spec,
        binding,
        review,
        prepared_at: Instant::now(),
    })
}

impl PreparedPreview {
    pub fn review(&self) -> PreviewReview<'_> {
        let document = self.manifest.document();
        PreviewReview {
            digest: self.review.digest(),
            registration_id: self.registration_id,
            project_id: document.project_id,
            manifest_revision: self.manifest.revision(),
            mode: &self.mode,
            executable: self.review.executable(),
            arguments: vec![WORKER_ARG],
            working_directory: self.review.cwd(),
            environment: Vec::new(),
            approved_grants: document
                .policy
                .grants
                .iter()
                .filter(|grant| self.access.grant_ids.contains(&grant.id))
                .collect(),
            approved_prompt_ids: &self.access.prompt_ids,
            file_roots: self
                .access
                .file_roots
                .iter()
                .map(|(reference, root)| (reference.as_str(), root.path.as_path()))
                .collect(),
            credential_sources: self
                .access
                .credential_sources
                .iter()
                .map(|(id, source)| {
                    let source = match source {
                        SourceReference::Environment { name } => {
                            CredentialReference::Environment { name }
                        }
                        SourceReference::Session => CredentialReference::Session,
                    };
                    (id.as_str(), source)
                })
                .collect(),
            limits: self.access.limits,
        }
    }

    /// A matching digest confirms the review the operator saw.
    pub fn approve(self, digest: ManifestRevision) -> Result<ApprovedPreview, PreviewError> {
        if self.review.digest() != digest || self.prepared_at.elapsed() >= APPROVAL_WINDOW {
            return Err(PreviewError::ApprovalRequired);
        }
        Ok(ApprovedPreview(self))
    }
}

pub struct ApprovedPreview(PreparedPreview);

impl ApprovedPreview {
    /// Consume this approval on admission, including on failure.
    pub async fn start(
        self,
        options: PreviewOptions,
        cancellation: &CancellationToken,
    ) -> Result<ManagedSession, PreviewError> {
        let plan = self.0;
        if plan.prepared_at.elapsed() >= APPROVAL_WINDOW {
            return Err(PreviewError::ApprovalRequired);
        }
        if cancellation.is_cancelled() {
            return Err(PreviewError::Cancelled);
        }
        // Native supervision opens and checks the reviewed identities before the worker
        // executes. The parent resolves values only after this admission has succeeded.
        let scenario = plan.mode.scenario(&plan.manifest)?.cloned();
        let reviewed_digest = plan.review.digest();
        let spec = plan.spec;
        let binding = plan.binding;
        let pending = cancellation.clone();
        let mut child =
            tokio::task::spawn_blocking(move || -> Result<SupervisedChild, PreviewError> {
                if pending.is_cancelled() {
                    return Err(PreviewError::Cancelled);
                }
                Ok(spawn_reviewed(&spec, binding, reviewed_digest, |_| None)?)
            })
            .await
            .map_err(|_| PreviewError::WorkerTask)??;
        let client = connect_worker(
            &mut child,
            &plan.manifest,
            &plan.mode,
            &plan.access,
            plan.credentials,
            scenario.as_ref(),
            options,
            cancellation,
        )
        .await;
        let (client, redactor) = match client {
            Ok(connected) => connected,
            Err(error) => {
                child.stop().await?;
                return Err(error);
            }
        };
        let write_tools = if plan.mode.is_fixture() {
            BTreeSet::new()
        } else {
            run::write_tools(&plan.manifest, &plan.access.grant_ids)
        };
        Ok(ManagedSession {
            client,
            child,
            scenario,
            redactor,
            write_tools,
            mode: plan.mode,
        })
    }
}

async fn connect_worker(
    child: &mut SupervisedChild,
    manifest: &ValidatedManifest,
    mode: &PreviewMode,
    access: &Access,
    sources: CredentialSources,
    scenario: Option<&ScenarioV1>,
    options: PreviewOptions,
    cancellation: &CancellationToken,
) -> Result<(StdioClient, Arc<Redactor>), PreviewError> {
    let mut stdin = child.take_stdin().ok_or(PreviewError::Handoff)?;
    let mut stdout = child.take_stdout().ok_or(PreviewError::Handoff)?;
    let stderr = child.take_stderr().ok_or(PreviewError::Handoff)?;
    if cancellation.is_cancelled() {
        return Err(PreviewError::Cancelled);
    }
    let values = access::resolve_values(sources)?;
    let paths = scenario
        .and_then(|item| item.sensitive_paths.as_deref())
        .unwrap_or_default();
    let redactor = access::redactor(&values, paths)?;
    worker::handoff(
        &mut stdin,
        &mut stdout,
        manifest,
        mode,
        access,
        values,
        cancellation,
    )
    .await?;
    let client = StdioClient::connect(
        stdout,
        stdin,
        stderr,
        ClientOptions {
            protocol: options.protocol,
            capture_payloads: options.capture_payloads,
            trust_builder_diagnostics: true,
            redactor: redactor.clone(),
        },
        cancellation,
    )
    .await?;
    Ok((client, redactor))
}

/// One supervised connection. All outward data has passed through Rust redaction.
pub struct ManagedSession {
    client: StdioClient,
    child: SupervisedChild,
    scenario: Option<ScenarioV1>,
    redactor: Arc<Redactor>,
    write_tools: BTreeSet<String>,
    mode: PreviewMode,
}

impl ManagedSession {
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    pub async fn discover(&self, mut options: RequestOptions) -> Result<Discovery, PreviewError> {
        options.redactor = None;
        Ok(self.client.discover(options).await?)
    }

    pub fn trace(&self) -> TraceSnapshot {
        self.client.trace()
    }

    /// Close the protocol writer to signal EOF and await native cleanup.
    pub async fn stop(self) -> Result<(), PreviewError> {
        let (close, stop) = tokio::join!(self.client.close(), self.child.stop());
        stop?;
        close?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum PreviewError {
    InvalidAccess,
    ApprovalRequired,
    ManifestChanged,
    WorkerUnavailable,
    ScenarioMode,
    ScenarioRequired,
    Scenario(ScenarioError),
    FileRootUnavailable,
    CredentialUnavailable,
    Handoff,
    Runtime,
    WorkerTask,
    Redaction,
    Cancelled,
    Launch(LaunchError),
    Client(ClientError),
}

impl PreviewError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidAccess => "playground.invalid_access",
            Self::ApprovalRequired | Self::ManifestChanged => "policy.approval_required",
            Self::WorkerUnavailable => "playground.launch_failed",
            Self::ScenarioMode => "scenario.mode_mismatch",
            Self::ScenarioRequired => "scenario.required",
            Self::Scenario(error) => error.code(),
            Self::FileRootUnavailable => "adapter.file_root_unavailable",
            Self::CredentialUnavailable => "credential.unavailable",
            Self::Handoff => "playground.handoff_failed",
            Self::Runtime => "playground.runtime_failed",
            Self::WorkerTask => "playground.launch_failed",
            Self::Redaction => "playground.redaction_failed",
            Self::Cancelled => "run.cancelled",
            Self::Launch(error) => error.code(),
            Self::Client(error) => error.code(),
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::InvalidAccess => "Review the selected grants and their local bindings.",
            Self::ApprovalRequired => "Approve the current launch review before starting preview.",
            Self::ManifestChanged => {
                "The project manifest changed. Review its current revision before starting preview."
            }
            Self::WorkerUnavailable => "The installed preview worker could not be located.",
            Self::ScenarioMode => "Select the execution mode saved in this scenario.",
            Self::ScenarioRequired => "Select a scenario during launch review before replaying it.",
            Self::Scenario(_) => "Check the selected scenario and its fixtures.",
            Self::FileRootUnavailable => "Check the approved local file root and review it again.",
            Self::CredentialUnavailable => "Supply the credential from its selected source.",
            Self::Handoff => "The preview worker did not accept its startup inputs.",
            Self::Runtime => "The preview worker could not start the project runtime.",
            Self::WorkerTask => "The approved preview worker could not start.",
            Self::Redaction => "The configured evidence redaction could not be applied.",
            Self::Cancelled => "The preview operation was cancelled.",
            Self::Launch(error) => match error.code() {
                "policy.approval_required" => {
                    "The launch changed. Review it again before starting preview."
                }
                "playground.launch_unsupported" => {
                    "Managed preview is unavailable on this platform."
                }
                "playground.stop_failed" => {
                    "The preview process did not stop within its shutdown budget."
                }
                _ => "The approved preview worker could not start.",
            },
            Self::Client(error) => error.message(),
        }
    }
}

impl fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for PreviewError {}

impl From<LaunchError> for PreviewError {
    fn from(error: LaunchError) -> Self {
        Self::Launch(error)
    }
}

impl From<ClientError> for PreviewError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}
