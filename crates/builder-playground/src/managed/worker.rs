use std::io;
use std::sync::Arc;
use std::time::Duration;

use builder_project::{MAX_MANIFEST_BYTES, ManifestRevision, ValidatedManifest, parse_manifest};
use mcp_runtime::Runtime;
use mcp_starter_adapters::StarterAdapter;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::access::{Access, CredentialValues};
use super::{OPERATOR, PreviewError, PreviewMode};

pub const WORKER_ARG: &str = "--managed-preview-worker";
pub(super) const HANDOFF_VERSION: u32 = 1;
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SETUP_BYTES: usize = 4 * 1024 * 1024;
const MAX_CREDENTIAL_WIRE_BYTES: usize = 1024 * 1024;
const MAGIC: &[u8; 8] = b"MCPBPREV";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Setup<A> {
    version: u32,
    manifest_revision: ManifestRevision,
    mode: PreviewMode,
    access: A,
}

#[repr(u8)]
enum Ack {
    Ready = 0,
    Handoff = 1,
    FileRoot = 2,
    Runtime = 3,
}

pub(super) async fn handoff(
    mut stdin: impl AsyncWrite + Unpin,
    mut stdout: impl AsyncRead + Unpin,
    manifest: &ValidatedManifest,
    mode: &PreviewMode,
    access: &Access,
    credentials: CredentialValues,
    cancellation: &CancellationToken,
) -> Result<(), PreviewError> {
    let setup = serde_json::to_vec(&Setup {
        version: HANDOFF_VERSION,
        manifest_revision: manifest.revision(),
        mode: mode.clone(),
        access,
    })
    .map_err(|_| PreviewError::Handoff)?;
    let mut credential_bytes = Zeroizing::new(Vec::with_capacity(MAX_CREDENTIAL_WIRE_BYTES));
    serde_json::to_writer(&mut *credential_bytes, &credentials)
        .map_err(|_| PreviewError::CredentialUnavailable)?;
    drop(credentials);
    if setup.len() > MAX_SETUP_BYTES || credential_bytes.len() > MAX_CREDENTIAL_WIRE_BYTES {
        return Err(PreviewError::Handoff);
    }
    let write_setup = async {
        stdin.write_all(MAGIC).await?;
        write_block(&mut stdin, &setup).await?;
        write_block(&mut stdin, manifest.bytes()).await?;
        write_block(&mut stdin, &credential_bytes).await?;
        stdin.flush().await?;
        stdout.read_u8().await
    };
    let ack = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(PreviewError::Cancelled),
        result = tokio::time::timeout(HANDOFF_TIMEOUT, write_setup) => {
            result.map_err(|_| PreviewError::Handoff)?.map_err(|_| PreviewError::Handoff)?
        }
    };
    match ack {
        n if n == Ack::Ready as u8 => Ok(()),
        n if n == Ack::FileRoot as u8 => Err(PreviewError::FileRootUnavailable),
        n if n == Ack::Runtime as u8 => Err(PreviewError::Runtime),
        _ => Err(PreviewError::Handoff),
    }
}

async fn write_block(writer: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> io::Result<()> {
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(bytes).await
}

async fn read_block(
    reader: &mut (impl AsyncRead + Unpin),
    max: usize,
) -> Result<Zeroizing<Vec<u8>>, PreviewError> {
    let length = reader.read_u32().await.map_err(|_| PreviewError::Handoff)? as usize;
    if length == 0 || length > max {
        return Err(PreviewError::Handoff);
    }
    let mut bytes = Zeroizing::new(vec![0; length]);
    reader
        .read_exact(&mut bytes)
        .await
        .map_err(|_| PreviewError::Handoff)?;
    Ok(bytes)
}

/// Private worker entry point. The host invokes it before initializing logging.
/// Startup status is a fixed byte on the same pipe that then carries MCP.
pub async fn serve_worker<R, W>(mut stdin: R, mut stdout: W) -> Result<(), PreviewError>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let runtime = tokio::time::timeout(HANDOFF_TIMEOUT, read_runtime(&mut stdin))
        .await
        .map_err(|_| PreviewError::Handoff)
        .and_then(|result| result);
    let ack = match &runtime {
        Ok(_) => Ack::Ready,
        Err(PreviewError::FileRootUnavailable) => Ack::FileRoot,
        Err(PreviewError::Runtime | PreviewError::Scenario(_)) => Ack::Runtime,
        Err(_) => Ack::Handoff,
    };
    stdout
        .write_u8(ack as u8)
        .await
        .map_err(|_| PreviewError::Handoff)?;
    stdout.flush().await.map_err(|_| PreviewError::Handoff)?;
    mcp_transport::stdio::serve_stream(
        stdin,
        stdout,
        Arc::new(runtime?),
        OPERATOR,
        CancellationToken::new(),
    )
    .await
    .map_err(|_| PreviewError::Runtime)
}

async fn read_runtime(reader: &mut (impl AsyncRead + Unpin)) -> Result<Runtime, PreviewError> {
    let mut magic = [0; MAGIC.len()];
    reader
        .read_exact(&mut magic)
        .await
        .map_err(|_| PreviewError::Handoff)?;
    if &magic != MAGIC {
        return Err(PreviewError::Handoff);
    }
    let setup = read_block(reader, MAX_SETUP_BYTES).await?;
    let setup: Setup<Access> = serde_json::from_slice(&setup).map_err(|_| PreviewError::Handoff)?;
    if setup.version != HANDOFF_VERSION {
        return Err(PreviewError::Handoff);
    }
    let manifest_bytes = read_block(reader, MAX_MANIFEST_BYTES).await?;
    let manifest = parse_manifest(&manifest_bytes).map_err(|_| PreviewError::Handoff)?;
    if setup.manifest_revision != manifest.revision() {
        return Err(PreviewError::Handoff);
    }
    setup.mode.scenario(&manifest)?;
    let values = read_block(reader, MAX_CREDENTIAL_WIRE_BYTES).await?;
    let credentials =
        serde_json::from_slice::<CredentialValues>(&values).map_err(|_| PreviewError::Handoff)?;
    drop(values);
    let bindings = builder_project::runtime_bindings(&manifest);
    let projected = setup.access.project(&manifest, &bindings, &setup.mode)?;
    if let PreviewMode::Fixture { scenario_id } = setup.mode {
        if !credentials.is_empty() {
            return Err(PreviewError::Handoff);
        }
        return crate::scenario::fixture_runtime(&manifest, &scenario_id, projected.policy)
            .map_err(PreviewError::Scenario);
    }
    let project_id = manifest.document().project_id;
    let (local, identities) =
        setup
            .access
            .local_bindings(credentials, projected.scopes, project_id)?;
    let adapter = StarterAdapter::new_reviewed(project_id, bindings, local, &identities).map_err(
        |error| match error {
            mcp_starter_adapters::SetupError::FileRootUnavailable => {
                PreviewError::FileRootUnavailable
            }
            _ => PreviewError::Runtime,
        },
    )?;
    Runtime::new(
        builder_project::runtime_definition(&manifest),
        projected.policy,
        Arc::new(adapter),
    )
    .map_err(|_| PreviewError::Runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_an_oversized_setup_before_waiting_for_its_bytes() {
        let (mut parent, child) = tokio::io::duplex(64);
        let (stdin, stdout) = tokio::io::split(child);
        let worker = tokio::spawn(serve_worker(stdin, stdout));
        parent.write_all(MAGIC).await.unwrap();
        parent.write_u32(MAX_SETUP_BYTES as u32 + 1).await.unwrap();
        assert_eq!(parent.read_u8().await.unwrap(), Ack::Handoff as u8);
        assert!(matches!(worker.await.unwrap(), Err(PreviewError::Handoff)));
    }

    #[tokio::test]
    async fn malformed_wire_message_is_rejected_and_a_following_request_succeeds() {
        use std::collections::{BTreeMap, BTreeSet};

        use serde_json::{Value, json};
        use tokio::io::{AsyncBufReadExt, BufReader};

        let manifest = parse_manifest(include_bytes!(
            "../../../builder-project/tests/fixtures/catalog-v1.json"
        ))
        .unwrap();
        let access = Access {
            grant_ids: BTreeSet::from(["catalog_read".into()]),
            prompt_ids: BTreeSet::new(),
            file_roots: BTreeMap::new(),
            credential_sources: BTreeMap::new(),
            limits: Default::default(),
        };
        let (parent, child) = tokio::io::duplex(32 * 1024);
        let (stdin, stdout) = tokio::io::split(child);
        let worker = tokio::spawn(serve_worker(stdin, stdout));
        let (stdout, mut stdin) = tokio::io::split(parent);
        let mut stdout = BufReader::new(stdout);
        handoff(
            &mut stdin,
            &mut stdout,
            &manifest,
            &PreviewMode::Fixture {
                scenario_id: "known_item".into(),
            },
            &access,
            BTreeMap::new(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();

        write_frame(
            &mut stdin,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                    "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": {"name": "m7-fixture", "version": "1"}
                }
            }),
        )
        .await;
        assert_eq!(
            read_frame(&mut stdout).await["result"]["protocolVersion"],
            "2025-11-25"
        );
        write_frame(
            &mut stdin,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await;
        stdin.write_all(b"{malformed\n").await.unwrap();
        assert_eq!(read_frame(&mut stdout).await["error"]["code"], -32700);
        write_frame(
            &mut stdin,
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": "lookup_item", "arguments": {"id": "42"}}
            }),
        )
        .await;
        let response = read_frame(&mut stdout).await;
        assert_eq!(response["id"], 2);
        assert_eq!(
            response["result"]["structuredContent"]["name"],
            "Sample item"
        );
        drop(stdin);
        drop(stdout);
        tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        async fn write_frame(stdin: &mut (impl AsyncWrite + Unpin), value: Value) {
            stdin
                .write_all(&serde_json::to_vec(&value).unwrap())
                .await
                .unwrap();
            stdin.write_all(b"\n").await.unwrap();
        }

        async fn read_frame(stdout: &mut (impl AsyncBufReadExt + Unpin)) -> Value {
            let mut line = String::new();
            tokio::time::timeout(Duration::from_secs(3), stdout.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            serde_json::from_str(&line).unwrap()
        }
    }
}
