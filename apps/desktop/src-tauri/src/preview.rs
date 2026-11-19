mod state;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use builder_app::playground::{
    CredentialSource, Discovery, Invocation, PreparedProjectPreview, PreviewAccess, PreviewError,
    PreviewMode, PreviewOptions, ProtocolPreference, RequestOptions, TraceSnapshot, failure_reply,
};
use builder_app::{DiagnosticCode, ManifestRevision, ProjectLocation, Reply};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::ipc::{self, Desktop, DesktopState, Fault, HANDLE_LIFETIME, Outcome};
use crate::paths::{PathRef, PinnedDirectory};

use state::{ApprovedLaunch, LaunchSettings, PendingLaunch, RunAction, StartTask};
pub(crate) use state::{Playground, PollResult};

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum LaunchSubject {
    Launch {
        registration_id: Uuid,
        expected_revision: ManifestRevision,
    },
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct AccessRequest {
    approved_grant_ids: BTreeSet<String>,
    approved_prompt_ids: BTreeSet<String>,
    file_roots: BTreeMap<String, PathRef>,
    credentials: BTreeMap<String, CredentialInput>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum CredentialInput {
    Environment { name: String },
    Session { value: Zeroizing<String> },
}

impl AccessRequest {
    async fn prepare(self, desktop: &Desktop) -> Outcome<(PreviewAccess, Vec<PinnedDirectory>)> {
        let mut bytes = 0_usize;
        for input in self.credentials.values() {
            if let CredentialInput::Session { value } = input {
                bytes = bytes.saturating_add(value.len());
                if value.is_empty() || value.len() > 16 * 1024 || bytes > 128 * 1024 {
                    return Err(Fault::new(
                        DiagnosticCode::Playground("credential.unavailable"),
                        "A session credential exceeds the allowed input size or is empty.",
                        "Supply a valid value or choose a named environment variable.",
                        None,
                    ));
                }
            }
        }
        let selected = self
            .file_roots
            .into_iter()
            .map(|(reference, selection)| Ok((reference, selection.file_root(desktop)?)))
            .collect::<Outcome<BTreeMap<_, _>>>()?;
        let (file_roots, pinned) = ipc::on_disk(move || {
            let mut file_roots = BTreeMap::new();
            let mut pinned = Vec::with_capacity(selected.len());
            for (reference, selection) in selected {
                let directory = selection.pin_now()?;
                file_roots.insert(reference, directory.path.clone());
                pinned.push(directory);
            }
            Ok((file_roots, pinned))
        })
        .await?;
        let credentials = self.credentials.into_iter().map(|(reference, input)| {
            let source = match input {
                CredentialInput::Environment { name } => CredentialSource::Environment(name),
                CredentialInput::Session { value } => CredentialSource::Session(value),
            };
            (reference, source)
        });
        Ok((
            PreviewAccess {
                approved_grant_ids: self.approved_grant_ids,
                approved_prompt_ids: self.approved_prompt_ids,
                file_roots,
                credentials: credentials.collect(),
                ..PreviewAccess::default()
            },
            pinned,
        ))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReviewRequest {
    subject: LaunchSubject,
    mode: PreviewMode,
    #[serde(default)]
    access: AccessRequest,
    #[serde(default)]
    protocol: ProtocolPreference,
    #[serde(default)]
    capture_payloads: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewResult {
    review_id: Uuid,
    digest: ManifestRevision,
    summary: Value,
}

#[tauri::command]
pub(crate) async fn policy_review<R: Runtime>(
    app: AppHandle<R>,
    request: ReviewRequest,
) -> Reply<ReviewResult> {
    let state: DesktopState<'_> = app.state();
    ipc::reply(review(&state, request).await)
}

async fn review(desktop: &Desktop, request: ReviewRequest) -> Outcome<ReviewResult> {
    let LaunchSubject::Launch {
        registration_id,
        expected_revision,
    } = request.subject;
    let (access, _roots) = request.access.prepare(desktop).await?;
    let expires = Instant::now() + HANDLE_LIFETIME;
    let (prepared, _) = ipc::unpack(
        desktop
            .projects
            .prepare_playground(
                ProjectLocation::Registration(registration_id),
                installed_worker()?,
                request.mode.clone(),
                access,
            )
            .await,
    )?;
    verify_prepared(&prepared, registration_id, expected_revision)?;
    let review = prepared.review();
    let digest = review.digest;
    let summary = serde_json::to_value(review).map_err(|_| Fault::internal())?;
    let review_id = desktop.preview.lock().expect("playground table").pending(
        PendingLaunch {
            settings: LaunchSettings {
                subject: request.subject,
                mode: request.mode,
                options: PreviewOptions {
                    protocol: request.protocol,
                    capture_payloads: request.capture_payloads,
                },
            },
            prepared,
        },
        expires,
    )?;
    Ok(ReviewResult {
        review_id,
        digest,
        summary,
    })
}

fn verify_prepared(
    prepared: &PreparedProjectPreview,
    registration: Uuid,
    expected: ManifestRevision,
) -> Outcome<()> {
    let review = prepared.review();
    if review.registration_id != registration || review.manifest_revision != expected {
        return Err(preview_fault(PreviewError::ManifestChanged));
    }
    Ok(())
}

pub(crate) fn installed_worker() -> Outcome<PathBuf> {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    {
        let filename = if cfg!(windows) {
            "mcp-builder.exe"
        } else {
            "mcp-builder"
        };
        std::env::current_exe()
            .map(|desktop| desktop.with_file_name(filename))
            .map_err(|_| preview_fault(PreviewError::WorkerUnavailable))
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Err(Fault::new(
            DiagnosticCode::Playground("playground.launch_unsupported"),
            "Managed preview is unavailable on this platform.",
            "Use a supported desktop installation for managed preview.",
            None,
        ))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ApproveRequest {
    review_id: Uuid,
    digest: ManifestRevision,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalResult {
    approval_id: Uuid,
    expires_at: String,
}

#[tauri::command]
pub(crate) fn policy_approve(
    state: DesktopState<'_>,
    request: ApproveRequest,
) -> Reply<ApprovalResult> {
    ipc::reply(approve(&state, request))
}

fn approve(desktop: &Desktop, request: ApproveRequest) -> Outcome<ApprovalResult> {
    let mut playground = desktop.preview.lock().expect("playground table");
    let (review, expires) = playground.take_review(request.review_id)?;
    let approved = review
        .prepared
        .approve(request.digest)
        .map_err(preview_fault)?;
    let approval_id = playground.approved(
        ApprovedLaunch {
            settings: review.settings,
            approved,
        },
        expires,
    )?;
    let expires_at = (OffsetDateTime::now_utc()
        + expires.saturating_duration_since(Instant::now()))
    .format(&Rfc3339)
    .map_err(|_| Fault::internal())?;
    Ok(ApprovalResult {
        approval_id,
        expires_at,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApprovalRef {
    approval_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartRequest {
    subject: LaunchSubject,
    approval: ApprovalRef,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartResult {
    operation_id: Uuid,
}

#[tauri::command]
pub(crate) fn playground_start(
    state: DesktopState<'_>,
    request: StartRequest,
) -> Reply<StartResult> {
    ipc::reply(start(state.inner(), request))
}

fn start(desktop: &Arc<Desktop>, request: StartRequest) -> Outcome<StartResult> {
    let task = desktop
        .preview
        .lock()
        .expect("playground table")
        .start(request.approval.approval_id, request.subject)?;
    let operation_id = task.operation.id;
    let desktop = Arc::clone(desktop);
    tauri::async_runtime::spawn(start_connection(desktop, task));
    Ok(StartResult { operation_id })
}

async fn start_connection(desktop: Arc<Desktop>, task: StartTask) {
    let id = task.operation.id;
    let cancellation = task.operation.cancellation;
    let launch = task.launch;
    let opened = launch
        .approved
        .start(launch.settings.options, &cancellation)
        .await;
    let session = match opened {
        Ok(session) => session,
        Err(error) => {
            desktop
                .preview
                .lock()
                .expect("playground table")
                .fail_start(id, error);
            return;
        }
    };
    let discovery = session
        .discover(RequestOptions {
            cancellation: cancellation.clone(),
            ..RequestOptions::default()
        })
        .await;
    let discovery = match discovery {
        Ok(discovery) => discovery,
        Err(error) => {
            let _ = session.stop().await;
            desktop
                .preview
                .lock()
                .expect("playground table")
                .fail_start(id, error);
            return;
        }
    };
    let unregistered = desktop.preview.lock().expect("playground table").connect(
        id,
        launch.settings,
        session,
        discovery,
    );
    if let Some(session) = unregistered {
        let error = session
            .stop()
            .await
            .err()
            .unwrap_or(PreviewError::Cancelled);
        desktop
            .preview
            .lock()
            .expect("playground table")
            .fail_start(id, error);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConnectionRequest {
    connection_id: Uuid,
}

#[tauri::command]
pub(crate) fn playground_discover(
    state: DesktopState<'_>,
    request: ConnectionRequest,
) -> Reply<Arc<Discovery>> {
    ipc::reply(
        state
            .preview
            .lock()
            .expect("playground table")
            .discover(request.connection_id),
    )
}

#[tauri::command]
pub(crate) async fn playground_trace<R: Runtime>(
    app: AppHandle<R>,
    request: ConnectionRequest,
) -> Reply<TraceSnapshot> {
    let state: DesktopState<'_> = app.state();
    let session = state
        .preview
        .lock()
        .expect("playground table")
        .session(request.connection_id);
    match session {
        Ok(session) => ipc::reply(
            session
                .read()
                .await
                .as_ref()
                .map(|session| session.trace())
                .ok_or_else(disconnected),
        ),
        Err(fault) => fault.into_reply(),
    }
}

#[derive(Serialize)]
pub(crate) struct StopResult {
    stopped: bool,
}

#[tauri::command]
pub(crate) async fn playground_stop<R: Runtime>(
    app: AppHandle<R>,
    request: ConnectionRequest,
) -> Reply<StopResult> {
    let state: DesktopState<'_> = app.state();
    let desktop = Arc::clone(state.inner());
    let result = desktop
        .preview
        .lock()
        .expect("playground table")
        .begin_stop(request.connection_id);
    let session = match result {
        Ok(session) => session,
        Err(fault) => return fault.into_reply(),
    };
    // Keep teardown running even if the window or caller drops this invocation.
    let stop = tauri::async_runtime::spawn(async move {
        let owned = session.write().await.take();
        let outcome = match owned {
            Some(owned) => owned.stop().await.map_err(preview_fault),
            None => Ok(()),
        };
        desktop
            .preview
            .lock()
            .expect("playground table")
            .finish_stop(request.connection_id);
        outcome.map(|()| StopResult { stopped: true })
    });
    ipc::reply(stop.await.unwrap_or_else(|_| Err(Fault::internal())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CallRequest {
    connection_id: Uuid,
    call: Invocation,
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScenarioRequest {
    registration_id: Uuid,
    expected_revision: ManifestRevision,
    scenario_id: String,
    connection_id: Uuid,
    #[serde(default)]
    allow_live: bool,
    timeout_ms: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunHandle {
    run_id: Uuid,
}

#[tauri::command]
pub(crate) fn playground_call(state: DesktopState<'_>, request: CallRequest) -> Reply<RunHandle> {
    ipc::reply(run(
        state.inner(),
        request.connection_id,
        request.timeout_ms,
        RunAction::Call(request.call),
    ))
}

#[tauri::command]
pub(crate) fn scenario_run(state: DesktopState<'_>, request: ScenarioRequest) -> Reply<RunHandle> {
    ipc::reply(run(
        state.inner(),
        request.connection_id,
        request.timeout_ms,
        RunAction::Replay {
            subject: LaunchSubject::Launch {
                registration_id: request.registration_id,
                expected_revision: request.expected_revision,
            },
            scenario_id: request.scenario_id,
            allow_live: request.allow_live,
        },
    ))
}

fn run(
    desktop: &Arc<Desktop>,
    connection_id: Uuid,
    timeout_ms: Option<u64>,
    action: RunAction,
) -> Outcome<RunHandle> {
    if timeout_ms.is_some_and(|value| !(1..=30_000).contains(&value)) {
        return Err(Fault::new(
            DiagnosticCode::InvalidInput,
            "The timeout must be between 1 and 30000 milliseconds.",
            "Enter a timeout within the supported range.",
            Some("/timeoutMs"),
        ));
    }
    let (operation, session) = desktop
        .preview
        .lock()
        .expect("playground table")
        .begin_run(connection_id, &action)?;
    let run_id = operation.id;
    let desktop = Arc::clone(desktop);
    tauri::async_runtime::spawn(async move {
        let mut options = RequestOptions {
            cancellation: operation.cancellation,
            ..RequestOptions::default()
        };
        if let Some(milliseconds) = timeout_ms {
            options.timeout = Duration::from_millis(milliseconds);
        }
        let result = {
            let lease = session.read().await;
            let result = match (lease.as_ref(), action) {
                (Some(session), RunAction::Call(call)) => session.invoke(&call, options).await,
                (Some(session), RunAction::Replay { .. }) => session.replay(options).await,
                (None, _) => Err(PreviewError::Cancelled),
            };
            result.map(|mut run| {
                run.result.run_id = run_id;
                run
            })
        };
        desktop
            .preview
            .lock()
            .expect("playground table")
            .complete_run(run_id, result);
    });
    Ok(RunHandle { run_id })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PollRequest {
    id: Uuid,
    after_cursor: Option<u32>,
}

#[tauri::command]
pub(crate) fn run_poll(state: DesktopState<'_>, request: PollRequest) -> Reply<PollResult> {
    ipc::reply(
        state
            .preview
            .lock()
            .expect("playground table")
            .poll(request.id, request.after_cursor.unwrap_or_default()),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CancelRequest {
    id: Uuid,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CancelResult {
    cancellation_requested: bool,
}

#[tauri::command]
pub(crate) fn run_cancel(state: DesktopState<'_>, request: CancelRequest) -> Reply<CancelResult> {
    ipc::reply(
        state
            .preview
            .lock()
            .expect("playground table")
            .cancel(request.id)
            .map(|cancellation_requested| CancelResult {
                cancellation_requested,
            }),
    )
}

fn disconnected() -> Fault {
    Fault::new(
        DiagnosticCode::Playground("playground.connection_unavailable"),
        "The reviewed playground connection is unavailable.",
        "Review and start a new preview.",
        None,
    )
}

fn preview_fault(error: PreviewError) -> Fault {
    ipc::unpack(failure_reply::<()>(error))
        .err()
        .unwrap_or_else(Fault::internal)
}
