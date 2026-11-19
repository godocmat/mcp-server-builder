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

fn builder(workspace: &Path) -> Command {
    builder_for_profile(workspace, "profile")
}

fn builder_for_profile(workspace: &Path, profile: &str) -> Command {
    let mut child = Command::new(BUILDER);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        child.creation_flags(CREATE_NO_WINDOW);
    }
    child.current_dir(workspace).args(["--profile", profile]);
    child
}

fn call(workspace: &Path, args: &[&str], command: &str, code: i32) -> Value {
    let output = builder(workspace)
        .arg("--json")
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
fn human_project_diagnostics_expose_only_recovery_actions_and_the_scoped_catalog() {
    const MARKER: &str = "corrupted-manifest-canary-8862";
    let workspace = workspace();
    let wd = workspace.path();
    let created = builder(wd)
        .args(["project", "create", "catalog", "--name", "Pilot catalog"])
        .output()
        .unwrap();
    assert!(created.status.success());
    assert!(String::from_utf8_lossy(&created.stdout).contains("Pilot catalog"));
    let opened = call(wd, &["project", "open", "catalog"], "project.open", 0);
    let mut changed = opened["data"]["document"].clone();
    changed["name"] = json!("Revised catalog");
    save(&wd.join("draft.json"), &changed);
    let revision = opened["data"]["revision"].as_str().unwrap();
    let saved = builder(wd)
        .args([
            "project",
            "save",
            "catalog",
            "--from",
            "draft.json",
            "--expect",
            revision,
        ])
        .output()
        .unwrap();
    assert!(saved.status.success());
    assert!(String::from_utf8_lossy(&saved.stdout).contains("Revised catalog"));
    let stale = builder(wd)
        .args([
            "project",
            "save",
            "catalog",
            "--from",
            "draft.json",
            "--expect",
            revision,
        ])
        .output()
        .unwrap();
    assert_eq!(stale.status.code(), Some(6));
    assert!(stale.stdout.is_empty());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("Current revision  sha256:"));

    fs::write(
        wd.join("catalog").join(MANIFEST),
        format!(r#"{{"{MARKER}":"#),
    )
    .unwrap();
    let damaged = builder(wd)
        .args(["project", "open", "catalog"])
        .output()
        .unwrap();
    assert_eq!(damaged.status.code(), Some(2));
    assert!(damaged.stdout.is_empty());
    let error = String::from_utf8_lossy(&damaged.stderr);
    assert!(error.contains("Recovery candidate  sha256:"));
    assert!(!error.contains(MARKER));

    let known = builder(wd).args(["project", "list"]).output().unwrap();
    assert!(known.status.success());
    assert!(String::from_utf8_lossy(&known.stdout).contains("Revised catalog"));
    let unrelated = builder_for_profile(wd, "unrelated")
        .args(["project", "list"])
        .output()
        .unwrap();
    assert!(unrelated.status.success());
    let output = String::from_utf8_lossy(&unrelated.stdout);
    assert!(output.contains("No projects are registered."));
    assert!(!output.contains("Revised catalog"));

    let wrong_option = builder(wd)
        .args(["project", "open", "catalog", "--unknown", MARKER])
        .output()
        .unwrap();
    assert_eq!(wrong_option.status.code(), Some(2));
    assert!(wrong_option.stdout.is_empty());
    let error = String::from_utf8_lossy(&wrong_option.stderr);
    assert!(error.contains("Invalid command or options."));
    assert!(!error.contains(MARKER));
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

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn cli_export_conflict_requests_a_new_review_without_printing_file_contents() {
    const MARKER: &str = "export-local-content-canary-1a719";
    let workspace = workspace();
    let wd = workspace.path();
    save(&wd.join(MANIFEST), &sample());
    let initial = builder(wd)
        .args(["generate", ".", "--out", "previous"])
        .output()
        .unwrap();
    assert!(initial.status.success());
    assert!(String::from_utf8_lossy(&initial.stdout).contains("Published"));
    let lock = wd.join("previous/Cargo.lock");
    let edited = format!("# {MARKER}\n");
    fs::write(&lock, &edited).unwrap();
    let regenerate = ["generate", ".", "--out", "new", "--from-export", "previous"];
    let preview = builder(wd)
        .args(regenerate)
        .arg("--preview")
        .output()
        .unwrap();
    assert!(preview.status.success());
    let out = String::from_utf8_lossy(&preview.stdout);
    assert!(out.contains("Review 1 changed files"));
    assert!(out.contains("Cargo.lock"));
    assert!(!out.contains(MARKER));

    let rejected = builder(wd).args(regenerate).output().unwrap();
    assert_eq!(rejected.status.code(), Some(6));
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("review each changed file"));
    assert!(!error.contains(MARKER));
    assert_eq!(fs::read_to_string(&lock).unwrap(), edited);
    assert!(!wd.join("new").exists());

    let mut inspect = regenerate.to_vec();
    inspect.push("--preview");
    let reported = call(wd, &inspect, "generate", 0);
    let conflict = &reported["data"]["conflicts"][0];
    assert_eq!(conflict["path"], "Cargo.lock");
    let decisions = wd.join("decisions.json");
    fs::write(&decisions, MARKER).unwrap();
    let mut publish = regenerate.to_vec();
    publish.extend(["--resolutions", "decisions.json"]);
    let invalid = call(wd, &publish, "generate", 2);
    assert_eq!(invalid["error"]["code"], "export.invalid_resolution");
    assert!(!invalid.to_string().contains(MARKER));
    assert!(!wd.join("new").exists());

    save(
        &decisions,
        &json!({
            "version": 1,
            "resolutions": [{
                "path": conflict["path"],
                "observedDigest": conflict["observedDigest"],
                "replacementDigest": conflict["replacementDigest"],
                "decision": "regenerate",
            }],
        }),
    );
    let published = call(wd, &publish, "generate", 0);
    assert_eq!(published["data"]["published"], true);
    assert!(wd.join("new/generation.json").is_file());
    assert!(
        !fs::read_to_string(wd.join("new/Cargo.lock"))
            .unwrap()
            .contains(MARKER)
    );
    assert_eq!(fs::read_to_string(lock).unwrap(), edited);
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
mod replay {
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

    #[test]
    fn human_launch_review_and_assertion_report_never_echo_private_fixture_values() {
        const MARKER: &str = "fixture-human-output-canary-94bc6";
        let workspace = workspace();
        let wd = workspace.path();
        let mut project = sample();
        project["scenarios"][0]["fixtures"][0]["body"]["private"] = json!(MARKER);
        save(&wd.join(MANIFEST), &project);
        let named_review = builder(wd)
            .args([
                "policy",
                "review",
                "launch",
                ".",
                "--scenario",
                "known_item",
            ])
            .args(ACCESS)
            .output()
            .unwrap();
        assert!(named_review.status.success());
        let named_review_text = String::from_utf8(named_review.stdout).unwrap();
        assert!(!named_review_text.contains(MARKER));
        let named_approval = named_review_text
            .lines()
            .find_map(|line| line.strip_prefix("Approval digest  "))
            .unwrap();
        let passed = builder(wd)
            .args([
                "test",
                "run",
                ".",
                "--scenario",
                "known_item",
                "--approve",
                named_approval,
            ])
            .args(ACCESS)
            .output()
            .unwrap();
        assert!(passed.status.success());
        let out = String::from_utf8_lossy(&passed.stdout);
        assert!(out.contains("Scenario passed in"));
        assert!(!out.contains(MARKER));
        assert!(!String::from_utf8_lossy(&passed.stderr).contains(MARKER));

        project["scenarios"][0]["assertions"][0]["expected"] = json!("A different item");
        save(&wd.join(MANIFEST), &project);

        let review = builder(wd)
            .args(["policy", "review", "launch", ".", "--all"])
            .args(ACCESS)
            .output()
            .unwrap();
        assert!(review.status.success());
        let review_text = String::from_utf8(review.stdout).unwrap();
        assert!(review_text.contains("Executable"));
        assert!(review_text.contains("Grant"));
        assert!(review_text.contains("Prompt  review_item"));
        assert!(!review_text.contains(MARKER));
        assert!(!String::from_utf8_lossy(&review.stderr).contains(MARKER));
        let approval = review_text
            .lines()
            .find_map(|line| line.strip_prefix("Suite approval digest  "))
            .unwrap();

        let run = builder(wd)
            .args(["test", "run", ".", "--all", "--approve", approval])
            .args(ACCESS)
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(4));
        assert!(run.stdout.is_empty());
        let error = String::from_utf8(run.stderr).unwrap();
        assert!(error.contains("scenario.assertion_failed"));
        assert!(error.contains("known_item  assertion failed"));
        assert!(error.contains("Assertion 1 differs at /structuredContent/name."));
        assert!(error.contains("review  passed"));
        assert!(!error.contains(MARKER));
    }
}
