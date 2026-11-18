use builder_app::{
    ConflictResolution, DiagnosticCode, ExportConflict, GenerationRecord, ManifestRevision, Reply,
    ReviewedGeneration,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};
use uuid::Uuid;

use crate::ipc::{self, Desktop, DesktopState, Fault, Outcome};
use crate::paths::{PathRef, SelectedDirectory, display_path};

pub(crate) struct PendingExport {
    reviewed: ReviewedGeneration,
    parent: SelectedDirectory,
    previous: Option<SelectedDirectory>,
    destination_display_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PlanRequest {
    registration_id: Uuid,
    expected_revision: ManifestRevision,
    destination: PathRef,
    folder_name: String,
    from_export: Option<PathRef>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanResult {
    plan_id: Uuid,
    plan_digest: ManifestRevision,
    destination_display_path: String,
    generation: Option<GenerationRecord>,
    conflicts: Vec<ExportConflict>,
}

#[tauri::command]
pub(crate) async fn generation_plan<R: Runtime>(
    app: AppHandle<R>,
    request: PlanRequest,
) -> Reply<PlanResult> {
    let state: DesktopState<'_> = app.state();
    ipc::reply(plan(&state, request).await)
}

async fn plan(state: &Desktop, request: PlanRequest) -> Outcome<PlanResult> {
    if request.folder_name.contains('/') || !builder_project::is_portable_path(&request.folder_name)
    {
        return Err(Fault::new(
            DiagnosticCode::InvalidInput,
            "The export folder name must be one portable name.",
            "Enter a new folder name without separators.",
            Some("/folderName"),
        ));
    }
    let parent = request.destination.export_parent(state)?;
    let previous = request
        .from_export
        .map(|selection| selection.previous_export(state))
        .transpose()?;
    let (parent, previous, directory, source) = ipc::on_disk(move || {
        let directory = parent.pin_now()?;
        let source = previous
            .as_ref()
            .map(SelectedDirectory::pin_now)
            .transpose()?;
        Ok((parent, previous, directory, source))
    })
    .await?;
    let destination = directory.path.join(request.folder_name);
    let destination_display_path = display_path(&destination);
    let previous_path = source.as_ref().map(|directory| directory.path.clone());
    let (reviewed, _) = ipc::unpack(
        state
            .projects
            .plan_generation_for_registration(
                request.registration_id,
                request.expected_revision,
                destination,
                previous_path,
            )
            .await,
    )?;
    let plan_digest = reviewed.digest;
    let generation = reviewed.plan.generation.clone();
    let conflicts = reviewed.plan.conflicts.clone();
    let plan_id = state
        .exports
        .lock()
        .expect("export table")
        .insert(PendingExport {
            reviewed,
            parent,
            previous,
            destination_display_path: destination_display_path.clone(),
        })?;
    Ok(PlanResult {
        plan_id,
        plan_digest,
        destination_display_path,
        generation,
        conflicts,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WriteRequest {
    plan_id: Uuid,
    plan_digest: ManifestRevision,
    #[serde(default)]
    resolutions: Vec<ConflictResolution>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Receipt {
    destination_display_path: String,
    generation: GenerationRecord,
}

#[tauri::command]
pub(crate) async fn generation_write<R: Runtime>(
    app: AppHandle<R>,
    request: WriteRequest,
) -> Reply<Receipt> {
    let state: DesktopState<'_> = app.state();
    ipc::reply(write(&state, request).await)
}

async fn write(state: &Desktop, request: WriteRequest) -> Outcome<Receipt> {
    let pending = state
        .exports
        .lock()
        .expect("export table")
        .take(request.plan_id)
        .filter(|pending| pending.reviewed.digest == request.plan_digest)
        .ok_or_else(|| {
            Fault::new(
                DiagnosticCode::ExportInputChanged,
                "The export plan is missing, expired, or changed.",
                "Preview the export again before writing.",
                None,
            )
        })?;
    ipc::on_disk(move || {
        let _parent = pending.parent.pin_now()?;
        let _previous = pending
            .previous
            .as_ref()
            .map(SelectedDirectory::pin_now)
            .transpose()?;
        let (generation, _) = ipc::unpack(pending.reviewed.publish(request.resolutions))?;
        Ok(Receipt {
            destination_display_path: pending.destination_display_path,
            generation,
        })
    })
    .await
}
