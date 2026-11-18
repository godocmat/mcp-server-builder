use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use builder_app::playground::{
    CancellationToken, Invocation, ManagedSession, PreviewAccess, PreviewMode, PreviewOptions,
    ProtocolPreference, ProtocolRevision, RequestOptions, RunOutcome,
};
use builder_app::{ManifestRevision, ProjectLocation, ProjectService, Reply};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

use super::{BUILDER, SAMPLE, sample};

const ATTEMPTS: usize = 10;
const STOP_BUDGET: Duration = Duration::from_secs(5);
const FIXTURE_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 32\r\nConnection: close\r\n\r\n{\"id\":\"42\",\"name\":\"Sample item\"}";

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum Scenario {
    AfterNormalCall,
    AfterCancellation,
}

impl Scenario {
    fn expected_outcome(self) -> RunOutcome {
        match self {
            Self::AfterNormalCall => RunOutcome::Passed,
            Self::AfterCancellation => RunOutcome::Cancelled,
        }
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Attempt {
    number: usize,
    manifest_revision: Option<ManifestRevision>,
    worker_pid: Option<u32>,
    negotiated_revision: Option<ProtocolRevision>,
    outcome: Option<RunOutcome>,
    stop_ms: Option<f64>,
    worker_exited: Option<bool>,
    active_job_processes_at_stop: Option<u32>,
    failures: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    revision: ProtocolRevision,
    scenario: Scenario,
    stop_p95_ms: Option<f64>,
    attempts: Vec<Attempt>,
}

#[tokio::test]
#[ignore = "writes an explicit M10 managed shutdown performance report"]
async fn m10_managed_stop_benchmark() {
    let report = PathBuf::from(
        env::var_os("MCP_BUILDER_STOP_BENCHMARK_REPORT").expect("set an absolute report path"),
    );
    assert!(report.is_absolute() && !report.exists());
    let worker = env::var_os("MCP_BUILDER_STOP_BENCHMARK_CLI")
        .map_or_else(|| PathBuf::from(BUILDER), PathBuf::from)
        .canonicalize()
        .expect("find the first-party CLI worker");
    let worker_digest = ManifestRevision::of(&fs::read(&worker).unwrap());
    let profile = tempfile::tempdir().unwrap();
    let service = ProjectService::with_profile(profile.path().to_path_buf());
    let project = tempfile::tempdir().unwrap();
    let mut profiles = Vec::new();
    for (preference, revision) in [
        (ProtocolPreference::Modern, ProtocolRevision::Modern),
        (ProtocolPreference::Legacy, ProtocolRevision::Legacy),
    ] {
        for scenario in [Scenario::AfterNormalCall, Scenario::AfterCancellation] {
            let mut attempts = Vec::with_capacity(ATTEMPTS);
            for number in 1..=ATTEMPTS {
                let mut attempt = Attempt {
                    number,
                    ..Attempt::default()
                };
                if let Err(error) = measure(
                    &service,
                    project.path(),
                    &worker,
                    preference,
                    scenario,
                    &mut attempt,
                )
                .await
                {
                    attempt.failures.push(error);
                }
                if attempt
                    .negotiated_revision
                    .is_some_and(|actual| actual != revision)
                {
                    attempt
                        .failures
                        .push("Unexpected negotiated protocol revision".into());
                }
                attempts.push(attempt);
            }
            let mut stop_times: Vec<_> = attempts
                .iter()
                .filter_map(|attempt| attempt.stop_ms)
                .collect();
            stop_times.sort_by(f64::total_cmp);
            let rank = (95 * stop_times.len()).div_ceil(100);
            profiles.push(Profile {
                revision,
                scenario,
                stop_p95_ms: rank.checked_sub(1).map(|index| stop_times[index]),
                attempts,
            });
        }
    }
    let failed = profiles
        .iter()
        .flat_map(|profile| &profile.attempts)
        .filter(|attempt| !attempt.failures.is_empty())
        .count();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&report)
        .unwrap();
    serde_json::to_writer_pretty(
        &mut file,
        &json!({
            "version": 1,
            "transport": "stdio",
            "fixtureDigest": ManifestRevision::of(SAMPLE.as_bytes()),
            "workerDigest": worker_digest,
            "stopBudgetMs": STOP_BUDGET.as_millis(),
            "failedAttempts": failed,
            "profiles": profiles,
        }),
    )
    .unwrap();
    writeln!(file).unwrap();
    println!(
        "Managed stop report at {} ({failed} failed attempts)",
        report.display()
    );
    assert_eq!(failed, 0, "see the complete report for failed attempts");
}

async fn measure(
    service: &ProjectService,
    root: &Path,
    worker: &Path,
    preference: ProtocolPreference,
    scenario: Scenario,
    attempt: &mut Attempt,
) -> Result<(), String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|error| error.to_string())?;
    attempt.manifest_revision = Some(
        write_fixture(root, &listener).map_err(|error| format!("Writing the fixture: {error}"))?,
    );
    let session = start(service, root, worker, preference).await?;
    let pid = session.process_id();
    attempt.worker_pid = Some(pid);
    let observed_worker = observe_worker(pid);
    let exercise = exercise_call(&session, listener, scenario, attempt).await;

    let starting_stop = Instant::now();
    let stopped = session.stop().await;
    let elapsed = starting_stop.elapsed();
    attempt.stop_ms = Some(elapsed.as_secs_f64() * 1_000.0);
    if let Err(error) = exercise {
        attempt.failures.push(error);
    }
    if elapsed > STOP_BUDGET {
        attempt
            .failures
            .push("Managed stop exceeded five seconds".into());
    }
    match stopped {
        // Native stop checks the signaled worker handle and queries the job's
        // ActiveProcesses counter before returning success.
        Ok(()) => attempt.active_job_processes_at_stop = Some(0),
        Err(error) => attempt
            .failures
            .push(format!("Managed stop: {}", error.code())),
    }
    match observed_worker.and_then(|handle| worker_exited(&handle)) {
        Ok(exited) => {
            attempt.worker_exited = Some(exited);
            if !exited {
                attempt
                    .failures
                    .push("Worker was still running after managed stop".into());
            }
        }
        Err(error) => attempt
            .failures
            .push(format!("Observing the worker handle: {error}")),
    }
    Ok(())
}

fn write_fixture(root: &Path, listener: &TcpListener) -> io::Result<ManifestRevision> {
    let origin = format!("http://{}", listener.local_addr()?);
    let mut document = sample();
    let binding = document["bindings"][0].as_object_mut().unwrap();
    binding.insert("origin".into(), json!(origin));
    binding.remove("headersFromSecrets");
    document["secretRefs"] = json!([]);
    document["capabilities"][0]["timeoutMs"] = json!(20_000);
    let grant = &mut document["policy"]["grants"][0];
    grant["target"]["origin"] = json!(origin);
    grant["target"]["pathTemplate"] = json!("/items/42");
    grant["target"]["allowedPeerRanges"] = json!(["127.0.0.1/32"]);
    grant["limits"]["timeoutMs"] = json!(20_000);
    let bytes = serde_json::to_vec(&document)?;
    fs::write(root.join("mcp-project.json"), &bytes)?;
    Ok(ManifestRevision::of(&bytes))
}

async fn start(
    service: &ProjectService,
    root: &Path,
    worker: &Path,
    protocol: ProtocolPreference,
) -> Result<ManagedSession, String> {
    let prepared = match service
        .prepare_playground(
            ProjectLocation::Root(root.to_path_buf()),
            worker.to_path_buf(),
            PreviewMode::Live { scenario_id: None },
            PreviewAccess {
                approved_grant_ids: ["catalog_read".into()].into(),
                ..PreviewAccess::default()
            },
        )
        .await
    {
        Reply::Success { data, .. } => data,
        Reply::Failure { error, .. } => return Err(format!("Preparing the preview: {error:?}")),
    };
    let digest = prepared.review().digest;
    let approved = prepared
        .approve(digest)
        .map_err(|error| format!("Approving: {}", error.code()))?;
    approved
        .start(
            PreviewOptions {
                protocol,
                capture_payloads: false,
            },
            &CancellationToken::new(),
        )
        .await
        .map_err(|error| format!("Starting: {}", error.code()))
}

async fn exercise_call(
    session: &ManagedSession,
    listener: TcpListener,
    scenario: Scenario,
    attempt: &mut Attempt,
) -> Result<(), String> {
    let cancellation = CancellationToken::new();
    let call = Invocation::Tool {
        name: "lookup_item".into(),
        arguments: [("id".into(), json!("42"))].into_iter().collect(),
    };
    let request = session.invoke(
        &call,
        RequestOptions {
            cancellation: cancellation.clone(),
            timeout: REQUEST_TIMEOUT,
            ..RequestOptions::default()
        },
    );
    let fixture = async {
        let result = timeout(
            FIXTURE_TIMEOUT,
            serve_once(listener, scenario, &cancellation),
        )
        .await;
        if !matches!(result, Ok(Ok(()))) {
            cancellation.cancel();
        }
        result
    };
    let (run, upstream) = tokio::join!(request, fixture);
    let run = run.map_err(|error| format!("Invoking the tool: {}", error.code()))?;
    attempt.negotiated_revision = Some(run.result.protocol_revision);
    attempt.outcome = Some(run.result.outcome);
    upstream
        .map_err(|_| "Loopback fixture reached its deadline".to_owned())?
        .map_err(|error| format!("Loopback fixture: {error}"))?;
    if run.result.outcome != scenario.expected_outcome() {
        return Err(format!(
            "Unexpected invocation outcome: {:?}",
            run.result.outcome
        ));
    }
    if matches!(scenario, Scenario::AfterNormalCall)
        && run
            .payload
            .as_ref()
            .and_then(|payload| payload.pointer("/structuredContent/id"))
            .and_then(Value::as_str)
            != Some("42")
    {
        return Err("Normal invocation returned the wrong catalog item".into());
    }
    Ok(())
}

async fn serve_once(
    listener: TcpListener,
    scenario: Scenario,
    cancellation: &CancellationToken,
) -> io::Result<()> {
    let (mut socket, _) = listener.accept().await?;
    let mut request = Vec::new();
    let mut block = [0u8; 512];
    while !request.ends_with(b"\r\n\r\n") {
        let size = socket.read(&mut block).await?;
        if size == 0 || request.len() + size > 8_192 {
            return Err(io::Error::other("Incomplete or oversized fixture request"));
        }
        request.extend_from_slice(&block[..size]);
    }
    if !request.starts_with(b"GET /items/42 HTTP/1.1\r\n") {
        return Err(io::Error::other("Unexpected fixture request"));
    }
    match scenario {
        Scenario::AfterNormalCall => socket.write_all(RESPONSE).await?,
        Scenario::AfterCancellation => {
            // Cancel only after the handler has made its upstream request.
            cancellation.cancel();
            match socket.read(&mut block).await {
                Ok(0) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                    ) => {}
                Err(error) => return Err(error),
                Ok(_) => return Err(io::Error::other("Cancelled upstream sent extra bytes")),
            }
        }
    }
    Ok(())
}

fn observe_worker(pid: u32) -> io::Result<OwnedHandle> {
    // SAFETY: The PID came from the reviewed session. A successful call creates
    // a new synchronization handle, owned here until the post-stop check.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcess returned a fresh handle and ownership is transferred once.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn worker_exited(worker: &OwnedHandle) -> io::Result<bool> {
    // SAFETY: The retained synchronization handle is valid and this wait is nonblocking.
    match unsafe { WaitForSingleObject(worker.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}
