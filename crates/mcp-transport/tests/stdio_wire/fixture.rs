use std::future::pending;
use std::io;
use std::sync::Arc;

use builder_project::{parse_manifest, runtime_definition};
use mcp_policy::{
    ApprovedGrant, CallerGrant, CallerIdentity, Effect, HttpMethod, HttpOrigin, OperatorPolicy,
    VerifiedTarget,
};
use mcp_runtime::{
    Adapter, AdapterContext, AdapterError, AdapterOutput, BoxFuture, CancellationToken, Invocation,
    InvocationInput, PreparationContext, PreparedCall, PreparedTarget, Progress, Runtime,
};
use mcp_transport::stdio::{sdk_diagnostic_filter, serve_stdio};
use serde_json::json;
use tracing_subscriber::{Layer, prelude::*};

const OPERATOR: &str = "wire-test-operator";

pub async fn run() -> io::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(tracing_subscriber::filter::filter_fn(sdk_diagnostic_filter)),
        )
        .init();
    let input = include_bytes!("../../../builder-project/tests/fixtures/catalog-v1.json");
    let mut manifest: serde_json::Value = serde_json::from_slice(input)?;
    manifest["policy"]["grants"]
        .as_array_mut()
        .expect("fixture grants")
        .push(json!({
            "id":"guide_read", "capabilityId":"catalog_guide","operation":"file.read",
            "target":{"kind":"fileRead","rootRef":"catalog_docs","relativePathTemplate":"guide.txt"}
        }));
    let manifest = parse_manifest(&serde_json::to_vec(&manifest)?)
        .map_err(|_| io::Error::other("invalid catalog fixture"))?;
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
    let runtime =
        Runtime::new(definition, operator, Arc::new(Fixture)).map_err(io::Error::other)?;
    serve_stdio(Arc::new(runtime), OPERATOR, CancellationToken::new()).await
}

struct Fixture;

impl Adapter for Fixture {
    fn prepare<'a>(
        &'a self,
        invocation: Invocation<'a>,
        _context: &'a PreparationContext,
    ) -> BoxFuture<'a, Result<Box<dyn PreparedCall + 'a>, AdapterError>> {
        Box::pin(async move {
            let call = match invocation.input {
                InvocationInput::Tool(arguments) => {
                    let id = arguments["id"]
                        .as_str()
                        .expect("validated fixture tool arguments");
                    let path = if id == "denied" {
                        "/outside/denied".into()
                    } else {
                        format!("/items/{id}")
                    };
                    Call::Tool {
                        origin: "https://catalog.example.test"
                            .parse()
                            .expect("fixture origin"),
                        id,
                        path,
                    }
                }
                InvocationInput::Resource { variables, .. } => {
                    let path = variables
                        .get("id")
                        .map_or_else(|| "guide.txt".into(), |id| format!("notes/{id}.txt"));
                    Call::File(path)
                }
            };
            Ok(Box::new(call) as Box<dyn PreparedCall>)
        })
    }
}

enum Call<'a> {
    Tool {
        origin: HttpOrigin,
        id: &'a str,
        path: String,
    },
    File(String),
}

impl PreparedCall for Call<'_> {
    fn target(&self) -> PreparedTarget<'_> {
        PreparedTarget::Verified(match self {
            Self::Tool { origin, path, .. } => VerifiedTarget::Rest {
                origin,
                method: HttpMethod::Get,
                path,
                peer: "93.184.215.14".parse().expect("fixture peer"),
                redirected: false,
            },
            Self::File(path) => VerifiedTarget::FileRead {
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
            let output = match *self {
                Self::Tool { id: "slow", .. } => {
                    context
                        .report_progress(Progress {
                            progress: 1.0,
                            total: Some(2.0),
                            message: Some(super::STDERR_CANARY.into()),
                        })
                        .await;
                    pending::<AdapterOutput>().await
                }
                Self::Tool { id, .. } => AdapterOutput::Tool {
                    text: None,
                    structured_content: Some([("id".into(), json!(id))].into_iter().collect()),
                    effect: Effect::None,
                },
                Self::File(path) => AdapterOutput::Resource { text: path },
            };
            Ok(output)
        })
    }
}
