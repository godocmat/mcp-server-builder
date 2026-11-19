use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{Diagnostic, DiagnosticCode, Reply, Severity, local_fs};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientConfigTarget {
    ClaudeCode,
    Codex,
    #[serde(rename = "opencode")]
    OpenCode,
    Generic,
}

impl ClientConfigTarget {
    fn file_suffix(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code.json",
            Self::Codex => "codex.toml",
            Self::OpenCode => "opencode.json",
            Self::Generic => "mcp-client.json",
        }
    }

    fn substitutes(self, path: &str) -> bool {
        match self {
            Self::ClaudeCode => path.contains("${"),
            Self::OpenCode => path.contains("{env:") || path.contains("{file:"),
            Self::Codex | Self::Generic => false,
        }
    }
}

#[derive(Debug)]
pub struct ClientConfigRequest {
    pub target: ClientConfigTarget,
    pub name: String,
    pub executable: PathBuf,
    pub manifest: PathBuf,
    pub runtime_config: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfigExport {
    pub target: ClientConfigTarget,
    pub suggested_file_name: String,
    pub contents: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
}

/// Render a local stdio client configuration and optionally save it to a new file.
/// Rendering inspects the metadata of the selected files and works independently
/// of any active client or server.
pub fn export_client_config(
    request: ClientConfigRequest,
    destination: Option<&Path>,
) -> Reply<ClientConfigExport> {
    reply(render(request).and_then(|export| match destination {
        Some(destination) => save(export, destination),
        None => Ok(export),
    }))
}

/// Save the exact bytes of a previously rendered configuration to a new file.
pub fn save_client_config(
    export: ClientConfigExport,
    destination: &Path,
) -> Reply<ClientConfigExport> {
    reply(save(export, destination))
}

fn render(request: ClientConfigRequest) -> Result<ClientConfigExport, Failure> {
    let ClientConfigRequest {
        target,
        name,
        executable,
        manifest,
        runtime_config,
    } = request;
    let mut name_bytes = name.bytes();
    if name.len() > 64
        || !name_bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !name_bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || matches!(name.as_str(), "constructor" | "prototype")
    {
        return Err(Failure::Name);
    }

    let command = selected_file(&executable, target, Input::Executable)?;
    let manifest = selected_file(&manifest, target, Input::Manifest)?;
    let runtime_config = runtime_config
        .as_deref()
        .map(|path| selected_file(path, target, Input::RuntimeConfig))
        .transpose()?;
    let mut args = vec!["--manifest", &manifest];
    if let Some(path) = &runtime_config {
        args.extend(["--config", path.as_str()]);
    }

    Ok(ClientConfigExport {
        target,
        suggested_file_name: format!("mcp-{name}.{}", target.file_suffix()),
        contents: encode(target, &name, &command, &args)?,
        destination: None,
    })
}

fn encode(
    target: ClientConfigTarget,
    name: &str,
    command: &str,
    args: &[&str],
) -> Result<String, Failure> {
    let mut contents = match target {
        ClientConfigTarget::ClaudeCode | ClientConfigTarget::Generic => {
            serde_json::to_string_pretty(&json!({
                "mcpServers": { (name): { "type": "stdio", "command": command, "args": args } }
            }))
            .map_err(|_| Failure::Encoding)?
        }
        ClientConfigTarget::Codex => {
            #[derive(Serialize)]
            struct Stdio<'a> {
                command: &'a str,
                args: &'a [&'a str],
            }

            #[derive(Serialize)]
            struct Config<'a> {
                mcp_servers: BTreeMap<&'a str, Stdio<'a>>,
            }

            let mcp_servers = BTreeMap::from([(name, Stdio { command, args })]);
            toml::to_string(&Config { mcp_servers }).map_err(|_| Failure::Encoding)?
        }
        ClientConfigTarget::OpenCode => {
            let command: Vec<&str> = std::iter::once(command)
                .chain(args.iter().copied())
                .collect();
            serde_json::to_string_pretty(&json!({
                "mcp": { "servers": { (name): { "type": "local", "command": command } } }
            }))
            .map_err(|_| Failure::Encoding)?
        }
    };
    if !contents.ends_with('\n') {
        contents.push('\n');
    }
    Ok(contents)
}

#[derive(Clone, Copy, Debug)]
enum Input {
    Executable,
    Manifest,
    RuntimeConfig,
}

impl Input {
    fn field(self) -> &'static str {
        match self {
            Self::Executable => "/executable",
            Self::Manifest => "/manifest",
            Self::RuntimeConfig => "/runtimeConfig",
        }
    }
}

fn selected_file(path: &Path, target: ClientConfigTarget, input: Input) -> Result<String, Failure> {
    regular_file(path, input)?;
    let resolved = dunce::canonicalize(path).map_err(|_| Failure::File(input))?;
    let metadata = regular_file(&resolved, input)?;
    if matches!(input, Input::Executable) && !is_executable(&resolved, &metadata) {
        return Err(Failure::Executable);
    }
    let path = resolved.to_str().ok_or(Failure::Unicode(input))?;
    if target.substitutes(path) {
        return Err(Failure::Interpolation(input));
    }
    Ok(path.to_owned())
}

fn regular_file(path: &Path, input: Input) -> Result<fs::Metadata, Failure> {
    let metadata = fs::symlink_metadata(path).map_err(|_| Failure::File(input))?;
    if !metadata.is_file() || local_fs::is_link(&metadata) {
        return Err(Failure::File(input));
    }
    Ok(metadata)
}

#[cfg(windows)]
fn is_executable(path: &Path, _metadata: &fs::Metadata) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

#[cfg(unix)]
fn is_executable(_path: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(any(windows, unix)))]
fn is_executable(_path: &Path, _metadata: &fs::Metadata) -> bool {
    false
}

fn save(mut export: ClientConfigExport, path: &Path) -> Result<ClientConfigExport, Failure> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| builder_project::is_portable_path(name))
        .ok_or(Failure::Destination)?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = dunce::canonicalize(parent).map_err(|_| Failure::Destination)?;
    if !parent.is_dir() {
        return Err(Failure::Destination);
    }
    let destination = parent.join(file_name);
    let destination_text = destination.to_str().ok_or(Failure::Destination)?;
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err(Failure::Exists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(Failure::Storage),
    }

    let mut stage = tempfile::NamedTempFile::new_in(&parent).map_err(|_| Failure::Storage)?;
    stage
        .write_all(export.contents.as_bytes())
        .and_then(|()| stage.as_file().sync_all())
        .map_err(|_| Failure::Storage)?;
    local_fs::publish_file(stage, &destination, false).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists
            || fs::symlink_metadata(&destination).is_ok()
        {
            Failure::Exists
        } else {
            Failure::Storage
        }
    })?;
    export.destination = Some(destination_text.to_owned());
    Ok(export)
}

#[cfg(test)]
mod tests;

fn reply(result: Result<ClientConfigExport, Failure>) -> Reply<ClientConfigExport> {
    match result {
        Ok(data) => Reply::Success {
            data,
            diagnostics: Vec::new(),
        },
        Err(failure) => Reply::Failure {
            error: failure.diagnostic(),
            diagnostics: Vec::new(),
        },
    }
}

#[derive(Debug)]
enum Failure {
    Name,
    File(Input),
    Executable,
    Unicode(Input),
    Interpolation(Input),
    Destination,
    Exists,
    Storage,
    Encoding,
}

impl Failure {
    fn diagnostic(self) -> Diagnostic {
        use DiagnosticCode as Code;
        let (code, field, message, action) = match self {
            Self::Name => (
                Code::InvalidInput,
                Some("/name"),
                "The MCP server name is invalid.",
                "Choose 1 to 64 ASCII letters, digits, hyphens, or underscores starting with a letter or digit. Avoid reserved names such as constructor and prototype.",
            ),
            Self::File(input) => (
                Code::InvalidInput,
                Some(input.field()),
                "The selected path must point to an existing regular file.",
                "Choose the actual file produced or used by your standalone server.",
            ),
            Self::Executable => (
                Code::InvalidInput,
                Some(Input::Executable.field()),
                "The selected file does not meet the executable requirements on this platform.",
                "Choose the compiled standalone server. Windows requires an .exe file and Unix requires executable permissions.",
            ),
            Self::Unicode(input) => (
                Code::InvalidInput,
                Some(input.field()),
                "The selected path cannot be represented in a UTF-8 client configuration.",
                "Move the input to a path with a UTF-8 name, then select it again.",
            ),
            Self::Interpolation(input) => (
                Code::InvalidInput,
                Some(input.field()),
                "The client would interpret part of the selected path as a substitution.",
                "Choose a path without the client's substitution syntax, or select a different client.",
            ),
            Self::Destination => (
                Code::InvalidInput,
                Some("/destination"),
                "The destination must name a new file in an accessible existing folder.",
                "Choose an existing folder and a portable file name.",
            ),
            Self::Exists => (
                Code::ExportDestinationExists,
                Some("/destination"),
                "A file or folder already exists at the destination.",
                "Choose a new file to save the configuration.",
            ),
            Self::Storage => (
                Code::StorageUnavailable,
                Some("/destination"),
                "The client configuration could not be saved.",
                "Check write access and available space, then choose a new file.",
            ),
            Self::Encoding => (
                Code::InternalError,
                None,
                "The client configuration could not be rendered.",
                "Check the installed builder version before retrying.",
            ),
        };
        Diagnostic {
            code,
            field: field.map(str::to_owned),
            message: message.into(),
            action: Some(action.into()),
            severity: Severity::Error,
            correlation_id: Uuid::new_v4(),
            capability_id: None,
            details: None,
        }
    }
}
