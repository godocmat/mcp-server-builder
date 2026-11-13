#![cfg(windows)]

use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use builder_playground::supervision::{
    EnvironmentSource, LaunchEnvironment, LaunchError, LaunchSpec, review_launch, spawn_reviewed,
};
use builder_project::ManifestRevision;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use windows_sys::Win32::{
    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
    Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType, SYNCHRONIZE},
    System::{
        JobObjects::IsProcessInJob,
        Threading::{
            CREATE_NO_WINDOW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
        },
    },
};

const CHILD_TEST: &str = "native_supervision_child_fixture";
const FIXTURE_MODE: &str = "MCP_BUILDER_SUPERVISION_FIXTURE_MODE";
const UNRELATED_HANDLE: &str = "MCP_BUILDER_SUPERVISION_UNRELATED_HANDLE";
const LARGE_INPUT_SIZE: usize = 256_000;
const REPORT_MARKER: &[u8] = b"SUPERVISION_REPORT_V1\0";
const STDERR_MARKER: &[u8] = b"supervision fixture stderr\n";

#[test]
#[ignore = "runs as a supervised subprocess"]
#[allow(clippy::zombie_processes)] // The outer test waits for the descendant's Windows handle.
fn native_supervision_child_fixture() {
    match env::var(FIXTURE_MODE).as_deref() {
        Ok("grandchild") => loop {
            std::thread::park();
        },
        Ok("tree") => {
            let child = Command::new(env::current_exe().unwrap())
                .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
                .creation_flags(CREATE_NO_WINDOW)
                .env_clear()
                .env(FIXTURE_MODE, "grandchild")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            write_report(&child.id().to_le_bytes());
            // Let the parent exit on stdin EOF. The supervisor still owns the descendant.
            assert_eq!(io::stdin().read(&mut [0]).unwrap(), 0);
        }
        Ok("report") => {
            let args: Vec<_> = env::args_os().skip(1).collect();
            let mut environment: Vec<_> = env::vars_os().collect();
            let candidate = env::var(UNRELATED_HANDLE)
                .unwrap()
                .parse::<usize>()
                .unwrap();
            // SAFETY: GetFileType validates the candidate without adopting it.
            assert_ne!(unsafe { GetFileType(candidate as _) }, FILE_TYPE_PIPE);
            environment.sort();
            let actual = format!("{args:?}\n{environment:?}\n");
            write_report(actual.as_bytes());
            io::stderr().write_all(STDERR_MARKER).unwrap();

            // Let the writer reach backpressure before consuming and verifying the message.
            std::thread::sleep(Duration::from_millis(100));
            let mut input = vec![0; LARGE_INPUT_SIZE];
            io::stdin().read_exact(&mut input).unwrap();
            assert!(input.iter().all(|&byte| byte == b'x'));
            assert_eq!(io::stdin().read(&mut [0]).unwrap(), 0);
            fs::write("observed-eof", b"yes").unwrap();
        }
        Ok("idle") => {
            write_report(b"ready");
            assert_eq!(io::stdin().read(&mut [0]).unwrap(), 0);
            fs::write("observed-eof", b"yes").unwrap();
        }
        Ok("supervisor-exit") => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let spec = spec(&env::current_dir().unwrap(), "tree", &[]);
                let review = review_launch(&spec, binding()).unwrap();
                let mut child =
                    spawn_reviewed(&spec, binding(), review.digest(), |_| None).unwrap();
                let mut report = child.id().to_le_bytes().to_vec();
                report.extend(read_report(child.take_stdout().unwrap()).await);
                write_report(&report);
                // Simulate a supervisor process exiting without its Rust destructors.
                assert_eq!(io::stdin().read(&mut [0]).unwrap(), 1);
                std::process::exit(0);
            });
        }
        other => panic!("unexpected test fixture mode {other:?}"),
    }
}

fn write_report(report: &[u8]) {
    let mut stdout = io::stdout().lock();
    stdout.write_all(REPORT_MARKER).unwrap();
    stdout
        .write_all(&(report.len() as u32).to_le_bytes())
        .unwrap();
    stdout.write_all(report).unwrap();
    stdout.flush().unwrap();
}

async fn read_report(mut stdout: impl AsyncRead + Unpin) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut marker = Vec::with_capacity(REPORT_MARKER.len());
        while !marker.ends_with(REPORT_MARKER) && marker.len() < 8_192 {
            marker.push(stdout.read_u8().await.unwrap());
        }
        assert!(marker.ends_with(REPORT_MARKER));
        let length = stdout.read_u32_le().await.unwrap() as usize;
        assert!(length < 8_192);
        let mut report = vec![0; length];
        stdout.read_exact(&mut report).await.unwrap();
        report
    })
    .await
    .expect("fixture should report promptly")
}

fn binding() -> ManifestRevision {
    ManifestRevision::of(b"synthetic test registration, fixture, grants and secret references")
}

fn spec(cwd: &Path, mode: &str, test_filters: &[&str]) -> LaunchSpec {
    let args = ["--exact", CHILD_TEST, "--ignored", "--nocapture", "--"]
        .into_iter()
        .chain(test_filters.iter().copied())
        .map(OsString::from)
        .collect();
    LaunchSpec {
        executable: env::current_exe().unwrap(),
        cwd: cwd.to_path_buf(),
        args,
        environment: vec![LaunchEnvironment {
            name: FIXTURE_MODE.into(),
            source: EnvironmentSource::Literal(mode.into()),
        }],
    }
}

fn process_handle(pid: u32) -> OwnedHandle {
    // SAFETY: The process ID came from the fixture or the supervisor. The returned
    // handle is used only for query and wait.
    let raw = unsafe { OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    assert!(!raw.is_null(), "{}", io::Error::last_os_error());
    // SAFETY: OpenProcess returned a new owned handle.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

fn assert_live_job_member(handle: &OwnedHandle) {
    let mut member = 0;
    // SAFETY: The handle is live and the output is writable. A null job asks about any job.
    assert_ne!(
        unsafe { IsProcessInJob(handle.as_raw_handle(), null_mut(), &mut member) },
        0
    );
    assert_ne!(member, 0);
    // SAFETY: The handle is live and this wait is nonblocking.
    assert_eq!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
}

fn assert_exited(handle: &OwnedHandle) {
    // SAFETY: The handle is live and this wait is bounded.
    assert_eq!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 1_000) },
        WAIT_OBJECT_0
    );
}

fn unrelated_inheritable_handles() -> (OwnedHandle, OwnedHandle) {
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Pipes::CreatePipe;

    let mut read_handle = null_mut();
    let mut write_handle = null_mut();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        bInheritHandle: 1,
        ..Default::default()
    };
    // SAFETY: The output handles and correctly sized security attributes are live.
    assert_ne!(
        unsafe { CreatePipe(&mut read_handle, &mut write_handle, &attributes, 0) },
        0
    );
    // SAFETY: CreatePipe returned two new owned handles.
    unsafe {
        (
            OwnedHandle::from_raw_handle(read_handle),
            OwnedHandle::from_raw_handle(write_handle),
        )
    }
}

#[test]
fn review_rejects_a_gui_executable() {
    let cwd = tempfile::tempdir().unwrap();
    let mut spec = spec(cwd.path(), "idle", &[]);
    let executable = cwd.path().join("gui fixture.exe");
    let mut bytes = fs::read(&spec.executable).unwrap();
    let pe_offset = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
    let subsystem = pe_offset + 4 + 20 + 68;
    bytes[subsystem..subsystem + 2].copy_from_slice(&2u16.to_le_bytes());
    fs::write(&executable, bytes).unwrap();
    spec.executable = executable;
    assert_eq!(
        review_launch(&spec, binding()).err(),
        Some(LaunchError::InvalidSpec)
    );
}

#[tokio::test]
async fn preserves_native_arguments_environment_and_stdio() {
    let cwd = tempfile::tempdir().unwrap();
    let working_directory = cwd.path().join("working directory ž");
    fs::create_dir(&working_directory).unwrap();
    let special_args = [
        "",
        "word with spaces",
        "\"quotes\"",
        r#"x\\"tail\"#,
        "žltý",
        "a\tb",
    ];
    let mut spec = spec(&working_directory, "report", &special_args);
    let executable = cwd.path().join("supervision fixture ž.exe");
    fs::copy(&spec.executable, &executable).unwrap();
    spec.executable = executable;
    let (unrelated_read, _unrelated_write) = unrelated_inheritable_handles();
    let unrelated_handle = (unrelated_read.as_raw_handle() as usize).to_string();
    spec.environment.extend([
        LaunchEnvironment {
            name: "mixed_Case".into(),
            source: EnvironmentSource::Literal("with=equals ž".into()),
        },
        LaunchEnvironment {
            name: "SYNTHETIC_SECRET".into(),
            source: EnvironmentSource::SecretReference("fixture-token".into()),
        },
        LaunchEnvironment {
            name: "ZZ_EMPTY".into(),
            source: EnvironmentSource::Literal(OsString::new()),
        },
        LaunchEnvironment {
            name: UNRELATED_HANDLE.into(),
            source: EnvironmentSource::Literal(unrelated_handle.clone().into()),
        },
    ]);
    let review = review_launch(&spec, binding()).unwrap();
    assert_eq!(review.args(), spec.args);
    let expected_args = spec.args.clone();
    let mut child = tokio::task::spawn_blocking(move || {
        spawn_reviewed(&spec, binding(), review.digest(), |reference| {
            assert_eq!(reference, "fixture-token");
            Some("synthetic value".into())
        })
    })
    .await
    .unwrap()
    .unwrap();
    let process = process_handle(child.id());
    assert_live_job_member(&process);
    let mut stdout = child.take_stdout().unwrap();
    let mut stderr = child.take_stderr().unwrap();
    let mut stdin = child.take_stdin().unwrap();
    let actual = String::from_utf8(read_report(&mut stdout).await).unwrap();
    let mut expected_environment = vec![
        (FIXTURE_MODE, "report"),
        ("mixed_Case", "with=equals ž"),
        ("SYNTHETIC_SECRET", "synthetic value"),
        ("ZZ_EMPTY", ""),
        (UNRELATED_HANDLE, unrelated_handle.as_str()),
    ];
    expected_environment.sort();
    assert_eq!(
        actual,
        format!("{expected_args:?}\n{expected_environment:?}\n")
    );
    let mut stderr_marker = vec![0; STDERR_MARKER.len()];
    tokio::time::timeout(
        Duration::from_secs(5),
        stderr.read_exact(&mut stderr_marker),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(stderr_marker, STDERR_MARKER);

    tokio::time::timeout(Duration::from_secs(5), async {
        stdin
            .write_all(&vec![b'x'; LARGE_INPUT_SIZE])
            .await
            .unwrap();
        stdin.shutdown().await.unwrap();
    })
    .await
    .unwrap();
    child.stop().await.unwrap();
    assert_exited(&process);
    assert_eq!(
        fs::read(working_directory.join("observed-eof")).unwrap(),
        b"yes"
    );
    assert_eq!(
        stdin.write(b"x").await.unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
}

#[tokio::test]
async fn changed_inputs_and_missing_credentials_have_no_child_side_effect() {
    let scratch = tempfile::tempdir().unwrap();
    let cwd = scratch.path().join("working directory");
    fs::create_dir(&cwd).unwrap();
    let executable = scratch.path().join("reviewed fixture.exe");
    let mut spec = spec(&cwd, "idle", &[]);
    fs::copy(&spec.executable, &executable).unwrap();
    spec.executable = executable.clone();
    spec.environment.push(LaunchEnvironment {
        name: "MCP_TEST_REFERENCE".into(),
        source: EnvironmentSource::SecretReference("original-reference".into()),
    });
    let review = review_launch(&spec, binding()).unwrap();
    let never_resolve = |_: &str| panic!("changed launch must fail before resolving secrets");
    let mut changed_arg = spec.clone();
    changed_arg.args.push("added".into());
    let mut changed_environment = spec.clone();
    changed_environment.environment[1].source =
        EnvironmentSource::SecretReference("changed-reference".into());
    let mut changed_literal = spec.clone();
    changed_literal.environment[0].source = EnvironmentSource::Literal("changed-mode".into());
    let mut changed_cwd = spec.clone();
    changed_cwd.cwd = scratch.path().to_path_buf();
    let mut changed_executable = spec.clone();
    changed_executable.executable = env::current_exe().unwrap();
    for changed in [
        &changed_arg,
        &changed_environment,
        &changed_literal,
        &changed_cwd,
        &changed_executable,
    ] {
        assert_eq!(
            spawn_reviewed(changed, binding(), review.digest(), never_resolve).err(),
            Some(LaunchError::ReviewMismatch)
        );
    }
    assert_eq!(
        spawn_reviewed(
            &spec,
            ManifestRevision::of(b"revoked grant"),
            review.digest(),
            never_resolve
        )
        .err(),
        Some(LaunchError::ReviewMismatch)
    );
    assert_eq!(
        spawn_reviewed(&spec, binding(), review.digest(), |_| None).err(),
        Some(LaunchError::CredentialUnavailable)
    );
    let previous_directory = scratch.path().join("previous directory");
    fs::rename(&cwd, &previous_directory).unwrap();
    fs::create_dir(&cwd).unwrap();
    assert_eq!(
        spawn_reviewed(&spec, binding(), review.digest(), never_resolve).err(),
        Some(LaunchError::ReviewMismatch)
    );

    let review = review_launch(&spec, binding()).unwrap();
    let linked = scratch.path().join("additional fixture name.exe");
    fs::hard_link(&executable, &linked).unwrap();
    assert_eq!(
        spawn_reviewed(&spec, binding(), review.digest(), never_resolve).err(),
        Some(LaunchError::ReviewMismatch)
    );
    fs::remove_file(linked).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&executable)
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    assert_eq!(
        spawn_reviewed(&spec, binding(), review.digest(), never_resolve).err(),
        Some(LaunchError::ReviewMismatch)
    );
    let review = review_launch(&spec, binding()).unwrap();
    let replaced = scratch.path().join("previous fixture.exe");
    fs::rename(&executable, &replaced).unwrap();
    fs::copy(&replaced, &executable).unwrap();
    assert_eq!(
        spawn_reviewed(&spec, binding(), review.digest(), never_resolve).err(),
        Some(LaunchError::ReviewMismatch)
    );
    assert!(!cwd.join("observed-eof").exists());

    let review = review_launch(&spec, binding()).unwrap();
    let mut child = spawn_reviewed(&spec, binding(), review.digest(), |_| {
        assert!(fs::rename(&cwd, scratch.path().join("moved cwd")).is_err());
        assert!(OpenOptions::new().append(true).open(&executable).is_err());
        Some("synthetic".into())
    })
    .unwrap();
    assert_eq!(read_report(child.take_stdout().unwrap()).await, b"ready");
    child.stop().await.unwrap();
    assert_eq!(fs::read(cwd.join("observed-eof")).unwrap(), b"yes");
}

#[tokio::test]
async fn stops_a_job_descendant_even_after_the_stdio_process_exits() {
    let cwd = tempfile::tempdir().unwrap();
    let spec = spec(cwd.path(), "tree", &[]);
    let review = review_launch(&spec, binding()).unwrap();
    let mut child = spawn_reviewed(&spec, binding(), review.digest(), |_| None).unwrap();
    let process = process_handle(child.id());
    let mut stdin = child.take_stdin().unwrap();
    let pid = u32::from_le_bytes(
        read_report(child.take_stdout().unwrap())
            .await
            .try_into()
            .unwrap(),
    );
    let descendant = process_handle(pid);
    assert_live_job_member(&process);
    assert_live_job_member(&descendant);

    stdin.shutdown().await.unwrap();
    assert_exited(&process);
    assert_live_job_member(&descendant);
    let started = Instant::now();
    child.stop().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_exited(&descendant);
}

#[tokio::test]
async fn dropping_the_supervisor_terminates_its_process_tree() {
    let cwd = tempfile::tempdir().unwrap();
    let spec = spec(cwd.path(), "tree", &[]);
    let review = review_launch(&spec, binding()).unwrap();
    let mut child = spawn_reviewed(&spec, binding(), review.digest(), |_| None).unwrap();
    let process = process_handle(child.id());
    let pid = u32::from_le_bytes(
        read_report(child.take_stdout().unwrap())
            .await
            .try_into()
            .unwrap(),
    );
    let descendant = process_handle(pid);
    assert_live_job_member(&descendant);
    drop(child);
    assert_exited(&process);
    assert_exited(&descendant);
}

#[tokio::test]
async fn abrupt_supervisor_exit_closes_the_job() {
    let cwd = tempfile::tempdir().unwrap();
    let spec = spec(cwd.path(), "supervisor-exit", &[]);
    let review = review_launch(&spec, binding()).unwrap();
    let mut supervisor = spawn_reviewed(&spec, binding(), review.digest(), |_| None).unwrap();
    let report = read_report(supervisor.take_stdout().unwrap()).await;
    let child = process_handle(u32::from_le_bytes(report[..4].try_into().unwrap()));
    let descendant = process_handle(u32::from_le_bytes(report[4..].try_into().unwrap()));
    assert_live_job_member(&child);
    assert_live_job_member(&descendant);
    let mut input = supervisor.take_stdin().unwrap();
    input.write_all(b"x").await.unwrap();
    // Keep the outer owner alive so only the inner supervisor's exit can kill its job.
    assert_exited(&child);
    assert_exited(&descendant);
    supervisor.stop().await.unwrap();
}
