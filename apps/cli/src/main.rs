use std::io::{self, Write};
use std::process::ExitCode;

use builder_app::{
    Diagnostic, DiagnosticCode, Reply, SMOKE_CONTRACT_VERSION, Severity, SmokeRequest, SmokeStatus,
    workspace_smoke,
};
use clap::{Parser, Subcommand};
use serde::Serialize;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

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
}

#[derive(Serialize)]
struct JsonResult<'a> {
    version: u32,
    command: &'static str,
    #[serde(flatten)]
    reply: &'a Reply<SmokeStatus>,
}

fn write_reply(command: &'static str, reply: &Reply<SmokeStatus>, json: bool) -> io::Result<()> {
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
        Reply::Success { data, .. } => writeln!(
            io::stdout().lock(),
            "Workspace smoke passed (builder {}, contract {}).",
            data.builder_version,
            data.contract_version
        ),
        Reply::Failure { error, .. } => {
            let mut stderr = io::stderr().lock();
            writeln!(stderr, "{}", error.message)?;
            if let Some(action) = &error.action {
                writeln!(stderr, "{action}")?;
            }
            Ok(())
        }
    }
}

fn run() -> io::Result<ExitCode> {
    let args: Vec<_> = std::env::args_os().collect();
    match Cli::try_parse_from(&args) {
        Ok(Cli {
            json,
            command: Command::Smoke { contract_version },
        }) => {
            let reply = workspace_smoke(SmokeRequest { contract_version });
            write_reply("smoke", &reply, json)?;
            Ok(match reply {
                Reply::Success { .. } => ExitCode::SUCCESS,
                Reply::Failure { .. } => ExitCode::from(2),
            })
        }
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
            let reply = Reply::Failure {
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
            write_reply("cli", &reply, json)?;
            Ok(ExitCode::from(2))
        }
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .json()
        .init();

    match run() {
        Ok(code) => code,
        Err(_) => {
            let _ = writeln!(io::stderr().lock(), "Unable to write the command result.");
            ExitCode::FAILURE
        }
    }
}
