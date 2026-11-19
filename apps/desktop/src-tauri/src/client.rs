use builder_app::{
    ClientConfigExport, ClientConfigRequest, ClientConfigTarget, Reply, export_client_config,
    save_client_config,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};
use uuid::Uuid;

use crate::ipc::{self, Desktop, DesktopState, Fault, Outcome};
use crate::paths::{PathRef, pick_new_file};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExportRequest {
    target: ClientConfigTarget,
    name: String,
    executable: PathRef,
    manifest: PathRef,
    runtime_config: Option<PathRef>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportResult {
    configuration_id: Uuid,
    #[serde(flatten)]
    export: ClientConfigExport,
}

#[tauri::command]
pub(crate) async fn client_config_export<R: Runtime>(
    app: AppHandle<R>,
    request: ExportRequest,
) -> Reply<ExportResult> {
    let state: DesktopState<'_> = app.state();
    ipc::reply(render(&state, request).await)
}

async fn render(state: &Desktop, request: ExportRequest) -> Outcome<ExportResult> {
    let executable = request.executable.client_executable(state)?;
    let manifest = request.manifest.client_manifest(state)?;
    let runtime_config = request
        .runtime_config
        .map(|path| path.client_runtime_config(state))
        .transpose()?;
    let export = ipc::on_disk(move || {
        let executable = executable.pin_now()?;
        let manifest = manifest.pin_now()?;
        let runtime_config = runtime_config.map(|path| path.pin_now()).transpose()?;
        ipc::unpack(export_client_config(
            ClientConfigRequest {
                target: request.target,
                name: request.name,
                executable: executable.path.clone(),
                manifest: manifest.path.clone(),
                runtime_config: runtime_config.as_ref().map(|file| file.path.clone()),
            },
            None,
        ))
        .map(|(export, _)| export)
    })
    .await?;
    let configuration_id = state
        .client_configs
        .lock()
        .expect("client config table")
        .insert(export.clone())?;
    Ok(ExportResult {
        configuration_id,
        export,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveRequest {
    configuration_id: Uuid,
}

#[tauri::command]
pub(crate) async fn client_config_save<R: Runtime>(
    window: WebviewWindow<R>,
    request: SaveRequest,
) -> Reply<Option<ClientConfigExport>> {
    let state: DesktopState<'_> = window.state();
    ipc::reply(save(&window, &state, request.configuration_id).await)
}

async fn save<R: Runtime>(
    window: &WebviewWindow<R>,
    state: &Desktop,
    configuration_id: Uuid,
) -> Outcome<Option<ClientConfigExport>> {
    let suggested = cached_export(state, configuration_id)?.suggested_file_name;
    let Some(destination) = pick_new_file(window, state, &suggested).await? else {
        return Ok(None);
    };
    let export = cached_export(state, configuration_id)?;
    ipc::on_disk(move || {
        ipc::unpack(save_client_config(export, &destination)).map(|(export, _)| Some(export))
    })
    .await
}

fn cached_export(state: &Desktop, id: Uuid) -> Outcome<ClientConfigExport> {
    state
        .client_configs
        .lock()
        .expect("client config table")
        .get(id)
        .cloned()
        .ok_or_else(|| {
            Fault::invalid(
                "The client configuration preview expired.",
                "Generate the client configuration again before saving.",
            )
        })
}
