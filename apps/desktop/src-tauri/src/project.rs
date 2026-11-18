use builder_app::{
    ManifestRevision, ProjectDocument, ProjectLocation, ProjectRecovery, ProjectSnapshot,
    ProjectSummary, ProjectValidation, Reply, project_read_draft,
};
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};
use uuid::Uuid;

use crate::ipc::{self, Desktop, DesktopState, Fault, Outcome};
use crate::paths::PathRef;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListRequest {}

#[tauri::command]
pub(crate) async fn project_list<R: Runtime>(
    app: AppHandle<R>,
    request: ListRequest,
) -> Reply<Vec<ProjectSummary>> {
    let ListRequest {} = request;
    let state: DesktopState<'_> = app.state();
    state.projects.list().await
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OpenRoot {
    Registration(RegisteredProject),
    Selected(PathRef),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisteredProject {
    registration_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenRequest {
    root: OpenRoot,
}

#[tauri::command]
pub(crate) async fn project_open<R: Runtime>(
    app: AppHandle<R>,
    request: OpenRequest,
) -> Reply<ProjectSnapshot> {
    let state: DesktopState<'_> = app.state();
    let location = match request.root {
        OpenRoot::Registration(project) => ProjectLocation::Registration(project.registration_id),
        OpenRoot::Selected(selection) => {
            return match select_open(&state, selection).await {
                Ok(reply) => reply,
                Err(fault) => fault.into_reply(),
            };
        }
    };
    state.projects.open(location).await
}

async fn select_open(state: &Desktop, selection: PathRef) -> Outcome<Reply<ProjectSnapshot>> {
    let directory = selection.project_root(state)?.pin().await?;
    Ok(state
        .projects
        .open(ProjectLocation::Root(directory.path.clone()))
        .await)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateRequest {
    destination: PathRef,
    name: String,
}

#[tauri::command]
pub(crate) async fn project_create<R: Runtime>(
    app: AppHandle<R>,
    request: CreateRequest,
) -> Reply<ProjectSnapshot> {
    let state: DesktopState<'_> = app.state();
    create(&state, request)
        .await
        .unwrap_or_else(Fault::into_reply)
}

async fn create(state: &Desktop, request: CreateRequest) -> Outcome<Reply<ProjectSnapshot>> {
    let directory = request
        .destination
        .project_destination(state)?
        .pin()
        .await?;
    Ok(state
        .projects
        .create(directory.path.clone(), request.name)
        .await)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidateRequest {
    document: Value,
}

async fn read_document(document: Value) -> Outcome<ProjectDocument> {
    // Keep the transport DTO separate from the portable document schema so the
    // shared parser can return the failing JSON pointer for edited values.
    ipc::on_disk(move || {
        let bytes = serde_json::to_vec(&document).map_err(|_| Fault::internal())?;
        let (document, _) = ipc::unpack(project_read_draft(&bytes))?;
        Ok(document)
    })
    .await
}

#[tauri::command]
pub(crate) async fn project_validate(request: ValidateRequest) -> Reply<ProjectValidation> {
    ipc::reply(
        read_document(request.document)
            .await
            .map(|_| ProjectValidation { valid: true }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveRequest {
    registration_id: Uuid,
    expected_revision: ManifestRevision,
    document: Value,
}

#[tauri::command]
pub(crate) async fn project_save<R: Runtime>(
    app: AppHandle<R>,
    request: SaveRequest,
) -> Reply<ProjectSnapshot> {
    let state: DesktopState<'_> = app.state();
    let document = match read_document(request.document).await {
        Ok(document) => document,
        Err(fault) => return fault.into_reply(),
    };
    state
        .projects
        .save(
            ProjectLocation::Registration(request.registration_id),
            request.expected_revision,
            document,
        )
        .await
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum RecoverySource {
    Backup { revision: ManifestRevision },
    Draft { document: Value },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecoverRequest {
    registration_id: Option<Uuid>,
    root: Option<PathRef>,
    expected_revision: Option<ManifestRevision>,
    source: RecoverySource,
    destination: Option<PathRef>,
}

#[tauri::command]
pub(crate) async fn project_recover<R: Runtime>(
    app: AppHandle<R>,
    request: RecoverRequest,
) -> Reply<ProjectSnapshot> {
    let state: DesktopState<'_> = app.state();
    recover(&state, request)
        .await
        .unwrap_or_else(Fault::into_reply)
}

async fn recover(state: &Desktop, request: RecoverRequest) -> Outcome<Reply<ProjectSnapshot>> {
    let (location, _source_directory) = match (request.registration_id, request.root) {
        (Some(registration), None) => (ProjectLocation::Registration(registration), None),
        (None, Some(root)) => {
            let source = root.project_root(state)?.pin().await?;
            (ProjectLocation::Root(source.path.clone()), Some(source))
        }
        _ => {
            return Err(Fault::invalid(
                "Recovery needs one registered project or selected project folder.",
                "Select the project to recover.",
            ));
        }
    };
    let directory = match request.destination {
        Some(destination) => Some(destination.recovery_destination(state)?.pin().await?),
        None => None,
    };
    let recovery = match (request.source, &directory) {
        (RecoverySource::Backup { revision }, None) => ProjectRecovery::Backup {
            expected_revision: request.expected_revision,
            revision,
        },
        (RecoverySource::Backup { revision }, Some(directory)) => ProjectRecovery::CopyBackup {
            revision,
            destination: directory.path.clone(),
        },
        (RecoverySource::Draft { document }, Some(directory)) => ProjectRecovery::Draft {
            document: Box::new(read_document(document).await?),
            destination: directory.path.clone(),
        },
        (RecoverySource::Draft { .. }, None) => {
            return Err(Fault::invalid(
                "A recovered draft needs its own project folder.",
                "Choose a destination for the recovered copy.",
            ));
        }
    };
    Ok(state.projects.recover(location, recovery).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_edited_document_uses_shared_pointer_diagnostics() {
        let mut document = serde_json::to_value(ProjectDocument::new("Catalog".into())).unwrap();
        document["capabilities"] = serde_json::json!(42);
        let value =
            serde_json::to_value(project_validate(ValidateRequest { document }).await).unwrap();
        assert_eq!(value["error"]["code"], "project.invalid");
        assert_eq!(value["diagnostics"][0]["field"], "/capabilities");
        assert_eq!(value["diagnostics"][0]["code"], "project.invalid_field");
    }

    #[test]
    fn open_request_rejects_a_raw_path_and_mixed_location_handles() {
        assert!(
            serde_json::from_value::<OpenRequest>(serde_json::json!({
                "root": { "path": "C:\\example" }
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<OpenRequest>(serde_json::json!({
                "root": { "registrationId": Uuid::new_v4(), "selectionId": Uuid::new_v4() }
            }))
            .is_err()
        );
    }
}
