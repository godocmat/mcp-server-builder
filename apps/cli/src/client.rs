use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use builder_app::{
    ClientConfigExport, ClientConfigRequest, ClientConfigTarget, export_client_config,
};
use clap::{Subcommand, ValueEnum};

#[derive(Subcommand)]
pub(crate) enum Action {
    /// Render local stdio configuration for a built standalone server.
    Export {
        /// MCP client configuration format.
        #[arg(value_enum, value_name = "PROFILE")]
        client_target: Profile,
        /// Server name used by the client.
        #[arg(long)]
        name: String,
        /// Built standalone server executable.
        #[arg(long, value_name = "FILE")]
        executable: PathBuf,
        /// Exported mcp-project.json manifest.
        #[arg(long, value_name = "FILE")]
        manifest: PathBuf,
        /// Reviewed runtime configuration to pass to the server.
        #[arg(long, value_name = "FILE")]
        runtime_config: Option<PathBuf>,
        /// New snippet file. With no output path, print the snippet to stdout.
        #[arg(long, value_name = "NEW_FILE")]
        out: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Profile {
    ClaudeCode,
    Codex,
    #[value(name = "opencode")]
    OpenCode,
    Generic,
}

impl From<Profile> for ClientConfigTarget {
    fn from(profile: Profile) -> Self {
        match profile {
            Profile::ClaudeCode => Self::ClaudeCode,
            Profile::Codex => Self::Codex,
            Profile::OpenCode => Self::OpenCode,
            Profile::Generic => Self::Generic,
        }
    }
}

pub(crate) fn run(action: Action, json: bool) -> io::Result<ExitCode> {
    let Action::Export {
        client_target,
        name,
        executable,
        manifest,
        runtime_config,
        out,
    } = action;
    let reply = export_client_config(
        ClientConfigRequest {
            target: client_target.into(),
            name,
            executable,
            manifest,
            runtime_config,
        },
        out.as_deref(),
    );
    super::finish("client.export", &reply, json, write_export)
}

fn write_export(export: &ClientConfigExport) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    if let Some(destination) = &export.destination {
        writeln!(stdout, "Wrote the client configuration to {destination}.")
    } else {
        stdout.write_all(export.contents.as_bytes())
    }
}
