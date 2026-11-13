use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use builder_app::playground::{
    CancellationToken, PreviewAccess, PreviewError, PreviewMode, PreviewOptions,
    ProtocolPreference, RequestOptions, RunOutcome, RunResultV1, TraceSnapshot, failure_reply,
    serve_worker,
};
use builder_app::{DiagnosticCode, ManifestRevision, ProjectLocation, ProjectService, Reply};
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
                    writeln!(output, "Environment  cleared")?;
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
                session.stop().await?;
                Ok::<_, PreviewError>(ReplayResult {
                    run: run?.result,
                    trace,
                })
            }
            .await;
            signal.abort();
            let data = match operation {
                Ok(data) => data,
                Err(error) => return write_error(command, failure_reply(error), json),
            };
            let exit = run_exit(data.run.outcome);
            write_reply(
                command,
                &Reply::Success {
                    data,
                    diagnostics: vec![],
                },
                json,
                |data| {
                    let mut output = io::stdout().lock();
                    if let Some(diagnostic) = &data.run.diagnostic {
                        writeln!(output, "{} ({})", diagnostic.message, diagnostic.code)?;
                    } else {
                        writeln!(output, "Scenario passed in {} ms.", data.run.duration_ms)?;
                    }
                    for assertion in data.run.assertions.iter().filter(|item| !item.passed) {
                        if let Some(path) = &assertion.path {
                            writeln!(
                                output,
                                "Assertion {} differs at {path}.",
                                assertion.index + 1
                            )?;
                        } else {
                            writeln!(output, "Assertion {} failed.", assertion.index + 1)?;
                        }
                    }
                    Ok(())
                },
            )?;
            Ok(exit)
        }
    }
}

fn write_error(command: &'static str, reply: Reply<()>, json: bool) -> io::Result<ExitCode> {
    use DiagnosticCode as Code;
    let Reply::Failure { error, .. } = &reply else {
        unreachable!("only failures enter this writer")
    };
    let status = match error.code {
        Code::Playground("policy.approval_required") => 3,
        Code::Playground("run.cancelled") => 130,
        Code::Playground(
            "runtime.timeout" | "playground.handoff_failed" | "playground.stop_failed",
        ) => 5,
        Code::Playground(
            "playground.invalid_access"
            | "playground.invalid_launch"
            | "scenario.not_found"
            | "scenario.mode_mismatch"
            | "scenario.live_requires_opt_in",
        )
        | Code::ProjectInvalid
        | Code::ProjectUnsafePath
        | Code::ProjectUnsupportedSchema => 2,
        _ => 6,
    };
    write_reply(command, &reply, json, |_| Ok(()))?;
    Ok(ExitCode::from(status))
}

fn run_exit(outcome: RunOutcome) -> ExitCode {
    ExitCode::from(match outcome {
        RunOutcome::Passed => 0,
        RunOutcome::Invalid => 2,
        RunOutcome::Denied => 3,
        RunOutcome::AssertionFailed => 4,
        RunOutcome::TimedOut => 5,
        RunOutcome::Failed => 6,
        RunOutcome::Cancelled => 130,
    })
}
