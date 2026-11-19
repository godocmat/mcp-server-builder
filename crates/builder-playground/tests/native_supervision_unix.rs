#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::ffi::OsString;
use std::fs::{self, File};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use builder_playground::supervision::{
    EnvironmentSource, LaunchEnvironment, LaunchError, LaunchSpec, review_launch, spawn_reviewed,
};
use builder_project::ManifestRevision;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::{Instant, sleep, timeout};

const FD_FIXTURE: &str = "MCP_NATIVE_UNIX_FIXTURE_FD";
const CREDENTIAL_REFERENCE: &str = "fixture-env";

#[test]
fn child_fixture() {
    let Some(descriptor) = std::env::var_os(FD_FIXTURE) else {
        return;
    };
    let descriptor: i32 = descriptor.to_str().unwrap().parse().unwrap();
    // SAFETY: F_GETFD checks the selected FD and getpgrp reads this process's group.
    assert_eq!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EBADF)
    );
    assert!(std::env::var_os("PATH").is_none());
    assert_eq!(std::env::var_os("MCP_REVIEWED_WORD").unwrap(), "two words");
    // SAFETY: getpgrp reads this process's own group ID.
    let group = unsafe { libc::getpgrp() };
    let mut descendant = Command::new("/bin/sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let descendant_pid = descendant.id();
    // The fixture normally reaps the child, but exits at EOF even if its
    // sleeper is still alive. This exercises cleanup after the leader exits.
    std::thread::spawn(move || descendant.wait().unwrap());
    println!(
        "mcp-fixture {} {} {}",
        std::process::id(),
        group,
        descendant_pid
    );
    std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
}

fn temp() -> tempfile::TempDir {
    let base = std::env::temp_dir().canonicalize().unwrap();
    tempfile::tempdir_in(base).unwrap()
}

fn spec(worker: PathBuf, cwd: &Path) -> LaunchSpec {
    LaunchSpec {
        executable: worker,
        args: vec![
            "--exact".into(),
            "child_fixture".into(),
            "--nocapture".into(),
        ],
        cwd: cwd.to_path_buf(),
        environment: vec![LaunchEnvironment {
            name: "MCP_REVIEWED_WORD".into(),
            source: EnvironmentSource::SecretReference(CREDENTIAL_REFERENCE.into()),
        }],
    }
}

fn binding() -> ManifestRevision {
    ManifestRevision::of(b"private unix process group")
}

#[tokio::test]
async fn closed_descriptors_and_private_group_stop_and_drop() {
    let cwd = temp();
    let opened = File::open("/dev/null").unwrap();
    // SAFETY: F_DUPFD makes a fresh deliberately inheritable test descriptor.
    let descriptor = unsafe { libc::fcntl(opened.as_raw_fd(), libc::F_DUPFD, 64) };
    assert!(descriptor >= 64);
    // SAFETY: This owns the descriptor returned by F_DUPFD.
    let inherited = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let mut spec = spec(std::env::current_exe().unwrap(), cwd.path());
    spec.environment.push(LaunchEnvironment {
        name: FD_FIXTURE.into(),
        source: EnvironmentSource::Literal(inherited.as_raw_fd().to_string().into()),
    });
    let review = review_launch(&spec, binding()).unwrap();
    assert_eq!(review.cwd(), cwd.path());
    for explicit_stop in [true, false] {
        let mut child = spawn_reviewed(&spec, binding(), review.digest(), |reference| {
            assert_eq!(reference, CREDENTIAL_REFERENCE);
            Some(OsString::from("two words"))
        })
        .unwrap();
        let worker = child.id();
        let mut lines = BufReader::new(child.take_stdout().unwrap()).lines();
        let ids = timeout(Duration::from_secs(10), async {
            while let Some(line) = lines.next_line().await.unwrap() {
                if let Some(report) = line.strip_prefix("mcp-fixture ") {
                    return report
                        .split_ascii_whitespace()
                        .map(|id| id.parse::<u32>().unwrap())
                        .collect::<Vec<_>>();
                }
            }
            panic!("the native child did not report process group membership");
        })
        .await
        .unwrap();
        assert_eq!(&ids[..2], &[worker, worker]);
        let descendant = ids[2];
        wait_until_exited(worker, false).await;
        if explicit_stop {
            child.stop().await.unwrap();
        } else {
            drop(child);
        }
        wait_until_exited(worker, true).await;
        wait_until_exited(descendant, true).await;
    }
}

async fn wait_until_exited(pid: u32, expected: bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if inactive(pid) == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "PID {pid} inactive expected {expected}"
        );
        sleep(Duration::from_millis(20)).await;
    }
}

fn inactive(pid: u32) -> bool {
    // SAFETY: Signal zero checks process existence without delivering a signal.
    if unsafe { libc::kill(pid.cast_signed(), 0) } != 0 {
        return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    #[cfg(target_os = "linux")]
    {
        // A test container's init may defer adopting and reaping a signaled orphan.
        let path = format!("/proc/{pid}/stat");
        fs::read_to_string(path)
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(") ")
                    .map(|(_, tail)| tail.starts_with('Z'))
            })
            .unwrap_or(false)
    }
    #[cfg(target_os = "macos")]
    false
}

#[tokio::test]
async fn changed_image_needs_review_before_resolving_environment() {
    let cwd = temp();
    let selected = cwd.path().join("reviewed");
    fs::copy(std::env::current_exe().unwrap(), &selected).unwrap();
    let spec = spec(selected.clone(), cwd.path());
    let approved = review_launch(&spec, binding()).unwrap().digest();
    fs::rename(&selected, cwd.path().join("previous")).unwrap();
    fs::write(&selected, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&selected, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        spawn_reviewed(&spec, binding(), approved, |_| panic!(
            "approval must be current"
        ))
        .err(),
        Some(LaunchError::ReviewMismatch)
    );
}
