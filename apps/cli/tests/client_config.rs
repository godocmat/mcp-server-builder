use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

const BUILDER: &str = env!("CARGO_BIN_EXE_mcp-builder");

#[test]
fn renders_native_arguments_and_saves_a_new_file_without_replacing_it() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("Catalog owner's export ž");
    fs::create_dir(&folder).unwrap();
    let executable = folder.join(if cfg!(windows) {
        "mcp-standalone-server.exe"
    } else {
        "mcp-standalone-server"
    });
    fs::write(&executable, b"server fixture").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let manifest = folder.join("mcp-project.json");
    fs::write(&manifest, b"manifest fixture").unwrap();
    let runtime_config = folder.join("runtime-config.local.json");
    let catalog_profile = folder.join("catalog-profile");
    const MARKER: &str = "MCP_BUILDER_UNRENDERED_RUNTIME_CONFIG_MARKER";
    fs::write(&runtime_config, MARKER).unwrap();

    let run = |json: bool, destination: Option<&Path>| -> Output {
        let mut command = Command::new(BUILDER);
        if json {
            command.arg("--json");
        }
        command
            .arg("--profile")
            .arg(&catalog_profile)
            .args(["client", "export", "claude-code", "--name", "catalog"])
            .arg("--executable")
            .arg(&executable)
            .arg("--manifest")
            .arg(&manifest)
            .arg("--runtime-config")
            .arg(&runtime_config);
        if let Some(destination) = destination {
            command.arg("--out").arg(destination);
        }
        command.output().unwrap()
    };

    let preview = run(false, None);
    assert!(preview.status.success(), "{preview:?}");
    let rendered = String::from_utf8(preview.stdout).unwrap();
    assert!(!rendered.contains(MARKER));
    let snippet: Value = serde_json::from_str(&rendered).unwrap();
    let server = &snippet["mcpServers"]["catalog"];
    assert_same_file(server["command"].as_str().unwrap(), &executable);
    let args = server["args"].as_array().unwrap();
    assert_eq!(args.len(), 4);
    assert_eq!(args[0], "--manifest");
    assert_same_file(args[1].as_str().unwrap(), &manifest);
    assert_eq!(args[2], "--config");
    assert_same_file(args[3].as_str().unwrap(), &runtime_config);

    let destination = folder.join("catalog-snippet.json");
    let saved = run(true, Some(&destination));
    assert!(saved.status.success(), "{saved:?}");
    let reply: Value = serde_json::from_slice(&saved.stdout).unwrap();
    assert_eq!(reply["version"], 1);
    assert_eq!(reply["command"], "client.export");
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["data"]["target"], "claude-code");
    assert_eq!(
        reply["data"]["contents"].as_str().unwrap().as_bytes(),
        rendered.as_bytes()
    );
    assert_same_file(reply["data"]["destination"].as_str().unwrap(), &destination);
    assert_eq!(fs::read(&destination).unwrap(), rendered.as_bytes());

    fs::write(&destination, b"existing client configuration").unwrap();
    let collision = run(true, Some(&destination));
    assert_eq!(collision.status.code(), Some(6), "{collision:?}");
    let reply: Value = serde_json::from_slice(&collision.stdout).unwrap();
    assert_eq!(reply["error"]["code"], "export.destination_exists");
    assert_eq!(
        fs::read(destination).unwrap(),
        b"existing client configuration"
    );
    assert!(!catalog_profile.exists());
}

fn assert_same_file(rendered: &str, expected: &Path) {
    assert_eq!(
        fs::canonicalize(rendered).unwrap(),
        fs::canonicalize(expected).unwrap(),
    );
}
