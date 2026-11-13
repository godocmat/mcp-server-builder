use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use builder_project::{MANIFEST_NAME, ManifestRevision, ValidatedManifest};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, SqliteConnection, SqlitePool};
use uuid::Uuid;

use crate::{local_fs, project_files};

const CATALOG_SCHEMA: i64 = 1;
const DATABASE: &str = "catalog.sqlite3";
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub(crate) struct Catalog {
    pool: SqlitePool,
}

pub(crate) struct Registration {
    pub id: Uuid,
    pub root: PathBuf,
    pub project_id: Uuid,
    pub name: String,
    pub revision: ManifestRevision,
}

type Row = (String, String, String, String, String);

#[derive(Debug)]
pub(crate) struct Unavailable;

pub(crate) fn user_profile() -> Option<PathBuf> {
    dirs::data_local_dir().map(|path| path.join("com.mcpserverbuilder.desktop"))
}

impl Catalog {
    pub async fn connect(profile: PathBuf) -> Result<Self, Unavailable> {
        // Take the migration lock outside the async runtime. It spans the consistent
        // backup and the transaction, and is also honored by other builder processes.
        let (database, _migration_lock) = tokio::task::spawn_blocking(move || {
            let parent = profile.parent().ok_or(Unavailable)?;
            if !profile.is_absolute() {
                return Err(Unavailable);
            }
            fs::create_dir_all(parent).map_err(|_| Unavailable)?;
            local_fs::create_private_dir(&profile).map_err(|_| Unavailable)?;
            let database = profile.join(DATABASE);
            let lock = local_fs::lock(&profile.join("catalog.lock")).map_err(|_| Unavailable)?;
            for filename in [
                DATABASE,
                "catalog.sqlite3-wal",
                "catalog.sqlite3-shm",
                "catalog.sqlite3-journal",
            ] {
                let path = profile.join(filename);
                if local_fs::file_exists(&path).map_err(|_| Unavailable)? {
                    local_fs::read_file(&path).map_err(|_| Unavailable)?;
                }
            }
            Ok((database, lock))
        })
        .await
        .map_err(|_| Unavailable)??;

        if local_fs::file_exists(&database).map_err(|_| Unavailable)? {
            let read_only = SqliteConnectOptions::new()
                .filename(&database)
                .read_only(true)
                .busy_timeout(TIMEOUT)
                .pragma("trusted_schema", "OFF")
                .disable_statement_logging();
            let mut connection = read_only.connect().await.map_err(|_| Unavailable)?;
            let version = catalog_version(&mut connection).await?;
            sqlx::Connection::close(connection)
                .await
                .map_err(|_| Unavailable)?;
            if !matches!(version, 0 | CATALOG_SCHEMA) {
                return Err(Unavailable);
            }
        }

        // Open before setting WAL to avoid changing a future catalog on a version rejection.
        let options = SqliteConnectOptions::new()
            .filename(&database)
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(TIMEOUT)
            .synchronous(SqliteSynchronous::Full)
            .pragma("trusted_schema", "OFF")
            .disable_statement_logging();
        let mut connection = options.connect().await.map_err(|_| Unavailable)?;
        initialize(&mut connection, database.parent().ok_or(Unavailable)?).await?;
        sqlx::Connection::close(connection)
            .await
            .map_err(|_| Unavailable)?;

        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .acquire_timeout(TIMEOUT)
            .connect_with(options.journal_mode(SqliteJournalMode::Wal))
            .await
            .map_err(|_| Unavailable)?;
        Ok(Self { pool })
    }

    pub async fn root(&self, id: Uuid) -> Result<Option<Registration>, Unavailable> {
        sqlx::query_as::<_, Row>(
            "SELECT registration_id, root, project_id, name, revision FROM projects WHERE registration_id = ?",
        )
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| Unavailable)?
            .map(decode)
            .transpose()
    }

    pub async fn list(&self) -> Result<Vec<Registration>, Unavailable> {
        sqlx::query_as::<_, Row>(
            "SELECT registration_id, root, project_id, name, revision FROM projects ORDER BY name COLLATE NOCASE, root",
        )
            .fetch_all(&self.pool)
            .await
            .map_err(|_| Unavailable)?
            .into_iter()
            .map(decode)
            .collect()
    }

    pub async fn reconcile(
        &self,
        id: Uuid,
        root: &Path,
        manifest: &ValidatedManifest,
    ) -> Result<(), Unavailable> {
        let root = root.to_str().ok_or(Unavailable)?;
        let document = manifest.document();
        let mut transaction = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|_| Unavailable)?;
        // Check under the catalog's write transaction so a slower open cannot
        // publish an older index entry after a concurrent save has reconciled.
        let live_path = Path::new(root).join(MANIFEST_NAME);
        let expected = manifest.revision();
        let current = tokio::task::spawn_blocking(move || project_files::read(&live_path))
            .await
            .map_err(|_| Unavailable)?
            .map_err(|_| Unavailable)?;
        if current.revision != expected {
            return Err(Unavailable);
        }
        // A different portable UUID installed in the same root gets a different
        // registration. Historical local state stays scoped to the prior registration.
        sqlx::query(
            "INSERT INTO projects (registration_id, root, project_id, name, revision)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(registration_id) DO UPDATE SET name = excluded.name, revision = excluded.revision",
        )
        .bind(id.to_string())
        .bind(root)
        .bind(document.project_id.to_string())
        .bind(&document.name)
        .bind(manifest.revision().to_string())
        .execute(&mut *transaction)
        .await
        .map_err(|_| Unavailable)?;
        transaction.commit().await.map_err(|_| Unavailable)
    }
}

fn decode((id, root, project_id, name, revision): Row) -> Result<Registration, Unavailable> {
    Ok(Registration {
        id: id.parse().map_err(|_| Unavailable)?,
        root: PathBuf::from(root),
        project_id: project_id.parse().map_err(|_| Unavailable)?,
        name,
        revision: revision.parse().map_err(|_| Unavailable)?,
    })
}

async fn initialize(connection: &mut SqliteConnection, profile: &Path) -> Result<(), Unavailable> {
    let version = catalog_version(connection).await?;
    match version {
        CATALOG_SCHEMA => return Ok(()),
        0 => {}
        _ => return Err(Unavailable),
    }

    let initialized: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*')",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| Unavailable)?;
    if initialized {
        backup(connection, profile).await?;
    }

    let mut migration = sqlx::Connection::begin_with(connection, "BEGIN EXCLUSIVE")
        .await
        .map_err(|_| Unavailable)?;
    sqlx::query(
        "CREATE TABLE projects (
            registration_id TEXT PRIMARY KEY NOT NULL,
            root TEXT NOT NULL,
            project_id TEXT NOT NULL,
            name TEXT NOT NULL,
            revision TEXT NOT NULL
         ) STRICT",
    )
    .execute(&mut *migration)
    .await
    .map_err(|_| Unavailable)?;
    sqlx::query("PRAGMA user_version = 1")
        .execute(&mut *migration)
        .await
        .map_err(|_| Unavailable)?;
    migration.commit().await.map_err(|_| Unavailable)
}

async fn catalog_version(connection: &mut SqliteConnection) -> Result<i64, Unavailable> {
    sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(connection)
        .await
        .map_err(|_| Unavailable)
}

async fn backup(connection: &mut SqliteConnection, profile: &Path) -> Result<(), Unavailable> {
    let path = profile.join("catalog.pre-schema-1.sqlite3");
    let staged = tempfile::NamedTempFile::with_prefix_in(".catalog-backup-", profile)
        .map_err(|_| Unavailable)?;
    let staged = staged.into_temp_path();
    sqlx::query("VACUUM main INTO ?")
        .bind(staged.to_str().ok_or(Unavailable)?)
        .execute(connection)
        .await
        .map_err(|_| Unavailable)?;
    tokio::task::spawn_blocking(move || -> io::Result<()> {
        // VACUUM INTO flushes its output with the source connection's FULL synchronous setting.
        // Preserve the first complete backup across retries of a failed migration.
        match staged.persist_noclobber(&path) {
            Ok(()) => {}
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                local_fs::read_file(&path)?;
            }
            Err(error) => return Err(error.error),
        }
        local_fs::flush_directory(path.parent().expect("backup has a parent"))
    })
    .await
    .map_err(|_| Unavailable)?
    .map_err(|_| Unavailable)
}
