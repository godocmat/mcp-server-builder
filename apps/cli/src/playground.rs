use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use builder_app::playground::{
    CancellationToken, PlaygroundRunDetails, PreparedProjectPreview, PreviewAccess, PreviewError,
    PreviewMode, PreviewOptions, PreviewReview, ProtocolPreference, RequestOptions, RunOutcome,
    RunResultV1, TraceSnapshot, failure_reply, serve_worker,
};
use builder_app::{
    Diagnostic, DiagnosticCode, DiagnosticDetails, ManifestRevision, ProjectLocation,
    ProjectService, Reply, Severity,
};
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::{failure, finish};

mod suite;

#[derive(Subcommand)]
pub enum Action {
    /// Show the launch and fixture permissions for operator approval.
    Review(Fixture),
    /// Start preview, replay the reviewed scenario and stop preview.
    Replay {
        #[command(flatten)]
        fixture: Fixture,
        #[command(flatten)]
        options: ReplayOptions,
    },
}

#[derive(Subcommand)]
pub enum PolicyAction {
    /// Print the current review and approval digest.
    Review {
        #[command(subcommand)]
        subject: ReviewSubject,
    },
}

#[derive(Subcommand)]
pub enum ReviewSubject {
    /// Review the first-party worker and its access for saved fixture scenarios.
    Launch(Selection),
}

#[derive(Subcommand)]
pub enum TestAction {
    /// Run a saved fixture or all saved fixtures after launch approval.
    Run {
        #[command(flatten)]
        selection: Selection,
        #[command(flatten)]
        options: ReplayOptions,
    },
}

#[derive(Args)]
pub struct Fixture {
    /// Folder containing mcp-project.json.
    root: PathBuf,
    /// ID of a saved fixture-mode scenario.
    #[arg(long)]
    scenario: String,
    #[command(flatten)]
    access: Access,
}

#[derive(Args)]
#[command(group(ArgGroup::new("scope").required(true).args(["scenario", "all"])))]
pub struct Selection {
    /// Folder containing mcp-project.json.
    root: PathBuf,
    /// ID of a saved fixture-mode scenario.
    #[arg(long)]
    scenario: Option<String>,
    /// Select all fixture-mode scenarios in manifest order.
    #[arg(long)]
    all: bool,
    #[command(flatten)]
    access: Access,
}

#[derive(Args)]
struct Access {
    /// Select a grant by ID. Repeat this option to include more grants.
    #[arg(long)]
    grant: Vec<String>,
    /// Select a prompt by ID. Repeat this option to include more prompts.
    #[arg(long)]
    prompt: Vec<String>,
}

#[derive(Args)]
pub struct ReplayOptions {
    /// Approve the digest returned by the matching launch review.
    #[arg(long)]
    approve: Option<ManifestRevision>,
    /// Include transient, sanitized trace payloads in JSON output for a single scenario.
    #[arg(long, requires = "scenario")]
    trace_payloads: bool,
    #[arg(long, value_enum, default_value_t = Protocol::Auto)]
    protocol: Protocol,
    #[arg(long, default_value_t = 10_000, value_parser = clap::value_parser!(u64).range(1..=30_000))]
    timeout_ms: u64,
}

#[derive(Clone, Copy, ValueEnum)]
enum Protocol {
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

enum Operation {
    Review,
    Replay(ReplayOptions),
}

struct Launch {
    service: ProjectService,
    root: PathBuf,
    worker: PathBuf,
    access: Access,
}

impl Launch {
    async fn prepare(&self, scenario: &str) -> Result<PreparedProjectPreview, Box<Reply<()>>> {
        reply_data(
            self.service
                .prepare_playground(
                    ProjectLocation::Root(self.root.clone()),
                    self.worker.clone(),
                    PreviewMode::Fixture {
                        scenario_id: scenario.into(),
                    },
                    PreviewAccess {
                        approved_grant_ids: self.access.grant.iter().cloned().collect(),
                        approved_prompt_ids: self.access.prompt.iter().cloned().collect(),
                        ..PreviewAccess::default()
                    },
                )
                .await,
        )
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayResult {
    run: RunResultV1,
    trace: TraceSnapshot,
}

pub fn run_legacy(action: Action, json: bool) -> io::Result<ExitCode> {
    let (fixture, command, operation) = match action {
        Action::Review(fixture) => (fixture, "playground.review", Operation::Review),
        Action::Replay { fixture, options } => {
            (fixture, "playground.replay", Operation::Replay(options))
        }
    };
    let selection = Selection {
        root: fixture.root,
        scenario: Some(fixture.scenario),
        all: false,
        access: fixture.access,
    };
    run(command, selection, operation, json)
}

pub fn run_policy(action: PolicyAction, json: bool) -> io::Result<ExitCode> {
    let PolicyAction::Review {
        subject: ReviewSubject::Launch(selection),
    } = action;
    run("policy.review.launch", selection, Operation::Review, json)
}

pub fn run_test(action: TestAction, json: bool) -> io::Result<ExitCode> {
    let TestAction::Run { selection, options } = action;
    run("test.run", selection, Operation::Replay(options), json)
}

fn run(
    command: &'static str,
    selection: Selection,
    operation: Operation,
    json: bool,
) -> io::Result<ExitCode> {
    let worker = match std::env::current_exe() {
        Ok(path) => path,
        Err(_) => {
            return write_failure(
                command,
                preview_failure(PreviewError::WorkerUnavailable),
                json,
            );
        }
    };
    let launch = Launch {
        service: ProjectService::for_user(),
        root: selection.root,
        worker,
        access: selection.access,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let cancellation = CancellationToken::new();
        let signal = if matches!(&operation, Operation::Replay(_)) {
            let token = cancellation.clone();
            Some(tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    token.cancel();
                }
            }))
        } else {
            None
        };
        let result = match selection.scenario {
            Some(scenario) => {
                single(command, &launch, scenario, operation, json, &cancellation).await
            }
            None => suite::execute(command, &launch, operation, json, &cancellation).await,
        };
        if let Some(signal) = signal {
            signal.abort();
        }
        result
    })
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

async fn single(
    command: &'static str,
    launch: &Launch,
    scenario: String,
    operation: Operation,
    json: bool,
    cancellation: &CancellationToken,
) -> io::Result<ExitCode> {
    let plan = match launch.prepare(&scenario).await {
        Ok(plan) => plan,
        Err(reply) => return write_failure(command, *reply, json),
    };
    match operation {
        Operation::Review => finish(
            command,
            &Reply::Success {
                data: plan.review(),
                diagnostics: vec![],
            },
            json,
            |review| {
                let mut output = io::stdout().lock();
                write_launch(&mut output, review)?;
                writeln!(output, "Scenario  {scenario}")?;
                writeln!(output, "Approval digest  {}", review.digest)
            },
        ),
        Operation::Replay(options) => {
            let Some(approval) = options.approve else {
                return write_failure(
                    command,
                    preview_failure(PreviewError::ApprovalRequired),
                    json,
                );
            };
            let (data, stop_error) =
                match replay(plan, approval, &options, json, true, cancellation).await {
                    Ok(result) => result,
                    Err(error) => return write_failure(command, preview_failure(error), json),
                };
            if let Some(mut error) = run_failure(&data.run, stop_error) {
                error.details = Some(Box::new(DiagnosticDetails::PlaygroundRun(Box::new(
                    PlaygroundRunDetails {
                        run_id: data.run.run_id,
                        run: data.run,
                        trace: data.trace,
                    },
                ))));
                return write_failure(
                    command,
                    Reply::Failure {
                        error,
                        diagnostics: vec![],
                    },
                    json,
                );
            }
            finish(
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
            )
        }
    }
}

async fn replay(
    plan: PreparedProjectPreview,
    approval: ManifestRevision,
    options: &ReplayOptions,
    json: bool,
    include_trace: bool,
    cancellation: &CancellationToken,
) -> Result<(ReplayResult, Option<PreviewError>), PreviewError> {
    let session = plan
        .approve(approval)?
        .start(
            PreviewOptions {
                protocol: options.protocol.into(),
                capture_payloads: options.trace_payloads && json,
            },
            cancellation,
        )
        .await?;
    let run = session
        .replay(RequestOptions {
            timeout: Duration::from_millis(options.timeout_ms),
            cancellation: cancellation.clone(),
            ..RequestOptions::default()
        })
        .await;
    let trace = if include_trace {
        session.trace()
    } else {
        TraceSnapshot::default()
    };
    let stop_error = session.stop().await.err();
    Ok((
        ReplayResult {
            run: run?.result,
            trace,
        },
        stop_error,
    ))
}

fn run_failure(run: &RunResultV1, stop_error: Option<PreviewError>) -> Option<Diagnostic> {
    if run.outcome != RunOutcome::Cancelled
        && let Some(error) = stop_error
    {
        return Some(preview_diagnostic(error));
    }
    run.diagnostic.as_ref().map(|diagnostic| Diagnostic {
        code: DiagnosticCode::Playground(diagnostic.code),
        message: diagnostic.message.into(),
        severity: Severity::Error,
        correlation_id: diagnostic.correlation_id,
        action: Some(playground_action(diagnostic.code).into()),
        field: None,
        capability_id: None,
        details: None,
    })
}

fn preview_failure(error: PreviewError) -> Reply<()> {
    Reply::Failure {
        error: preview_diagnostic(error),
        diagnostics: vec![],
    }
}

fn preview_diagnostic(error: PreviewError) -> Diagnostic {
    let code = error.code();
    let Reply::Failure { mut error, .. } = failure_reply::<()>(error) else {
        unreachable!("preview failure always contains a diagnostic")
    };
    error.action = Some(playground_action(code).into());
    error
}

fn playground_action(code: &str) -> &'static str {
    match code {
        "policy.approval_required" => {
            "Run the matching launch review and pass its current digest with --approve."
        }
        "policy.denied" => {
            "Review the requested operation and the selected grants before retrying."
        }
        "scenario.assertion_failed" => {
            "Compare the failed fields with the saved fixture and assertions."
        }
        "fixture.missing" => {
            "Add a fixture for the scenario's requested operation and review again."
        }
        "run.cancelled" => "Run a new review before retrying the cancelled operation.",
        _ => "Check the project, selected scenario and launch review before retrying.",
    }
}

fn reply_data<T>(reply: Reply<T>) -> Result<T, Box<Reply<()>>> {
    match reply {
        Reply::Success { data, .. } => Ok(data),
        Reply::Failure { error, diagnostics } => {
            Err(Box::new(Reply::Failure { error, diagnostics }))
        }
    }
}

fn write_failure(command: &'static str, reply: Reply<()>, json: bool) -> io::Result<ExitCode> {
    finish(command, &reply, json, |_| Ok(()))
}

fn write_launch(output: &mut impl Write, review: &PreviewReview<'_>) -> io::Result<()> {
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
    for grant in &review.approved_grants {
        writeln!(output, "Grant  {}", serde_json::to_string(grant)?)?;
    }
    for prompt in review.approved_prompt_ids {
        writeln!(output, "Prompt  {prompt}")?;
    }
    Ok(())
}

pub fn write_run_details(output: &mut impl Write, diagnostic: &Diagnostic) -> io::Result<()> {
    match diagnostic.details.as_deref() {
        Some(DiagnosticDetails::PlaygroundRun(details)) => {
            write_assertion_failures(output, &details.run)
        }
        Some(DiagnosticDetails::PlaygroundSuite { runs, .. }) => suite::write_runs(output, runs),
        _ => Ok(()),
    }
}

fn write_assertion_failures(output: &mut impl Write, run: &RunResultV1) -> io::Result<()> {
    for assertion in run.assertions.iter().filter(|item| !item.passed) {
        let number = assertion.index + 1;
        if let Some(path) = &assertion.path {
            writeln!(output, "Assertion {number} differs at {path}.")?;
        } else {
            writeln!(output, "Assertion {number} failed.")?;
        }
    }
    Ok(())
}
