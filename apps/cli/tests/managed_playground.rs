#![cfg(any(windows, target_os = "linux", target_os = "macos"))]

#[path = "managed_playground/stop_benchmark.rs"]
#[cfg(windows)]
mod stop_benchmark;

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use builder_app::playground::{
    CancellationToken, CredentialSource, Invocation, PreviewAccess, PreviewMode, PreviewOptions,
    ProtocolPreference, RequestOptions, RunOutcome,
};
use builder_app::{ProjectLocation, ProjectService, Reply};
use serde_json::{Value, json};
use tempfile::TempDir;
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0},
    System::Threading::{CREATE_NO_WINDOW, OpenProcess, WaitForSingleObject},
};

const BUILDER: &str = env!("CARGO_BIN_EXE_mcp-builder");
const SAMPLE: &str = include_str!("../../../crates/builder-project/tests/fixtures/catalog-v1.json");
const CANARY: &str = "m7-local-synthetic-canary-a4d3";

fn sample() -> Value {
    serde_json::from_str(SAMPLE).unwrap()
}

fn project(document: &Value) -> TempDir {
    let parent = std::env::temp_dir();
    #[cfg(target_os = "macos")]
    let parent = parent.canonicalize().unwrap();
    let directory = tempfile::Builder::new()
        .prefix("m7 project ž ")
        .tempdir_in(parent)
        .unwrap();
    save(directory.path(), document);
    directory
}

fn save(root: &Path, document: &Value) {
    fs::write(
        root.join("mcp-project.json"),
        serde_json::to_vec(document).unwrap(),
    )
    .unwrap();
}

fn cli(action: &str, root: &Path, scenario: &str, more: &[&str]) -> (Output, Value) {
    let mut command = Command::new(BUILDER);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = command
        .args(["--json", "playground", action])
        .arg(root)
        .args(["--scenario", scenario, "--grant", "catalog_read"])
        .args(more)
        .env("RUST_LOG", "trace")
        .output()
        .unwrap();
    let reply = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "CLI returned {:?}, with JSON error {error} and stderr {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output, reply)
}

fn review(root: &Path, scenario: &str) -> String {
    let (output, reply) = cli("review", root, scenario, &[]);
    assert!(output.status.success(), "{reply}");
    let environment = reply["data"]["environment"].as_object().unwrap();
    #[cfg(windows)]
    {
        assert_eq!(environment.len(), 1);
        assert!(Path::new(environment["SystemRoot"].as_str().unwrap()).is_absolute());
    }
    #[cfg(not(windows))]
    assert!(environment.is_empty());
    assert_eq!(reply["data"]["approvedGrants"][0]["id"], "catalog_read");
    reply["data"]["digest"].as_str().unwrap().to_owned()
}

#[test]
fn real_cli_replays_both_revisions_and_rejects_stale_approval() {
    let mut document = sample();
    document["scenarios"][0]["fixtures"][0]["body"]["private"] = json!(CANARY);
    let directory = project(&document);
    let root = directory.path();
    let (unapproved, reply) = cli("replay", root, "known_item", &[]);
    assert_eq!(unapproved.status.code(), Some(3));
    assert_eq!(reply["error"]["code"], "policy.approval_required");

    let approval = review(root, "known_item");
    for (protocol, revision) in [("modern", "2026-07-28"), ("legacy", "2025-11-25")] {
        let (output, reply) = cli(
            "replay",
            root,
            "known_item",
            &[
                "--approve",
                &approval,
                "--trace-payloads",
                "--protocol",
                protocol,
            ],
        );
        assert!(output.status.success(), "{reply}");
        assert_eq!(reply["data"]["run"]["version"], 1);
        assert_eq!(reply["data"]["run"]["protocolRevision"], revision);
        assert_eq!(reply["data"]["run"]["outcome"], "passed");
        assert_eq!(reply["data"]["run"]["effect"], "none");
        let entries = reply["data"]["trace"]["entries"].as_array().unwrap();
        assert!(entries.iter().any(|entry| entry.get("payload").is_some()));
        assert!(!String::from_utf8_lossy(&output.stdout).contains(CANARY));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(CANARY));
    }

    document["scenarios"][0]["assertions"][0]["expected"] = json!("Changed expectation");
    save(root, &document);
    let (stale, reply) = cli("replay", root, "known_item", &["--approve", &approval]);
    assert_eq!(stale.status.code(), Some(3));
    assert_eq!(reply["error"]["code"], "policy.approval_required");

    let approval = review(root, "known_item");
    let (output, reply) = cli("replay", root, "known_item", &["--approve", &approval]);
    assert_eq!(output.status.code(), Some(4), "{reply}");
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "scenario.assertion_failed");
    let details = &reply["error"]["details"];
    assert_eq!(details["run"]["outcome"], "assertionFailed");
    assert_eq!(details["run"]["runId"], details["runId"]);
    assert_eq!(
        details["run"]["assertions"][0]["path"],
        "/structuredContent/name"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Changed expectation"));
    assert!(
        details["trace"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry.get("payload").is_none())
    );

    let approval = review(root, "invalid_item");
    let (output, reply) = cli("replay", root, "invalid_item", &["--approve", &approval]);
    assert!(output.status.success(), "{reply}");
    assert_eq!(reply["data"]["run"]["assertions"][0]["passed"], true);

    document["scenarios"][0]
        .as_object_mut()
        .unwrap()
        .remove("fixtures");
    save(root, &document);
    let approval = review(root, "known_item");
    let (output, reply) = cli("replay", root, "known_item", &["--approve", &approval]);
    assert_eq!(output.status.code(), Some(5), "{reply}");
    assert_eq!(reply["error"]["code"], "fixture.missing");
}

async fn prepare(
    root: &Path,
    mode: PreviewMode,
    access: PreviewAccess,
) -> builder_app::playground::PreparedProjectPreview {
    match ProjectService::for_user()
        .prepare_playground(
            ProjectLocation::Root(root.to_path_buf()),
            PathBuf::from(BUILDER),
            mode,
            access,
        )
        .await
    {
        Reply::Success { data, .. } => data,
        Reply::Failure { error, .. } => panic!("preparing preview failed: {error:?}"),
    }
}

#[tokio::test]
async fn live_file_preview_checks_the_reviewed_root_and_reaps_on_stop() {
    let directory = project(&sample());
    let root = directory.path();
    let files = root.join("selected");
    fs::create_dir_all(files.join("notes")).unwrap();
    fs::write(files.join("notes/42.txt"), b"Selected note").unwrap();
    let access = || PreviewAccess {
        approved_grant_ids: ["note_read".into()].into(),
        approved_prompt_ids: ["review_item".into()].into(),
        file_roots: [("catalog_docs".into(), files.clone())].into(),
        ..PreviewAccess::default()
    };
    let mode = PreviewMode::Live { scenario_id: None };
    let cancellation = CancellationToken::new();
    let prepared = prepare(root, mode.clone(), access()).await;
    let digest = prepared.review().digest;
    let approved = prepared.approve(digest).unwrap();

    let prior = root.join("prior");
    fs::rename(&files, &prior).unwrap();
    fs::create_dir_all(files.join("notes")).unwrap();
    fs::write(files.join("notes/42.txt"), b"Unapproved replacement").unwrap();
    let error = approved
        .start(PreviewOptions::default(), &cancellation)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code(), "adapter.file_root_unavailable");

    fs::rename(&files, root.join("replacement")).unwrap();
    fs::rename(&prior, &files).unwrap();
    let prepared = prepare(root, mode, access()).await;
    assert_eq!(prepared.review().digest, digest);
    let session = prepared
        .approve(digest)
        .unwrap()
        .start(
            PreviewOptions {
                protocol: ProtocolPreference::Auto,
                capture_payloads: false,
            },
            &cancellation,
        )
        .await
        .unwrap();
    let pid = session.process_id();
    let discovery =
        serde_json::to_value(session.discover(RequestOptions::default()).await.unwrap()).unwrap();
    assert!(discovery["tools"].as_array().unwrap().is_empty());
    assert_eq!(discovery["resourceTemplates"].as_array().unwrap().len(), 1);
    assert_eq!(discovery["prompts"].as_array().unwrap().len(), 1);
    let call = Invocation::Resource {
        uri: "catalog://docs/items/42".into(),
    };
    let result = session
        .invoke(&call, RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(result.result.outcome, RunOutcome::Passed);
    assert_eq!(
        result.payload.unwrap()["contents"][0]["text"],
        "Selected note"
    );
    let denied = session
        .invoke(
            &Invocation::Resource {
                uri: "catalog://docs/items/43".into(),
            },
            RequestOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(denied.result.outcome, RunOutcome::Denied);

    let begin = Instant::now();
    session.stop().await.unwrap();
    assert!(begin.elapsed() < Duration::from_secs(5));
    assert_process_exited(pid);
}

#[tokio::test]
async fn prepared_project_checks_current_bytes_before_launch() {
    let mut document = sample();
    let directory = project(&document);
    let root = directory.path();
    let prepared = prepare(
        root,
        PreviewMode::Fixture {
            scenario_id: "known_item".into(),
        },
        PreviewAccess::default(),
    )
    .await;
    let digest = prepared.review().digest;
    let approved = prepared.approve(digest).unwrap();
    document["name"] = json!("Edited project");
    save(root, &document);
    let error = approved
        .start(PreviewOptions::default(), &CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert_eq!(error.code(), "policy.approval_required");
}

#[tokio::test]
async fn session_credentials_are_private_and_used_in_evidence_redaction() {
    let mut document = sample();
    document["capabilities"][3]["messages"][0]["textTemplate"] =
        json!(format!("Canary {CANARY} {{id}}"));
    let directory = project(&document);
    let prepared = prepare(
        directory.path(),
        PreviewMode::Live { scenario_id: None },
        PreviewAccess {
            approved_grant_ids: ["catalog_read".into()].into(),
            approved_prompt_ids: ["review_item".into()].into(),
            credentials: [(
                "catalog_auth".into(),
                CredentialSource::Session(format!("Bearer {CANARY}").into()),
            )]
            .into(),
            ..PreviewAccess::default()
        },
    )
    .await;
    let review = serde_json::to_value(prepared.review()).unwrap();
    assert_eq!(
        review["credentialSources"]["catalog_auth"]["kind"],
        "session"
    );
    assert_eq!(
        review["credentialTargets"],
        json!([{
            "capabilityId": "lookup_item", "bindingId": "catalog_lookup",
            "origin": "https://catalog.example.test", "references": ["catalog_auth"]
        }])
    );
    assert!(!review.to_string().contains(CANARY));
    let digest = prepared.review().digest;
    let session = prepared
        .approve(digest)
        .unwrap()
        .start(
            PreviewOptions {
                capture_payloads: true,
                ..PreviewOptions::default()
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let run = session
        .invoke(
            &Invocation::Prompt {
                name: "review_item".into(),
                arguments: [("id".into(), CANARY.into())].into(),
            },
            RequestOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(run.result.outcome, RunOutcome::Passed);
    let display = serde_json::to_string(&run).unwrap();
    let trace = serde_json::to_string(&session.trace()).unwrap();
    assert!(display.contains("[REDACTED]"));
    assert!(trace.contains("[REDACTED]"));
    assert!(!display.contains(CANARY));
    assert!(!trace.contains(CANARY));
    session.stop().await.unwrap();
}

#[tokio::test]
async fn an_unavailable_explicit_environment_source_fails_after_approval() {
    let directory = project(&sample());
    let prepared = prepare(
        directory.path(),
        PreviewMode::Live { scenario_id: None },
        PreviewAccess {
            approved_grant_ids: ["catalog_read".into()].into(),
            credentials: [(
                "catalog_auth".into(),
                CredentialSource::Environment("MCP_BUILDER_M7_ABSENT_FIXTURE_4314".into()),
            )]
            .into(),
            ..PreviewAccess::default()
        },
    )
    .await;
    let digest = prepared.review().digest;
    let error = prepared
        .approve(digest)
        .unwrap()
        .start(PreviewOptions::default(), &CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert_eq!(error.code(), "credential.unavailable");
}

#[tokio::test]
async fn a_saved_live_mutation_requires_an_explicit_invocation() {
    let mut document = sample();
    document["bindings"][0]["method"] = json!("POST");
    document["policy"]["grants"][0]["operation"] = json!("http.write");
    document["policy"]["grants"][0]["target"]["method"] = json!("POST");
    document["scenarios"][0]["mode"] = json!("live");
    document["scenarios"][0]
        .as_object_mut()
        .unwrap()
        .remove("fixtures");
    let directory = project(&document);
    let reply = ProjectService::for_user()
        .prepare_playground(
            ProjectLocation::Root(directory.path().to_path_buf()),
            PathBuf::from(BUILDER),
            PreviewMode::Live {
                scenario_id: Some("known_item".into()),
            },
            PreviewAccess {
                approved_grant_ids: ["catalog_read".into()].into(),
                ..PreviewAccess::default()
            },
        )
        .await;
    let Reply::Failure { error, .. } = reply else {
        panic!("a mutating live scenario must not prepare");
    };
    assert_eq!(
        serde_json::to_value(error).unwrap()["code"],
        "scenario.live_mutation_requires_confirmation"
    );
}

#[tokio::test]
async fn timed_out_request_releases_its_socket_and_the_same_preview_completes_a_later_call() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let mut document = sample();
    document["bindings"][0]["origin"] = json!(origin);
    document["bindings"][0]
        .as_object_mut()
        .unwrap()
        .remove("headersFromSecrets");
    document["policy"]["grants"][0]["target"]["origin"] = json!(origin);
    document["policy"]["grants"][0]["target"]["allowedPeerRanges"] = json!(["127.0.0.1/32"]);
    document["policy"]["grants"][0]["limits"]["timeoutMs"] = json!(2000);
    let directory = project(&document);
    let prepared = prepare(
        directory.path(),
        PreviewMode::Live { scenario_id: None },
        PreviewAccess {
            approved_grant_ids: ["catalog_read".into()].into(),
            ..PreviewAccess::default()
        },
    )
    .await;
    let digest = prepared.review().digest;
    let session = prepared
        .approve(digest)
        .unwrap()
        .start(PreviewOptions::default(), &CancellationToken::new())
        .await
        .unwrap();
    let upstream = thread::spawn(move || {
        let mut first = accept_before(&listener);
        assert!(read_request(&mut first).starts_with("GET /items/slow HTTP/1.1"));
        let closed = first.read(&mut [0; 1]);
        assert!(
            matches!(closed, Ok(0))
                || closed.is_err_and(|error| {
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                    )
                })
        );
        let mut next = accept_before(&listener);
        assert!(read_request(&mut next).starts_with("GET /items/42 HTTP/1.1"));
        next.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"id\":\"42\"}").unwrap();
    });
    let tool = |id: &str| Invocation::Tool {
        name: "lookup_item".into(),
        arguments: [("id".into(), json!(id))].into_iter().collect(),
    };
    let timeout = session
        .invoke(&tool("slow"), RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(
        timeout.result.outcome,
        RunOutcome::TimedOut,
        "{:?}",
        timeout.result.diagnostic
    );
    let later = session
        .invoke(&tool("42"), RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(
        later.result.outcome,
        RunOutcome::Passed,
        "{:?}",
        later.result.diagnostic
    );
    assert_eq!(later.payload.unwrap()["structuredContent"]["id"], "42");
    session.stop().await.unwrap();
    upstream.join().unwrap();
}

fn accept_before(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((socket, _)) => {
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                return socket;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("waiting for preview upstream: {error}"),
        }
    }
}

fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    while !bytes.ends_with(b"\r\n\r\n") {
        let read = socket.read(&mut buffer).unwrap();
        assert!(read > 0 && bytes.len() + read <= 8192);
        bytes.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(bytes).unwrap()
}

#[cfg(windows)]
fn assert_process_exited(pid: u32) {
    // SAFETY: OpenProcess returns a fresh handle, if the PID still exists.
    let handle = unsafe { OpenProcess(0x0010_0000, 0, pid) }; // SYNCHRONIZE
    if handle.is_null() {
        assert_eq!(io_error_code(), ERROR_INVALID_PARAMETER as i32);
    } else {
        // SAFETY: This is the valid synchronization handle above and it is closed once.
        let exited = unsafe {
            let result = WaitForSingleObject(handle, 0);
            CloseHandle(handle);
            result
        };
        assert_eq!(exited, WAIT_OBJECT_0);
    }
}

#[cfg(windows)]
fn io_error_code() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap()
}

#[cfg(not(windows))]
fn assert_process_exited(pid: u32) {
    // SAFETY: Signal zero checks process existence and does not deliver a signal.
    assert_eq!(unsafe { libc::kill(pid.cast_signed(), 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}
