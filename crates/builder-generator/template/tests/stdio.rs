use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use builder_project::ManifestRevision;
use serde_json::{Value, json};

const MODERN: &str = "2026-07-28";
const LEGACY: &str = "2025-11-25";
const VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
const DIAGNOSTIC: &str = "mcp-server-builder/diagnostic";
const CATALOG: &str = "wire-catalog";
const FILE_RESOURCES: bool = cfg!(any(windows, target_os = "linux"));
const WAIT: Duration = Duration::from_secs(8);
const POLL: Duration = Duration::from_millis(10);
const FRAME_LIMIT: usize = 1024 * 1024;

#[test]
fn default_manifest_loads_without_approvals() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("mcp-project.json")).unwrap()).unwrap();
    let mut server = Wire::start(command().current_dir(root));

    // A modern request can open the connection before discovery.
    let tools = server.result(Era::Modern, "tools/list", json!({}));
    assert_eq!(tools["tools"], json!([]));
    let discovery = server.result(Era::Modern, "server/discover", json!({}));
    assert_eq!(
        discovery["_meta"][SERVER_INFO]["name"],
        manifest["server"]["name"]
    );
    assert_eq!(
        discovery["_meta"][SERVER_INFO]["version"],
        manifest["server"]["version"]
    );
    assert_eq!(discovery["capabilities"], json!({}));
    assert!(server.finish().success());
}

#[test]
fn modern_stdio_serves_the_approved_catalog() {
    exercise_catalog(Era::Modern);
}

#[test]
fn legacy_stdio_serves_the_approved_catalog() {
    exercise_catalog(Era::Legacy);
}

#[test]
fn changed_manifest_rejects_the_reviewed_config() {
    let directory = fixture_directory();
    let listener = loopback_listener();
    let (manifest, config) = write_catalog(directory.path(), &origin(&listener));
    let mut bytes = fs::read(&manifest).unwrap();
    bytes.push(b'\n');
    fs::write(&manifest, bytes).unwrap();

    let diagnostics = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
    let server = Wire::start(
        command()
            .arg("--manifest")
            .arg(manifest)
            .arg("--config")
            .arg(config)
            .stderr(diagnostics.reopen().unwrap()),
    );
    assert!(!server.finish().success());
    let stderr = fs::read_to_string(diagnostics.path()).unwrap();
    assert!(
        stderr.contains("runtime_config.manifest_revision_mismatch"),
        "{stderr}"
    );
    assert_no_connection(&listener);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Era {
    Modern,
    Legacy,
}

impl Era {
    fn params(self, mut params: Value) -> Value {
        if self == Self::Modern {
            params["_meta"] = json!({
                VERSION: MODERN,
                "io.modelcontextprotocol/clientCapabilities": {}
            });
        }
        params
    }
}

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mcp-standalone-server"))
}

fn exercise_catalog(era: Era) {
    let directory = fixture_directory();
    let listener = loopback_listener();
    let (manifest, config) = write_catalog(directory.path(), &origin(&listener));
    let mut server = Wire::start(
        command()
            .arg("--manifest")
            .arg(manifest)
            .arg("--config")
            .arg(config),
    );

    let mut capabilities = json!({"tools": {}, "prompts": {}});
    if FILE_RESOURCES {
        capabilities["resources"] = json!({});
    }
    if era == Era::Modern {
        let discovery = server.result(era, "server/discover", json!({}));
        assert_eq!(discovery["supportedVersions"], json!([MODERN, LEGACY]));
        assert_eq!(discovery["_meta"][SERVER_INFO]["name"], CATALOG);
        assert_eq!(discovery["capabilities"], capabilities);
        assert_cache(&discovery, era);
    } else {
        let initialized = server.result(
            era,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": {},
                "clientInfo": {"name": "raw-stdio-test", "version": "1"}
            }),
        );
        assert_eq!(initialized["protocolVersion"], LEGACY);
        assert_eq!(initialized["serverInfo"]["name"], CATALOG);
        assert_eq!(initialized["capabilities"], capabilities);
        server.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        assert_eq!(server.result(era, "ping", json!({})), json!({}));
    }

    let tools = server.result(era, "tools/list", json!({}));
    assert_cache(&tools, era);
    assert_eq!(tools["tools"].as_array().unwrap().len(), 1);
    let tool = &tools["tools"][0];
    assert_eq!(tool["name"], "lookup_item");
    assert_eq!(tool["inputSchema"]["properties"]["id"]["type"], "string");
    assert_eq!(tool["outputSchema"]["type"], "object");

    if era == Era::Modern {
        let missing = server.request("tools/list", json!({}));
        assert_eq!(missing["error"]["code"], -32602);
        let mut params = era.params(json!({}));
        params["_meta"][VERSION] = json!("2099-01-01");
        let unsupported = server.request("tools/list", params);
        assert_eq!(unsupported["error"]["code"], -32022);
        assert_eq!(
            unsupported["error"]["data"]["supported"],
            json!([MODERN, LEGACY])
        );
    }

    for (id, code) in [(json!(42), "input.invalid"), (json!("43"), "policy.denied")] {
        let result = server.result(era, "tools/call", lookup(id));
        assert_eq!(result["isError"], true);
        assert_eq!(result["_meta"][DIAGNOSTIC]["code"], code);
        assert_no_connection(&listener);
    }

    thread::scope(|scope| {
        let upstream = scope.spawn(|| reply_to_lookup(&listener));
        let item = server.result(era, "tools/call", lookup(json!("42")));
        let expected: Value = serde_json::from_str(ITEM).unwrap();
        assert_eq!(item["structuredContent"], expected);
        assert_eq!(item["content"][0]["type"], "text");
        let text = item["content"][0]["text"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), expected);
        upstream.join().unwrap();
    });

    if FILE_RESOURCES {
        check_resources(&mut server, era);
    }
    let prompts = server.result(era, "prompts/list", json!({}));
    assert_eq!(prompts["prompts"][0]["name"], "review_item");
    assert_cache(&prompts, era);
    let prompt = server.result(
        era,
        "prompts/get",
        json!({"name": "review_item", "arguments": {"id": "42"}}),
    );
    assert_eq!(
        prompt["messages"],
        json!([{"role": "user", "content": {"type": "text", "text": "Review item 42."}}])
    );

    assert!(server.finish().success());
    assert_no_connection(&listener);
}

fn check_resources(server: &mut Wire, era: Era) {
    let resources = server.result(era, "resources/list", json!({}));
    assert_eq!(resources["resources"][0]["uri"], "catalog://docs/guide");
    assert_cache(&resources, era);
    let templates = server.result(era, "resources/templates/list", json!({}));
    assert_eq!(
        templates["resourceTemplates"][0]["uriTemplate"],
        "catalog://docs/items/{id}"
    );
    assert_cache(&templates, era);
    for (uri, text) in [
        ("catalog://docs/guide", "Catalog guide"),
        ("catalog://docs/items/42", "Café note"),
    ] {
        let resource = server.result(era, "resources/read", json!({"uri": uri}));
        assert_eq!(
            resource["contents"],
            json!([{"uri": uri, "mimeType": "text/plain", "text": text}])
        );
        assert_cache(&resource, era);
    }
    let unknown = server.request(
        "resources/read",
        era.params(json!({"uri": "catalog://docs/missing"})),
    );
    let code = if era == Era::Modern { -32602 } else { -32002 };
    assert_eq!(unknown["error"]["code"], code);
    assert_eq!(
        unknown["error"]["data"][DIAGNOSTIC]["code"],
        "mcp.unknown_resource"
    );
}

fn assert_cache(result: &Value, era: Era) {
    if era == Era::Modern {
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["ttlMs"], 0);
        assert_eq!(result["cacheScope"], "private");
    } else {
        for key in ["resultType", "ttlMs", "cacheScope"] {
            assert!(result.get(key).is_none(), "unexpected legacy {key}");
        }
    }
}

fn lookup(id: Value) -> Value {
    json!({"name": "lookup_item", "arguments": {"id": id}})
}

struct Wire {
    child: Child,
    frames: Receiver<io::Result<Vec<u8>>>,
    next_id: u64,
}

impl Wire {
    fn start(command: &mut Command) -> Self {
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start the emitted server");
        let (sender, frames) = sync_channel(1);
        let mut server = Self {
            child,
            frames,
            next_id: 1,
        };
        let mut stdout = BufReader::new(server.child.stdout.take().unwrap());
        thread::spawn(move || {
            loop {
                let mut frame = Vec::new();
                let read = (&mut stdout)
                    .take((FRAME_LIMIT + 1) as u64)
                    .read_until(b'\n', &mut frame);
                let message = match read {
                    Ok(0) => return,
                    Ok(_) if frame.len() <= FRAME_LIMIT && frame.ends_with(b"\n") => Ok(frame),
                    Ok(_) => Err(io::Error::other("invalid stdout framing")),
                    Err(error) => Err(error),
                };
                let failed = message.is_err();
                if sender.send(message).is_err() || failed {
                    return;
                }
            }
        });
        server
    }

    fn send(&mut self, message: Value) {
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(b'\n');
        let stdin = self.child.stdin.as_mut().unwrap();
        stdin.write_all(&bytes).unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let frame = self
            .frames
            .recv_timeout(WAIT)
            .unwrap_or_else(|error| panic!("{method} did not return a stdout frame: {error}"))
            .unwrap();
        let reply: Value = serde_json::from_slice(&frame).expect("stdout frame must be JSON");
        assert_eq!(reply["jsonrpc"], "2.0", "{method}");
        assert_eq!(reply["id"], id, "{method}: {reply}");
        assert_ne!(
            reply.get("error").is_some(),
            reply.get("result").is_some(),
            "{method}: {reply}"
        );
        reply
    }

    fn result(&mut self, era: Era, method: &str, params: Value) -> Value {
        let mut reply = self.request(method, era.params(params));
        assert!(reply.get("result").is_some(), "{method}: {reply}");
        reply["result"].take()
    }

    fn finish(mut self) -> ExitStatus {
        drop(self.child.stdin.take());
        let status = wait_for_exit(&mut self.child).expect("server must exit after stdin EOF");
        assert!(
            matches!(
                self.frames.recv_timeout(WAIT),
                Err(RecvTimeoutError::Disconnected)
            ),
            "expected stdout EOF after server exit"
        );
        status
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = wait_for_exit(&mut self.child);
        }
    }
}

fn wait_for_exit(child: &mut Child) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        thread::sleep(POLL);
    }
}

fn loopback_listener() -> TcpListener {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    listener
}

fn origin(listener: &TcpListener) -> String {
    format!("http://{}", listener.local_addr().unwrap())
}

fn assert_no_connection(listener: &TcpListener) {
    assert_eq!(
        listener
            .accept()
            .expect_err("unexpected upstream connection")
            .kind(),
        io::ErrorKind::WouldBlock
    );
}

const ITEM: &str = r#"{"id":"42","name":"Sample café"}"#;

fn reply_to_lookup(listener: &TcpListener) {
    let deadline = Instant::now() + WAIT;
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(POLL);
            }
            Err(error) => panic!("missing loopback lookup: {error}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.set_write_timeout(Some(WAIT)).unwrap();
    let mut request = BufReader::new(&mut stream).take(8192);
    let mut line = String::new();
    request.read_line(&mut line).unwrap();
    assert_eq!(line, "GET /items/42 HTTP/1.1\r\n");
    loop {
        line.clear();
        assert_ne!(
            request.read_line(&mut line).unwrap(),
            0,
            "truncated HTTP headers"
        );
        if line == "\r\n" {
            break;
        }
    }
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ITEM}",
        ITEM.len()
    )
    .unwrap();
}

fn fixture_directory() -> tempfile::TempDir {
    // The Linux file adapter requires ext4 or tmpfs. Prefer tmpfs when /tmp is on an overlay.
    #[cfg(target_os = "linux")]
    if let Ok(directory) = tempfile::tempdir_in("/dev/shm") {
        return directory;
    }
    tempfile::tempdir().unwrap()
}

fn write_catalog(directory: &Path, origin: &str) -> (PathBuf, PathBuf) {
    let mut manifest = json!({
        "format": "mcp-server-builder.project",
        "schemaVersion": 1,
        "projectId": "8d1ea3a7-18c4-4e36-8a8c-6e84af79eed0",
        "name": "Standalone wire test catalog",
        "server": {"name": CATALOG, "version": "1.0.0"},
        "transport": "stdio",
        "capabilities": [
            {
                "id": "lookup_item", "kind": "tool", "name": "lookup_item",
                "description": "Find a test item", "bindingId": "catalog_lookup",
                "inputSchema": {
                    "type": "object", "properties": {"id": {"type": "string"}},
                    "required": ["id"], "additionalProperties": false
                },
                "outputSchema": {"type": "object"}
            },
            {
                "id": "review_item", "kind": "prompt", "name": "review_item",
                "description": "Review a test item",
                "arguments": [{"name": "id", "required": true}],
                "messages": [{"role": "user", "textTemplate": "Review item {id}."}]
            }
        ],
        "bindings": [{
            "id": "catalog_lookup", "kind": "rest", "origin": origin,
            "method": "GET", "pathTemplate": "/items/{id}", "pathParams": {"id": "/id"}
        }],
        "policy": {"grants": [{
            "id": "catalog_read", "capabilityId": "lookup_item", "operation": "http.read",
            "target": {
                "kind": "rest", "origin": origin, "method": "GET",
                "pathTemplate": "/items/42", "allowedPeerRanges": ["127.0.0.1/32"]
            }
        }]},
        "secretRefs": [], "scenarios": []
    });
    let mut config = json!({
        "configVersion": 1,
        "kind": "stdio",
        "approvedGrantIds": ["catalog_read"],
        "approvedPromptIds": ["review_item"],
        "targetBindings": {},
        "secretSources": {},
        "limits": {"timeoutMs": 5000, "maxOutputBytes": 16384}
    });
    if FILE_RESOURCES {
        let root = directory.join("docs");
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(root.join("guide.txt"), "Catalog guide").unwrap();
        fs::write(root.join("notes/42.txt"), "Café note").unwrap();

        manifest["capabilities"].as_array_mut().unwrap().extend([
            json!({
                "id": "catalog_guide", "kind": "resource", "name": "Catalog guide",
                "description": "Read the guide", "uri": "catalog://docs/guide",
                "mimeType": "text/plain", "bindingId": "guide_file"
            }),
            json!({
                "id": "catalog_note", "kind": "resourceTemplate", "name": "Catalog note",
                "description": "Read an item note", "uriTemplate": "catalog://docs/items/{id}",
                "variables": {"id": {"type": "string", "pattern": "^[0-9]+$"}},
                "mimeType": "text/plain", "bindingId": "note_file"
            }),
        ]);
        for (id, capability, path, approved_path) in [
            ("guide_file", "catalog_guide", "guide.txt", "guide.txt"),
            (
                "note_file",
                "catalog_note",
                "notes/{id}.txt",
                "notes/42.txt",
            ),
        ] {
            manifest["bindings"].as_array_mut().unwrap().push(json!({
                "id": id, "kind": "fileRead", "rootRef": "catalog_docs", "relativePathTemplate": path
            }));
            manifest["policy"]["grants"].as_array_mut().unwrap().push(json!({
                "id": id, "capabilityId": capability, "operation": "file.read",
                "target": {
                    "kind": "fileRead", "rootRef": "catalog_docs", "relativePathTemplate": approved_path
                }
            }));
            config["approvedGrantIds"]
                .as_array_mut()
                .unwrap()
                .push(json!(id));
        }
        config["targetBindings"]["catalog_docs"] =
            json!({"kind": "fileRead", "path": root.canonicalize().unwrap()});
    }
    let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    config["manifestRevision"] = json!(ManifestRevision::of(&bytes).to_string());
    let manifest_path = directory.join("catalog.json");
    let config_path = directory.join("runtime.json");
    fs::write(&manifest_path, bytes).unwrap();
    fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    (manifest_path, config_path)
}
