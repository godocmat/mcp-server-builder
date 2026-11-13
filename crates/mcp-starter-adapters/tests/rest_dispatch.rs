use std::net::Ipv4Addr;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use builder_project::{ValidatedManifest, parse_manifest, runtime_bindings, runtime_definition};
use mcp_policy::{
    ApprovedGrant, CallerGrant, CallerIdentity, DenyReason, Effect, LimitOverrides, OperatorPolicy,
    RestRoute,
};
use mcp_runtime::{
    CancellationToken, ErrorKind, ExecutionContext, Principal, Runtime, RuntimeError,
};
use mcp_starter_adapters::{LocalBindings, SetupError, StarterAdapter};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const OPERATOR: &str = "starter-fixture";
const LOCAL: Principal<'static> = Principal::Local { operator: OPERATOR };
const WAIT: Duration = Duration::from_secs(3);
const QUIET: Duration = Duration::from_millis(100);
const CANARY: &str = "harmless-upstream-error-body-fixture";

fn manifest(origin: &str, method: &str, grant_path: &str, allow_loopback: bool) -> Value {
    let read = matches!(method, "GET" | "HEAD" | "OPTIONS");
    let mut target = json!({
        "kind":"rest","origin":origin,"method":method,"pathTemplate":grant_path
    });
    if allow_loopback {
        target["allowedPeerRanges"] = json!(["127.0.0.1/32"]);
    }
    json!({
        "format":"mcp-server-builder.project","schemaVersion":1,
        "projectId":"8d1ea3a7-18c4-4e36-8a8c-6e84af79eed0","name":"Live fixture",
        "server":{"name":"fixture","version":"0.1.0"},"transport":"stdio",
        "capabilities":[{
            "id":"fetch","kind":"tool","name":"fetch","description":"Read fixture",
            "inputSchema":{"type":"object","properties":{"id":{"type":"string"},"data":{"type":"object"}},
                "required":["id"],"additionalProperties":false},
            "bindingId":"fixture"
        }],
        "bindings":[{
            "id":"fixture","kind":"rest","origin":origin,"method":method,
            "pathTemplate":"/items/{id}","pathParams":{"id":"/id"}
        }],
        "policy":{"grants":[{
            "id":"fetch","capabilityId":"fetch",
            "operation":if read { "http.read" } else { "http.write" },
            "target":target
        }]},
        "secretRefs":[],"scenarios":[]
    })
}

fn validated(document: Value) -> ValidatedManifest {
    parse_manifest(&serde_json::to_vec(&document).unwrap()).unwrap()
}

fn runtime(document: Value, approval: bool, redirects: Vec<RestRoute>) -> Runtime {
    let manifest = validated(document);
    let definition = runtime_definition(&manifest);
    let grants: Vec<_> = definition
        .requested_policy
        .grants
        .iter()
        .filter(|_| approval)
        .cloned()
        .map(|grant| ApprovedGrant {
            grant,
            redirects: redirects.clone(),
        })
        .collect();
    let operator = OperatorPolicy {
        caller_grants: vec![CallerGrant {
            identity: CallerIdentity::Local {
                operator: OPERATOR.into(),
            },
            capability_ids: ["fetch".into()].into(),
            grant_ids: grants.iter().map(|entry| entry.grant.id.clone()).collect(),
            required_scopes: Default::default(),
            limits: Default::default(),
        }],
        approved_grants: grants,
        ..Default::default()
    };
    let adapter = StarterAdapter::new(
        manifest.document().project_id,
        runtime_bindings(&manifest),
        LocalBindings::default(),
    )
    .unwrap();
    Runtime::new(definition, operator, Arc::new(adapter)).unwrap()
}

fn arguments(id: &str) -> Map<String, Value> {
    Map::from_iter([("id".into(), json!(id))])
}

async fn bind() -> TcpListener {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap()
}

fn origin(listener: &TcpListener) -> String {
    format!("http://{}", listener.local_addr().unwrap())
}

async fn quiet(listener: &TcpListener) {
    assert!(
        tokio::time::timeout(QUIET, listener.accept())
            .await
            .is_err()
    );
}

fn response_once(listener: TcpListener, response: String) -> JoinHandle<(TcpListener, String)> {
    tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(WAIT, listener.accept())
            .await
            .unwrap()
            .unwrap();
        let request = tokio::time::timeout(WAIT, read_request(&mut socket))
            .await
            .unwrap();
        socket.write_all(response.as_bytes()).await.unwrap();
        (listener, request)
    })
}

async fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() <= 8192);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = std::str::from_utf8(&bytes[..index]).unwrap();
            let body_len = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        return Some(value.trim().parse::<usize>().unwrap());
                    }
                    None
                })
                .unwrap_or_default();
            if bytes.len() >= index + 4 + body_len {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }
}

fn response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    )
}

fn assert_clean(error: &RuntimeError) {
    assert!(!format!("{error:?} {error} {}", error.message()).contains(CANARY));
}

#[tokio::test]
async fn policy_and_input_denials_open_zero_connections_before_an_allowed_request() {
    let listener = bind().await;
    let endpoint = origin(&listener);
    let path = "/items/caf%C3%A9%20one";
    let call = ExecutionContext::new(&LOCAL, CancellationToken::new());
    let denied_peer = runtime(manifest(&endpoint, "GET", path, false), true, vec![]);
    assert_eq!(
        denied_peer
            .call_tool("fetch", arguments("café one"), &call)
            .await
            .unwrap_err()
            .denial,
        Some(DenyReason::TargetDenied),
    );
    let default_deny = runtime(manifest(&endpoint, "GET", path, true), false, vec![]);
    assert_eq!(
        default_deny
            .call_tool("fetch", arguments("café one"), &call)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::UnknownTool,
    );
    let live = runtime(manifest(&endpoint, "GET", path, true), true, vec![]);
    assert_eq!(
        live.call_tool("fetch", arguments("other"), &call)
            .await
            .unwrap_err()
            .denial,
        Some(DenyReason::TargetDenied),
    );
    for id in ["../outside", "a/b", "%2F", "?", ";", "\\", ""] {
        assert_eq!(
            live.call_tool("fetch", arguments(id), &call)
                .await
                .unwrap_err()
                .kind,
            ErrorKind::InvalidArguments,
        );
    }
    quiet(&listener).await;
    let reply = response_once(
        listener,
        response(
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"{"name":"catalog"}"#,
        ),
    );
    let output = live
        .call_tool("fetch", arguments("café one"), &call)
        .await
        .unwrap();
    assert_eq!(output.structured_content.unwrap()["name"], "catalog");
    let (_, request) = reply.await.unwrap();
    assert!(request.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
    assert!(request.contains(&format!(
        "host: {}\r\n",
        endpoint.trim_start_matches("http://")
    )));
}

#[tokio::test]
async fn explicit_writes_send_only_the_declared_json_body() {
    let listener = bind().await;
    let endpoint = origin(&listener);
    let mut document = manifest(&endpoint, "POST", "/items/{id}", true);
    document["bindings"][0]["bodyPointer"] = json!("/data");
    let call = ExecutionContext::new(&LOCAL, CancellationToken::new());
    let deny = runtime(document.clone(), false, vec![]);
    assert_eq!(
        deny.call_tool("fetch", arguments("42"), &call)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::UnknownTool
    );
    let live = runtime(document, true, vec![]);
    assert_eq!(
        live.call_tool("fetch", arguments("42"), &call)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArguments
    );
    quiet(&listener).await;
    let reply = response_once(
        listener,
        response(
            "202 Accepted",
            "Content-Type: application/json\r\n",
            r#"{"queued":true}"#,
        ),
    );
    let mut arguments = arguments("42");
    arguments.insert("data".into(), json!({"marker":"approved fixture"}));
    assert_eq!(
        live.call_tool("fetch", arguments, &call)
            .await
            .unwrap()
            .structured_content
            .unwrap()["queued"],
        true
    );
    let (_, request) = reply.await.unwrap();
    assert!(request.starts_with("POST /items/42 HTTP/1.1\r\n"));
    assert!(request.contains("content-type: application/json\r\n"));
    assert!(request.ends_with("\r\n\r\n{\"marker\":\"approved fixture\"}"));
}

#[tokio::test]
async fn an_approved_write_is_sent_once_and_reports_an_unknown_effect_on_redirect() {
    let listener = bind().await;
    let endpoint = origin(&listener);
    let route = RestRoute {
        origin: endpoint.parse().unwrap(),
        method: mcp_policy::HttpMethod::Post,
        path: "/items/next".into(),
    };
    let runtime = runtime(
        manifest(&endpoint, "POST", "/items/{id}", true),
        true,
        vec![route],
    );
    let call = ExecutionContext::new(&LOCAL, CancellationToken::new());
    let reply = response_once(
        listener,
        response("307 Temporary Redirect", "Location: /items/next\r\n", ""),
    );
    let error = runtime
        .call_tool("fetch", arguments("start"), &call)
        .await
        .unwrap_err();
    assert_eq!(error.denial, Some(DenyReason::RedirectDenied));
    assert_eq!(error.effect, Effect::Unknown);
    let (listener, request) = reply.await.unwrap();
    assert!(request.starts_with("POST /items/start HTTP/1.1\r\n"));
    quiet(&listener).await;
}

#[tokio::test]
async fn redirects_are_default_off_and_only_explicit_same_origin_reads_follow() {
    let listener = bind().await;
    let outside = bind().await;
    let endpoint = origin(&listener);
    let document = manifest(&endpoint, "GET", "/items/{id}", true);
    let call = ExecutionContext::new(&LOCAL, CancellationToken::new());
    let default_deny = runtime(document.clone(), true, vec![]);
    let reply = response_once(
        listener,
        response("302 Found", "Location: /items/allowed\r\n", ""),
    );
    let denied = default_deny
        .call_tool("fetch", arguments("start"), &call)
        .await
        .unwrap_err();
    assert_eq!(denied.denial, Some(DenyReason::RedirectDenied));
    let (listener, _) = reply.await.unwrap();
    quiet(&listener).await;

    let approved = RestRoute {
        origin: endpoint.parse().unwrap(),
        method: mcp_policy::HttpMethod::Get,
        path: "/items/allowed".into(),
    };
    let live = runtime(document, true, vec![approved]);
    let reply = response_once(
        listener,
        response(
            "302 Found",
            &format!("Location: {}/items/allowed\r\n", origin(&outside)),
            "",
        ),
    );
    let denied = live
        .call_tool("fetch", arguments("start"), &call)
        .await
        .unwrap_err();
    assert_eq!(denied.denial, Some(DenyReason::RedirectDenied));
    let (listener, _) = reply.await.unwrap();
    quiet(&outside).await;

    let socket = tokio::spawn(async move {
        let (mut initial, _) = listener.accept().await.unwrap();
        assert!(
            read_request(&mut initial)
                .await
                .starts_with("GET /items/start HTTP/1.1\r\n")
        );
        initial
            .write_all(response("307 Temporary Redirect", "Location: allowed\r\n", "").as_bytes())
            .await
            .unwrap();
        drop(initial);
        let (mut redirected, _) = listener.accept().await.unwrap();
        assert!(
            read_request(&mut redirected)
                .await
                .starts_with("GET /items/allowed HTTP/1.1\r\n")
        );
        redirected
            .write_all(response("200 OK", "", "followed").as_bytes())
            .await
            .unwrap();
    });
    assert_eq!(
        live.call_tool("fetch", arguments("start"), &call)
            .await
            .unwrap()
            .content[0]
            .text,
        "followed"
    );
    socket.await.unwrap();
}

#[tokio::test]
async fn errors_and_streaming_limits_are_bounded_and_do_not_publish_upstream_data() {
    let listener = bind().await;
    let endpoint = origin(&listener);
    let document = manifest(&endpoint, "GET", "/items/{id}", true);
    let call = ExecutionContext::new(&LOCAL, CancellationToken::new());
    let live = runtime(document, true, vec![]);
    let reply = response_once(
        listener,
        response(
            "500 Internal Server Error",
            &format!("X-Error: {CANARY}\r\n"),
            CANARY,
        ),
    );
    let failed = live
        .call_tool("fetch", arguments("42"), &call)
        .await
        .unwrap_err();
    assert_eq!(failed.kind, ErrorKind::AdapterFailed);
    assert_eq!(failed.effect, Effect::None);
    assert_clean(&failed);
    let (listener, _) = reply.await.unwrap();

    let limited =
        ExecutionContext::new(&LOCAL, CancellationToken::new()).with_limits(LimitOverrides {
            max_output_bytes: NonZeroU64::new(12),
            ..Default::default()
        });
    let reply = response_once(listener, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\n12345678\r\n8\r\nabcdefgh\r\n0\r\n\r\n".into());
    let failed = live
        .call_tool("fetch", arguments("42"), &limited)
        .await
        .unwrap_err();
    assert_eq!(failed.kind, ErrorKind::OutputLimit);
    let (listener, _) = reply.await.unwrap();

    let reply = response_once(
        listener,
        response("200 OK", "Content-Type: application/json\r\n", CANARY),
    );
    let failed = live
        .call_tool("fetch", arguments("42"), &call)
        .await
        .unwrap_err();
    assert_eq!(failed.kind, ErrorKind::AdapterFailed);
    assert_clean(&failed);
    let (listener, _) = reply.await.unwrap();

    let head = runtime(
        manifest(&endpoint, "HEAD", "/items/{id}", true),
        true,
        vec![],
    );
    let reply = response_once(listener, "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 12345678\r\nConnection: close\r\n\r\n".into());
    assert_eq!(
        head.call_tool("fetch", arguments("42"), &call)
            .await
            .unwrap()
            .content[0]
            .text,
        ""
    );
    reply.await.unwrap();
}

#[tokio::test]
async fn deadline_and_cancellation_close_sockets_and_leave_runtime_usable() {
    let listener = bind().await;
    let endpoint = origin(&listener);
    let runtime = runtime(
        manifest(&endpoint, "GET", "/items/{id}", true),
        true,
        vec![],
    );
    let local = LOCAL;
    let timed =
        ExecutionContext::new(&local, CancellationToken::new()).with_limits(LimitOverrides {
            timeout_ms: NonZeroU64::new(250),
            ..Default::default()
        });
    let (call, listener) = tokio::join!(
        runtime.call_tool("fetch", arguments("timeout"), &timed),
        async {
            let (mut socket, _) = tokio::time::timeout(WAIT, listener.accept())
                .await
                .unwrap()
                .unwrap();
            assert!(
                read_request(&mut socket)
                    .await
                    .starts_with("GET /items/timeout")
            );
            assert_closed(&mut socket).await;
            listener
        }
    );
    assert_eq!(call.unwrap_err().kind, ErrorKind::TimedOut);
    let token = CancellationToken::new();
    let cancelled = ExecutionContext::new(&local, token.clone());
    let (call, listener) = tokio::join!(
        runtime.call_tool("fetch", arguments("cancel"), &cancelled),
        async {
            let (mut socket, _) = tokio::time::timeout(WAIT, listener.accept())
                .await
                .unwrap()
                .unwrap();
            assert!(
                read_request(&mut socket)
                    .await
                    .starts_with("GET /items/cancel")
            );
            token.cancel();
            assert_closed(&mut socket).await;
            listener
        }
    );
    assert_eq!(call.unwrap_err().kind, ErrorKind::Cancelled);
    let reply = response_once(listener, response("200 OK", "", "ready"));
    let call = ExecutionContext::new(&local, CancellationToken::new());
    assert_eq!(
        runtime
            .call_tool("fetch", arguments("ready"), &call)
            .await
            .unwrap()
            .content[0]
            .text,
        "ready"
    );
    reply.await.unwrap();
}

async fn assert_closed(socket: &mut TcpStream) {
    match tokio::time::timeout(WAIT, socket.read(&mut [0; 1]))
        .await
        .unwrap()
    {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
            ) => {}
        result => panic!("connection remained open: {result:?}"),
    }
}

#[tokio::test]
async fn plaintext_credential_bindings_are_editable_but_cannot_start() {
    let listener = bind().await;
    let mut document = manifest(&origin(&listener), "GET", "/items/{id}", true);
    document["secretRefs"] = json!([{"id":"api_key","purpose":"Fixture"}]);
    document["bindings"][0]["headersFromSecrets"] = json!({"Authorization":"api_key"});
    let manifest = validated(document);
    assert!(matches!(
        StarterAdapter::new(
            manifest.document().project_id,
            runtime_bindings(&manifest),
            LocalBindings::default(),
        ),
        Err(SetupError::CredentialRequiresHttps)
    ));
    quiet(&listener).await;
}
