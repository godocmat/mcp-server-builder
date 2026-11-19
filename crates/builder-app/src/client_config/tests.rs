use serde_json::Value;
use tempfile::TempDir;

use super::*;

struct Files {
    folder: TempDir,
    executable: PathBuf,
    manifest: PathBuf,
    runtime_config: PathBuf,
}

impl Files {
    fn new() -> Self {
        let folder = tempfile::tempdir().unwrap();
        let export = folder.path().join("standalone's spéced folder");
        fs::create_dir(&export).unwrap();
        let executable = export.join(if cfg!(windows) {
            "server.EXE"
        } else {
            "server"
        });
        let manifest = export.join("mcp-project.json");
        let runtime_config = export.join("private config.json");
        fs::write(&executable, "placeholder").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(&manifest, "manifest content stays private").unwrap();
        fs::write(&runtime_config, "private canary never embedded").unwrap();
        Self {
            folder,
            executable,
            manifest,
            runtime_config,
        }
    }

    fn request(&self, target: ClientConfigTarget) -> ClientConfigRequest {
        ClientConfigRequest {
            target,
            name: "my-example_17".into(),
            executable: self.executable.clone(),
            manifest: self.manifest.clone(),
            runtime_config: Some(self.runtime_config.clone()),
        }
    }
}

fn success(reply: Reply<ClientConfigExport>) -> ClientConfigExport {
    match reply {
        Reply::Success { data, .. } => data,
        Reply::Failure { error, .. } => panic!("unexpected export diagnostic: {error:?}"),
    }
}

fn expect_failure(reply: Reply<ClientConfigExport>, code: DiagnosticCode, field: &str) {
    let Reply::Failure { error, .. } = reply else {
        panic!("the request unexpectedly succeeded");
    };
    assert_eq!(error.code, code);
    assert_eq!(error.field.as_deref(), Some(field));
}

#[test]
fn exported_configs_match_each_supported_client_shape() {
    let files = Files::new();
    let canonical = |path| {
        dunce::canonicalize(path)
            .unwrap()
            .into_os_string()
            .into_string()
            .unwrap()
    };
    let command = canonical(&files.executable);
    let args = vec![
        "--manifest".into(),
        canonical(&files.manifest),
        "--config".into(),
        canonical(&files.runtime_config),
    ];
    assert!(Path::new(&command).is_absolute());

    for (target, label, suffix) in [
        (
            ClientConfigTarget::ClaudeCode,
            "claude-code",
            "claude-code.json",
        ),
        (ClientConfigTarget::Codex, "codex", "codex.toml"),
        (ClientConfigTarget::OpenCode, "opencode", "opencode.json"),
        (ClientConfigTarget::Generic, "generic", "mcp-client.json"),
    ] {
        let export = success(export_client_config(files.request(target), None));
        let envelope = serde_json::to_value(&export).unwrap();
        assert_eq!(envelope["target"], label);
        assert_eq!(
            envelope["suggestedFileName"],
            format!("mcp-my-example_17.{suffix}")
        );
        assert!(envelope.get("destination").is_none());
        assert_eq!(
            serde_json::from_value::<ClientConfigTarget>(json!(label)).unwrap(),
            target
        );
        assert!(export.contents.ends_with('\n'));
        assert!(!export.contents.contains("manifest content stays private"));
        assert!(!export.contents.contains("private canary never embedded"));
        let expected = match target {
            ClientConfigTarget::ClaudeCode | ClientConfigTarget::Generic => json!({
                "mcpServers": { "my-example_17": { "type": "stdio", "command": command, "args": args } }
            }),
            ClientConfigTarget::Codex => {
                assert!(export.contents.starts_with("[mcp_servers.my-example_17]\n"));
                json!({ "mcp_servers": { "my-example_17": { "command": command, "args": args } } })
            }
            ClientConfigTarget::OpenCode => {
                let argv: Vec<&str> = std::iter::once(command.as_str())
                    .chain(args.iter().map(String::as_str))
                    .collect();
                json!({ "mcp": { "servers": { "my-example_17": { "type": "local", "command": argv } } } })
            }
        };
        let decoded: Value = if target == ClientConfigTarget::Codex {
            toml::from_str(&export.contents).unwrap()
        } else {
            serde_json::from_str(&export.contents).unwrap()
        };
        assert_eq!(decoded, expected);
    }
}

#[test]
fn codecs_round_trip_windows_and_unix_paths_without_shell_quoting() {
    let name = "catalog";
    for (command, manifest) in [
        (
            r"C:\Program Files\MCP Sandbox\server.exe",
            r"C:\Users\Catalog Owner\résumé's & notes\mcp-project.json",
        ),
        (
            "/Users/catalog owner/bin/my \"server\"",
            "/Users/catalog owner/résumé's & notes/mcp-project.json",
        ),
        (
            "/home/catalog owner/bin/my server",
            "/home/catalog owner/résumé's & notes/mcp-project.json",
        ),
    ] {
        let args = ["--manifest", manifest];
        let toml = encode(ClientConfigTarget::Codex, name, command, &args).unwrap();
        let decoded: Value = toml::from_str(&toml).unwrap();
        assert_eq!(
            decoded["mcp_servers"][name],
            json!({ "command": command, "args": args })
        );
        let json = encode(ClientConfigTarget::Generic, name, command, &args).unwrap();
        let decoded: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            decoded["mcpServers"][name],
            json!({ "type": "stdio", "command": command, "args": args })
        );
    }
}

#[test]
fn optional_config_is_omitted_from_the_argument_vector() {
    let files = Files::new();
    let mut request = files.request(ClientConfigTarget::Generic);
    request.runtime_config = None;
    let export = success(export_client_config(request, None));
    let decoded: Value = serde_json::from_str(&export.contents).unwrap();
    let args = decoded["mcpServers"]["my-example_17"]["args"]
        .as_array()
        .unwrap();
    assert_eq!(args.len(), 2);
    assert_eq!(args[0], "--manifest");
}

#[test]
fn saves_reviewed_bytes_only_to_a_new_file() {
    let files = Files::new();
    let name = "configuration with spaces.json";
    let destination = files.folder.path().join(name);
    let draft = success(export_client_config(
        files.request(ClientConfigTarget::Generic),
        None,
    ));
    assert!(!destination.exists());

    let written = success(save_client_config(draft.clone(), &destination));
    assert_eq!(
        Path::new(written.destination.as_deref().unwrap()),
        dunce::canonicalize(files.folder.path()).unwrap().join(name),
    );
    assert_eq!(fs::read(&destination).unwrap(), draft.contents.as_bytes());

    let another = files.folder.path().join(".mcp.json");
    fs::write(&another, b"existing client settings").unwrap();
    expect_failure(
        save_client_config(draft, &another),
        DiagnosticCode::ExportDestinationExists,
        "/destination",
    );
    assert_eq!(fs::read(&another).unwrap(), b"existing client settings");
}

#[test]
fn one_of_two_concurrent_exports_claims_the_destination() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let files = Files::new();
    let destination = files.folder.path().join("client.json");
    let one = files.request(ClientConfigTarget::Generic);
    let mut two = files.request(ClientConfigTarget::Generic);
    two.name = "other-server".into();
    let barrier = Arc::new(Barrier::new(2));
    let writes: Vec<_> = [one, two]
        .into_iter()
        .map(|request| {
            let destination = destination.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                export_client_config(request, Some(&destination))
            })
        })
        .collect();
    let mut contents = Vec::new();
    for write in writes {
        match write.join().unwrap() {
            Reply::Success { data, .. } => contents.push(data.contents),
            failure => expect_failure(
                failure,
                DiagnosticCode::ExportDestinationExists,
                "/destination",
            ),
        }
    }
    assert_eq!(contents.len(), 1);
    assert_eq!(fs::read_to_string(destination).unwrap(), contents[0]);
}

#[test]
fn reports_name_and_path_validation_with_actionable_fields() {
    let files = Files::new();
    for name in [
        "",
        "_example",
        "name.with.dots",
        "name with spaces",
        "constructor",
        "prototype",
        "é",
    ] {
        let mut request = files.request(ClientConfigTarget::Generic);
        request.name = name.into();
        expect_failure(
            export_client_config(request, None),
            DiagnosticCode::InvalidInput,
            "/name",
        );
    }
    let mut request = files.request(ClientConfigTarget::Generic);
    request.manifest = files.folder.path().join("missing.json");
    expect_failure(
        export_client_config(request, None),
        DiagnosticCode::InvalidInput,
        "/manifest",
    );
    let mut request = files.request(ClientConfigTarget::Generic);
    request.runtime_config = Some(files.folder.path().into());
    expect_failure(
        export_client_config(request, None),
        DiagnosticCode::InvalidInput,
        "/runtimeConfig",
    );
    let mut request = files.request(ClientConfigTarget::Generic);
    request.executable = files.manifest.clone();
    expect_failure(
        export_client_config(request, None),
        DiagnosticCode::InvalidInput,
        "/executable",
    );
    let destination = files
        .folder
        .path()
        .join("missing-folder")
        .join("config.json");
    let request = files.request(ClientConfigTarget::Generic);
    expect_failure(
        export_client_config(request, Some(&destination)),
        DiagnosticCode::InvalidInput,
        "/destination",
    );
}

#[test]
fn literal_paths_with_client_substitution_syntax_are_never_emitted_for_that_client() {
    let files = Files::new();
    let claude_path = files.folder.path().join("template ${MCP_WORK_ROOT}.json");
    fs::write(&claude_path, "{}").unwrap();
    let mut claude = files.request(ClientConfigTarget::ClaudeCode);
    claude.manifest = claude_path.clone();
    expect_failure(
        export_client_config(claude, None),
        DiagnosticCode::InvalidInput,
        "/manifest",
    );
    let mut inspector = files.request(ClientConfigTarget::Generic);
    inspector.manifest = claude_path;
    success(export_client_config(inspector, None));

    // The colon in an OpenCode token is a legal Unix file-name byte but not a
    // regular Windows file-name character.
    assert!(ClientConfigTarget::OpenCode.substitutes("a path with {env:HOME}"));
    assert!(ClientConfigTarget::OpenCode.substitutes("a path with {file:notes}"));
    #[cfg(unix)]
    {
        let opencode_path = files.folder.path().join("template {file:secret}.json");
        fs::write(&opencode_path, "{}").unwrap();
        let mut opencode = files.request(ClientConfigTarget::OpenCode);
        opencode.runtime_config = Some(opencode_path);
        expect_failure(
            export_client_config(opencode, None),
            DiagnosticCode::InvalidInput,
            "/runtimeConfig",
        );
    }
}

#[cfg(unix)]
#[test]
fn linked_and_non_unicode_files_do_not_become_launch_arguments() -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    let files = Files::new();
    let link = files.folder.path().join("manifest-link");
    symlink(&files.manifest, &link)?;
    let mut request = files.request(ClientConfigTarget::Generic);
    request.manifest = link;
    expect_failure(
        export_client_config(request, None),
        DiagnosticCode::InvalidInput,
        "/manifest",
    );
    // macOS file systems may refuse non-UTF-8 names, so verify this as a real
    // file only on systems where creating it is possible.
    let invalid = files
        .folder
        .path()
        .join(std::ffi::OsStr::from_bytes(b"manifest-\xff"));
    if fs::write(&invalid, b"").is_ok() {
        let mut request = files.request(ClientConfigTarget::Generic);
        request.manifest = invalid;
        expect_failure(
            export_client_config(request, None),
            DiagnosticCode::InvalidInput,
            "/manifest",
        );
    }
    Ok(())
}
