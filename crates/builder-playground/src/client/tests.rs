use std::collections::BTreeMap;

use mcp_policy::REDACTED;
use serde_json::Map;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::*;

const MODERN: &str = "2026-07-28";
const LEGACY: &str = "2025-11-25";
const SECRET: &str = "resolved-secret-canary";
const PRIVATE: &str = "private-path-canary";
const PEER_DIAGNOSTIC: &str = "raw-peer-diagnostic-canary";
const CASE_TIMEOUT: Duration = Duration::from_secs(5);

struct RawPeer {
    input: BufReader<ReadHalf<DuplexStream>>,
    output: WriteHalf<DuplexStream>,
    stderr: DuplexStream,
}

fn peers() -> (
    ReadHalf<DuplexStream>,
    WriteHalf<DuplexStream>,
    DuplexStream,
    RawPeer,
) {
    let (client, peer) = tokio::io::duplex(2048);
    let (stderr_reader, stderr) = tokio::io::duplex(1024);
    let (client_stdout, client_stdin) = tokio::io::split(client);
    let (input, output) = tokio::io::split(peer);
    (
        client_stdout,
        client_stdin,
        stderr_reader,
        RawPeer {
            input: BufReader::new(input),
            output,
            stderr,
        },
    )
}

async fn verify_peer(task: JoinHandle<()>) {
    timeout(CASE_TIMEOUT, task).await.unwrap().unwrap();
}

impl RawPeer {
    async fn read(&mut self, method: &str) -> Value {
        let mut line = Vec::new();
        self.input.read_until(b'\n', &mut line).await.unwrap();
        assert!(line.ends_with(b"\n"));
        let value: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        if !method.is_empty() {
            assert_eq!(value["method"], method);
        }
        value
    }

    async fn write(&mut self, frame: &Value) {
        let mut frame = serde_json::to_vec(frame).unwrap();
        frame.push(b'\n');
        self.output.write_all(&frame).await.unwrap();
    }

    async fn reply(&mut self, request: &Value, result: Value) {
        self.write(&json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
            .await;
    }

    async fn modern(&mut self, capabilities: Value) {
        let discover = self.read("server/discover").await;
        let meta = &discover["params"]["_meta"];
        assert_eq!(meta["io.modelcontextprotocol/protocolVersion"], MODERN);
        assert!(meta["io.modelcontextprotocol/clientCapabilities"].is_object());
        self.reply(
            &discover,
            json!({
                "supportedVersions":[MODERN,LEGACY], "capabilities":capabilities,
                "resultType":"complete", "ttlMs":0, "cacheScope":"private",
                "_meta":{"io.modelcontextprotocol/serverInfo":{"name":format!("fixture {SECRET}"),"version":"1"}},
            }),
        )
        .await;
    }

    async fn initialize(&mut self, revision: &str, capabilities: Value) {
        let initialize = self.read("initialize").await;
        assert_eq!(initialize["params"]["protocolVersion"], LEGACY);
        self.reply(
            &initialize,
            json!({
                "protocolVersion":revision, "capabilities":capabilities,
                "serverInfo":{"name":"fixture","version":"1"},
            }),
        )
        .await;
    }

    async fn legacy_probe_error(&mut self) {
        let discover = self.read("server/discover").await;
        self.write(&json!({
            "jsonrpc":"2.0","id":discover["id"],
            "error":{"code":-32601,"message":PEER_DIAGNOSTIC},
        }))
        .await;
    }

    async fn eof(&mut self) {
        assert!(self.input.fill_buf().await.unwrap().is_empty());
    }
}

fn tool(name: &str) -> Invocation {
    Invocation::Tool {
        name: name.to_owned(),
        arguments: Map::new(),
    }
}

fn request_timeout() -> RequestOptions {
    RequestOptions {
        timeout: CASE_TIMEOUT,
        ..RequestOptions::default()
    }
}

#[tokio::test]
async fn negotiates_explicit_and_auto_profiles_and_uses_the_corresponding_wire() {
    for (preference, revision) in [
        (ProtocolPreference::Auto, ProtocolRevision::Modern),
        (ProtocolPreference::Modern, ProtocolRevision::Modern),
        (ProtocolPreference::Auto, ProtocolRevision::Legacy),
        (ProtocolPreference::Legacy, ProtocolRevision::Legacy),
    ] {
        let (stdout, stdin, stderr, mut peer) = peers();
        let peer_task = tokio::spawn(async move {
            match revision {
                ProtocolRevision::Modern => peer.modern(json!({"tools":{}})).await,
                ProtocolRevision::Legacy => {
                    if preference == ProtocolPreference::Auto {
                        peer.legacy_probe_error().await;
                    }
                    peer.initialize(LEGACY, json!({"tools":{}})).await;
                    let initialized = peer.read("notifications/initialized").await;
                    assert!(initialized.get("id").is_none());
                }
            }
            let list = peer.read("tools/list").await;
            let version = &list["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"];
            if revision == ProtocolRevision::Modern {
                assert_eq!(version, MODERN);
            } else {
                assert!(version.is_null());
            }
            peer.reply(
                &list,
                json!({"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}),
            )
            .await;
            peer.eof().await;
        });
        let client = timeout(
            CASE_TIMEOUT,
            StdioClient::connect(
                stdout,
                stdin,
                stderr,
                ClientOptions {
                    protocol: preference,
                    ..ClientOptions::default()
                },
                &CancellationToken::new(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let discovery = client.discover(request_timeout()).await.unwrap();
        assert_eq!(discovery.protocol_revision, revision);
        assert_eq!(discovery.tools[0]["name"], "echo");
        assert!(
            client
                .trace()
                .entries
                .iter()
                .all(|entry| entry.payload.is_none())
        );
        client.close().await.unwrap();
        verify_peer(peer_task).await;
    }
}

#[tokio::test]
async fn rejects_unsupported_legacy_revision_before_using_capabilities() {
    for preference in [ProtocolPreference::Auto, ProtocolPreference::Legacy] {
        let (stdout, stdin, stderr, mut peer) = peers();
        let peer_task = tokio::spawn(async move {
            if preference == ProtocolPreference::Auto {
                peer.legacy_probe_error().await;
            }
            peer.initialize("2025-03-26", json!({"tools":{}})).await;
            peer.eof().await;
        });
        let result = timeout(
            CASE_TIMEOUT,
            StdioClient::connect(
                stdout,
                stdin,
                stderr,
                ClientOptions {
                    protocol: preference,
                    ..ClientOptions::default()
                },
                &CancellationToken::new(),
            ),
        )
        .await
        .unwrap();
        let Err(error) = result else {
            panic!("an unsupported revision opened a connection");
        };
        assert_eq!(error.code(), "mcp.unsupported_protocol");
        verify_peer(peer_task).await;
    }
}

#[tokio::test]
async fn auto_does_not_fall_back_after_a_recognized_modern_error() {
    let (stdout, stdin, stderr, mut peer) = peers();
    let peer_task = tokio::spawn(async move {
        let discover = peer.read("server/discover").await;
        peer.write(&json!({
            "jsonrpc":"2.0","id":discover["id"],
            "error":{"code":-32021,"message":PEER_DIAGNOSTIC},
        }))
        .await;
        peer.eof().await;
    });
    let Err(error) = timeout(
        CASE_TIMEOUT,
        StdioClient::connect(
            stdout,
            stdin,
            stderr,
            ClientOptions::default(),
            &CancellationToken::new(),
        ),
    )
    .await
    .unwrap() else {
        panic!("a required capability error opened a connection");
    };
    assert_eq!(error.code(), "mcp.unsupported_capability");
    assert!(!format!("{error:?}").contains(PEER_DIAGNOSTIC));
    verify_peer(peer_task).await;
}

#[tokio::test]
async fn discovers_lists_and_invokes_all_supported_capability_kinds() {
    let (stdout, stdin, stderr, mut peer) = peers();
    let peer_task = tokio::spawn(async move {
        peer.modern(json!({"tools":{},"resources":{},"prompts":{}}))
            .await;
        let tools = peer.read("tools/list").await;
        peer.reply(&tools, json!({
            "tools":[{"name":"echo","inputSchema":{"type":"object","properties":{"_meta":{"type":"string"}}}}],
            "nextCursor":"second-page"
        })).await;
        let second = peer.read("tools/list").await;
        assert_eq!(second["params"]["cursor"], "second-page");
        peer.reply(
            &second,
            json!({"tools":[{"name":"second","inputSchema":{"type":"object"}}]}),
        )
        .await;
        let resources = peer.read("resources/list").await;
        peer.reply(
            &resources,
            json!({"resources":[{"name":"items","uri":"example://items"}]}),
        )
        .await;
        let templates = peer.read("resources/templates/list").await;
        peer.reply(
            &templates,
            json!({"resourceTemplates":[{"name":"item","uriTemplate":"example://items/{id}"}]}),
        )
        .await;
        let prompts = peer.read("prompts/list").await;
        peer.reply(
            &prompts,
            json!({"prompts":[{"name":"greet","arguments":[{"name":"name","required":true}]}]}),
        )
        .await;

        let tool = peer.read("tools/call").await;
        peer.reply(
            &tool,
            json!({
                "content":[{"type":"text","text":"ok","_meta":{"wire":"only"}}],
                "structuredContent":{"_meta":"application data"}, "isError":false,
                "resultType":"complete", "_meta":{"internal":PEER_DIAGNOSTIC},
            }),
        )
        .await;
        let read = peer.read("resources/read").await;
        assert_eq!(read["params"]["uri"], "example://items/42");
        peer.reply(&read, json!({"contents":[{"uri":"example://items/42","text":"item 42"}],"resultType":"complete"})).await;
        let get = peer.read("prompts/get").await;
        assert_eq!(get["params"]["arguments"]["name"], "Pat");
        peer.reply(&get, json!({"description":"Greeting","messages":[{"role":"user","content":{"type":"text","text":"Hello Pat"}}],"resultType":"complete"})).await;
        peer.eof().await;
    });
    let client = timeout(
        CASE_TIMEOUT,
        StdioClient::connect(
            stdout,
            stdin,
            stderr,
            ClientOptions {
                redactor: Arc::new(Redactor::new([SECRET], [] as [&str; 0]).unwrap()),
                ..ClientOptions::default()
            },
            &CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let discovery = client.discover(request_timeout()).await.unwrap();
    assert_eq!(
        discovery.server_info.unwrap()["name"],
        format!("fixture {REDACTED}")
    );
    assert_eq!(discovery.tools.len(), 2);
    assert!(
        discovery.tools[0]["inputSchema"]["properties"]
            .get("_meta")
            .is_some()
    );
    assert_eq!(discovery.resources[0]["uri"], "example://items");
    assert_eq!(
        discovery.resource_templates[0]["uriTemplate"],
        "example://items/{id}"
    );
    assert_eq!(discovery.prompts[0]["name"], "greet");
    let result = client
        .invoke(&tool("echo"), request_timeout())
        .await
        .unwrap();
    assert_eq!(
        result,
        json!({"content":[{"type":"text","text":"ok"}],"structuredContent":{"_meta":"application data"}})
    );
    let result = client
        .invoke(
            &Invocation::Resource {
                uri: "example://items/42".into(),
            },
            request_timeout(),
        )
        .await
        .unwrap();
    assert_eq!(
        result,
        json!({"contents":[{"uri":"example://items/42","text":"item 42"}]})
    );
    let result = client
        .invoke(
            &Invocation::Prompt {
                name: "greet".into(),
                arguments: BTreeMap::from([("name".into(), "Pat".into())]),
            },
            request_timeout(),
        )
        .await
        .unwrap();
    assert_eq!(result["description"], "Greeting");
    assert_eq!(result["messages"][0]["content"]["text"], "Hello Pat");
    client.close().await.unwrap();
    verify_peer(peer_task).await;
}

#[tokio::test]
async fn cancels_and_times_out_only_the_addressed_request() {
    for revision in [ProtocolRevision::Modern, ProtocolRevision::Legacy] {
        let (stdout, stdin, stderr, mut peer) = peers();
        let (ready, cancel) = oneshot::channel();
        let peer_task = tokio::spawn(async move {
            match revision {
                ProtocolRevision::Modern => peer.modern(json!({"tools":{}})).await,
                ProtocolRevision::Legacy => {
                    peer.initialize(LEGACY, json!({"tools":{}})).await;
                    peer.read("notifications/initialized").await;
                }
            }
            let first = peer.read("tools/call").await;
            let second = peer.read("tools/call").await;
            let (target, sibling) = if first["params"]["name"] == "cancel" {
                (first, second)
            } else {
                (second, first)
            };
            ready.send(()).unwrap();
            let notification = peer.read("notifications/cancelled").await;
            assert_eq!(notification["params"]["requestId"], target["id"]);
            peer.reply(&target, json!({"content":[{"type":"text","text":"late"}]}))
                .await;
            peer.reply(
                &sibling,
                json!({"content":[{"type":"text","text":"healthy sibling"}]}),
            )
            .await;
            let slow = peer.read("tools/call").await;
            let token = &slow["params"]["_meta"]["progressToken"];
            peer.write(&json!({
                "jsonrpc":"2.0","method":"notifications/progress",
                "params":{"progressToken":token,"progress":1,"message":PEER_DIAGNOSTIC},
            }))
            .await;
            let first = peer.read("").await;
            let second = peer.read("").await;
            let (notification, next) = if first["method"] == "notifications/cancelled" {
                (first, second)
            } else {
                (second, first)
            };
            assert_eq!(notification["method"], "notifications/cancelled");
            assert_eq!(notification["params"]["requestId"], slow["id"]);
            assert_eq!(next["method"], "tools/call");
            assert_eq!(next["params"]["name"], "next");
            peer.reply(
                &next,
                json!({"content":[{"type":"text","text":"healthy next call"}]}),
            )
            .await;
            peer.eof().await;
        });
        let client = timeout(
            CASE_TIMEOUT,
            StdioClient::connect(
                stdout,
                stdin,
                stderr,
                ClientOptions {
                    protocol: match revision {
                        ProtocolRevision::Modern => ProtocolPreference::Modern,
                        ProtocolRevision::Legacy => ProtocolPreference::Legacy,
                    },
                    capture_payloads: true,
                    ..ClientOptions::default()
                },
                &CancellationToken::new(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let cancel_options = request_timeout();
        let token = cancel_options.cancellation.clone();
        let cancel_id = cancel_options.correlation_id;
        let cancel_call = tool("cancel");
        let sibling_call = tool("sibling");
        let (cancelled, sibling, ()) = tokio::join!(
            client.invoke(&cancel_call, cancel_options),
            client.invoke(&sibling_call, request_timeout()),
            async {
                timeout(CASE_TIMEOUT, cancel).await.unwrap().unwrap();
                token.cancel();
            },
        );
        assert_eq!(cancelled.unwrap_err().code(), "run.cancelled");
        assert_eq!(sibling.unwrap()["content"][0]["text"], "healthy sibling");
        let slow_options = RequestOptions {
            timeout: Duration::from_millis(80),
            ..RequestOptions::default()
        };
        let slow_id = slow_options.correlation_id;
        assert_eq!(
            client
                .invoke(&tool("slow"), slow_options)
                .await
                .unwrap_err()
                .code(),
            "runtime.timeout"
        );
        assert_eq!(
            client
                .invoke(&tool("next"), request_timeout())
                .await
                .unwrap()["content"][0]["text"],
            "healthy next call"
        );
        let trace = client.trace();
        assert!(
            trace.entries.iter().any(
                |entry| entry.correlation_id == cancel_id && entry.kind == TraceKind::Cancelled
            )
        );
        assert!(
            trace
                .entries
                .iter()
                .any(|entry| entry.correlation_id == slow_id && entry.kind == TraceKind::Progress)
        );
        assert!(
            !serde_json::to_string(&trace)
                .unwrap()
                .contains(PEER_DIAGNOSTIC)
        );
        client.close().await.unwrap();
        assert!(
            client
                .trace()
                .entries
                .iter()
                .all(|entry| entry.payload.is_none())
        );
        verify_peer(peer_task).await;
    }
}

#[tokio::test]
async fn recovers_from_fixture_frames_and_omits_diagnostics_and_sensitive_payloads() {
    let (stdout, stdin, stderr, mut peer) = peers();
    let peer_task = tokio::spawn(async move {
        // The fixture cannot answer discovery until stderr has been drained.
        peer.stderr.write_all(&[b'x'; 4096]).await.unwrap();
        peer.stderr
            .write_all(PEER_DIAGNOSTIC.as_bytes())
            .await
            .unwrap();
        peer.modern(json!({"tools":{}})).await;
        let call = peer.read("tools/call").await;
        assert_eq!(call["params"]["arguments"]["public"], SECRET);
        peer.output
            .write_all(b"invalid synthetic line\n")
            .await
            .unwrap();
        let mut oversized = vec![b'x'; MAX_FRAME_BYTES];
        oversized.push(b'\n');
        peer.output.write_all(&oversized).await.unwrap();
        let structured = json!({"private":PRIVATE,"safe":"visible","safeEcho":SECRET});
        peer.reply(
            &call,
            json!({
                "content":[{"type":"text","text":structured.to_string()}],
                "structuredContent":structured,
            }),
        )
        .await;
        let large = peer.read("tools/call").await;
        peer.reply(
            &large,
            json!({"content":[{"type":"text","text":"x".repeat(MAX_ENTRY_BYTES * 2)}]}),
        )
        .await;
        let malformed = peer.read("tools/call").await;
        peer.reply(&malformed, json!({"contents":[]})).await;
        let failure = peer.read("tools/call").await;
        peer.reply(
            &failure,
            json!({
                "isError":true,
                "content":[{"type":"text","text":PEER_DIAGNOSTIC}],
                "_meta":{"mcp-server-builder/diagnostic":{
                    "code":"policy.denied","correlationId":Uuid::new_v4(),
                    "message":PEER_DIAGNOSTIC,
                }},
            }),
        )
        .await;
        peer.eof().await;
    });
    let client = timeout(
        CASE_TIMEOUT,
        StdioClient::connect(
            stdout,
            stdin,
            stderr,
            ClientOptions {
                capture_payloads: true,
                trust_builder_diagnostics: true,
                redactor: Arc::new(Redactor::new([SECRET], ["/arguments/private"]).unwrap()),
                ..ClientOptions::default()
            },
            &CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let options = RequestOptions {
        redactor: Some(Arc::new(
            Redactor::new([] as [&str; 0], ["/structuredContent/private"]).unwrap(),
        )),
        ..request_timeout()
    };
    let correlation_id = options.correlation_id;
    let result = client
        .invoke(
            &Invocation::Tool {
                name: "echo".into(),
                arguments: Map::from_iter([
                    ("public".into(), json!(SECRET)),
                    ("private".into(), json!(PRIVATE)),
                ]),
            },
            options,
        )
        .await
        .unwrap();
    assert_eq!(result["structuredContent"]["private"], PRIVATE);
    assert_eq!(
        client
            .invoke(&tool("large"), request_timeout())
            .await
            .unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .len(),
        MAX_ENTRY_BYTES * 2
    );
    assert_eq!(
        client
            .invoke(&tool("malformed"), request_timeout())
            .await
            .unwrap_err()
            .code(),
        "transport.invalid_message"
    );
    let error = client
        .invoke(&tool("denied"), request_timeout())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "policy.denied");
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains(PEER_DIAGNOSTIC)
    );
    let trace = client.trace();
    let serialized = serde_json::to_string(&trace).unwrap();
    for canary in [SECRET, PRIVATE, PEER_DIAGNOSTIC] {
        assert!(!serialized.contains(canary));
    }
    assert!(trace.summary.truncated);
    assert!(
        trace
            .entries
            .iter()
            .any(|entry| entry.direction == TraceDirection::Stderr)
    );
    assert!(
        trace
            .entries
            .iter()
            .any(|entry| entry.kind == TraceKind::InvalidFrame)
    );
    assert!(
        trace
            .entries
            .iter()
            .any(|entry| entry.kind == TraceKind::FrameLimit)
    );
    let entry = trace
        .entries
        .iter()
        .find(|entry| entry.correlation_id == correlation_id && entry.kind == TraceKind::Response)
        .unwrap();
    assert_eq!(
        entry.payload.as_ref().unwrap()["structuredContent"],
        json!({"private":REDACTED,"safe":"visible","safeEcho":REDACTED})
    );
    assert_eq!(entry.payload.as_ref().unwrap()["content"], json!([]));
    client.close().await.unwrap();
    verify_peer(peer_task).await;
}
