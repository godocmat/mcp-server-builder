mod config;
mod handlers;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use builder_project::{MANIFEST_NAME, MAX_MANIFEST_BYTES, parse_manifest};
use mcp_runtime::CancellationToken;
use mcp_transport::stdio::serve_stdio;

use crate::config::{RuntimeConfig, STDIO_OPERATOR, read_file};

const USAGE: &str =
    "standalone.usage\nUsage: mcp-standalone-server [--manifest PATH] [--config PATH]";

struct Arguments {
    manifest: PathBuf,
    config: Option<PathBuf>,
}

fn arguments(args: impl IntoIterator<Item = OsString>) -> Result<Arguments, &'static str> {
    let mut args = args.into_iter();
    let mut manifest = None;
    let mut config = None;
    while let Some(arg) = args.next() {
        let slot = match arg.to_str() {
            Some("--manifest") if manifest.is_none() => &mut manifest,
            Some("--config") if config.is_none() => &mut config,
            _ => return Err(USAGE),
        };
        let path = args.next().filter(|path| !path.is_empty()).ok_or(USAGE)?;
        *slot = Some(PathBuf::from(path));
    }
    Ok(Arguments {
        manifest: manifest.unwrap_or_else(|| PathBuf::from(MANIFEST_NAME)),
        config,
    })
}

fn run() -> Result<(), &'static str> {
    let args = arguments(std::env::args_os().skip(1))?;
    let manifest_bytes = read_file(
        &args.manifest,
        MAX_MANIFEST_BYTES,
        "project.manifest_unavailable",
        "project.manifest_too_large",
    )?;
    let manifest = parse_manifest(&manifest_bytes).map_err(|_| "project.invalid_manifest")?;
    let config = match args.config {
        Some(path) => RuntimeConfig::read(&path)?,
        None => RuntimeConfig::default(),
    };
    let server = handlers::build(&manifest, config.into_inputs(&manifest)?)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "standalone.runtime_unavailable")?;
    let result = runtime.block_on(serve_stdio(
        server,
        STDIO_OPERATOR,
        CancellationToken::new(),
    ));
    runtime.shutdown_timeout(Duration::from_secs(2));
    result.map_err(|_| "transport.stdio_failed")
}

fn main() -> ExitCode {
    // Keep all diagnostic paths on stderr and avoid logging raw panic data.
    std::panic::set_hook(Box::new(|_| eprintln!("standalone.internal_error")));
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("{code}");
            ExitCode::FAILURE
        }
    }
}
