use std::fs;
use std::path::{Path, PathBuf};

use builder_app::{
    DiagnosticCode, ManifestRevision, ProjectLocation, ProjectRecovery, ProjectService,
    ProjectStatus, Reply, project_validate, project_validate_root,
};
use builder_project::{MANIFEST_NAME, parse_manifest};
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{ConnectOptions, Connection, SqliteConnection};

const FIXTURE: &[u8] = include_bytes!("../../builder-project/tests/fixtures/catalog-v1.json");
const BACKUP: &str = ".mcp-project.last-valid.json";

struct Fixture {
    _directory: tempfile::TempDir,
    profile: PathBuf,
    root: PathBuf,
    service: ProjectService,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("profile");
        let root = directory.path().join("project");
        let service = ProjectService::with_profile(profile.clone());
        Self {
            _directory: directory,
            profile,
            root,
            service,
        }
    }

    fn at_root(&self) -> ProjectLocation {
        ProjectLocation::Root(self.root.clone())
    }

    fn install(&self, bytes: &[u8]) {
        fs::create_dir_all(&self.root).unwrap();
        fs::write(self.root.join(MANIFEST_NAME), bytes).unwrap();
    }
}

fn ok<T: std::fmt::Debug>(reply: Reply<T>) -> T {
    match reply {
        Reply::Success { data, diagnostics } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            data
        }
        error => panic!("expected success, got {error:?}"),
    }
}

fn failure<T: serde::Serialize>(reply: Reply<T>, code: &str) -> Value {
    let json = serde_json::to_value(reply).unwrap();
    assert_eq!(json["ok"], false, "{json}");
    assert_eq!(json["error"]["code"], code, "{json}");
    for diagnostic in json["diagnostics"].as_array().unwrap() {
        assert_eq!(diagnostic["correlationId"], json["error"]["correlationId"]);
    }
    json
}

async fn sqlite(path: &Path) -> SqliteConnection {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .connect()
        .await
        .unwrap()
}

#[tokio::test]
async fn creates_opens_saves_and_lists_an_isolated_catalog() {
    let fixture = Fixture::new();
    let initial = ok(fixture
        .service
        .create(fixture.root.clone(), "Initial".into())
        .await);
    let mut document = parse_manifest(FIXTURE).unwrap().into_document();
    document.project_id = initial.document.project_id;
    let location = ProjectLocation::Registration(initial.registration_id);
    let saved = ok(fixture
        .service
        .save(location.clone(), initial.revision, document.clone())
        .await);
    let independent = ProjectService::with_profile(fixture.profile.clone());
    let opened = ok(independent.open(location).await);
    assert_eq!(opened.revision, saved.revision);
    assert_eq!(opened.document, document);
    assert_eq!(opened.registration_id, initial.registration_id);
    assert_eq!(
        opened.revision,
        ManifestRevision::of(&fs::read(fixture.root.join(MANIFEST_NAME)).unwrap())
    );
    assert_eq!(
        ManifestRevision::of(&fs::read(fixture.root.join(BACKUP)).unwrap()),
        initial.revision
    );

    let copy = fixture.root.with_file_name("copy");
    fs::create_dir(&copy).unwrap();
    fs::copy(fixture.root.join(MANIFEST_NAME), copy.join(MANIFEST_NAME)).unwrap();
    let copied = ok(independent.open(ProjectLocation::Root(copy)).await);
    assert_eq!(copied.document, opened.document);
    assert_ne!(copied.registration_id, opened.registration_id);
    assert_eq!(ok(independent.list().await).len(), 2);

    let mut db = sqlite(&fixture.profile.join("catalog.sqlite3")).await;
    let wal: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(wal, "wal");
    let row_count: i64 = sqlx::query_scalar("SELECT count(*) FROM projects")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(row_count, 2);
}

#[tokio::test]
async fn read_only_validation_and_open_leave_project_bytes_and_credentials_inert() {
    let fixture = Fixture::new();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let bytes = std::str::from_utf8(FIXTURE).unwrap().replace(
        "https://catalog.example.test",
        &format!("http://{}", listener.local_addr().unwrap()),
    );
    fixture.install(bytes.as_bytes());
    ok(project_validate(bytes.as_bytes()));
    ok(project_validate_root(fixture.root.clone()).await);
    assert!(!fixture.profile.exists());

    let opened = ok(fixture.service.open(fixture.at_root()).await);
    assert_eq!(opened.revision, ManifestRevision::of(bytes.as_bytes()));
    assert_eq!(opened.document.secret_refs[0].id, "catalog_auth");
    assert_eq!(
        fs::read(fixture.root.join(MANIFEST_NAME)).unwrap(),
        bytes.as_bytes()
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let entries: Vec<_> = fs::read_dir(&fixture.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, [MANIFEST_NAME]);

    let inline = bytes.replace(r#""catalog_auth"}"#, r#""M1_SYNTHETIC_SECRET_VALUE"}"#);
    fixture.install(inline.as_bytes());
    let error = failure(
        fixture.service.open(fixture.at_root()).await,
        "project.invalid",
    );
    assert!(!error.to_string().contains("M1_SYNTHETIC_SECRET_VALUE"));
    assert_eq!(
        error["diagnostics"][0]["field"],
        "/bindings/0/headersFromSecrets"
    );
    let listed = ok(fixture.service.list().await);
    assert_eq!(listed[0].status, ProjectStatus::Changed);
    assert_eq!(listed[0].last_known_revision, opened.revision);
}

#[tokio::test]
async fn unsupported_manifests_are_preserved_and_do_not_advance_the_index() {
    let fixture = Fixture::new();
    fixture.install(FIXTURE);
    let initial = ok(fixture.service.open(fixture.at_root()).await);
    let mut next = initial.document.clone();
    next.name = "Edited".into();
    ok(fixture
        .service
        .save(fixture.at_root(), initial.revision, next)
        .await);
    let bytes = br#"{"format":"mcp-server-builder.project","schemaVersion":2,"transport":{"kind":"stdio"},"newField":true}"#;
    fixture.install(bytes);
    let expected = ManifestRevision::of(bytes);
    let error = failure(
        fixture.service.open(fixture.at_root()).await,
        "project.unsupported_schema",
    );
    assert_eq!(
        error["error"]["details"],
        json!({
            "format": "mcp-server-builder.project", "schemaVersion": 2, "supportedVersions": [1]
        })
    );
    assert_eq!(
        error["diagnostics"][0]["details"]["currentRevision"],
        json!(expected)
    );
    failure(
        fixture
            .service
            .save(fixture.at_root(), expected, initial.document.clone())
            .await,
        "project.unsupported_schema",
    );
    failure(
        fixture
            .service
            .recover(
                fixture.at_root(),
                ProjectRecovery::Backup {
                    expected_revision: Some(expected),
                    revision: initial.revision,
                },
            )
            .await,
        "project.unsupported_schema",
    );
    assert_eq!(fs::read(fixture.root.join(MANIFEST_NAME)).unwrap(), bytes);
    assert_eq!(fs::read(fixture.root.join(BACKUP)).unwrap(), FIXTURE);
    assert_eq!(
        ok(fixture.service.list().await)[0].status,
        ProjectStatus::Changed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn competing_saves_have_one_winner_and_the_losing_draft_can_be_copied() {
    let fixture = Fixture::new();
    fixture.install(FIXTURE);
    let initial = ok(fixture.service.open(fixture.at_root()).await);
    let mut left = initial.document.clone();
    let mut right = left.clone();
    left.name = "Left".into();
    right.name = "Right".into();
    let independent = ProjectService::with_profile(fixture.profile.clone());
    let alias = fixture.root.join(".");
    let (first, second) = tokio::join!(
        fixture.service.save(
            ProjectLocation::Registration(initial.registration_id),
            initial.revision,
            left.clone()
        ),
        independent.save(
            ProjectLocation::Root(alias),
            initial.revision,
            right.clone()
        ),
    );
    let (winner, loser, draft) = match first {
        Reply::Success { .. } => (ok(first), second, right),
        Reply::Failure { .. } => (ok(second), first, left),
    };
    let conflict = failure(loser, "project.revision_conflict");
    assert_eq!(
        conflict["error"]["details"]["currentRevision"],
        json!(winner.revision)
    );
    assert_eq!(fs::read(fixture.root.join(BACKUP)).unwrap(), FIXTURE);
    let recovered = ok(fixture
        .service
        .recover(
            fixture.at_root(),
            ProjectRecovery::Draft {
                document: Box::new(draft.clone()),
                destination: fixture.root.with_file_name("losing-draft"),
            },
        )
        .await);
    assert_ne!(recovered.document.project_id, initial.document.project_id);
    assert_eq!(recovered.document.name, draft.name);
    let registered = ok(independent
        .open(ProjectLocation::Registration(winner.registration_id))
        .await);
    assert_eq!(registered.revision, winner.revision);
}

#[tokio::test]
async fn restores_exact_backup_bytes_after_damage_and_reports_interrupted_drafts() {
    let fixture = Fixture::new();
    fixture.install(FIXTURE);
    let initial = ok(fixture.service.open(fixture.at_root()).await);
    let mut edit = initial.document.clone();
    edit.name = "Current".into();
    let current = ok(fixture
        .service
        .save(fixture.at_root(), initial.revision, edit)
        .await);
    let staged_path = fixture.root.join(".mcp-project-stage-fixture");
    fs::copy(fixture.root.join(MANIFEST_NAME), &staged_path).unwrap();
    fixture.install(b"{");
    let damaged = ManifestRevision::of(b"{");
    let broken = failure(
        fixture.service.open(fixture.at_root()).await,
        "project.invalid",
    );
    let recovery = &broken["diagnostics"][1];
    assert_eq!(recovery["code"], "project.recovery_available");
    assert_eq!(recovery["details"]["currentRevision"], json!(damaged));
    let candidates = recovery["details"]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0]["kind"], "lastValid");
    assert_eq!(candidates[0]["revision"], json!(initial.revision));
    assert_eq!(candidates[1]["kind"], "interruptedWrite");
    assert_eq!(candidates[1]["revision"], json!(current.revision));

    let stale = fixture
        .service
        .recover(
            fixture.at_root(),
            ProjectRecovery::Backup {
                expected_revision: Some(current.revision),
                revision: initial.revision,
            },
        )
        .await;
    failure(stale, "project.revision_conflict");
    let wrong_backup = fixture
        .service
        .recover(
            fixture.at_root(),
            ProjectRecovery::Backup {
                expected_revision: Some(damaged),
                revision: current.revision,
            },
        )
        .await;
    failure(wrong_backup, "project.recovery_unavailable");
    let restored = ok(fixture
        .service
        .recover(
            ProjectLocation::Registration(initial.registration_id),
            ProjectRecovery::Backup {
                expected_revision: Some(damaged),
                revision: initial.revision,
            },
        )
        .await);
    assert_eq!(restored.revision, initial.revision);
    assert_eq!(fs::read(fixture.root.join(MANIFEST_NAME)).unwrap(), FIXTURE);
    assert_eq!(fs::read(fixture.root.join(BACKUP)).unwrap(), FIXTURE);
}

#[tokio::test]
async fn recovers_to_a_new_copy_or_restores_a_missing_manifest_without_clobbering() {
    let fixture = Fixture::new();
    fixture.install(FIXTURE);
    let original = ok(fixture.service.open(fixture.at_root()).await);
    let mut edit = original.document.clone();
    edit.name = "Current".into();
    let current = ok(fixture
        .service
        .save(fixture.at_root(), original.revision, edit)
        .await);

    let copied = ok(fixture
        .service
        .recover(
            fixture.at_root(),
            ProjectRecovery::CopyBackup {
                revision: original.revision,
                destination: fixture.root.with_file_name("recovered"),
            },
        )
        .await);
    assert_ne!(copied.document.project_id, original.document.project_id);
    assert_eq!(copied.document.name, original.document.name);
    let conflict = failure(
        fixture
            .service
            .recover(
                fixture.at_root(),
                ProjectRecovery::Backup {
                    expected_revision: None,
                    revision: original.revision,
                },
            )
            .await,
        "project.revision_conflict",
    );
    assert_eq!(
        conflict["error"]["details"]["currentRevision"],
        json!(current.revision)
    );

    fs::remove_file(fixture.root.join(MANIFEST_NAME)).unwrap();
    let missing = failure(
        fixture.service.open(fixture.at_root()).await,
        "project.missing",
    );
    assert_eq!(
        missing["diagnostics"][0]["details"]["currentRevision"],
        Value::Null
    );
    let restored = ok(fixture
        .service
        .recover(
            fixture.at_root(),
            ProjectRecovery::Backup {
                expected_revision: None,
                revision: original.revision,
            },
        )
        .await);
    assert_eq!(restored.revision, original.revision);
    assert_eq!(fs::read(fixture.root.join(MANIFEST_NAME)).unwrap(), FIXTURE);
}

#[tokio::test]
async fn create_validates_before_writing_and_never_overwrites_an_existing_project() {
    let fixture = Fixture::new();
    failure(
        fixture
            .service
            .create(fixture.root.clone(), " ".into())
            .await,
        "project.invalid",
    );
    assert!(!fixture.root.exists());
    assert!(!fixture.profile.exists());

    fixture.install(FIXTURE);
    failure(
        fixture
            .service
            .create(fixture.root.clone(), "Existing".into())
            .await,
        "project.exists",
    );
    assert_eq!(fs::read(fixture.root.join(MANIFEST_NAME)).unwrap(), FIXTURE);
    assert!(!fixture.root.join(".mcp-project.lock").exists());
    assert!(!fixture.profile.exists());
}

#[tokio::test]
async fn catalog_write_failure_returns_committed_revision_and_open_repairs_the_index() {
    let fixture = Fixture::new();
    fixture.install(FIXTURE);
    let original = ok(fixture.service.open(fixture.at_root()).await);
    let mut db = sqlite(&fixture.profile.join("catalog.sqlite3")).await;
    sqlx::query(
        "CREATE TRIGGER fixture_failure BEFORE UPDATE ON projects
                 BEGIN SELECT RAISE(ABORT, 'M1_SYNTHETIC_SQL_ERROR'); END",
    )
    .execute(&mut db)
    .await
    .unwrap();
    let mut draft = original.document;
    draft.name = "Committed despite catalog failure".into();
    let reply = fixture
        .service
        .save(fixture.at_root(), original.revision, draft.clone())
        .await;
    let (snapshot, warnings) = match reply {
        Reply::Success { data, diagnostics } => (data, diagnostics),
        error => panic!("{error:?}"),
    };
    assert_eq!(warnings[0].code, DiagnosticCode::ProjectIndexRepairRequired);
    let serialized = serde_json::to_value(&warnings[0]).unwrap();
    assert_eq!(
        serialized["details"]["committedRevision"],
        json!(snapshot.revision)
    );
    assert!(!serialized.to_string().contains("M1_SYNTHETIC_SQL_ERROR"));
    assert_eq!(snapshot.document, draft);
    assert_eq!(
        snapshot.revision,
        ManifestRevision::of(&fs::read(fixture.root.join(MANIFEST_NAME)).unwrap())
    );
    let listed = ok(fixture.service.list().await);
    assert_eq!(listed[0].status, ProjectStatus::Changed);
    assert_eq!(listed[0].last_known_revision, original.revision);

    sqlx::query("DROP TRIGGER fixture_failure")
        .execute(&mut db)
        .await
        .unwrap();
    let reconciled = ok(fixture.service.open(fixture.at_root()).await);
    assert_eq!(reconciled.registration_id, snapshot.registration_id);
    assert_eq!(reconciled.revision, snapshot.revision);
    assert_eq!(
        ok(fixture.service.list().await)[0].status,
        ProjectStatus::Available
    );
    ok(fixture
        .service
        .save(fixture.at_root(), snapshot.revision, draft)
        .await);
    assert_eq!(fs::read(fixture.root.join(BACKUP)).unwrap(), FIXTURE);
}

#[tokio::test]
async fn external_edit_uses_exact_bytes_and_missing_project_is_visible() {
    let fixture = Fixture::new();
    fixture.install(FIXTURE);
    let opened = ok(fixture.service.open(fixture.at_root()).await);
    let mut external = FIXTURE.to_vec();
    external.extend(b"\n ");
    fixture.install(&external);
    let conflict = failure(
        fixture
            .service
            .save(fixture.at_root(), opened.revision, opened.document)
            .await,
        "project.revision_conflict",
    );
    assert_eq!(
        conflict["error"]["details"]["currentRevision"],
        json!(ManifestRevision::of(&external))
    );
    assert!(!fixture.root.join(BACKUP).exists());
    fs::remove_file(fixture.root.join(MANIFEST_NAME)).unwrap();
    assert_eq!(
        ok(fixture.service.list().await)[0].status,
        ProjectStatus::Missing
    );
}

#[tokio::test]
async fn catalog_migration_keeps_a_consistent_pre_migration_backup() {
    let fixture = Fixture::new();
    create_profile(&fixture.profile);
    let database = fixture.profile.join("catalog.sqlite3");
    let mut db = sqlite(&database).await;
    sqlx::query("PRAGMA wal_autocheckpoint = 0")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE prior_data (value TEXT)")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO prior_data VALUES ('before migration')")
        .execute(&mut db)
        .await
        .unwrap();
    assert!(ok(fixture.service.list().await).is_empty());

    let mut backup = SqliteConnectOptions::new()
        .filename(fixture.profile.join("catalog.pre-schema-1.sqlite3"))
        .read_only(true)
        .connect()
        .await
        .unwrap();
    let prior: String = sqlx::query_scalar("SELECT value FROM prior_data")
        .fetch_one(&mut backup)
        .await
        .unwrap();
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut backup)
        .await
        .unwrap();
    assert_eq!((prior.as_str(), version), ("before migration", 0));
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(version, 1);
}

#[tokio::test]
async fn failed_or_future_catalog_migration_leaves_direct_project_open_available() {
    let fixture = Fixture::new();
    create_profile(&fixture.profile);
    fixture.install(FIXTURE);
    let database = fixture.profile.join("catalog.sqlite3");
    let mut db = sqlite(&database).await;
    sqlx::query("CREATE TABLE projects (marker TEXT)")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO projects VALUES ('prior catalog')")
        .execute(&mut db)
        .await
        .unwrap();
    failure(fixture.service.list().await, "storage.unavailable");
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(version, 0);
    let marker: String = sqlx::query_scalar("SELECT marker FROM projects")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(marker, "prior catalog");
    let backup = fixture.profile.join("catalog.pre-schema-1.sqlite3");
    let backup_bytes = fs::read(&backup).unwrap();

    sqlx::query("PRAGMA user_version = 99")
        .execute(&mut db)
        .await
        .unwrap();
    db.close().await.unwrap();
    let bytes = fs::read(&database).unwrap();
    let direct = serde_json::to_value(fixture.service.open(fixture.at_root()).await).unwrap();
    assert_eq!(direct["ok"], true);
    assert_eq!(
        direct["diagnostics"][0]["code"],
        "project.index_repair_required"
    );
    assert_eq!(
        direct["data"]["revision"],
        json!(ManifestRevision::of(FIXTURE))
    );
    assert_eq!(fs::read(backup).unwrap(), backup_bytes);
    assert_eq!(fs::read(database).unwrap(), bytes);
    assert_eq!(fs::read(fixture.root.join(MANIFEST_NAME)).unwrap(), FIXTURE);
}

fn create_profile(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    #[cfg(windows)]
    fs::create_dir(path).unwrap();
}

#[tokio::test]
async fn refuses_hard_linked_manifest_without_touching_its_target() {
    let fixture = Fixture::new();
    let elsewhere = fixture.root.with_file_name("elsewhere.json");
    fs::write(&elsewhere, FIXTURE).unwrap();
    fs::create_dir(&fixture.root).unwrap();
    fs::hard_link(&elsewhere, fixture.root.join(MANIFEST_NAME)).unwrap();
    failure(
        fixture.service.open(fixture.at_root()).await,
        "project.unsafe_path",
    );
    assert_eq!(fs::read(elsewhere).unwrap(), FIXTURE);
    assert!(!fixture.profile.exists());
}
