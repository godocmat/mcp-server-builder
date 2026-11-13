use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use builder_project::{
    MANIFEST_NAME, ManifestError, ManifestRevision, ProjectDocument, ValidatedManifest,
    parse_manifest, validate_document,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::catalog::{self, Catalog, Registration};
use crate::project_files::{self, FileError, LockedRoot};
use crate::{DiagnosticCode, DiagnosticDetails, Reply};

mod diagnostics;
use diagnostics::{Failure, Operation};

pub(crate) fn invalid_manifest_reply<T>(error: ManifestError) -> Reply<T> {
    Operation::new().finish(Err(Failure::manifest(error)))
}

#[derive(Clone, Debug)]
pub enum ProjectLocation {
    /// A native path already selected by the operator or passed on the CLI.
    Root(PathBuf),
    Registration(Uuid),
}

#[derive(Debug)]
pub enum ProjectRecovery {
    /// A null expected revision requests an atomic restore into a missing manifest.
    Backup {
        expected_revision: Option<ManifestRevision>,
        revision: ManifestRevision,
    },
    Draft {
        document: Box<ProjectDocument>,
        destination: PathBuf,
    },
    CopyBackup {
        revision: ManifestRevision,
        destination: PathBuf,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSnapshot {
    pub registration_id: Uuid,
    pub root_display_path: String,
    pub revision: ManifestRevision,
    pub document: ProjectDocument,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub registration_id: Uuid,
    pub root_display_path: String,
    pub name: String,
    pub last_known_revision: ManifestRevision,
    pub status: ProjectStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectStatus {
    Available,
    Changed,
    Missing,
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct ProjectValidation {
    pub valid: bool,
}

#[derive(Clone)]
pub struct ProjectService {
    profile: Option<PathBuf>,
    catalog: Arc<OnceCell<Catalog>>,
}

impl ProjectService {
    pub fn for_user() -> Self {
        Self::new(catalog::user_profile())
    }

    /// Use a dedicated application data directory for an alternate local profile.
    pub fn with_profile(profile: PathBuf) -> Self {
        Self::new(Some(profile))
    }

    fn new(profile: Option<PathBuf>) -> Self {
        Self {
            profile,
            catalog: Arc::new(OnceCell::new()),
        }
    }

    async fn catalog(&self) -> Result<&Catalog, Failure> {
        self.catalog
            .get_or_try_init(|| async {
                let profile = self.profile.clone().ok_or_else(Failure::catalog)?;
                Catalog::connect(profile)
                    .await
                    .map_err(|_| Failure::catalog())
            })
            .await
    }

    async fn resolve(&self, location: ProjectLocation) -> Result<Location, Failure> {
        match location {
            ProjectLocation::Root(root) => Ok(Location {
                root,
                registration: None,
            }),
            ProjectLocation::Registration(id) => self
                .catalog()
                .await?
                .root(id)
                .await
                .map_err(|_| Failure::catalog())?
                .map(|record| Location {
                    root: record.root,
                    registration: Some((id, record.project_id)),
                })
                .ok_or_else(Failure::registration),
        }
    }

    pub async fn list(&self) -> Reply<Vec<ProjectSummary>> {
        let operation = Operation::new();
        let result = async {
            let rows = self
                .catalog()
                .await?
                .list()
                .await
                .map_err(|_| Failure::catalog())?;
            on_disk(move || Ok(rows.into_iter().map(summarize).collect())).await
        }
        .await;
        operation.finish(result)
    }

    pub async fn open(&self, location: ProjectLocation) -> Reply<ProjectSnapshot> {
        let mut operation = Operation::new();
        let result = async {
            let location = self.resolve(location).await?;
            let (root, manifest) = on_disk(move || {
                let root = project_files::canonical_root(&location.root)?;
                let manifest = read_valid(&root, location.project_id())?;
                location.check_identity(&root, &manifest)?;
                Ok((root, manifest))
            })
            .await?;
            Ok(self.snapshot(&mut operation, &root, manifest).await)
        }
        .await;
        operation.finish(result)
    }

    pub async fn create(&self, destination: PathBuf, name: String) -> Reply<ProjectSnapshot> {
        self.mutate(Mutation::Create {
            destination,
            document: Box::new(ProjectDocument::new(name)),
        })
        .await
    }

    pub async fn save(
        &self,
        location: ProjectLocation,
        expected_revision: ManifestRevision,
        document: ProjectDocument,
    ) -> Reply<ProjectSnapshot> {
        self.mutate(Mutation::Save {
            location,
            expected_revision,
            document: Box::new(document),
        })
        .await
    }

    pub async fn recover(
        &self,
        location: ProjectLocation,
        source: ProjectRecovery,
    ) -> Reply<ProjectSnapshot> {
        let mutation = match source {
            ProjectRecovery::Backup {
                expected_revision,
                revision,
            } => Mutation::Restore {
                location,
                expected_revision,
                revision,
            },
            ProjectRecovery::Draft {
                mut document,
                destination,
            } => {
                document.project_id = Uuid::new_v4();
                Mutation::Create {
                    destination,
                    document,
                }
            }
            ProjectRecovery::CopyBackup {
                revision,
                destination,
            } => Mutation::CopyBackup {
                location,
                revision,
                destination,
            },
        };
        self.mutate(mutation).await
    }

    async fn mutate(&self, mutation: Mutation) -> Reply<ProjectSnapshot> {
        let service = self.clone();
        // Once admitted, let the native save and index transaction finish even if the
        // caller stops waiting for its reply.
        tokio::spawn(async move {
            let mut operation = Operation::new();
            let result = async {
                let committed = match mutation {
                    Mutation::Create {
                        destination,
                        document,
                    } => on_disk(move || create_file(&destination, *document)).await?,
                    Mutation::Save {
                        location,
                        expected_revision,
                        document,
                    } => {
                        let location = service.resolve(location).await?;
                        on_disk(move || save_file(location, expected_revision, *document)).await?
                    }
                    Mutation::Restore {
                        location,
                        expected_revision,
                        revision,
                    } => {
                        let location = service.resolve(location).await?;
                        on_disk(move || restore_file(location, expected_revision, revision)).await?
                    }
                    Mutation::CopyBackup {
                        location,
                        revision,
                        destination,
                    } => {
                        let location = service.resolve(location).await?;
                        on_disk(move || {
                            let root = project_files::canonical_root(&location.root)?;
                            let backup = project_files::read_backup(&root, revision)?;
                            location.check_identity(&root, &backup)?;
                            let mut document = backup.into_document();
                            document.project_id = Uuid::new_v4();
                            create_file(&destination, document)
                        })
                        .await?
                    }
                };
                if !committed.durable {
                    operation.warn(
                        DiagnosticCode::StorageDurabilityUnconfirmed,
                        "The manifest was published and its directory flush failed.",
                        "Reopen the folder to verify the committed revision.",
                        DiagnosticDetails::CommittedRevision {
                            committed_revision: committed.manifest.revision(),
                        },
                    );
                }
                Ok(service
                    .snapshot(&mut operation, &committed.root.path, committed.manifest)
                    .await)
            }
            .await;
            operation.finish(result)
        })
        .await
        .unwrap_or_else(|_| Operation::new().finish(Err(Failure::internal())))
    }

    async fn snapshot(
        &self,
        operation: &mut Operation,
        root: &Path,
        manifest: ValidatedManifest,
    ) -> ProjectSnapshot {
        let revision = manifest.revision();
        let id = registration_id(root, manifest.document().project_id);
        let reconciled = match self.catalog().await {
            Ok(catalog) => catalog.reconcile(id, root, &manifest).await.is_ok(),
            Err(_) => false,
        };
        if !reconciled {
            operation.warn(
                DiagnosticCode::ProjectIndexRepairRequired,
                "The project is available on disk and its catalog index needs repair.",
                "Reopen the project by its folder to retry indexing.",
                DiagnosticDetails::CommittedRevision {
                    committed_revision: revision,
                },
            );
        }
        ProjectSnapshot {
            registration_id: id,
            root_display_path: display_path(root),
            revision,
            document: manifest.into_document(),
        }
    }
}

pub fn project_validate(bytes: &[u8]) -> Reply<ProjectValidation> {
    Operation::new().finish(
        parse_manifest(bytes)
            .map(|_| ProjectValidation { valid: true })
            .map_err(Failure::manifest),
    )
}

pub fn project_read_draft(bytes: &[u8]) -> Reply<ProjectDocument> {
    Operation::new().finish(
        parse_manifest(bytes)
            .map(ValidatedManifest::into_document)
            .map_err(Failure::manifest),
    )
}

pub fn project_validate_document(document: ProjectDocument) -> Reply<ProjectValidation> {
    Operation::new().finish(
        validate_document(document)
            .map(|_| ProjectValidation { valid: true })
            .map_err(Failure::manifest),
    )
}

pub async fn project_validate_root(root: PathBuf) -> Reply<ProjectValidation> {
    let result = on_disk(move || {
        read_valid(&project_files::canonical_root(&root)?, None)?;
        Ok(ProjectValidation { valid: true })
    })
    .await;
    Operation::new().finish(result)
}

enum Mutation {
    Create {
        destination: PathBuf,
        document: Box<ProjectDocument>,
    },
    Save {
        location: ProjectLocation,
        expected_revision: ManifestRevision,
        document: Box<ProjectDocument>,
    },
    Restore {
        location: ProjectLocation,
        expected_revision: Option<ManifestRevision>,
        revision: ManifestRevision,
    },
    CopyBackup {
        location: ProjectLocation,
        revision: ManifestRevision,
        destination: PathBuf,
    },
}

struct Location {
    root: PathBuf,
    registration: Option<(Uuid, Uuid)>,
}

impl Location {
    fn project_id(&self) -> Option<Uuid> {
        self.registration.map(|(_, project)| project)
    }

    fn check_identity(&self, root: &Path, manifest: &ValidatedManifest) -> Result<(), Failure> {
        if self
            .registration
            .is_some_and(|(id, _)| id != registration_id(root, manifest.document().project_id))
        {
            return Err(Failure::registration());
        }
        Ok(())
    }
}

struct Committed {
    root: LockedRoot,
    manifest: ValidatedManifest,
    durable: bool,
}

fn create_file(destination: &Path, document: ProjectDocument) -> Result<Committed, Failure> {
    let manifest = validate_document(document).map_err(Failure::manifest)?;
    let created_root = match fs::create_dir(destination) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(FileError::from(error).into()),
    };
    let path = project_files::canonical_root(destination)?;
    if crate::local_fs::file_exists(&path.join(MANIFEST_NAME)).map_err(FileError::from)? {
        return Err(FileError::Exists.into());
    }
    let mut root = LockedRoot::open(&path)?;
    let durable = root
        .create(&manifest)
        .map_err(|error| Failure::file(error).recoverable(&root.path, None, None))?;
    let durable = durable
        && (!created_root
            || root
                .path
                .parent()
                .is_some_and(|parent| crate::local_fs::flush_directory(parent).is_ok()));
    Ok(Committed {
        root,
        manifest,
        durable,
    })
}

fn save_file(
    location: Location,
    expected: ManifestRevision,
    document: ProjectDocument,
) -> Result<Committed, Failure> {
    let manifest = validate_document(document).map_err(Failure::manifest)?;
    let mut root = LockedRoot::open(&location.root)?;
    let current = expect_current(&root, expected, location.project_id())?;
    let prior = current.validate().map_err(|error| {
        Failure::manifest(error).recoverable(
            &root.path,
            Some(current.revision),
            location.project_id(),
        )
    })?;
    location.check_identity(&root.path, &prior)?;
    if prior.document().project_id != manifest.document().project_id {
        return Err(Failure::field(
            "/projectId",
            "Keep the project UUID when saving an existing project.",
        ));
    }
    let durable = root
        .replace(&current, Some(&prior), &manifest)
        .map_err(|error| {
            Failure::file(error).recoverable(
                &root.path,
                Some(current.revision),
                Some(prior.document().project_id),
            )
        })?;
    Ok(Committed {
        root,
        manifest,
        durable,
    })
}

fn restore_file(
    location: Location,
    expected: Option<ManifestRevision>,
    backup: ManifestRevision,
) -> Result<Committed, Failure> {
    let mut root = LockedRoot::open(&location.root)?;
    let current = match expected {
        Some(expected) => Some(expect_current(&root, expected, location.project_id())?),
        None => match root.read() {
            Err(FileError::Missing) => None,
            Ok(file) => return Err(FileError::Conflict(Some(file.revision)).into()),
            Err(error) => return Err(error.into()),
        },
    };
    let current_revision = current.as_ref().map(|file| file.revision);
    let prior = match current.as_ref().map(|file| file.validate()) {
        Some(Ok(manifest)) => Some(manifest),
        Some(Err(error @ ManifestError::UnsupportedSchema { .. })) => {
            return Err(Failure::manifest(error).recoverable(
                &root.path,
                current_revision,
                location.project_id(),
            ));
        }
        _ => None,
    };
    let manifest = project_files::read_backup(&root.path, backup).map_err(|error| {
        Failure::file(error).recoverable(&root.path, current_revision, location.project_id())
    })?;
    location.check_identity(&root.path, &manifest)?;
    if prior
        .as_ref()
        .is_some_and(|prior| prior.document().project_id != manifest.document().project_id)
    {
        return Err(FileError::RecoveryUnavailable.into());
    }
    let durable = match &current {
        Some(current) => root.replace(current, prior.as_ref(), &manifest),
        None => match root.create(&manifest) {
            Err(FileError::Exists) => Err(FileError::Conflict(
                root.read().ok().map(|file| file.revision),
            )),
            result => result,
        },
    }
    .map_err(|error| {
        Failure::file(error).recoverable(
            &root.path,
            current_revision,
            Some(manifest.document().project_id),
        )
    })?;
    Ok(Committed {
        root,
        manifest,
        durable,
    })
}

fn expect_current(
    root: &LockedRoot,
    expected: ManifestRevision,
    project_id: Option<Uuid>,
) -> Result<project_files::CurrentFile, Failure> {
    match root.read() {
        Ok(file) if file.revision == expected => Ok(file),
        Ok(file) => Err(FileError::Conflict(Some(file.revision)).into()),
        Err(FileError::Missing) => {
            Err(Failure::file(FileError::Conflict(None)).recoverable(&root.path, None, project_id))
        }
        Err(error) => Err(Failure::file(error).recoverable(&root.path, None, project_id)),
    }
}

fn read_valid(root: &Path, project_id: Option<Uuid>) -> Result<ValidatedManifest, Failure> {
    let file = project_files::read(&root.join(MANIFEST_NAME))
        .map_err(|error| Failure::file(error).recoverable(root, None, project_id))?;
    file.validate().map_err(|error| {
        Failure::manifest(error).recoverable(root, Some(file.revision), project_id)
    })
}

fn summarize(record: Registration) -> ProjectSummary {
    let status = match project_files::read(&record.root.join(MANIFEST_NAME)) {
        Ok(file) if file.revision == record.revision => ProjectStatus::Available,
        Ok(_) => ProjectStatus::Changed,
        Err(FileError::Missing) => ProjectStatus::Missing,
        Err(_) => ProjectStatus::Unavailable,
    };
    ProjectSummary {
        registration_id: record.id,
        root_display_path: display_path(&record.root),
        name: record.name,
        last_known_revision: record.revision,
        status,
    }
}

fn registration_id(root: &Path, project_id: Uuid) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(b"mcp-server-builder.local-registration\0");
    hash.update(project_id.as_bytes());
    hash.update(root.as_os_str().as_encoded_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash.finalize()[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

fn display_path(path: &Path) -> String {
    dunce::simplified(path).to_string_lossy().into_owned()
}

async fn on_disk<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| Failure::internal())?
}
