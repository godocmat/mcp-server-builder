use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use builder_app::playground::{
    CancellationToken, PlaygroundRunDetails, PreviewAccess, PreviewError, PreviewMode,
    PreviewOptions, ProtocolPreference, RequestOptions, RunOutcome, RunResultV1, TraceSnapshot,
    failure_reply, serve_worker,
};
use builder_app::{
    Diagnostic, DiagnosticCode, DiagnosticDetails, ManifestRevision, ProjectLocation,
    ProjectService, Reply, Severity,
};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::write_reply;

#[derive(Subcommand)]
pub enum Action {
    /// Show the launch and fixture permissions for operator approval.
    Review(Fixture),
    /// Start preview, replay the reviewed scenario and stop preview.
    Replay {
        #[command(flatten)]
        fixture: Fixture,
        /// Approve the digest returned by playground review.
        #[arg(long)]
        approve: Option<ManifestRevision>,
        /// Explicitly include transient, sanitized trace payloads in JSON output.
        #[arg(long)]
        trace_payloads: bool,
        #[arg(long, value_enum, default_value_t = Protocol::Auto)]
        protocol: Protocol,
        #[arg(long, default_value_t = 10_000, value_parser = clap::value_parser!(u64).range(1..=30_000))]
        timeout_ms: u64,
    },
}

#[derive(Args)]
pub struct Fixture {
    /// Folder containing mcp-project.json.
    root: PathBuf,
    /// ID of a saved fixture-mode scenario.
    #[arg(long)]
    scenario: String,
    /// Select a grant by ID. Repeat this option to include more grants.
    #[arg(long)]
    grant: Vec<String>,
    /// Select a prompt by ID. Repeat this option to include more prompts.
    #[arg(long)]
    prompt: Vec<String>,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Protocol {
    Auto,
    Modern,
    Legacy,
}

impl From<Protocol> for ProtocolPreference {
    fn from(value: Protocol) -> Self {
        match value {
            Protocol::Auto => Self::Auto,
            Protocol::Modern => Self::Modern,
            Protocol::Legacy => Self::Legacy,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayResult {
    run: RunResultV1,
    trace: TraceSnapshot,
}

pub fn run(action: Action, json: bool) -> io::Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(execute(action, json))
}

pub fn worker() -> ExitCode {
    std::panic::set_hook(Box::new(|_| {
        let _ = writeln!(io::stderr().lock(), "playground.worker_failed");
    }));
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::FAILURE;
    };
    let result = runtime.block_on(serve_worker(tokio::io::stdin(), tokio::io::stdout()));
    runtime.shutdown_timeout(Duration::from_millis(250));
    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn execute(action: Action, json: bool) -> io::Result<ExitCode> {
    let (fixture, command) = match &action {
        Action::Review(fixture) => (fixture, "playground.review"),
        Action::Replay { fixture, .. } => (fixture, "playground.replay"),
    };
    let worker = match std::env::current_exe() {
        Ok(path) => path,
        Err(_) => {
            return write_error(
                command,
                failure_reply(PreviewError::WorkerUnavailable),
                json,
            );
        }
    };
    let plan = ProjectService::for_user()
        .prepare_playground(
            ProjectLocation::Root(fixture.root.clone()),
            worker,
            PreviewMode::Fixture {
                scenario_id: fixture.scenario.clone(),
            },
            PreviewAccess {
                approved_grant_ids: fixture.grant.iter().cloned().collect(),
                approved_prompt_ids: fixture.prompt.iter().cloned().collect(),
                ..PreviewAccess::default()
            },
        )
        .await;
    let plan = match plan {
        Reply::Success { data, .. } => data,
        Reply::Failure { error, diagnostics } => {
            return write_error(command, Reply::Failure { error, diagnostics }, json);
        }
    };
    match action {
        Action::Review(fixture) => {
            write_reply(
                command,
                &Reply::Success {
                    data: plan.review(),
                    diagnostics: vec![],
                },
                json,
                |review| {
                    let mut output = io::stdout().lock();
                    writeln!(output, "Executable  {}", review.executable.display())?;
                    writeln!(output, "Arguments  {}", review.arguments.join(" "))?;
                    writeln!(
                        output,
                        "Working directory  {}",
                        review.working_directory.display()
                    )?;
                    writeln!(output, "Manifest revision  {}", review.manifest_revision)?;
                    for (name, value) in &review.environment {
                        writeln!(output, "Environment  {name}={value}")?;
                    }
                    writeln!(output, "Scenario  {}", fixture.scenario)?;
                    for grant in &review.approved_grants {
                        writeln!(output, "Grant  {}", serde_json::to_string(grant)?)?;
                    }
                    for prompt in review.approved_prompt_ids {
                        writeln!(output, "Prompt  {prompt}")?;
                    }
                    writeln!(output, "Approval digest  {}", review.digest)
                },
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Action::Replay {
            approve,
            trace_payloads,
            protocol,
            timeout_ms,
            ..
        } => {
            let Some(approve) = approve else {
                return write_error(command, failure_reply(PreviewError::ApprovalRequired), json);
            };
            let approved = match plan.approve(approve) {
                Ok(approved) => approved,
                Err(error) => return write_error(command, failure_reply(error), json),
            };
            let cancellation = CancellationToken::new();
            let signal = tokio::spawn({
                let cancellation = cancellation.clone();
                async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        cancellation.cancel();
                    }
                }
            });
            let operation = async {
                let session = approved
                    .start(
                        PreviewOptions {
                            protocol: protocol.into(),
                            capture_payloads: trace_payloads && json,
                        },
                        &cancellation,
                    )
                    .await?;
                let run = session
                    .replay(RequestOptions {
                        timeout: Duration::from_millis(timeout_ms),
                        cancellation,
                        ..RequestOptions::default()
                    })
                    .await;
                let trace = session.trace();
                let stop_error = session.stop().await.err();
                let data = ReplayResult {
                    run: run?.result,
                    trace,
                };
                Ok::<_, PreviewError>((data, stop_error))
            }
            .await;
            signal.abort();
            let (data, stop_error) = match operation {
                Ok(data) => data,
                Err(error) => return write_error(command, failure_reply(error), json),
            };
            let failure = stop_error
                .as_ref()
                .map(|error| (error.code(), error.message()))
                .or_else(|| {
                    data.run
                        .diagnostic
                        .as_ref()
                        .map(|diagnostic| (diagnostic.code, diagnostic.message))
                });
            if let Some((code, message)) = failure {
                let status = exit_code(code);
                if !json {
                    let mut stderr = io::stderr().lock();
                    writeln!(stderr, "{message} ({code})")?;
                    for assertion in data.run.assertions.iter().filter(|item| !item.passed) {
                        if let Some(path) = &assertion.path {
                            writeln!(
                                stderr,
                                "Assertion {} differs at {path}.",
                                assertion.index + 1
                            )?;
                        } else {
                            writeln!(stderr, "Assertion {} failed.", assertion.index + 1)?;
                        }
                    }
                    return Ok(status);
                }
                let reply = Reply::<()>::Failure {
                    error: Diagnostic {
                        code: DiagnosticCode::Playground(code),
                        message: message.into(),
                        severity: Severity::Error,
                        correlation_id: data.run.correlation_id,
                        action: None,
                        field: None,
                        capability_id: None,
                        details: Some(Box::new(DiagnosticDetails::PlaygroundRun(Box::new(
                            PlaygroundRunDetails {
                                run_id: data.run.run_id,
                                run: data.run,
                                trace: data.trace,
                            },
                        )))),
                    },
                    diagnostics: vec![],
                };
                write_reply(command, &reply, true, |_| Ok(()))?;
                return Ok(status);
            }
            debug_assert_eq!(data.run.outcome, RunOutcome::Passed);
            write_reply(
                command,
                &Reply::Success {
                    data,
                    diagnostics: vec![],
                },
                json,
                |data| {
                    writeln!(
                        io::stdout().lock(),
                        "Scenario passed in {} ms.",
                        data.run.duration_ms
                    )
                },
            )?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn write_error(command: &'static str, reply: Reply<()>, json: bool) -> io::Result<ExitCode> {
    use DiagnosticCode as Code;
    let Reply::Failure { error, .. } = &reply else {
        unreachable!("only failures enter this writer")
    };
    let status = match error.code {
        Code::Playground(code) => exit_code(code),
        Code::ProjectInvalid | Code::ProjectUnsafePath | Code::ProjectUnsupportedSchema => {
            ExitCode::from(2)
        }
        Code::InternalError => ExitCode::FAILURE,
        _ => ExitCode::from(6),
    };
    write_reply(command, &reply, json, |_| Ok(()))?;
    Ok(status)
}

fn exit_code(code: &str) -> ExitCode {
    ExitCode::from(match code {
        "input.invalid"
        | "mcp.unknown_tool"
        | "mcp.unknown_resource"
        | "mcp.unknown_prompt"
        | "playground.invalid_access"
        | "playground.invalid_launch"
        | "scenario.required"
        | "scenario.invalid"
        | "scenario.not_found"
        | "scenario.mode_mismatch"
        | "scenario.live_requires_opt_in"
        | "credential.invalid" => 2,
        "policy.denied"
        | "policy.approval_required"
        | "auth.required"
        | "auth.insufficient_scope" => 3,
        "scenario.assertion_failed" => 4,
        "credential.missing"
        | "credential.unavailable"
        | "adapter.file_root_unavailable"
        | "playground.launch_unsupported" => 6,
        "run.cancelled" => 130,
        _ => 5,
    })
}
