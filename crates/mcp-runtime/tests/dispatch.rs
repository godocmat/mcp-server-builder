use std::collections::BTreeMap;
use std::future::pending;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mcp_policy::{
    ApprovedGrant, CallerGrant, CallerIdentity, DenyReason, Effect, HttpMethod, HttpOrigin,
    LimitOverrides, Operation, OperatorPolicy, RestRoute, VerifiedTarget,
};
use mcp_runtime::{
    Adapter, AdapterBinding, AdapterContext, AdapterError, AdapterErrorKind, AdapterOutput,
    BoxFuture, CancellationToken, DefinitionErrorKind, ErrorKind, ExecutionContext, Invocation,
    InvocationInput, PreparationContext, PreparedCall, PreparedTarget, Principal, Progress,
    ProgressSink, PromptArgument, PromptDefinition, PromptInfo, PromptTemplateMessage,
    ResourceDefinition, ResourceInfo, ResourceTemplateDefinition, ResourceTemplateInfo, Role,
    Runtime, ServerDefinition, ServerInfo, ToolDefinition, ToolInfo,
};
use serde_json::{Map, Value, json};
use tokio::sync::Notify;

const OPERATOR: &str = "fixture-operator";
const ORIGIN: &str = "https://catalog.example.test";

fn object(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn binding(id: &str, operation: Operation) -> AdapterBinding {
    AdapterBinding {
        id: id.into(),
        operation,
        timeout_ms: None,
    }
}

fn definition() -> ServerDefinition {
    ServerDefinition {
        info: ServerInfo { name: "catalog".into(), version: "0.1.0".into(), description: None, instructions: None },
        requested_policy: serde_json::from_value(json!({"grants": [
            {"id":"lookup", "capabilityId":"lookup", "operation":"http.read", "target":{"kind":"rest", "origin":ORIGIN, "method":"GET", "pathTemplate":"/items/{id}"}},
            {"id":"narrow", "capabilityId":"lookup", "operation":"http.read", "target":{"kind":"rest", "origin":ORIGIN, "method":"GET", "pathTemplate":"/items/narrow"}, "limits":{"timeoutMs":50,"maxOutputBytes":64}},
            {"id":"guide", "capabilityId":"guide", "operation":"file.read", "target":{"kind":"fileRead", "rootRef":"docs", "relativePathTemplate":"guide.txt"}},
            {"id":"note", "capabilityId":"note", "operation":"file.read", "target":{"kind":"fileRead", "rootRef":"docs", "relativePathTemplate":"notes/{id}.txt"}}
        ]})).unwrap(),
        tools: vec![ToolDefinition {
            id: "lookup".into(),
            info: ToolInfo {
                name: "lookup".into(), description: "Look up an item".into(),
                input_schema: json!({"type":"object","properties":{"id":{"$ref":"#/$defs/id"}},"required":["id"],"additionalProperties":false,"$defs":{"id":{"type":"string"}}}),
                output_schema: Some(json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]})),
            },
            binding: binding("rest", Operation::HttpRead),
        }],
        resources: vec![ResourceDefinition {
            id: "guide".into(),
            info: ResourceInfo { name: "Guide".into(), description: "A guide".into(), uri: "catalog://docs/guide".into(), mime_type: "text/plain".into() },
            binding: binding("file", Operation::FileRead),
        }],
        resource_templates: vec![ResourceTemplateDefinition {
            id: "note".into(),
            info: ResourceTemplateInfo { name: "Note".into(), description: "A note".into(), uri_template: "catalog://docs/notes/{id}.txt".into(), mime_type: "text/plain".into() },
            binding: binding("file", Operation::FileRead),
            variables: BTreeMap::from([("id".into(), json!({"type":"string","pattern":"^[0-9]+$"}))]),
        }],
        prompts: vec![PromptDefinition {
            id: "review".into(),
            info: PromptInfo {
                name: "review".into(), description: "Review an item".into(),
                arguments: vec![
                    PromptArgument { name: "id".into(), description: None, required: true },
                    PromptArgument { name: "note".into(), description: None, required: false },
                ],
            },
            messages: vec![PromptTemplateMessage { role: Role::User, text_template: "Review {{item}} {id} {note}".into() }],
        }],
    }
}

fn approvals(definition: &ServerDefinition) -> OperatorPolicy {
    let grants = &definition.requested_policy.grants;
    let prompts: Vec<_> = definition
        .prompts
        .iter()
        .map(|prompt| prompt.id.clone())
        .collect();
    OperatorPolicy {
        approved_grants: grants.iter().cloned().map(ApprovedGrant::new).collect(),
        approved_prompt_ids: prompts.iter().cloned().collect(),
        caller_grants: vec![CallerGrant {
            identity: CallerIdentity::Local {
                operator: OPERATOR.into(),
            },
            capability_ids: grants
                .iter()
                .map(|grant| grant.capability_id.clone())
                .chain(prompts)
                .collect(),
            grant_ids: grants.iter().map(|grant| grant.id.clone()).collect(),
            required_scopes: Default::default(),
            limits: Default::default(),
        }],
        ..OperatorPolicy::default()
    }
}

#[derive(Default)]
struct Fixture {
    prepared: AtomicUsize,
    executed: AtomicUsize,
    started: Notify,
    cancelled_before_drop: Mutex<bool>,
}

impl Adapter for Fixture {
    fn prepare<'a>(
        &'a self,
        invocation: Invocation<'a>,
        _context: &'a PreparationContext,
    ) -> BoxFuture<'a, Result<Box<dyn PreparedCall + 'a>, AdapterError>> {
        Box::pin(async move {
            self.prepared.fetch_add(1, Ordering::Relaxed);
            let call = match invocation.input {
                InvocationInput::Tool(arguments) => {
                    assert_eq!(invocation.binding_id, "rest");
                    let id = arguments["id"].as_str().unwrap();
                    let path = if id == "denied" {
                        "/other/denied".into()
                    } else {
                        format!("/items/{id}")
                    };
                    Call::Tool {
                        route: RestRoute {
                            origin: ORIGIN.parse().unwrap(),
                            method: HttpMethod::Get,
                            path,
                        },
                        id,
                    }
                }
                InvocationInput::Resource { variables, .. } => {
                    assert_eq!(invocation.binding_id, "file");
                    let path = variables
                        .get("id")
                        .map_or_else(|| "guide.txt".into(), |id| format!("notes/{id}.txt"));
                    Call::File(path)
                }
            };
            Ok(Box::new(Prepared {
                fixture: self,
                call,
            }) as Box<dyn PreparedCall>)
        })
    }
}

enum Call<'a> {
    Tool { route: RestRoute, id: &'a str },
    File(String),
}

struct Prepared<'a> {
    fixture: &'a Fixture,
    call: Call<'a>,
}

fn rest_target<'a>(origin: &'a HttpOrigin, path: &'a str) -> VerifiedTarget<'a> {
    VerifiedTarget::Rest {
        origin,
        method: HttpMethod::Get,
        path,
        peer: "93.184.215.14".parse().unwrap(),
        redirected: false,
    }
}

impl PreparedCall for Prepared<'_> {
    fn target(&self) -> PreparedTarget<'_> {
        PreparedTarget::Verified(match &self.call {
            Call::Tool { route, id } if id.starts_with("preflight-") => {
                return PreparedTarget::RestRoute(route);
            }
            Call::Tool { route, .. } => rest_target(&route.origin, &route.path),
            Call::File(path) => VerifiedTarget::FileRead {
                root_ref: "docs",
                relative_path: path,
            },
        })
    }

    fn execute<'a>(
        self: Box<Self>,
        context: &'a AdapterContext<'_>,
    ) -> BoxFuture<'a, Result<AdapterOutput, AdapterError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            self.fixture.executed.fetch_add(1, Ordering::Relaxed);
            let id = match &self.call {
                Call::File(path) => return Ok(AdapterOutput::Resource { text: path.clone() }),
                Call::Tool { id, .. } => *id,
            };
            if let Call::Tool { route, .. } = &self.call
                && matches!(id, "preflight-match" | "preflight-mismatch")
            {
                let path = if id == "preflight-match" {
                    &route.path
                } else {
                    "/items/different"
                };
                let decision = context.authorize_target(&rest_target(&route.origin, path));
                if id == "preflight-match" {
                    assert!(decision.is_ok());
                } else {
                    assert_eq!(decision.unwrap_err(), DenyReason::InvalidTarget);
                }
            }
            if matches!(id, "slow" | "retarget-timeout") {
                let _drop = OnDrop {
                    cancelled: context.cancellation(),
                    observed: &self.fixture.cancelled_before_drop,
                };
                if let Call::Tool { route, .. } = &self.call
                    && id == "retarget-timeout"
                {
                    let before = context.deadline();
                    let after = context
                        .authorize_target(&rest_target(&route.origin, "/items/narrow"))
                        .unwrap();
                    assert!(after.deadline < before);
                    assert_eq!(context.limits().max_output_bytes(), 64);
                }
                self.fixture.started.notify_one();
                pending::<()>().await;
            }
            if let Call::Tool { route, .. } = &self.call
                && id == "retarget-denied"
            {
                assert_eq!(
                    context
                        .authorize_target(&rest_target(&route.origin, "/outside/denied"))
                        .unwrap_err(),
                    DenyReason::TargetDenied
                );
                return Err(AdapterError {
                    kind: AdapterErrorKind::Failed,
                    effect: Effect::None,
                });
            }
            if id == "progress" {
                for value in [1.0, 1.0, f64::NAN, 2.0] {
                    context
                        .report_progress(Progress {
                            progress: value,
                            total: Some(2.0),
                            message: Some("FIXTURE_CANARY".into()),
                        })
                        .await;
                }
                tokio::time::sleep(Duration::from_millis(55)).await;
                context
                    .report_progress(Progress {
                        progress: 2.0,
                        total: Some(2.0),
                        message: None,
                    })
                    .await;
            }
            let output = if id == "bad-output" {
                json!({"id":42})
            } else {
                json!({"id":id})
            };
            Ok(AdapterOutput::Tool {
                text: None,
                structured_content: Some(object(output)),
                effect: if id == "bad-output" {
                    Effect::Committed
                } else {
                    Effect::None
                },
            })
        })
    }
}

#[tokio::test]
async fn preflight_requires_peer_authorization_for_the_same_route() {
    let (runtime, _) = runtime();
    let operator = Principal::Local { operator: OPERATOR };
    let context = ExecutionContext::new(&operator, CancellationToken::new());
    for id in ["preflight-mismatch", "preflight-no-peer"] {
        let denied = runtime
            .call_tool("lookup", object(json!({"id":id})), &context)
            .await
            .unwrap_err();
        assert_eq!(denied.kind, ErrorKind::Denied);
        assert_eq!(denied.denial, Some(DenyReason::InvalidTarget));
    }
    let output = runtime
        .call_tool("lookup", object(json!({"id":"preflight-match"})), &context)
        .await
        .unwrap();
    assert_eq!(output.structured_content.unwrap()["id"], "preflight-match");
}

struct OnDrop<'a> {
    cancelled: &'a CancellationToken,
    observed: &'a Mutex<bool>,
}

impl Drop for OnDrop<'_> {
    fn drop(&mut self) {
        *self.observed.lock().unwrap() = self.cancelled.is_cancelled();
    }
}

fn runtime() -> (Runtime, Arc<Fixture>) {
    let definition = definition();
    let operator = approvals(&definition);
    let fixture = Arc::new(Fixture::default());
    (
        Runtime::new(definition, operator, fixture.clone()).unwrap(),
        fixture,
    )
}

#[tokio::test(start_paused = true)]
async fn checks_visibility_input_target_and_output_at_distinct_boundaries() {
    let (runtime, fixture) = runtime();
    let operator = Principal::Local { operator: OPERATOR };
    let context = ExecutionContext::new(&operator, CancellationToken::new());
    let anonymous = ExecutionContext::new(&Principal::Anonymous, CancellationToken::new());
    assert!(!runtime.capabilities(&Principal::Anonymous).tools);
    assert!(
        runtime
            .list_tools(&Principal::Anonymous, None)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(runtime.capabilities(&operator).tools);
    assert_eq!(
        runtime.list_tools(&operator, None).unwrap().items[0].name,
        "lookup"
    );

    let hidden = runtime
        .call_tool("lookup", object(json!({"id":42})), &anonymous)
        .await
        .unwrap_err();
    assert_eq!(hidden.kind, ErrorKind::UnknownTool);
    let invalid = runtime
        .call_tool("lookup", object(json!({"id":42})), &context)
        .await
        .unwrap_err();
    assert_eq!(invalid.kind, ErrorKind::InvalidArguments);
    assert_eq!(invalid.correlation_id, context.correlation_id());
    assert_eq!(fixture.prepared.load(Ordering::Relaxed), 0);

    let denied = runtime
        .call_tool("lookup", object(json!({"id":"denied"})), &context)
        .await
        .unwrap_err();
    assert_eq!(denied.kind, ErrorKind::Denied);
    assert_eq!(denied.denial, Some(DenyReason::TargetDenied));
    assert_eq!(denied.effect, Effect::None);
    assert_eq!(fixture.prepared.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.executed.load(Ordering::Relaxed), 0);

    let result = runtime
        .call_tool("lookup", object(json!({"id":"42"})), &context)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"content":[{"type":"text","text":"{\"id\":\"42\"}"}],"structuredContent":{"id":"42"}})
    );
    let invalid = runtime
        .call_tool("lookup", object(json!({"id":"bad-output"})), &context)
        .await
        .unwrap_err();
    assert_eq!(invalid.kind, ErrorKind::InvalidOutput);
    assert_eq!(invalid.effect, Effect::Committed);

    let changed = runtime
        .call_tool("lookup", object(json!({"id":"retarget-denied"})), &context)
        .await
        .unwrap_err();
    assert_eq!(changed.kind, ErrorKind::Denied);
    assert_eq!(changed.denial, Some(DenyReason::TargetDenied));
    let oversized = runtime
        .call_tool("lookup", object(json!({"id":"narrow"})), &context)
        .await
        .unwrap_err();
    assert_eq!(oversized.kind, ErrorKind::OutputLimit);
}

#[tokio::test]
async fn matches_resources_and_templates_and_bounds_prompt_rendering() {
    let (runtime, fixture) = runtime();
    let operator = Principal::Local { operator: OPERATOR };
    let context = ExecutionContext::new(&operator, CancellationToken::new());
    assert!(runtime.capabilities(&operator).resources);
    assert_eq!(
        runtime.list_resources(&operator, None).unwrap().items[0].uri,
        "catalog://docs/guide"
    );
    assert_eq!(
        runtime
            .list_resource_templates(&operator, None)
            .unwrap()
            .items[0]
            .uri_template,
        "catalog://docs/notes/{id}.txt"
    );
    assert_eq!(
        runtime
            .read_resource("CATALOG://DOCS/guide", &context)
            .await
            .unwrap()
            .contents[0]
            .text,
        "guide.txt"
    );
    assert_eq!(
        runtime
            .read_resource("catalog://docs/notes/%34%32.txt", &context)
            .await
            .unwrap()
            .contents[0]
            .text,
        "notes/42.txt"
    );
    for uri in [
        "catalog://docs/notes/not-numeric.txt",
        "catalog://docs/notes/%2F.txt",
        "catalog://docs/notes/%252F.txt",
    ] {
        assert_eq!(
            runtime.read_resource(uri, &context).await.unwrap_err().kind,
            ErrorKind::InvalidArguments
        );
    }
    assert_eq!(fixture.prepared.load(Ordering::Relaxed), 2);

    assert!(runtime.capabilities(&operator).prompts);
    assert_eq!(
        runtime.list_prompts(&operator, None).unwrap().items[0].name,
        "review"
    );
    let arguments = BTreeMap::from([("id".into(), "42 {note}".into())]);
    let result = runtime
        .get_prompt("review", arguments.clone(), &context)
        .await
        .unwrap();
    assert_eq!(result.messages[0].content.text, "Review {item} 42 {note} ");
    assert_eq!(
        runtime
            .get_prompt("review", BTreeMap::new(), &context)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArguments
    );
    let bounded = context.with_limits(LimitOverrides {
        max_output_bytes: NonZeroU64::new(16),
        ..Default::default()
    });
    assert_eq!(
        runtime
            .get_prompt("review", arguments, &bounded)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::OutputLimit
    );
}

#[derive(Default)]
struct RecordingProgress(Mutex<Vec<Progress>>);

impl ProgressSink for RecordingProgress {
    fn send(&self, event: Progress) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.0.lock().unwrap().push(event) })
    }
}

#[tokio::test]
async fn cancellation_is_per_call_and_progress_is_ordered_and_sanitized() {
    let (runtime, fixture) = runtime();
    let operator = Principal::Local { operator: OPERATOR };
    let cancel = CancellationToken::new();
    cancel.cancel();
    let early = ExecutionContext::new(&operator, cancel);
    let result = runtime
        .call_tool("lookup", object(json!({"id":"42"})), &early)
        .await
        .unwrap_err();
    assert_eq!(
        (result.kind, result.effect),
        (ErrorKind::Cancelled, Effect::None)
    );
    assert_eq!(fixture.prepared.load(Ordering::Relaxed), 0);

    let cancellation = CancellationToken::new();
    let slow = ExecutionContext::new(&operator, cancellation.clone());
    let progress = RecordingProgress::default();
    let fast = ExecutionContext::new(&operator, CancellationToken::new()).with_progress(&progress);
    let (stopped, completed) = tokio::join!(
        runtime.call_tool("lookup", object(json!({"id":"slow"})), &slow),
        async {
            fixture.started.notified().await;
            cancellation.cancel();
            runtime
                .call_tool("lookup", object(json!({"id":"progress"})), &fast)
                .await
        },
    );
    let stopped = stopped.unwrap_err();
    assert_eq!(
        (stopped.kind, stopped.effect),
        (ErrorKind::Cancelled, Effect::Unknown)
    );
    assert!(*fixture.cancelled_before_drop.lock().unwrap());
    assert!(completed.is_ok());
    let events = progress.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.progress)
            .collect::<Vec<_>>(),
        [1.0, 2.0]
    );
    assert!(events.iter().all(|event| event.message.is_none()));
}

#[tokio::test(start_paused = true)]
async fn a_target_recheck_can_shorten_the_active_timer() {
    let (runtime, fixture) = runtime();
    let operator = Principal::Local { operator: OPERATOR };
    let context = ExecutionContext::new(&operator, CancellationToken::new());
    let before = tokio::time::Instant::now();
    let error = runtime
        .call_tool("lookup", object(json!({"id":"retarget-timeout"})), &context)
        .await
        .unwrap_err();
    assert_eq!(
        (error.kind, error.effect),
        (ErrorKind::TimedOut, Effect::Unknown)
    );
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(*fixture.cancelled_before_drop.lock().unwrap());
}

#[test]
fn rejects_ambiguous_uris_and_tool_header_annotations_before_serving() {
    let mut ambiguous = definition();
    ambiguous.resources[0].info.uri = "CATALOG://DOCS/notes/%34%32.txt".into();
    let operator = approvals(&ambiguous);
    let result = Runtime::new(ambiguous, operator, Arc::new(Fixture::default()));
    assert_eq!(
        result.err().unwrap().kind,
        DefinitionErrorKind::AmbiguousResource
    );

    let mut bad_schema = definition();
    bad_schema.tools[0].info.input_schema["properties"]["id"]["x-mcp-header"] = json!("X-Id");
    let operator = approvals(&bad_schema);
    let result = Runtime::new(bad_schema, operator, Arc::new(Fixture::default()));
    assert_eq!(
        result.err().unwrap().kind,
        DefinitionErrorKind::InvalidSchema
    );
}

#[test]
fn pagination_is_stable_and_binds_visible_catalog_and_caller() {
    let mut definition = definition();
    let prototype = definition.prompts[0].clone();
    definition.prompts = (0..40)
        .rev()
        .map(|i| {
            let mut prompt = prototype.clone();
            prompt.id = format!("prompt_{i:02}");
            prompt.info.name = prompt.id.clone();
            prompt
        })
        .collect();
    let mut operator = approvals(&definition);
    let mut second = operator.caller_grants[0].clone();
    second.identity = CallerIdentity::Local {
        operator: "second".into(),
    };
    operator.caller_grants.push(second);
    let mut remote = operator.caller_grants[0].clone();
    remote.identity = CallerIdentity::Remote {
        issuer: "https://issuer.example.test".into(),
        subject: "alice".into(),
        client_id: None,
    };
    remote.required_scopes.insert("prompts:read".into());
    operator.caller_grants.push(remote);
    let runtime = Runtime::new(
        definition.clone(),
        operator.clone(),
        Arc::new(Fixture::default()),
    )
    .unwrap();
    let local = Principal::Local { operator: OPERATOR };
    let first = runtime.list_prompts(&local, None).unwrap();
    assert_eq!(first.items.len(), 32);
    assert_eq!(first.items[0].name, "prompt_00");
    assert_eq!(
        runtime.list_prompts(&local, None).unwrap().next_cursor,
        first.next_cursor
    );
    let cursor = first.next_cursor.as_deref();
    let next = runtime.list_prompts(&local, cursor).unwrap();
    assert_eq!(next.items[0].name, "prompt_32");
    assert_eq!(next.items.last().unwrap().name, "prompt_39");
    assert!(next.next_cursor.is_none());
    assert_eq!(
        runtime.list_tools(&local, cursor).unwrap_err().kind,
        ErrorKind::InvalidCursor
    );
    assert_eq!(
        runtime
            .list_prompts(&Principal::Local { operator: "second" }, cursor)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidCursor
    );
    let replacement = Runtime::new(definition, operator, Arc::new(Fixture::default())).unwrap();
    assert_eq!(
        replacement.list_prompts(&local, cursor).unwrap_err().kind,
        ErrorKind::InvalidCursor
    );

    let remote = |scopes| Principal::Remote {
        issuer: "https://issuer.example.test",
        subject: "alice",
        client_id: None,
        scopes,
        expires_at: std::time::Instant::now() + Duration::from_secs(30),
    };
    let remote_cursor = runtime
        .list_prompts(&remote(&["prompts:read"]), None)
        .unwrap()
        .next_cursor
        .unwrap();
    assert_eq!(
        runtime
            .list_prompts(&remote(&[]), Some(&remote_cursor))
            .unwrap_err()
            .kind,
        ErrorKind::InvalidCursor
    );
}
