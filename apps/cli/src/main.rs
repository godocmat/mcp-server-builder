use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use builder_app::{
    Diagnostic, DiagnosticCode, GenerationRequest, GenerationResult, Reply, ResolutionSource,
    SMOKE_CONTRACT_VERSION, Severity, SmokeRequest, SmokeStatus, generate, workspace_smoke,
};
use clap::{Parser, Subcommand};
use serde::Serialize;
use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};
use uuid::Uuid;

mod playground;

#[derive(Parser)]
#[command(version, about = "MCP Server Builder")]
struct Cli {
    /// Write a single versioned JSON result.
    #[arg(long, global = true)]
    json: bool,
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

    match reply {
        Reply::Success { data, .. } => write_success(data),
        Reply::Failure { error, diagnostics } => {
            let mut stderr = io::stderr().lock();
            writeln!(stderr, "{}", error.message)?;
            for detail in diagnostics {
                if let Some(field) = &detail.field {
                    writeln!(stderr, "{field}  {}", detail.message)?;
                }
            }
            if let Some(action) = &error.action {
                writeln!(stderr, "{action}")?;
            }
            Ok(())
        }
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

fn generation_exit(reply: &Reply<GenerationResult>) -> ExitCode {
    use DiagnosticCode as Code;
    match reply {
        Reply::Success { .. } => ExitCode::SUCCESS,
        Reply::Failure { error, .. } => ExitCode::from(match error.code {
            Code::ProjectInvalid
            | Code::ProjectUnsupportedSchema
            | Code::ExportInvalidPrevious
            | Code::ExportInvalidResolution
            | Code::ExportUnsafePath
            | Code::ExportLimit => 2,
            Code::ExportConflict
            | Code::ExportInputChanged
            | Code::ExportDestinationExists
            | Code::ProjectMissing
            | Code::StorageUnavailable => 6,
            _ => 1,
        }),
    }
}

fn run(args: &[std::ffi::OsString]) -> io::Result<ExitCode> {
    match Cli::try_parse_from(args) {
        Ok(Cli {
            json,
            command: Command::Smoke { contract_version },
        }) => {
            let reply = workspace_smoke(SmokeRequest { contract_version });
            write_reply("smoke", &reply, json, write_smoke)?;
            Ok(match reply {
                Reply::Success { .. } => ExitCode::SUCCESS,
                Reply::Failure { .. } => ExitCode::from(2),
            })
        }
        Ok(Cli {
            json,
            command:
                Command::Generate {
                    root,
                    out,
                    from_export,
                    resolutions,
                    preview,
                },
        }) => {
            let reply = generate(GenerationRequest {
                project_root: root,
                destination: out,
                previous_export: from_export,
                resolutions: resolutions.map(ResolutionSource::File),
                preview,
            });
            write_reply("generate", &reply, json, write_generation)?;
            Ok(generation_exit(&reply))
        }
        Ok(Cli {
            json,
            command: Command::Playground { action },
        }) => playground::run(action, json),
        Err(error) if !error.use_stderr() => {
            error.print()?;
            Ok(ExitCode::SUCCESS)
        }
        Err(_) => {
            let json = args
                .iter()
                .skip(1)
                .take_while(|arg| *arg != "--")
                .any(|arg| arg == "--json");
            let reply = Reply::<()>::Failure {
                error: Diagnostic {
                    code: DiagnosticCode::CliUsage,
                    message: "Invalid command or options.".into(),
                    severity: Severity::Error,
                    correlation_id: Uuid::new_v4(),
                    action: Some("Run mcp-builder --help for usage.".into()),
                    field: None,
                    capability_id: None,
                    details: None,
                },
                diagnostics: vec![],
            };
            write_reply("cli", &reply, json, |_| Ok(()))?;
            Ok(ExitCode::from(2))
        }
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
