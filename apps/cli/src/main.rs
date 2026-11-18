use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use builder_app::{
    Diagnostic, DiagnosticCode, DiagnosticDetails, GenerationRequest, GenerationResult,
    ProjectService, Reply, ResolutionSource, SMOKE_CONTRACT_VERSION, Severity, SmokeRequest,
    SmokeStatus, generate, workspace_smoke,
};
use clap::{Parser, Subcommand};
use serde::Serialize;
use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};
use uuid::Uuid;

mod playground;
mod project;

#[derive(Parser)]
#[command(version, about = "MCP Server Builder")]
struct Cli {
    /// Write a single versioned JSON result.
    #[arg(long, global = true)]
    json: bool,
    /// Use a separate local catalog directory. Defaults to the current user catalog.
    #[arg(long, global = true, value_name = "DIRECTORY")]
    profile: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that the shared workspace service responds.
    Smoke {
        #[arg(long, default_value_t = SMOKE_CONTRACT_VERSION)]
        contract_version: u32,
    },
    /// Manage and validate portable projects.
    Project {
        #[command(subcommand)]
        action: project::Action,
    },
    /// Inspect a policy decision before authorizing an operation.
    Policy {
        #[command(subcommand)]
        action: playground::PolicyAction,
    },
    /// Replay saved scenarios.
    Test {
        #[command(subcommand)]
        action: playground::TestAction,
    },
    /// Generate a standalone Rust server in a new directory.
    Generate {
        /// Folder containing mcp-project.json.
        root: PathBuf,
        /// New output folder with an existing parent directory.
        #[arg(long)]
        out: PathBuf,
        /// Previous export whose project-owned files should be carried forward.
        #[arg(long)]
        from_export: Option<PathBuf>,
        /// Versioned JSON conflict decisions from the current preview.
        #[arg(long, requires = "from_export")]
        resolutions: Option<PathBuf>,
        /// Show the generation plan and conflicts before writing files.
        #[arg(long)]
        preview: bool,
    },
    /// Review and replay a saved scenario in a managed stdio preview.
    Playground {
        #[command(subcommand)]
        action: playground::Action,
    },
}

#[derive(Serialize)]
struct JsonResult<'a, T> {
    version: u32,
    command: &'static str,
    #[serde(flatten)]
    reply: &'a Reply<T>,
}

fn write_reply<T: Serialize>(
    command: &'static str,
    reply: &Reply<T>,
    json: bool,
    write_success: impl FnOnce(&T) -> io::Result<()>,
) -> io::Result<()> {
    if json {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer(
            &mut stdout,
            &JsonResult {
                version: 1,
                command,
                reply,
            },
        )?;
        return writeln!(stdout);
    }

    let mut stderr = io::stderr().lock();
    let diagnostics = match reply {
        Reply::Success { data, diagnostics } => {
            write_success(data)?;
            diagnostics
        }
        Reply::Failure { error, diagnostics } => {
            write_diagnostic(&mut stderr, error)?;
            diagnostics
        }
    };
    for diagnostic in diagnostics {
        write_diagnostic(&mut stderr, diagnostic)?;
    }
    Ok(())
}

fn write_diagnostic(output: &mut impl Write, diagnostic: &Diagnostic) -> io::Result<()> {
    if let Some(field) = diagnostic
        .field
        .as_deref()
        .filter(|field| !field.is_empty())
    {
        writeln!(output, "{field}  {}", diagnostic.message)?;
    } else if let DiagnosticCode::Playground(code) = diagnostic.code {
        writeln!(output, "{} ({code})", diagnostic.message)?;
    } else {
        writeln!(output, "{}", diagnostic.message)?;
    }
    match diagnostic.details.as_deref() {
        Some(DiagnosticDetails::CurrentRevision {
            current_revision: Some(revision),
        }) => {
            writeln!(output, "Current revision  {revision}")?;
        }
        Some(DiagnosticDetails::Recovery { candidates, .. }) => {
            for candidate in candidates {
                writeln!(
                    output,
                    "Recovery candidate  {}  {}",
                    candidate.revision, candidate.path
                )?;
            }
        }
        _ => {}
    }
    playground::write_run_details(output, diagnostic)?;
    if let Some(action) = &diagnostic.action {
        writeln!(output, "{action}")?;
    }
    Ok(())
}

fn failure<T>(code: DiagnosticCode, message: &str, action: &str) -> Reply<T> {
    Reply::Failure {
        error: Diagnostic {
            code,
            message: message.into(),
            severity: Severity::Error,
            correlation_id: Uuid::new_v4(),
            action: Some(action.into()),
            field: None,
            capability_id: None,
            details: None,
        },
        diagnostics: vec![],
    }
}

fn finish<T: Serialize>(
    command: &'static str,
    reply: &Reply<T>,
    json: bool,
    write_success: impl FnOnce(&T) -> io::Result<()>,
) -> io::Result<ExitCode> {
    write_reply(command, reply, json, write_success)?;
    Ok(match reply {
        Reply::Success { .. } => ExitCode::SUCCESS,
        Reply::Failure { error, .. } => exit_code(error.code),
    })
}

fn project_service(profile: Option<PathBuf>) -> ProjectService {
    match profile {
        Some(profile) => {
            // Resolve a relative shell argument before handing it to native storage checks.
            ProjectService::with_profile(std::path::absolute(&profile).unwrap_or(profile))
        }
        None => ProjectService::for_user(),
    }
}

fn write_smoke(data: &SmokeStatus) -> io::Result<()> {
    writeln!(
        io::stdout().lock(),
        "Workspace smoke passed (builder {}, contract {}).",
        data.builder_version,
        data.contract_version
    )
}

fn write_generation(data: &GenerationResult) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    if let Some(generation) = &data.generation {
        if data.published {
            writeln!(
                stdout,
                "Published {} files and generation.json.",
                generation.files.len()
            )?;
        } else {
            writeln!(
                stdout,
                "The export would publish {} files and generation.json.",
                generation.files.len()
            )?;
        }
    } else {
        writeln!(
            stdout,
            "Review {} changed files before publishing.",
            data.conflicts.len()
        )?;
        for conflict in &data.conflicts {
            writeln!(stdout, "{}", conflict.path)?;
        }
        writeln!(stdout, "Use --json to see the digests for each conflict.")?;
    }
    Ok(())
}

fn exit_code(code: DiagnosticCode) -> ExitCode {
    use DiagnosticCode as Code;
    ExitCode::from(match code {
        Code::CliUsage
        | Code::InvalidInput
        | Code::ProjectInvalid
        | Code::ProjectUnsupportedSchema
        | Code::ProjectJson
        | Code::ProjectDuplicateKey
        | Code::ProjectInvalidField
        | Code::ProjectUnsupportedSetting
        | Code::ProjectDuplicateId
        | Code::ProjectInvalidReference
        | Code::ProjectInvalidSchema
        | Code::ProjectAmbiguousResource
        | Code::ProjectLimit
        | Code::ProjectUnsafePath
        | Code::ExportInvalidPrevious
        | Code::ExportInvalidResolution
        | Code::ExportUnsafePath
        | Code::ExportLimit => 2,
        Code::ProjectMissing
        | Code::ProjectExists
        | Code::ProjectLocked
        | Code::ProjectRevisionConflict
        | Code::ProjectRecoveryUnavailable
        | Code::ExportConflict
        | Code::ExportInputChanged
        | Code::ExportDestinationExists
        | Code::StorageUnavailable => 6,
        Code::Playground(code) => match code {
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
            | "scenario.live_mutation_requires_confirmation"
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
            "internal.error" => 1,
            "run.cancelled" => 130,
            _ => 5,
        },
        Code::ProjectRecoveryAvailable
        | Code::ProjectIndexRepairRequired
        | Code::StorageDurabilityUnconfirmed
        | Code::InternalError => 1,
    })
}

fn run(args: &[std::ffi::OsString]) -> io::Result<ExitCode> {
    let Cli {
        json,
        profile,
        command,
    } = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) if !error.use_stderr() => {
            error.print()?;
            return Ok(ExitCode::SUCCESS);
        }
        Err(_) => {
            let json = args
                .iter()
                .skip(1)
                .take_while(|arg| *arg != "--")
                .any(|arg| arg == "--json");
            let reply = failure::<()>(
                DiagnosticCode::CliUsage,
                "Invalid command or options.",
                "Run mcp-builder --help for usage.",
            );
            return finish("cli", &reply, json, |_| Ok(()));
        }
    };

    match command {
        Command::Smoke { contract_version } => {
            let reply = workspace_smoke(SmokeRequest { contract_version });
            finish("smoke", &reply, json, write_smoke)
        }
        Command::Project { action } => project::run(action, profile, json),
        Command::Policy { action } => playground::run_policy(action, json),
        Command::Test { action } => playground::run_test(action, json),
        Command::Generate {
            root,
            out,
            from_export,
            resolutions,
            preview,
        } => {
            let reply = generate(GenerationRequest {
                project_root: root,
                destination: out,
                previous_export: from_export,
                resolutions: resolutions.map(ResolutionSource::File),
                preview,
            });
            finish("generate", &reply, json, write_generation)
        }
        Command::Playground { action } => playground::run_legacy(action, json),
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    if let [_, worker] = args.as_slice()
        && worker == builder_app::playground::WORKER_ARG
    {
        return playground::worker();
    }
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(io::stderr)
                .json()
                .with_filter(filter_fn(builder_app::playground::sdk_diagnostic_filter)),
        )
        .init();

    match run(&args) {
        Ok(code) => code,
        Err(_) => {
            let _ = writeln!(io::stderr().lock(), "Unable to write the command result.");
            ExitCode::FAILURE
        }
    }
}
