use std::fs;
use std::path::{Path, PathBuf};

use builder_app::{DiagnosticCode, Reply};
use serde::{Deserialize, Serialize};
use tauri::{Manager, Runtime, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

use crate::ipc::{self, Desktop, DesktopState, Fault, Outcome};

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Purpose {
    ProjectRoot,
    ProjectDestination,
    RecoveryDestination,
    ExportParent,
    PreviousExport,
    FileRoot,
}

impl Purpose {
    fn title(self) -> &'static str {
        match self {
            Self::ProjectRoot => "Open a project folder",
            Self::ProjectDestination => "Choose a folder for the new project",
            Self::RecoveryDestination => "Choose a folder for the recovered copy",
            Self::ExportParent => "Choose the folder that will contain the export",
            Self::PreviousExport => "Choose an existing standalone export",
            Self::FileRoot => "Choose an approved root for file reads",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectRequest {
    purpose: Purpose,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectedPath {
    selection_id: Uuid,
    display_path: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PathRef {
    selection_id: Uuid,
}

pub(crate) struct Selection {
    purpose: Purpose,
    directory: SelectedDirectory,
}

impl PathRef {
    pub(crate) fn project_root(self, state: &Desktop) -> Outcome<SelectedDirectory> {
        self.take(state, Purpose::ProjectRoot)
    }

    pub(crate) fn project_destination(self, state: &Desktop) -> Outcome<SelectedDirectory> {
        self.take(state, Purpose::ProjectDestination)
    }

    pub(crate) fn recovery_destination(self, state: &Desktop) -> Outcome<SelectedDirectory> {
        self.take(state, Purpose::RecoveryDestination)
    }

    pub(crate) fn export_parent(self, state: &Desktop) -> Outcome<SelectedDirectory> {
        self.take(state, Purpose::ExportParent)
    }

    pub(crate) fn previous_export(self, state: &Desktop) -> Outcome<SelectedDirectory> {
        self.take(state, Purpose::PreviousExport)
    }

    pub(crate) fn file_root(self, state: &Desktop) -> Outcome<SelectedDirectory> {
        self.take(state, Purpose::FileRoot)
    }

    fn take(self, state: &Desktop, purpose: Purpose) -> Outcome<SelectedDirectory> {
        state
            .paths
            .lock()
            .expect("path table")
            .take_if(self.selection_id, |item| item.purpose == purpose)
            .map(|(selection, _)| selection.directory)
            .ok_or_else(|| {
                Fault::invalid(
                    "The folder selection expired or is for a different action.",
                    "Choose the folder again.",
                )
            })
    }
}

#[tauri::command]
pub(crate) async fn select_path<R: Runtime>(
    window: WebviewWindow<R>,
    request: SelectRequest,
) -> Reply<Option<SelectedPath>> {
    let state: DesktopState<'_> = window.state();
    ipc::reply(pick(&window, &state, request.purpose).await)
}

async fn pick<R: Runtime>(
    window: &WebviewWindow<R>,
    state: &Desktop,
    purpose: Purpose,
) -> Outcome<Option<SelectedPath>> {
    let _picker = state.picking.try_lock().map_err(|_| {
        Fault::invalid(
            "A folder picker is already open.",
            "Finish or cancel the open picker before choosing another folder.",
        )
    })?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(window)
        .set_title(purpose.title())
        .pick_folder(move |selection| {
            let _ = sender.send(selection);
        });
    let Some(path) = receiver.await.map_err(|_| Fault::internal())? else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|_| unsafe_directory())?;
    let directory = ipc::on_disk(move || SelectedDirectory::new(path)).await?;
    let display_path = display_path(&directory.path);
    let selection_id = state
        .paths
        .lock()
        .expect("path table")
        .insert(Selection { purpose, directory })?;
    Ok(Some(SelectedPath {
        selection_id,
        display_path,
    }))
}

pub(crate) fn display_path(path: &Path) -> String {
    dunce::simplified(path).to_string_lossy().into_owned()
}

pub(crate) fn unsafe_directory() -> Fault {
    Fault::new(
        DiagnosticCode::ProjectUnsafePath,
        "The selected local folder is unavailable or has changed.",
        "Choose an accessible local folder and try again.",
        None,
    )
}

pub(crate) struct SelectedDirectory {
    path: PathBuf,
    identity: DirectoryIdentity,
}

impl SelectedDirectory {
    fn new(path: PathBuf) -> Outcome<Self> {
        if !path.is_absolute() || path.to_str().is_none() {
            return Err(unsafe_directory());
        }
        let (_, identity) = pin_directory(&path)?;
        Ok(Self { path, identity })
    }

    pub(crate) async fn pin(self) -> Outcome<PinnedDirectory> {
        ipc::on_disk(move || self.pin_now()).await
    }

    pub(crate) fn pin_now(&self) -> Outcome<PinnedDirectory> {
        let (directory, identity) = pin_directory(&self.path)?;
        if identity != self.identity {
            return Err(unsafe_directory());
        }
        Ok(directory)
    }
}

pub(crate) struct PinnedDirectory {
    pub path: PathBuf,
    #[cfg(windows)]
    _ancestors: Vec<fs::File>,
}

#[derive(PartialEq, Eq)]
struct DirectoryIdentity(u64, u64);

#[cfg(windows)]
fn pin_directory(path: &Path) -> Outcome<(PinnedDirectory, DirectoryIdentity)> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Component, Prefix};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
        GetFileInformationByHandle,
    };

    if !matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    ) {
        return Err(unsafe_directory());
    }
    let components: Vec<_> = path.ancestors().filter(|item| item.is_absolute()).collect();
    let mut ancestors = Vec::with_capacity(components.len());
    // Prevent replacement of the selected directory or any path ancestor during I/O.
    for component in components.into_iter().rev() {
        let file = fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(component)
            .map_err(|_| unsafe_directory())?;
        let metadata = file.metadata().map_err(|_| unsafe_directory())?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(unsafe_directory());
        }
        ancestors.push(file);
    }
    let file = ancestors.last().ok_or_else(unsafe_directory)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: The handle is live and the buffer is valid for one complete result.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(unsafe_directory());
    }
    let identity = DirectoryIdentity(
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    );
    Ok((
        PinnedDirectory {
            path: path.to_path_buf(),
            _ancestors: ancestors,
        },
        identity,
    ))
}

#[cfg(unix)]
fn pin_directory(path: &Path) -> Outcome<(PinnedDirectory, DirectoryIdentity)> {
    use std::os::unix::fs::MetadataExt;

    for ancestor in path.ancestors() {
        if !fs::symlink_metadata(ancestor).is_ok_and(|item| item.is_dir()) {
            return Err(unsafe_directory());
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| unsafe_directory())?;
    Ok((
        PinnedDirectory {
            path: path.to_path_buf(),
        },
        DirectoryIdentity(metadata.dev(), metadata.ino()),
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use builder_app::ProjectService;

    pub(crate) fn record_fixture_selection(state: &Desktop, path: &Path, purpose: &str) -> Uuid {
        let purpose = serde_json::from_value(serde_json::json!(purpose)).unwrap();
        let directory = SelectedDirectory::new(fs::canonicalize(path).unwrap()).unwrap();
        state
            .paths
            .lock()
            .unwrap()
            .insert(Selection { purpose, directory })
            .unwrap()
    }

    #[test]
    fn selections_are_purpose_bound_and_consumed_only_once() {
        let folder = tempfile::tempdir().unwrap();
        let state = Desktop::new(ProjectService::for_user());
        let selection_id = record_fixture_selection(&state, folder.path(), "projectRoot");
        let chosen = PathRef { selection_id };
        assert!(chosen.export_parent(&state).is_err());
        assert!(chosen.project_root(&state).is_ok());
        assert!(chosen.project_root(&state).is_err());
    }
}
