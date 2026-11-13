#[path = "http_wire/client.rs"]
mod client;
#[path = "http_wire/fixture.rs"]
mod fixture;
#[cfg(feature = "client-http-rustls")]
#[path = "http_wire/client_auto.rs"]
mod http_client_auto;
#[path = "http_wire/sdk_probe.rs"]
mod sdk_probe;

use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{Method, header};
use mcp_transport::http::LoopbackProfile;
use serde_json::{Value, json};

use client::{
    LEGACY, MODERN, Reply, SESSION, VERSION, initialize, notification, post, request, rpc, send,
    tool,
};
use fixture::Server;

const DIAGNOSTIC: &str = "mcp-server-builder/diagnostic";
const CANCELLATION_WAIT: Duration = Duration::from_secs(1);

#[tokio::test]
async fn modern_and_legacy_exercise_the_same_runtime_catalog() {
    let server = Server::start(LoopbackProfile::DualRevision, None).await;
    let mut discover_request = post(
        server.address,
        &rpc("discover", "server/discover", json!({}), true),
        true,
        None,
    );
    discover_request
        .headers_mut()
        .insert(SESSION, "ignored-session".parse().unwrap());
    discover_request
        .headers_mut()
        .insert("last-event-id", "ignored-event".parse().unwrap());
    let mut discover = send(server.address, discover_request).await;
    assert_eq!(discover.status(), 200);
    assert!(!discover.is_sse());
    assert!(!discover.headers().contains_key(SESSION));
    let message = discover.terminal().await;
    assert_eq!(message["id"], "discover");
    let result = &message["result"];
    assert_eq!(result["supportedVersions"], json!([MODERN, LEGACY]));
    assert_eq!(
        result["capabilities"],
        json!({"tools": {}, "resources": {}, "prompts": {}})
    );
    assert_cache(result);

    let session = start_legacy(&server).await.unwrap();
    for (modern, session) in [(true, None), (false, Some(session.as_str()))] {
        let listed = exchange(
            &server,
            &rpc(10, "tools/list", json!({}), modern),
            modern,
            session,
        )
        .await;
        assert_eq!(listed["id"], 10);
        let tools = &listed["result"]["tools"];
        assert_eq!(tools[0]["name"], "lookup_item");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
        assert_eq!(tools[0]["outputSchema"]["required"], json!(["id"]));
        assert_cache_if_modern(&listed["result"], modern);
        let cursor = exchange(
            &server,
            &rpc(28, "tools/list", json!({"cursor":"invalid-cursor"}), modern),
            modern,
            session,
        )
        .await;
        assert_eq!(cursor["error"]["code"], -32602);
        assert_diagnostic(&cursor["error"]["data"], "input.invalid_cursor");
        let result = exchange(
            &server,
            &tool(11, json!("42"), modern, None),
            modern,
            session,
        )
        .await;
        assert_eq!(result["id"], 11);
        assert_eq!(result["result"]["structuredContent"], json!({"id":"42"}));
        assert_eq!(result["result"]["content"][0]["type"], "text");
        assert_eq!(
            serde_json::from_str::<Value>(result["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap(),
            json!({"id":"42"})
        );
        let dispatch_before = server.effects();
        for (id, input, code) in [
            (12, json!(42), "input.invalid"),
            (13, json!("denied"), "policy.denied"),
        ] {
            let result = exchange(&server, &tool(id, input, modern, None), modern, session).await;
            assert_eq!(result["id"], id);
            assert_eq!(result["result"]["isError"], true);
            assert_diagnostic(&result["result"]["_meta"], code);
            assert_eq!(server.effects(), dispatch_before);
        }
        let invalid_output = exchange(
            &server,
            &tool(14, json!("invalid-output"), modern, None),
            modern,
            session,
        )
        .await;
        assert_diagnostic(&invalid_output["result"]["_meta"], "mcp.invalid_output");
        assert_eq!(server.effects(), dispatch_before + 1);

        for (id, method, key, name) in [
            (20, "resources/list", "resources", "Catalog guide"),
            (
                21,
                "resources/templates/list",
                "resourceTemplates",
                "Catalog note",
            ),
            (22, "prompts/list", "prompts", "review_item"),
        ] {
            let result = exchange(
                &server,
                &rpc(id, method, json!({}), modern),
                modern,
                session,
            )
            .await;
            assert_eq!(result["id"], id);
            assert_eq!(result["result"][key][0]["name"], name);
            assert_cache_if_modern(&result["result"], modern);
        }
        for (id, uri, text) in [
            (23, "catalog://docs/guide", "guide.txt"),
            (24, "catalog://docs/items/42", "notes/42.txt"),
        ] {
            let result = exchange(
                &server,
                &rpc(id, "resources/read", json!({"uri":uri}), modern),
                modern,
                session,
            )
            .await;
            assert_eq!(result["id"], id);
            assert_eq!(
                result["result"]["contents"][0],
                json!({"uri":uri,"mimeType":"text/plain","text":text})
            );
            assert_cache_if_modern(&result["result"], modern);
        }
        let missing = exchange(
            &server,
            &rpc(
                25,
                "resources/read",
                json!({"uri":"catalog://docs/missing"}),
                modern,
            ),
            modern,
            session,
        )
        .await;
        assert_eq!(
            missing["error"]["code"],
            if modern { -32602 } else { -32002 }
        );
        assert_diagnostic(&missing["error"]["data"], "mcp.unknown_resource");
        let prompt = exchange(
            &server,
            &rpc(
                26,
                "prompts/get",
                json!({"name":"review_item","arguments":{"id":"42"}}),
                modern,
            ),
            modern,
            session,
        )
        .await;
        assert_eq!(
            prompt["result"]["messages"][0],
            json!({"role":"user","content":{"type":"text","text":"Review catalog item 42."}})
        );
        let missing = exchange(
            &server,
            &rpc(27, "prompts/get", json!({"name":"missing"}), modern),
            modern,
            session,
        )
        .await;
        assert_eq!(missing["error"]["code"], -32602);
        assert_diagnostic(&missing["error"]["data"], "mcp.unknown_prompt");
    }
    server.stop().await;
}

#[tokio::test]
async fn modern_routing_checks_reject_before_adapter_preparation() {
    let server = Server::start(LoopbackProfile::ModernOnly, None).await;
    let call = tool(2, json!("42"), true, None);
    for key in [VERSION, "mcp-method", "mcp-name"] {
        let mut request = post(server.address, &call, true, None);
        request.headers_mut().remove(key);
        assert_error(send(server.address, request).await, 400, -32020, Some(2)).await;
    }
    for (key, value) in [
        (VERSION, LEGACY),
        ("mcp-method", "tools/list"),
        ("mcp-name", "fixture-canary"),
        ("mcp-name", "=?base64?bad-data!?="),
    ] {
        let mut request = post(server.address, &call, true, None);
        request.headers_mut().insert(key, value.parse().unwrap());
        assert_error(send(server.address, request).await, 400, -32020, Some(2)).await;
    }
    for (key, value) in [
        (VERSION, MODERN),
        ("mcp-method", "tools/call"),
        ("mcp-name", "lookup_item"),
    ] {
        let mut request = post(server.address, &call, true, None);
        request.headers_mut().append(key, value.parse().unwrap());
        assert_error(send(server.address, request).await, 400, -32020, Some(2)).await;
    }
    let mut future = call.clone();
    future["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2099-01-01");
    let mut request = post(server.address, &future, true, None);
    request
        .headers_mut()
        .insert(VERSION, "2099-01-01".parse().unwrap());
    let error = assert_error(send(server.address, request).await, 400, -32022, Some(2)).await;
    assert_eq!(error["error"]["data"]["supported"], json!([MODERN]));
    let mut missing_meta = call.clone();
    missing_meta["params"]
        .as_object_mut()
        .unwrap()
        .remove("_meta");
    assert_error(
        send(
            server.address,
            post(server.address, &missing_meta, true, None),
        )
        .await,
        400,
        -32602,
        Some(2),
    )
    .await;
    let mut malformed_capabilities = call.clone();
    malformed_capabilities["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"] =
        json!("invalid");
    assert_error(
        send(
            server.address,
            post(server.address, &malformed_capabilities, true, None),
        )
        .await,
        400,
        -32602,
        Some(2),
    )
    .await;
    let unknown = rpc(3, "fixture/unknown", json!({}), true);
    assert_error(
        send(server.address, post(server.address, &unknown, true, None)).await,
        404,
        -32601,
        Some(3),
    )
    .await;
    let malformed = rpc(
        4,
        "tools/call",
        json!({"name":"lookup_item","arguments":[]}),
        true,
    );
    assert_error(
        send(server.address, post(server.address, &malformed, true, None)).await,
        400,
        -32602,
        Some(4),
    )
    .await;
    let malformed_json = client::request(server.address, Method::POST, Body::from("{malformed"));
    assert_error(
        send(server.address, malformed_json).await,
        400,
        -32700,
        None,
    )
    .await;
    let batch = client::request(server.address, Method::POST, Body::from("[]"));
    assert_error(send(server.address, batch).await, 400, -32600, None).await;
    for (key, value, status) in [
        (header::ORIGIN, "https://fixture.invalid", 403),
        (header::HOST, "fixture.invalid", 403),
        (header::CONTENT_TYPE, "text/plain", 415),
        (
            header::ACCEPT,
            "application/json, text/event-stream;q=0",
            406,
        ),
    ] {
        let mut request = post(server.address, &call, true, None);
        request.headers_mut().insert(key, value.parse().unwrap());
        assert_eq!(send(server.address, request).await.status(), status);
    }
    let mut forwarded = post(server.address, &call, true, None);
    let headers = forwarded.headers_mut();
    headers.insert(header::HOST, "fixture.invalid".parse().unwrap());
    headers.insert(
        "x-forwarded-host",
        server.address.to_string().parse().unwrap(),
    );
    headers.insert("x-forwarded-proto", "http".parse().unwrap());
    assert_eq!(send(server.address, forwarded).await.status(), 403);
    for method in [Method::GET, Method::DELETE] {
        let reply = send(
            server.address,
            client::request(server.address, method, Body::empty()),
        )
        .await;
        assert_eq!(reply.status(), 405);
        assert_eq!(reply.headers()[header::ALLOW], "POST");
    }
    assert_eq!(server.preparations(), 0);
    assert_eq!(server.effects(), 0);
    let mut accepted = post(server.address, &call, true, None);
    accepted
        .headers_mut()
        .insert("mcp-name", "=?base64?bG9va3VwX2l0ZW0=?=".parse().unwrap());
    accepted.headers_mut().insert(
        header::ORIGIN,
        format!("http://{}", server.address).parse().unwrap(),
    );
    let mut accepted = send(server.address, accepted).await;
    assert!(accepted.is_sse());
    assert!(!accepted.headers().contains_key(SESSION));
    assert_eq!(
        accepted.terminal().await["result"]["structuredContent"]["id"],
        "42"
    );
    assert_eq!(server.effects(), 1);
    server.stop().await;
}

#[tokio::test]
async fn request_and_response_bytes_are_bounded() {
    let server = Server::start(LoopbackProfile::DualRevision, None).await;
    let valid = tool(2, json!("42"), true, None);
    let mut oversized = post(server.address, &valid, true, None);
    let mut payload = serde_json::to_vec(&valid).unwrap();
    payload.resize(1024 * 1024 + 1, b' ');
    let (left, right) = payload.split_at(512 * 1024);
    let parts = futures_util::stream::iter(
        [left, right].map(|part| Ok::<_, std::convert::Infallible>(Bytes::copy_from_slice(part))),
    );
    *oversized.body_mut() = Body::from_stream(parts);
    assert_eq!(send(server.address, oversized).await.status(), 413);
    assert_eq!(server.preparations(), 0);
    let mut exact = post(server.address, &valid, true, None);
    payload.pop();
    *exact.body_mut() = Body::from(payload);
    let mut accepted = send(server.address, exact).await;
    assert_eq!(
        accepted.terminal().await["result"]["structuredContent"]["id"],
        "42"
    );

    let long_id = "x".repeat(1000);
    let request = tool(long_id.clone(), json!("wide"), true, None);
    let mut oversized = send(server.address, post(server.address, &request, true, None)).await;
    let terminal = oversized.terminal().await;
    assert_eq!(terminal["id"], long_id);
    assert_eq!(terminal["error"]["code"], -32603);
    assert!(oversized.event().await.is_none());
    assert_eq!(exchange(&server, &valid, true, None).await["id"], 2);
    server.stop().await;
}

#[test]
fn diagnostic_sink_filters_sdk_payloads() {
    let output = capture_diagnostics();
    assert!(output.contains("fixture diagnostic sink ready"));
    assert!(!output.contains("fixture-sdk-log-canary"));
    assert!(!output.contains("rmcp::"));
}

fn capture_diagnostics() -> String {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::prelude::*;

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    impl Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().write(bytes)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let logs = LogBuffer::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .with_filter(tracing_subscriber::filter::filter_fn(
                mcp_transport::sdk_diagnostic_filter,
            ))
            .with_filter(tracing_subscriber::EnvFilter::new(
                "mcp_transport=trace,rmcp=trace",
            )),
    );
    // A current-thread runtime keeps the subscriber local to this test while
    // covering the tasks spawned by the HTTP listener and SDK.
    tracing::subscriber::with_default(subscriber, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tracing::info!(target: "mcp_transport::http", "fixture diagnostic sink ready");
                let server = Server::start(LoopbackProfile::DualRevision, None).await;
                let session = start_legacy(&server).await.unwrap();
                let item = "fixture-sdk-log-canary";
                let result = exchange(
                    &server,
                    &tool(2, json!(item), false, Some(item)),
                    false,
                    Some(&session),
                )
                .await;
                assert_eq!(result["result"]["structuredContent"]["id"], item);
                server.stop().await;
            });
    });
    String::from_utf8(logs.0.lock().unwrap().clone()).unwrap()
}

#[tokio::test]
async fn issued_legacy_sessions_require_ids_expire_delete_and_reinitialize() {
    let server = Server::start(
        LoopbackProfile::DualRevision,
        Some(Duration::from_millis(300)),
    )
    .await;
    let legacy = rpc(5, "ping", json!({}), false);
    assert_error(
        send(server.address, post(server.address, &legacy, false, None)).await,
        400,
        -32600,
        Some(5),
    )
    .await;
    let first = start_legacy(&server).await.unwrap();
    let mut wrong_version = post(server.address, &legacy, false, Some(&first));
    wrong_version
        .headers_mut()
        .insert(VERSION, "2025-03-26".parse().unwrap());
    assert_eq!(send(server.address, wrong_version).await.status(), 400);
    let mut missing_version = post(server.address, &legacy, false, Some(&first));
    missing_version.headers_mut().remove(VERSION);
    assert_eq!(send(server.address, missing_version).await.status(), 400);
    assert_eq!(
        exchange(&server, &legacy, false, Some(&first)).await["result"],
        json!({})
    );

    let mut get = request(server.address, Method::GET, Body::empty());
    get.headers_mut().insert(VERSION, LEGACY.parse().unwrap());
    get.headers_mut().insert(SESSION, first.parse().unwrap());
    get.headers_mut()
        .insert("last-event-id", "old-event".parse().unwrap());
    assert_eq!(send(server.address, get).await.status(), 405);
    tokio::time::sleep(Duration::from_millis(550)).await;
    assert_eq!(
        send(
            server.address,
            post(server.address, &legacy, false, Some(&first))
        )
        .await
        .status(),
        404
    );

    let second = start_legacy(&server).await.unwrap();
    assert_ne!(first, second);
    let mut delete = request(server.address, Method::DELETE, Body::empty());
    delete
        .headers_mut()
        .insert(VERSION, LEGACY.parse().unwrap());
    delete
        .headers_mut()
        .insert(SESSION, second.parse().unwrap());
    let mut reply = send(server.address, delete).await;
    assert_eq!(reply.status(), 202);
    assert!(reply.bytes().await.is_empty());
    assert_eq!(
        send(
            server.address,
            post(server.address, &legacy, false, Some(&second))
        )
        .await
        .status(),
        404
    );
    let third = start_legacy(&server).await.unwrap();
    assert_ne!(second, third);
    assert_eq!(
        exchange(&server, &legacy, false, Some(&third)).await["result"],
        json!({})
    );
    server.stop().await;
    let restarted = Server::start(LoopbackProfile::DualRevision, None).await;
    assert_eq!(
        send(
            restarted.address,
            post(restarted.address, &legacy, false, Some(&third))
        )
        .await
        .status(),
        404
    );
    restarted.stop().await;
}

#[tokio::test]
async fn legacy_session_capacity_preserves_modern_calls_and_recovers_after_delete() {
    let server = Server::start(LoopbackProfile::DualRevision, None).await;
    let mut sessions = std::collections::HashSet::new();
    for _ in 0..128 {
        assert!(sessions.insert(start_legacy(&server).await.unwrap()));
    }
    let mut refused = send(
        server.address,
        post(server.address, &initialize(), false, None),
    )
    .await;
    assert_eq!(refused.status(), 503);
    assert!(!refused.headers().contains_key(SESSION));
    assert!(refused.bytes().await.is_empty());
    let modern = tool("modern", json!("42"), true, None);
    assert_eq!(
        exchange(&server, &modern, true, None).await["result"]["structuredContent"]["id"],
        "42"
    );
    let mut delete = request(server.address, Method::DELETE, Body::empty());
    let headers = delete.headers_mut();
    headers.insert(VERSION, LEGACY.parse().unwrap());
    headers.insert(SESSION, sessions.iter().next().unwrap().parse().unwrap());
    assert_eq!(send(server.address, delete).await.status(), 202);
    assert!(sessions.insert(start_legacy(&server).await.unwrap()));
    server.stop().await;
}

#[tokio::test]
async fn modern_disconnect_cancels_only_the_request_stream() {
    let server = Server::start(LoopbackProfile::DualRevision, None).await;
    for (item, token) in [("wait-progress", Some("progress-a")), ("wait-quiet", None)] {
        let mut request = tool("same-id", json!(item), true, token);
        request["params"]["_meta"]["io.modelcontextprotocol/clientInfo"] =
            json!({"name":"cancelled-client","version":"1"});
        let (started, mut reply) = tokio::join!(
            server.started(),
            send(server.address, post(server.address, &request, true, None)),
        );
        assert_eq!(started, item);
        assert!(reply.is_sse());
        if let Some(token) = token {
            let progress = reply.event().await.unwrap();
            assert_eq!(progress["method"], "notifications/progress");
            assert_eq!(progress["params"]["progressToken"], token);
        }
        let mut sibling = start_sibling(&server, "same-id", true, None).await;
        assert_eq!(server.active(), 2);
        drop(reply);
        assert_ended_promptly(&server, item).await;
        finish_sibling(&server, &mut sibling, "same-id").await;
    }
    server.stop().await;
}

#[tokio::test]
async fn legacy_disconnect_and_cancellation_differ_with_and_without_a_session() {
    for (profile, drop_cancels) in [
        (LoopbackProfile::DualRevision, false),
        (LoopbackProfile::SessionlessComparison, true),
    ] {
        let server = Server::start(profile, None).await;
        let session = start_legacy(&server).await;
        assert_eq!(session.is_none(), drop_cancels);
        let call = tool(
            "slow",
            json!("wait-progress"),
            false,
            Some("legacy-progress"),
        );
        let mut reply = send(
            server.address,
            post(server.address, &call, false, session.as_deref()),
        )
        .await;
        assert!(reply.is_sse());
        assert_eq!(server.started().await, "wait-progress");
        let progress = reply.event().await.unwrap();
        assert_eq!(progress["params"]["progressToken"], "legacy-progress");
        drop(reply);
        if drop_cancels {
            assert_ended_promptly(&server, "wait-progress").await;
        } else {
            assert!(server.stays_active().await);
            assert_cancel_accepted(&server, session.as_deref(), "slow").await;
            assert_ended_promptly(&server, "wait-progress").await;
        }

        let mut reply = send(
            server.address,
            post(server.address, &call, false, session.as_deref()),
        )
        .await;
        assert_eq!(server.started().await, "wait-progress");
        assert_eq!(
            reply.event().await.unwrap()["params"]["progressToken"],
            "legacy-progress"
        );
        assert_cancel_accepted(&server, session.as_deref(), "unrelated").await;
        assert!(server.stays_active().await);
        let mut sibling = if drop_cancels {
            None
        } else {
            Some(start_sibling(&server, "sibling", false, session.as_deref()).await)
        };
        assert_cancel_accepted(&server, session.as_deref(), "slow").await;
        if drop_cancels {
            assert!(server.stays_active().await);
            let terminal = reply.terminal().await;
            assert_diagnostic(&terminal["result"]["_meta"], "runtime.timeout");
            assert_eq!(server.ended().await, "wait-progress");
        } else {
            assert_ended_promptly(&server, "wait-progress").await;
            assert!(reply.event().await.is_none());
            finish_sibling(&server, sibling.as_mut().unwrap(), "sibling").await;
        }
        let quick = tool("slow", json!("42"), false, None);
        assert_eq!(
            exchange(&server, &quick, false, session.as_deref()).await["result"]["structuredContent"]
                ["id"],
            "42"
        );
        server.stop().await;
    }
}

async fn start_sibling(server: &Server, id: &str, modern: bool, session: Option<&str>) -> Reply {
    let mut call = tool(id, json!("wait-sibling"), modern, None);
    if modern {
        call["params"]["_meta"]["io.modelcontextprotocol/clientInfo"] =
            json!({"name":"sibling-client","version":"1"});
    }
    let reply = send(server.address, post(server.address, &call, modern, session)).await;
    assert_eq!(reply.status(), 200);
    assert!(reply.is_sse());
    assert_eq!(server.started().await, "wait-sibling");
    reply
}

async fn finish_sibling(server: &Server, sibling: &mut Reply, id: &str) {
    assert_eq!(server.active(), 1);
    server.release_sibling();
    let terminal = sibling.terminal().await;
    assert_eq!(terminal["id"], id);
    assert_eq!(
        terminal["result"]["structuredContent"]["id"],
        "wait-sibling"
    );
    assert_eq!(server.ended().await, "wait-sibling");
}

async fn assert_ended_promptly(server: &Server, item: &str) {
    let ended = tokio::time::timeout(CANCELLATION_WAIT, server.ended())
        .await
        .expect("fixture work did not stop before its execution deadline");
    assert_eq!(ended, item);
}

async fn start_legacy(server: &Server) -> Option<String> {
    let mut reply = send(
        server.address,
        post(server.address, &initialize(), false, None),
    )
    .await;
    assert_eq!(reply.status(), 200);
    let session = reply
        .headers()
        .get(SESSION)
        .map(|value| value.to_str().unwrap().to_owned());
    let result = reply.terminal().await;
    assert_eq!(result["id"], 1);
    assert_eq!(result["result"]["protocolVersion"], LEGACY);
    let initialized = notification("notifications/initialized", None);
    let mut ack = send(
        server.address,
        post(server.address, &initialized, false, session.as_deref()),
    )
    .await;
    assert_eq!(ack.status(), 202);
    assert!(ack.bytes().await.is_empty());
    session
}

async fn assert_cancel_accepted(server: &Server, session: Option<&str>, id: &str) {
    let cancel = notification("notifications/cancelled", Some(json!({"requestId":id})));
    let mut reply = send(
        server.address,
        post(server.address, &cancel, false, session),
    )
    .await;
    assert_eq!(reply.status(), 202);
    assert!(reply.bytes().await.is_empty());
}

async fn exchange(server: &Server, message: &Value, modern: bool, session: Option<&str>) -> Value {
    let mut reply = send(
        server.address,
        post(server.address, message, modern, session),
    )
    .await;
    assert_eq!(reply.status(), 200);
    assert!(!reply.headers().contains_key(SESSION));
    reply.terminal().await
}

async fn assert_error(mut reply: Reply, status: u16, code: i64, id: Option<i64>) -> Value {
    assert_eq!(reply.status(), status);
    assert!(!reply.is_sse());
    let value = reply.terminal().await;
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["error"]["code"], code, "{value}");
    assert_eq!(value.get("id").and_then(Value::as_i64), id);
    assert!(!value.to_string().contains("fixture-canary"));
    value
}

fn assert_cache(result: &Value) {
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["ttlMs"], 0);
    assert_eq!(result["cacheScope"], "private");
}

fn assert_cache_if_modern(result: &Value, modern: bool) {
    if modern {
        assert_cache(result);
    } else {
        assert!(result.get("resultType").is_none());
        assert!(result.get("ttlMs").is_none());
        assert!(result.get("cacheScope").is_none());
    }
}

fn assert_diagnostic(value: &Value, code: &str) {
    let diagnostic = &value[DIAGNOSTIC];
    assert_eq!(diagnostic["code"], code, "{value}");
    assert_eq!(diagnostic["correlationId"].as_str().unwrap().len(), 36);
}
