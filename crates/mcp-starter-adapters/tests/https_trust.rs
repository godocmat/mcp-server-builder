use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use builder_project::{parse_manifest, runtime_bindings, runtime_definition};
use mcp_policy::{ApprovedGrant, CallerGrant, CallerIdentity, Effect, HttpOrigin, OperatorPolicy};
use mcp_runtime::{
    BoxFuture, CancellationToken, ErrorKind, ExecutionContext, Principal, Runtime, RuntimeError,
    ToolResult,
};
use mcp_starter_adapters::{
    LocalBindings, SecretError, SecretRequest, SecretResolver, StarterAdapter,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{AlertDescription, ServerConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use uuid::Uuid;
use zeroize::Zeroizing;

const OPERATOR: &str = "https-fixture-operator";
const LOCAL: Principal<'static> = Principal::Local { operator: OPERATOR };
const PROJECT_ID: Uuid = Uuid::from_u128(0x8d1e_a3a7_18c4_4e36_8a8c_6e84_af79_eed0);
const WAIT: Duration = Duration::from_secs(12);
const FETCH: &str = "fetch";
const REFERENCE: &str = "fixture_auth";
const TOKEN: &str = "synthetic-https-fixture-canary";
const AUTHORIZATION: &str = "Bearer synthetic-https-fixture-canary";
const CA: &[u8] = include_bytes!("https_trust_fixtures/test-ca.pem");
const VALID_LEAF: &[u8] = include_bytes!("https_trust_fixtures/valid-localhost.pem");
const WRONG_HOST_LEAF: &[u8] = include_bytes!("https_trust_fixtures/wrong-hostname.pem");
const EXPIRED_LEAF: &[u8] = include_bytes!("https_trust_fixtures/expired-localhost.pem");
const LEAF_KEY: &[u8] = include_bytes!("https_trust_fixtures/test-only-leaf-key.pem");

struct Secrets {
    origin: HttpOrigin,
    lookups: AtomicUsize,
    failure: Option<SecretError>,
}

impl SecretResolver for Secrets {
    fn resolve<'a>(
        &'a self,
        request: SecretRequest<'a>,
    ) -> BoxFuture<'a, Result<Zeroizing<String>, SecretError>> {
        Box::pin(async move {
            self.lookups.fetch_add(1, Ordering::Relaxed);
            assert_eq!(request.project_id, PROJECT_ID);
            assert!(matches!(
                request.principal,
                Principal::Local { operator } if *operator == OPERATOR
            ));
            assert_eq!(request.capability_id, FETCH);
            assert_eq!(request.binding_id, "fetch_rest");
            assert_eq!(request.origin, &self.origin);
            assert_eq!(request.reference, REFERENCE);
            if let Some(error) = self.failure {
                return Err(error);
            }
            Ok(Zeroizing::new(AUTHORIZATION.into()))
        })
    }
}

fn runtime(
    routes: &[(&str, &HttpOrigin)],
    trusted_origin: Option<&HttpOrigin>,
    failure: Option<SecretError>,
) -> (Runtime, Arc<Secrets>) {
    let mut capabilities = Vec::new();
    let mut bindings = Vec::new();
    let mut grants = Vec::new();
    for &(tool, origin) in routes {
        let binding = format!("{tool}_rest");
        capabilities.push(json!({
            "id": tool, "kind": "tool", "name": tool, "description": "Read the HTTPS fixture",
            "inputSchema": {"type": "object", "additionalProperties": false},
            "outputSchema": {"type": "object"}, "bindingId": binding,
        }));
        bindings.push(json!({
            "id": binding, "kind": "rest", "origin": origin, "method": "GET",
            "pathTemplate": "/health", "pathParams": {}, "headersFromSecrets": {"Authorization": REFERENCE},
        }));
        grants.push(json!({
            "id": tool, "capabilityId": tool, "operation": "http.read",
            "target": {
                "kind": "rest", "origin": origin, "method": "GET", "pathTemplate": "/health",
                "allowedPeerRanges": ["127.0.0.1/32", "::1/128"],
            },
        }));
    }
    let document = json!({
        "format": "mcp-server-builder.project", "schemaVersion": 1,
        "projectId": PROJECT_ID, "name": "HTTPS fixture", "transport": "stdio",
        "server": {"name": "https-fixture", "version": "0.1.0"},
        "capabilities": capabilities, "bindings": bindings, "policy": {"grants": grants},
        "secretRefs": [{"id": REFERENCE, "purpose": "Authorize fixture requests"}], "scenarios": [],
    });
    let manifest = parse_manifest(&serde_json::to_vec(&document).unwrap()).unwrap();
    let definition = runtime_definition(&manifest);
    let requested = &definition.requested_policy.grants;
    let operator = OperatorPolicy {
        approved_grants: requested.iter().cloned().map(ApprovedGrant::new).collect(),
        caller_grants: vec![CallerGrant {
            identity: CallerIdentity::Local {
                operator: OPERATOR.into(),
            },
            capability_ids: requested
                .iter()
                .map(|grant| grant.capability_id.clone())
                .collect(),
            grant_ids: requested.iter().map(|grant| grant.id.clone()).collect(),
            required_scopes: Default::default(),
            limits: Default::default(),
        }],
        ..Default::default()
    };
    let secrets = Arc::new(Secrets {
        origin: routes[0].1.clone(),
        lookups: AtomicUsize::new(0),
        failure,
    });
    let adapter = StarterAdapter::new(
        manifest.document().project_id,
        runtime_bindings(&manifest),
        LocalBindings {
            private_cas: trusted_origin
                .map(|origin| {
                    let ca = CertificateDer::from_pem_slice(CA).unwrap();
                    (origin.clone(), vec![ca.to_vec()])
                })
                .into_iter()
                .collect(),
            secrets: Some(secrets.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    (
        Runtime::new(definition, operator, Arc::new(adapter)).unwrap(),
        secrets,
    )
}

#[derive(Debug, Eq, PartialEq)]
enum ServerOutcome {
    HttpRequest,
    NoHttpRequest,
    TlsAlert(AlertDescription),
}

struct HttpsFixture {
    ipv4: TcpListener,
    ipv6: Option<TcpListener>,
    origin: HttpOrigin,
    acceptor: TlsAcceptor,
}

impl HttpsFixture {
    async fn bind(leaf: &[u8]) -> Self {
        let cert = CertificateDer::from_pem_slice(leaf).unwrap();
        let key = PrivateKeyDer::from_pem_slice(LEAF_KEY).unwrap();
        let mut config = ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];

        let ipv4 = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = ipv4.local_addr().unwrap().port();
        // Reserve the same port on both loopback families when IPv6 is available.
        let ipv6 = match TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await {
            Ok(listener) => Some(listener),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
                ) =>
            {
                None
            }
            Err(error) => panic!("could not bind the IPv6 HTTPS fixture: {error}"),
        };
        Self {
            ipv4,
            ipv6,
            origin: format!("https://localhost:{port}").parse().unwrap(),
            acceptor: TlsAcceptor::from(Arc::new(config)),
        }
    }

    async fn call(
        &self,
        runtime: &Runtime,
        tool: &str,
    ) -> (ServerOutcome, Result<ToolResult, RuntimeError>) {
        let context = ExecutionContext::new(&LOCAL, CancellationToken::new());
        let (observed, result) = tokio::join!(
            tokio::time::timeout(WAIT, self.serve_once()),
            runtime.call_tool(tool, Default::default(), &context),
        );
        (observed.expect("HTTPS fixture did not complete"), result)
    }

    async fn serve_once(&self) -> ServerOutcome {
        let (socket, _) = if let Some(ipv6) = &self.ipv6 {
            tokio::select! {
                accepted = self.ipv4.accept() => accepted,
                accepted = ipv6.accept() => accepted,
            }
        } else {
            self.ipv4.accept().await
        }
        .unwrap();
        let mut stream = match self.acceptor.accept(socket).await {
            Ok(stream) => stream,
            Err(error) => match error.get_ref().and_then(|source| source.downcast_ref()) {
                Some(rustls::Error::AlertReceived(alert)) => {
                    return ServerOutcome::TlsAlert(*alert);
                }
                _ => panic!("expected a client TLS alert, got {error}"),
            },
        };
        assert_eq!(stream.get_ref().1.server_name(), Some("localhost"));
        assert_eq!(
            stream.get_ref().1.alpn_protocol(),
            Some(b"http/1.1".as_slice())
        );
        let mut first = [0; 1];
        match stream.read(&mut first).await {
            Ok(0) => return ServerOutcome::NoHttpRequest,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                ) =>
            {
                return ServerOutcome::NoHttpRequest;
            }
            Err(error) => panic!("could not read fixture request: {error}"),
            Ok(_) => {}
        }
        let mut request = String::new();
        let input = std::io::Cursor::new(first).chain(&mut stream);
        let mut reader = BufReader::new(input.take(8192));
        loop {
            let start = request.len();
            assert_ne!(
                reader.read_line(&mut request).await.unwrap(),
                0,
                "incomplete HTTP request"
            );
            if &request[start..] == "\r\n" {
                break;
            }
        }
        assert_eq!(request.lines().next(), Some("GET /health HTTP/1.1"));
        assert_eq!(header(&request, "host"), self.origin.authority());
        let authorization = header(&request, "authorization");
        assert_eq!(authorization, AUTHORIZATION);
        let token = authorization.strip_prefix("Bearer ").unwrap();
        let body = json!({
            "public": "loopback",
            "echo": format!("before {token} after"),
            "Authorization": authorization,
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
        ServerOutcome::HttpRequest
    }
}

fn header<'a>(request: &'a str, expected: &str) -> &'a str {
    let mut values = request
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case(expected))
        .map(|(_, value)| value.trim());
    let value = values.next().expect("missing request header");
    assert!(values.next().is_none(), "duplicate {expected} header");
    value
}

fn assert_success(outcome: ServerOutcome, result: ToolResult, secrets: &Secrets) {
    assert_eq!(outcome, ServerOutcome::HttpRequest);
    assert_eq!(secrets.lookups.load(Ordering::Relaxed), 1);
    let expected = json!({"public": "loopback", "echo": "before [REDACTED] after"});
    assert!(!serde_json::to_string(&result).unwrap().contains(TOKEN));
    assert_eq!(result.structured_content.as_ref(), expected.as_object());
    assert_eq!(result.content.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&result.content[0].text).unwrap(),
        expected,
    );
}

fn assert_rejected(
    outcome: ServerOutcome,
    error: RuntimeError,
    alert: AlertDescription,
    secrets: &Secrets,
) {
    // Observing the client alert proves the request reached TLS and failed
    // before the fixture could read any HTTP data.
    assert_eq!(outcome, ServerOutcome::TlsAlert(alert));
    assert_eq!(secrets.lookups.load(Ordering::Relaxed), 0);
    assert_eq!(error.kind, ErrorKind::AdapterFailed);
    assert_eq!(error.effect, Effect::None);
    assert_eq!(error.denial, None);
    assert_eq!(
        error.message(),
        "The adapter could not complete the operation."
    );
    assert_eq!(
        error.to_string(),
        format!("adapter.failed ({})", error.correlation_id)
    );
}

async fn rejected(leaf: &[u8], trust_ca: bool, alert: AlertDescription) {
    let fixture = HttpsFixture::bind(leaf).await;
    let (runtime, secrets) = runtime(
        &[(FETCH, &fixture.origin)],
        trust_ca.then_some(&fixture.origin),
        None,
    );
    let (outcome, result) = fixture.call(&runtime, FETCH).await;
    assert_rejected(outcome, result.unwrap_err(), alert, &secrets);
}

#[tokio::test]
async fn scoped_ca_authenticates_localhost_and_redacts_the_echoed_credential() {
    for host in ["localhost", "localhost."] {
        let mut fixture = HttpsFixture::bind(VALID_LEAF).await;
        fixture.origin = format!("https://{host}:{}", fixture.origin.port())
            .parse()
            .unwrap();
        let (runtime, secrets) = runtime(&[(FETCH, &fixture.origin)], Some(&fixture.origin), None);
        let (outcome, result) = fixture.call(&runtime, FETCH).await;
        assert_success(outcome, result.unwrap(), &secrets);
    }
}

#[tokio::test]
async fn rejects_an_unknown_ca_before_resolving_credentials() {
    rejected(VALID_LEAF, false, AlertDescription::UnknownCA).await;
}

#[tokio::test]
async fn rejects_a_wrong_hostname_even_with_the_scoped_ca() {
    rejected(WRONG_HOST_LEAF, true, AlertDescription::BadCertificate).await;
}

#[tokio::test]
async fn rejects_an_expired_leaf_even_with_the_scoped_ca() {
    rejected(EXPIRED_LEAF, true, AlertDescription::CertificateExpired).await;
}

#[tokio::test]
async fn scoped_ca_does_not_apply_to_a_different_approved_port() {
    let trusted = HttpsFixture::bind(VALID_LEAF).await;
    let other = HttpsFixture::bind(VALID_LEAF).await;
    assert_ne!(trusted.origin.port(), other.origin.port());
    let (runtime, secrets) = runtime(
        &[(FETCH, &trusted.origin), ("other", &other.origin)],
        Some(&trusted.origin),
        None,
    );
    let (outcome, result) = other.call(&runtime, "other").await;
    assert_rejected(
        outcome,
        result.unwrap_err(),
        AlertDescription::UnknownCA,
        &secrets,
    );
    let (outcome, result) = trusted.call(&runtime, FETCH).await;
    assert_success(outcome, result.unwrap(), &secrets);
}

#[tokio::test]
async fn unavailable_credentials_after_tls_send_no_http_request() {
    for (failure, expected) in [
        (SecretError::Missing, ErrorKind::CredentialMissing),
        (SecretError::Unavailable, ErrorKind::CredentialUnavailable),
    ] {
        let fixture = HttpsFixture::bind(VALID_LEAF).await;
        let (runtime, secrets) = runtime(
            &[(FETCH, &fixture.origin)],
            Some(&fixture.origin),
            Some(failure),
        );
        let (outcome, result) = fixture.call(&runtime, FETCH).await;
        assert_eq!(outcome, ServerOutcome::NoHttpRequest);
        assert_eq!(secrets.lookups.load(Ordering::Relaxed), 1);
        let error = result.unwrap_err();
        assert_eq!(error.kind, expected);
        assert_eq!(error.effect, Effect::None);
        assert!(!format!("{error:?} {error} {}", error.message()).contains(TOKEN));
    }
}
