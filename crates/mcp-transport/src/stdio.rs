mod frames;

use std::io;
use std::time::Duration;

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

async fn serve<R, W>(
    reader: R,
    writer: W,
    handler: impl ServerHandler,
    disconnected: CancellationToken,
    shutdown: CancellationToken,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let transport = frames::StdioTransport::new(reader, writer, disconnected.clone());
    let wire = transport.wire();
    let _connection = disconnected.clone().drop_guard();
    let watcher = tokio::spawn({
        let disconnected = disconnected.clone();
        async move {
            tokio::select! {
                _ = shutdown.cancelled() => disconnected.cancel(),
                _ = disconnected.cancelled() => {}
            }
        }
    });
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
    let _ = watcher.await;
    match result {
        Ok(rmcp::service::QuitReason::Closed | rmcp::service::QuitReason::Cancelled) => {
            wire.io_result()
        }
        _ => Err(io::Error::other("MCP stdio service failed.")),
    }
}
