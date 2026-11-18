use std::io::{self, Write};
use std::process::ExitCode;

use builder_app::{
    Diagnostic, DiagnosticCode, DiagnosticDetails, ManifestRevision, ProjectLocation, Reply,
    ScenarioMode,
};
use serde::Serialize;
use uuid::Uuid;

use super::{
    CancellationToken, Launch, Operation, PreparedProjectPreview, PreviewError, PreviewReview,
    ReplayOptions, RunOutcome, RunResultV1, failure, finish, preview_diagnostic, preview_failure,
    replay, reply_data, run_failure, write_assertion_failures, write_failure, write_launch,
};

struct Plan {
    first: PreparedProjectPreview,
    scenarios: Vec<ScenarioApproval>,
    digest: ManifestRevision,
}

#[derive(Serialize)]
struct ScenarioApproval {
    id: String,
    digest: ManifestRevision,
}

#[derive(Serialize)]
struct Review<'a> {
    digest: ManifestRevision,
    launch: PreviewReview<'a>,
    scenarios: &'a [ScenarioApproval],
}

#[derive(Serialize)]
struct SuiteResult {
    runs: Vec<RunResultV1>,
}

pub(super) async fn execute(
    command: &'static str,
    launch: &Launch,
    operation: Operation,
    json: bool,
    cancellation: &CancellationToken,
) -> io::Result<ExitCode> {
    let plan = match prepare(launch, cancellation).await {
        Ok(plan) => plan,
        Err(reply) => return write_failure(command, *reply, json),
    };
    match operation {
        Operation::Review => finish(
            command,
            &Reply::Success {
                data: Review {
                    digest: plan.digest,
                    launch: plan.first.review(),
                    scenarios: &plan.scenarios,
                },
                diagnostics: vec![],
            },
            json,
            |review| {
                let mut output = io::stdout().lock();
                write_launch(&mut output, &review.launch)?;
                for scenario in review.scenarios {
                    writeln!(output, "Scenario  {}", scenario.id)?;
                }
                writeln!(output, "Suite approval digest  {}", review.digest)
            },
        ),
        Operation::Replay(options) => {
            if options.approve != Some(plan.digest) {
                return write_failure(
                    command,
                    preview_failure(PreviewError::ApprovalRequired),
                    json,
                );
            }
            run(command, launch, plan, &options, json, cancellation).await
        }
    }
}

async fn prepare(
    launch: &Launch,
    cancellation: &CancellationToken,
) -> Result<Plan, Box<Reply<()>>> {
    let snapshot = reply_data(
        launch
            .service
            .inspect(ProjectLocation::Root(launch.root.clone()))
            .await,
    )?;
    let mut ids = snapshot
        .document
        .scenarios
        .into_iter()
        .filter(|scenario| scenario.mode == ScenarioMode::Fixture)
        .map(|scenario| scenario.id);
    let first_id = ids.next().ok_or_else(|| {
        Box::new(failure(
            DiagnosticCode::Playground("scenario.required"),
            "The project has no saved fixture scenarios.",
            "Add a fixture scenario and review the launch again.",
        ))
    })?;
    let first = launch.prepare(&first_id).await?;
    let mut scenarios = vec![checked_approval(
        first_id,
        &first.review(),
        snapshot.registration_id,
        snapshot.revision,
    )?];
    for id in ids {
        if cancellation.is_cancelled() {
            return Err(Box::new(preview_failure(PreviewError::Cancelled)));
        }
        let plan = launch.prepare(&id).await?;
        scenarios.push(checked_approval(
            id,
            &plan.review(),
            snapshot.registration_id,
            snapshot.revision,
        )?);
    }
    if cancellation.is_cancelled() {
        return Err(Box::new(preview_failure(PreviewError::Cancelled)));
    }
    // The individual digests already bind each scenario ID and the resolved native
    // launch. Hash their fixed-width encodings in the same order with a suite prefix.
    let mut contents = String::from("mcp-builder.cli.fixture-suite.v1\n");
    for scenario in &scenarios {
        contents.push_str(&scenario.digest.to_string());
    }
    Ok(Plan {
        first,
        scenarios,
        digest: ManifestRevision::of(contents.as_bytes()),
    })
}

fn checked_approval(
    id: String,
    review: &PreviewReview<'_>,
    registration: Uuid,
    revision: ManifestRevision,
) -> Result<ScenarioApproval, Box<Reply<()>>> {
    if review.manifest_revision != revision || review.registration_id != registration {
        return Err(Box::new(preview_failure(PreviewError::ManifestChanged)));
    }
    Ok(ScenarioApproval {
        id,
        digest: review.digest,
    })
}

struct SuiteFailure {
    error: Diagnostic,
    run_id: Option<Uuid>,
    diagnostics: Vec<Diagnostic>,
}

fn record_failure(first: &mut Option<SuiteFailure>, failure: SuiteFailure) {
    if first.is_none() || failure.error.code == DiagnosticCode::Playground("run.cancelled") {
        *first = Some(failure);
    }
}

async fn run(
    command: &'static str,
    launch: &Launch,
    plan: Plan,
    options: &ReplayOptions,
    json: bool,
    cancellation: &CancellationToken,
) -> io::Result<ExitCode> {
    let mut first_plan = Some(plan.first);
    let mut runs = Vec::with_capacity(plan.scenarios.len());
    let mut failure = None;
    for scenario in plan.scenarios {
        if cancellation.is_cancelled() {
            break;
        }
        // Recreate each remaining review immediately before use. A new revision or
        // changed launch identity fails this exact digest check before process creation.
        let prepared = match first_plan.take() {
            Some(plan) => Ok(plan),
            None => launch.prepare(&scenario.id).await,
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(reply) => {
                let Reply::Failure { error, diagnostics } = *reply else {
                    unreachable!("prepare errors contain a diagnostic");
                };
                record_failure(
                    &mut failure,
                    SuiteFailure {
                        error,
                        run_id: None,
                        diagnostics,
                    },
                );
                break;
            }
        };
        let (data, stop_error) = match replay(
            prepared,
            scenario.digest,
            options,
            json,
            false,
            cancellation,
        )
        .await
        {
            Ok(data) => data,
            Err(error) => {
                record_failure(
                    &mut failure,
                    SuiteFailure {
                        error: preview_diagnostic(error),
                        run_id: None,
                        diagnostics: vec![],
                    },
                );
                break;
            }
        };
        let stopped = stop_error.is_none();
        if let Some(error) = run_failure(&data.run, stop_error) {
            record_failure(
                &mut failure,
                SuiteFailure {
                    error,
                    run_id: Some(data.run.run_id),
                    diagnostics: vec![],
                },
            );
        }
        runs.push(data.run);
        if !stopped {
            break;
        }
    }

    if cancellation.is_cancelled()
        && !failure.as_ref().is_some_and(|failure| {
            failure.error.code == DiagnosticCode::Playground("run.cancelled")
        })
    {
        record_failure(
            &mut failure,
            SuiteFailure {
                error: preview_diagnostic(PreviewError::Cancelled),
                run_id: None,
                diagnostics: vec![],
            },
        );
    }
    if let Some(SuiteFailure {
        mut error,
        run_id,
        diagnostics,
    }) = failure
    {
        error.details = Some(Box::new(DiagnosticDetails::PlaygroundSuite {
            run_id,
            runs,
        }));
        return write_failure(command, Reply::Failure { error, diagnostics }, json);
    }
    finish(
        command,
        &Reply::Success {
            data: SuiteResult { runs },
            diagnostics: vec![],
        },
        json,
        |data| {
            let mut output = io::stdout().lock();
            write_runs(&mut output, &data.runs)?;
            writeln!(output, "{} fixture scenarios passed.", data.runs.len())
        },
    )
}

pub(super) fn write_runs(output: &mut impl Write, runs: &[RunResultV1]) -> io::Result<()> {
    for run in runs {
        let outcome = match run.outcome {
            RunOutcome::Passed => "passed",
            RunOutcome::AssertionFailed => "assertion failed",
            RunOutcome::Invalid => "invalid",
            RunOutcome::Denied => "denied",
            RunOutcome::Failed => "failed",
            RunOutcome::TimedOut => "timed out",
            RunOutcome::Cancelled => "cancelled",
        };
        writeln!(
            output,
            "{}  {outcome}  {} ms",
            run.scenario_id.as_deref().unwrap_or("Scenario"),
            run.duration_ms
        )?;
        write_assertion_failures(output, run)?;
    }
    Ok(())
}
