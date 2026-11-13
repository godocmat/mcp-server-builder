use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use builder_project::{parse_manifest, runtime_bindings, runtime_definition};
use mcp_policy::{ApprovedGrant, CallerGrant, CallerIdentity, DenyReason, OperatorPolicy};
use mcp_runtime::{CancellationToken, ExecutionContext, Principal, Runtime};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::{LocalBindings, StarterAdapter};

static LOOKUPS: AtomicUsize = AtomicUsize::new(0);

fn controlled_dns(host: &str, port: u16) -> Vec<SocketAddr> {
    assert_eq!(host, "fixture.invalid");
    let lookup = LOOKUPS.fetch_add(1, Ordering::SeqCst);
    let allowed = SocketAddr::from(([127, 0, 0, 1], port));
    if lookup == 1 {
        return vec![allowed, SocketAddr::from(([127, 0, 0, 2], port))];
    }
    vec![allowed]
}

#[tokio::test]
async fn a_denied_route_skips_dns_and_a_changed_answer_cannot_open_a_second_socket() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let origin = format!("http://fixture.invalid:{port}");
    let manifest = json!({
        "format":"mcp-server-builder.project","schemaVersion":1,
        "projectId":"8d1ea3a7-18c4-4e36-8a8c-6e84af79eed0","name":"DNS fixture",
        "server":{"name":"fixture","version":"0.1.0"},"transport":"stdio",
        "capabilities":[{
            "id":"lookup","kind":"tool","name":"lookup","description":"Read fixture",
            "inputSchema":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]},
            "bindingId":"lookup"
        }],
        "bindings":[{
            "id":"lookup","kind":"rest","origin":origin,"method":"GET",
            "pathTemplate":"/items/{id}","pathParams":{"id":"/id"}
        }],
        "policy":{"grants":[{
            "id":"lookup","capabilityId":"lookup","operation":"http.read",
            "target":{"kind":"rest","origin":origin,"method":"GET","pathTemplate":"/items/start","allowedPeerRanges":["127.0.0.1/32"]}
        },{
            "id":"next","capabilityId":"lookup","operation":"http.read",
            "target":{"kind":"rest","origin":origin,"method":"GET","pathTemplate":"/items/next","allowedPeerRanges":["127.0.0.1/32"]}
        }]},
        "secretRefs":[],"scenarios":[]
    });
    let manifest = parse_manifest(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    let definition = runtime_definition(&manifest);
    let grant = ApprovedGrant::new(definition.requested_policy.grants[0].clone());
    let mut next = ApprovedGrant::new(definition.requested_policy.grants[1].clone());
    next.redirects.push(RestRoute {
        origin: origin.parse().unwrap(),
        method: HttpMethod::Get,
        path: "/items/next".into(),
    });
    let operator = OperatorPolicy {
        approved_grants: vec![grant, next],
        caller_grants: vec![CallerGrant {
            identity: CallerIdentity::Local {
                operator: "dns-fixture".into(),
            },
            capability_ids: ["lookup".into()].into(),
            grant_ids: ["lookup".into(), "next".into()].into(),
            required_scopes: Default::default(),
            limits: Default::default(),
        }],
        ..Default::default()
    };
    let mut adapter = StarterAdapter::new(
        manifest.document().project_id,
        runtime_bindings(&manifest),
        LocalBindings::default(),
    )
    .unwrap();
    adapter.rest.resolver = Some(controlled_dns);
    let runtime = Runtime::new(definition, operator, Arc::new(adapter)).unwrap();
    let caller = Principal::Local {
        operator: "dns-fixture",
    };
    let context = ExecutionContext::new(&caller, CancellationToken::new());
    let invoke = |id: &str| {
        runtime.call_tool(
            "lookup",
            Map::from_iter([("id".into(), json!(id))]),
            &context,
        )
    };
    let denied = invoke("outside").await.unwrap_err();
    assert_eq!(denied.denial, Some(DenyReason::TargetDenied));
    assert_eq!(LOOKUPS.load(Ordering::SeqCst), 0);

    for (location, expected_lookups, denial) in [
        ("/items/next", 2, DenyReason::TargetDenied),
        ("/items/start", 3, DenyReason::RedirectDenied),
    ] {
        let response = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let (result, ()) = tokio::join!(invoke("start"), async {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = Vec::new();
            loop {
                request.push(socket.read_u8().await.unwrap());
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
                assert!(request.len() < 4096);
            }
            assert!(request.starts_with(b"GET /items/start HTTP/1.1\r\n"));
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        assert_eq!(result.unwrap_err().denial, Some(denial));
        assert_eq!(LOOKUPS.load(Ordering::SeqCst), expected_lookups);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    }
}
