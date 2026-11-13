mod frames;
mod handler;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use mcp_runtime::Runtime;
use rmcp::service::ServerInitializeError;
use rmcp::{ServerHandler, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Exclude SDK events from a host's tracing layer.
///
/// Apply this predicate to every layer that records diagnostics or evidence.
/// SDK debug fields and cancellation traces can contain raw request data.
pub fn sdk_diagnostic_filter(metadata: &tracing::Metadata<'_>) -> bool {
    metadata.target() != "rmcp" && !metadata.target().starts_with("rmcp::")
}

/// Serve one local MCP connection over the process's standard streams.
///
/// The launcher supplies the reviewed runtime and trusted local operator
/// identity. Configure the host's diagnostic layers with
/// [`sdk_diagnostic_filter`] before starting the server.
pub async fn serve_stdio(
    runtime: Arc<Runtime>,
    operator: impl Into<String>,
    shutdown: CancellationToken,
) -> io::Result<()> {
    let (reader, writer) = rmcp::transport::stdio_pipes();
    serve_stream(reader, writer, runtime, operator, shutdown).await
}

/// Serve one local MCP connection over an owned pair of byte streams.
pub async fn serve_stream<R, W>(
    reader: R,
    writer: W,
    runtime: Arc<Runtime>,
    operator: impl Into<String>,
    shutdown: CancellationToken,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let handler = handler::LocalHandler::new(runtime, operator.into());
    serve(reader, writer, handler, shutdown.child_token()).await
}

async fn serve<R, W>(
    reader: R,
    writer: W,
    handler: impl ServerHandler,
    disconnected: CancellationToken,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let transport = frames::StdioTransport::new(reader, writer, disconnected.clone());
    let wire = transport.wire();
    let _connection = disconnected.clone().drop_guard();
    let service = match handler.serve_with_ct(transport, disconnected.clone()).await {
        Ok(service) => service,
        Err(ServerInitializeError::ConnectionClosed(_) | ServerInitializeError::Cancelled) => {
            return wire.io_result();
        }
        Err(_) => return Err(io::Error::other("MCP stdio startup failed.")),
    };
    let waiting = service.waiting();
    tokio::pin!(waiting);
    let result = tokio::select! {
        result = &mut waiting => result,
        _ = disconnected.cancelled() => {
            tokio::time::timeout(SHUTDOWN_GRACE, &mut waiting).await
                .map_err(|_| io::Error::other("MCP stdio shutdown timed out."))?
        }
    };
    wire.disconnect();
    match result {
        Ok(rmcp::service::QuitReason::Closed | rmcp::service::QuitReason::Cancelled) => {
            wire.io_result()
        }
        _ => Err(io::Error::other("MCP stdio service failed.")),
    }
}
