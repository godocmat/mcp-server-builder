use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;

const BUILDER: &str = env!("CARGO_BIN_EXE_mcp-builder");
const SAMPLE: &str = include_str!("../../../crates/builder-project/tests/fixtures/catalog-v1.json");
const MANIFEST: &str = "mcp-project.json";

fn workspace() -> TempDir {
    tempfile::Builder::new()
        .prefix("m9 project ž ")
        .tempdir()
        .unwrap()
}

fn call(workspace: &Path, args: &[&str], command: &str, code: i32) -> Value {
    let mut child = Command::new(BUILDER);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        child.creation_flags(CREATE_NO_WINDOW);
    }
    let output = child
        .current_dir(workspace)
        .args(["--json", "--profile", "profile"])
        .args(args)
        .output()
        .unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "expected one JSON reply for {args:?}, got {error} with stderr {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(output.status.code(), Some(code), "{args:?}: {reply}");
    assert_eq!(reply["version"], 1);
    assert_eq!(reply["command"], command);
    assert_eq!(reply["ok"], code == 0);
    assert!(reply["diagnostics"].is_array());
    reply
}

fn save(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn sample() -> Value {
    serde_json::from_str(SAMPLE).unwrap()
}

#[test]
fn fresh_shells_validate_generate_and_recover_portable_projects() {
    let workspace = workspace();
    let wd = workspace.path();
    let root = "catalog project";
    let created = call(
        wd,
        &["project", "create", root, "--name", "Catalog"],
        "project.create",
        0,
    );
    let initial = created["data"]["revision"].as_str().unwrap();

    let mut edited = sample();
    edited["projectId"] = created["data"]["document"]["projectId"].clone();
    save(&wd.join("draft.json"), &edited);
    let saved = call(
        wd,
        &[
            "project",
            "save",
            root,
            "--from",
            "draft.json",
            "--expect",
            initial,
        ],
        "project.save",
        0,
    );
    let revision = saved["data"]["revision"].as_str().unwrap();
    assert_ne!(initial, revision);
    let reopened = call(wd, &["project", "open", root], "project.open", 0);
    assert_eq!(reopened["data"]["revision"], revision);
    assert_eq!(
        reopened["data"]["document"]["capabilities"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    let copy = wd.join("portable copy");
    fs::create_dir(&copy).unwrap();
    fs::copy(wd.join(root).join(MANIFEST), copy.join(MANIFEST)).unwrap();
    let valid = call(
        wd,
        &["project", "validate", "portable copy"],
        "project.validate",
        0,
    );
    assert_eq!(valid["data"]["valid"], true);
    let catalog = call(wd, &["project", "list"], "project.list", 0);
    let registered = catalog["data"].as_array().unwrap();
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0]["status"], "available");

    #[cfg(any(windows, target_os = "linux"))]
    {
        let planned = call(
            wd,
            &[
                "generate",
                "portable copy",
                "--out",
                "standalone",
                "--preview",
            ],
            "generate",
            0,
        );
        assert_eq!(planned["data"]["published"], false);
        assert!(!wd.join("standalone").exists());
        let published = call(
            wd,
            &["generate", "portable copy", "--out", "standalone"],
            "generate",
            0,
        );
        assert_eq!(published["data"]["published"], true);
        assert!(wd.join("standalone").join("Cargo.lock").is_file());
        assert!(wd.join("standalone").join("generation.json").is_file());
    }

    let stale = call(
        wd,
        &[
            "project",
            "save",
            root,
            "--from",
            "draft.json",
            "--expect",
            initial,
        ],
        "project.save",
        6,
    );
    assert_eq!(stale["error"]["code"], "project.revision_conflict");
    assert_eq!(stale["error"]["details"]["currentRevision"], revision);

    edited["capabilities"][0]["name"] = json!("");
    save(&copy.join(MANIFEST), &edited);
    let invalid = call(
        wd,
        &["project", "validate", "portable copy"],
        "project.validate",
        2,
    );
    assert_eq!(invalid["error"]["code"], "project.invalid");
    assert!(
        invalid["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["field"] == "/capabilities/0/name")
    );
    assert!(invalid["error"]["action"].is_string());

    let restored = call(
        wd,
        &[
            "project",
            "recover",
            root,
            "--from-revision",
            initial,
            "--expect",
            revision,
        ],
        "project.recover",
        0,
    );
    assert_eq!(restored["data"]["revision"], initial);
    let recovered = call(
        wd,
        &[
            "project",
            "recover",
            root,
            "--draft",
            "draft.json",
            "--out",
            "recovered draft",
        ],
        "project.recover",
        0,
    );
    assert_ne!(
        recovered["data"]["document"]["projectId"],
        created["data"]["document"]["projectId"]
    );
    assert_eq!(recovered["data"]["document"]["name"], "Catalog sample");
}

#[cfg(windows)]
mod windows {
    use super::*;

    const ACCESS: &[&str] = &[
        "--grant",
        "catalog_read",
        "--grant",
        "note_read",
        "--prompt",
        "review_item",
    ];

    fn review(wd: &Path, selection: &[&str]) -> Value {
        let args = ["policy", "review", "launch", "."]
            .into_iter()
            .chain(selection.iter().copied())
            .chain(ACCESS.iter().copied())
            .collect::<Vec<_>>();
        call(wd, &args, "policy.review.launch", 0)
    }

    fn run(wd: &Path, selection: &[&str], approval: Option<&str>, code: i32) -> Value {
        let mut args = ["test", "run", "."]
            .into_iter()
            .chain(selection.iter().copied())
            .chain(ACCESS.iter().copied())
            .collect::<Vec<_>>();
        if let Some(approval) = approval {
            args.extend(["--approve", approval]);
        }
        call(wd, &args, "test.run", code)
    }

    #[test]
    fn explicit_approval_binds_the_fixture_scope_and_saved_revision() {
        let workspace = workspace();
        let wd = workspace.path();
        let mut project = sample();
        save(&wd.join(MANIFEST), &project);

        let scenario = ["--scenario", "known_item"];
        let named = review(wd, &scenario);
        let digest = named["data"]["digest"].as_str().unwrap();
        let missing_approval = run(wd, &scenario, None, 3);
        assert_eq!(
            missing_approval["error"]["code"],
            "policy.approval_required"
        );
        let passed = run(wd, &scenario, Some(digest), 0);
        assert_eq!(passed["data"]["run"]["outcome"], "passed");

        let suite = review(wd, &["--all"]);
        let suite_digest = suite["data"]["digest"].as_str().unwrap();
        assert_ne!(suite_digest, digest);
        let denied = run(wd, &["--all"], Some(digest), 3);
        assert_eq!(denied["error"]["code"], "policy.approval_required");
        let passed = run(wd, &["--all"], Some(suite_digest), 0);
        let runs = passed["data"]["runs"].as_array().unwrap();
        let order: Vec<_> = runs
            .iter()
            .map(|run| run["scenarioId"].as_str().unwrap())
            .collect();
        assert_eq!(order, ["known_item", "invalid_item", "note", "review"]);
        assert!(runs.iter().all(|run| run["outcome"] == "passed"));
        assert!(
            !wd.join("profile").exists(),
            "review should leave the catalog untouched"
        );

        project["scenarios"][0]["assertions"][0]["expected"] = json!("Different name");
        save(&wd.join(MANIFEST), &project);
        let stale = run(wd, &["--all"], Some(suite_digest), 3);
        assert_eq!(stale["error"]["code"], "policy.approval_required");
        let revised = review(wd, &["--all"]);
        let failed = run(wd, &["--all"], revised["data"]["digest"].as_str(), 4);
        assert_eq!(failed["error"]["code"], "scenario.assertion_failed");
        let details = &failed["error"]["details"];
        assert_eq!(details["runs"].as_array().unwrap().len(), 4);
        assert_eq!(details["runId"], details["runs"][0]["runId"]);
        assert_eq!(
            details["runs"][0]["assertions"][0]["path"],
            "/structuredContent/name"
        );
        assert_eq!(details["runs"][3]["outcome"], "passed");
    }

    #[test]
    fn scoped_file_denial_has_a_distinct_exit_and_a_saved_denial_can_pass() {
        let workspace = workspace();
        let wd = workspace.path();
        let mut project = sample();
        project["scenarios"][2]["call"]["uri"] = json!("catalog://docs/items/43");
        project["scenarios"][2]["fixtures"][0]["relativePath"] = json!("notes/43.txt");
        save(&wd.join(MANIFEST), &project);

        let selection = ["--scenario", "note"];
        let reviewed = review(wd, &selection);
        let denied = run(wd, &selection, reviewed["data"]["digest"].as_str(), 3);
        assert_eq!(denied["error"]["code"], "policy.denied");
        assert!(denied["error"]["action"].is_string());
        assert_eq!(denied["error"]["details"]["run"]["effect"], "none");

        project["scenarios"][2]["assertions"] =
            json!([{"kind":"errorCode","code":"policy.denied"}]);
        save(&wd.join(MANIFEST), &project);
        let reviewed = review(wd, &selection);
        let passed = run(wd, &selection, reviewed["data"]["digest"].as_str(), 0);
        assert_eq!(passed["data"]["run"]["outcome"], "passed");
    }
}
