use std::future::pending;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use builder_project::{parse_manifest, runtime_definition};
use mcp_policy::{
    ApprovedGrant, CallerGrant, CallerIdentity, Effect, HttpMethod, HttpOrigin, OperatorPolicy,
    VerifiedTarget,
};
use mcp_runtime::{
    Adapter, AdapterContext, AdapterError, AdapterOutput, BoxFuture, CancellationToken, Invocation,
    InvocationInput, PreparationContext, PreparedCall, PreparedTarget, Progress, Runtime,
};
use mcp_transport::http::{LoopbackOptions, LoopbackProfile, bind_loopback};
use serde_json::{Value, json};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, Notify};

const OPERATOR: &str = "wire-test-operator";
const WAIT: Duration = Duration::from_secs(4);

pub(super) struct Server {
    pub address: SocketAddr,
    fixture: Arc<Fixture>,
    started: Mutex<UnboundedReceiver<String>>,
    ended: Mutex<UnboundedReceiver<String>>,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Server {
    pub(super) async fn start(
        profile: LoopbackProfile,
        legacy_idle_timeout: Option<Duration>,
    ) -> Self {
        let (start_signal, started) = mpsc::unbounded_channel();
        let (end_signal, ended) = mpsc::unbounded_channel();
        let fixture = Arc::new(Fixture {
            preparations: AtomicUsize::new(0),
            effects: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            sibling_release: Notify::new(),
            start_signal,
            end_signal,
        });
        let input = include_bytes!("../../../builder-project/tests/fixtures/catalog-v1.json");
        let mut manifest: Value = serde_json::from_slice(input).unwrap();
        manifest["capabilities"][0]["timeoutMs"] = json!(2500);
        manifest["capabilities"][0]["outputSchema"] = json!({
            "type":"object", "properties":{"id":{"type":"string"}},
            "required":["id"], "additionalProperties":false
        });
        manifest["policy"]["grants"][0]["limits"] = json!({
            "timeoutMs":2500, "maxOutputBytes":1048576
        });
        manifest["policy"]["grants"].as_array_mut().unwrap().push(json!({
            "id":"guide_read", "capabilityId":"catalog_guide","operation":"file.read",
            "target":{"kind":"fileRead","rootRef":"catalog_docs","relativePathTemplate":"guide.txt"}
        }));
        let manifest = parse_manifest(&serde_json::to_vec(&manifest).unwrap()).unwrap();
        let definition = runtime_definition(&manifest);
        let grants = &definition.requested_policy.grants;
        let operator = OperatorPolicy {
            approved_grants: grants.iter().cloned().map(ApprovedGrant::new).collect(),
            approved_prompt_ids: ["review_item".to_owned()].into(),
            caller_grants: vec![CallerGrant {
                identity: CallerIdentity::Local {
                    operator: OPERATOR.into(),
                },
                capability_ids: grants
                    .iter()
                    .map(|grant| grant.capability_id.clone())
                    .chain(["review_item".to_owned()])
                    .collect(),
                grant_ids: grants.iter().map(|grant| grant.id.clone()).collect(),
                required_scopes: Default::default(),
                limits: Default::default(),
            }],
            ..Default::default()
        };
        let runtime = Arc::new(Runtime::new(definition, operator, fixture.clone()).unwrap());
        let mut options = LoopbackOptions {
            profile,
            ..Default::default()
        };
        if let Some(idle) = legacy_idle_timeout {
            options.legacy_idle_timeout = idle;
        }
        let shutdown = CancellationToken::new();
        let (address, server) = bind_loopback(runtime, OPERATOR.into(), options, shutdown.clone())
            .await
            .unwrap();
        assert!(address.ip().is_loopback());
        assert_ne!(address.port(), 0);
        Self {
            address,
            fixture,
            started: Mutex::new(started),
            ended: Mutex::new(ended),
            shutdown,
            task: tokio::spawn(server),
        }
    }

    pub(super) fn preparations(&self) -> usize {
        self.fixture.preparations.load(Ordering::SeqCst)
    }

    pub(super) fn effects(&self) -> usize {
        self.fixture.effects.load(Ordering::SeqCst)
    }

    pub(super) fn active(&self) -> usize {
        self.fixture.active.load(Ordering::SeqCst)
    }

    pub(super) fn release_sibling(&self) {
        self.fixture.sibling_release.notify_one();
    }

    pub(super) async fn stays_active(&self) -> bool {
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.active() > 0
    }

    pub(super) async fn started(&self) -> String {
        tokio::time::timeout(WAIT, self.started.lock().await.recv())
            .await
            .expect("fixture call did not start")
            .unwrap()
    }

    pub(super) async fn ended(&self) -> String {
        tokio::time::timeout(WAIT, self.ended.lock().await.recv())
            .await
            .expect("fixture call did not end")
            .unwrap()
    }

    pub(super) async fn stop(mut self) {
        self.shutdown.cancel();
        tokio::time::timeout(WAIT, &mut self.task)
            .await
            .expect("server did not shut down")
            .unwrap()
            .unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.task.abort();
    }
}

struct Fixture {
    preparations: AtomicUsize,
    effects: AtomicUsize,
    active: AtomicUsize,
    sibling_release: Notify,
    start_signal: UnboundedSender<String>,
    end_signal: UnboundedSender<String>,
}

impl Adapter for Fixture {
    fn prepare<'a>(
        &'a self,
        invocation: Invocation<'a>,
        _context: &'a PreparationContext,
    ) -> BoxFuture<'a, Result<Box<dyn PreparedCall + 'a>, AdapterError>> {
        Box::pin(async move {
            self.preparations.fetch_add(1, Ordering::SeqCst);
            let target = match invocation.input {
                InvocationInput::Tool(arguments) => {
                    let id = arguments["id"]
                        .as_str()
                        .expect("validated fixture tool arguments");
                    let path = if id == "denied" {
                        "/outside/denied".into()
                    } else {
                        format!("/items/{id}")
                    };
                    Target::Tool {
                        origin: "https://catalog.example.test".parse().unwrap(),
                        id,
                        path,
                    }
                }
                InvocationInput::Resource { variables, .. } => {
                    let path = variables
                        .get("id")
                        .map_or_else(|| "guide.txt".into(), |id| format!("notes/{id}.txt"));
                    Target::File(path)
                }
            };
            Ok(Box::new(Call {
                fixture: self,
                target,
            }) as Box<dyn PreparedCall>)
        })
    }
}

struct Call<'a> {
    fixture: &'a Fixture,
    target: Target<'a>,
}

enum Target<'a> {
    Tool {
        origin: HttpOrigin,
        id: &'a str,
        path: String,
    },
    File(String),
}

impl PreparedCall for Call<'_> {
    fn target(&self) -> PreparedTarget<'_> {
        PreparedTarget::Verified(match &self.target {
            Target::Tool { origin, path, .. } => VerifiedTarget::Rest {
                origin,
                method: HttpMethod::Get,
                path,
                peer: "93.184.215.14".parse().unwrap(),
                redirected: false,
            },
            Target::File(path) => VerifiedTarget::FileRead {
                root_ref: "catalog_docs",
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
            self.fixture.effects.fetch_add(1, Ordering::SeqCst);
            let output = match self.target {
                Target::Tool {
                    id: id @ ("wait-progress" | "wait-quiet" | "wait-sibling"),
                    ..
                } => {
                    self.fixture.active.fetch_add(1, Ordering::SeqCst);
                    let _active = Active {
                        fixture: self.fixture,
                        id,
                    };
                    self.fixture.start_signal.send(id.to_owned()).unwrap();
                    if id == "wait-progress" {
                        context
                            .report_progress(Progress {
                                progress: 1.0,
                                total: Some(2.0),
                                message: None,
                            })
                            .await;
                    }
                    if id != "wait-sibling" {
                        return pending().await;
                    }
                    self.fixture.sibling_release.notified().await;
                    AdapterOutput::Tool {
                        text: None,
                        structured_content: Some([("id".into(), json!(id))].into_iter().collect()),
                        effect: Effect::None,
                    }
                }
                Target::Tool { id, .. } => AdapterOutput::Tool {
                    text: (id == "wide").then(|| "x".repeat(1024 * 1024 - 512)),
                    structured_content: Some(
                        [(
                            "id".into(),
                            if id == "invalid-output" {
                                json!(42)
                            } else {
                                json!(id)
                            },
                        )]
                        .into_iter()
                        .collect(),
                    ),
                    effect: Effect::None,
                },
                Target::File(path) => AdapterOutput::Resource { text: path },
            };
            Ok(output)
        })
    }
}

struct Active<'a> {
    fixture: &'a Fixture,
    id: &'a str,
}

impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.fixture.active.fetch_sub(1, Ordering::SeqCst);
        let _ = self.fixture.end_signal.send(self.id.to_owned());
    }
}
