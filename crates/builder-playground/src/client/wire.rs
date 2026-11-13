use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};

use rmcp::RoleClient;
use rmcp::model::{
    ClientJsonRpcMessage, ClientNotification, ErrorCode, ErrorData, GetExtensions, GetMeta,
    NumberOrString, ProgressToken, ProtocolVersion, RequestId, ServerJsonRpcMessage,
    ServerNotification, ServerResult,
};
use rmcp::transport::Transport;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::trace::{TraceBuffer, TraceContext, TraceDirection, TraceKind, TracePayload};

pub(super) const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub(super) const MAX_ACTIVE_REQUESTS: usize = 16;
pub(super) const INVALID_FRAME: ErrorCode = ErrorCode(-31990);

#[derive(Clone, Copy, Debug)]
pub(super) enum WireError {
    FrameLimit,
    InvalidMessage,
    Busy,
    Disconnected,
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::FrameLimit => "MCP frame exceeds the local limit",
            Self::InvalidMessage => "Invalid MCP frame",
            Self::Busy => "MCP write queue is full",
            Self::Disconnected => "MCP pipes disconnected",
        })
    }
}

impl std::error::Error for WireError {}

#[derive(Clone)]
pub(super) struct OutboundTrace {
    pub context: TraceContext,
    pub payload: TracePayload,
    pub retired: CancellationToken,
}

#[derive(Clone)]
struct Pending {
    context: TraceContext,
    progress_token: Option<ProgressToken>,
    retired: Option<CancellationToken>,
}

struct WriteItem {
    bytes: Vec<u8>,
    context: TraceContext,
    kind: TraceKind,
    payload: TracePayload,
    retired: Option<CancellationToken>,
    done: oneshot::Sender<Result<(), WireError>>,
}

pub(super) struct PipeTransport<R> {
    read: BufReader<R>,
    frame: Vec<u8>,
    discarding: bool,
    startup: Option<RequestId>,
    stale_probe: Option<RequestId>,
    pending: Arc<Mutex<HashMap<RequestId, Pending>>>,
    writes: mpsc::Sender<WriteItem>,
    trace: Arc<TraceBuffer>,
    connection: TraceContext,
    closing: CancellationToken,
}

impl<R: AsyncRead> PipeTransport<R> {
    pub fn new<W>(
        read: R,
        write: W,
        trace: Arc<TraceBuffer>,
        connection: TraceContext,
        closing: CancellationToken,
    ) -> (Self, watch::Receiver<bool>)
    where
        W: AsyncWrite + Unpin + Send + 'static,
    {
        // The writer serializes entire frames and reserves capacity for cancellation.
        let (writes, queue) = mpsc::channel(MAX_ACTIVE_REQUESTS * 2 + 2);
        let (closed, writer_done) = watch::channel(false);
        tokio::spawn(write_pipe(
            write,
            queue,
            trace.clone(),
            closing.clone(),
            closed,
        ));
        (
            Self {
                read: BufReader::new(read),
                frame: Vec::new(),
                discarding: false,
                startup: None,
                stale_probe: None,
                pending: Arc::new(Mutex::new(HashMap::new())),
                writes,
                trace,
                connection,
                closing,
            },
            writer_done,
        )
    }
}

impl<R> PipeTransport<R> {
    fn stop(&self) {
        self.closing.cancel();
        self.trace.close_payloads();
    }
}

impl<R> Drop for PipeTransport<R> {
    fn drop(&mut self) {
        self.stop();
    }
}

impl<R: AsyncRead + Unpin + Send> Transport<RoleClient> for PipeTransport<R> {
    type Error = WireError;

    fn send(
        &mut self,
        mut item: ClientJsonRpcMessage,
    ) -> impl Future<Output = Result<(), WireError>> + Send + 'static {
        let queued = self.enqueue(&mut item);
        async move {
            let (done, pending) = queued?;
            let result = done.await.unwrap_or(Err(WireError::Disconnected));
            if result.is_err()
                && let Some((pending, id)) = pending
            {
                pending.lock().unwrap().remove(&id);
            }
            result
        }
    }

    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        loop {
            match self.read_frame().await {
                Ok(Frame::Eof) | Err(_) => {
                    self.stop();
                    return None;
                }
                Ok(Frame::Oversized) => {
                    self.trace.record(
                        self.connection,
                        TraceDirection::Received,
                        TraceKind::FrameLimit,
                        TracePayload::Omitted,
                    );
                }
                Ok(Frame::Complete) => {
                    let frame = self
                        .frame
                        .strip_prefix(b"\xef\xbb\xbf")
                        .unwrap_or(&self.frame);
                    let message = serde_json::from_slice::<ServerJsonRpcMessage>(frame);
                    let fallback = message
                        .is_err()
                        .then(|| malformed_response(frame))
                        .flatten();
                    let empty = frame.iter().all(u8::is_ascii_whitespace);
                    self.frame.clear();
                    let message = match message {
                        Ok(message) => message,
                        Err(_) if empty => continue,
                        Err(_) => {
                            self.trace.record(
                                self.connection,
                                TraceDirection::Received,
                                TraceKind::InvalidFrame,
                                TracePayload::None,
                            );
                            let Some(message) = fallback else {
                                continue;
                            };
                            message
                        }
                    };
                    if let Some(message) = self.route(message) {
                        return Some(message);
                    }
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), WireError> {
        self.stop();
        Ok(())
    }
}

type PendingWrites = Arc<Mutex<HashMap<RequestId, Pending>>>;
type Queued = (
    oneshot::Receiver<Result<(), WireError>>,
    Option<(PendingWrites, RequestId)>,
);

impl<R: AsyncRead + Unpin> PipeTransport<R> {
    fn enqueue(&mut self, message: &mut ClientJsonRpcMessage) -> Result<Queued, WireError> {
        if self.closing.is_cancelled() {
            return Err(WireError::Disconnected);
        }
        let reservation = self.writes.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => WireError::Busy,
            mpsc::error::TrySendError::Closed(_) => WireError::Disconnected,
        })?;
        let bytes = encode(message)?;
        let mut payload = TracePayload::None;
        let mut retired = None;
        let mut pending_id = None;
        let (context, kind) = match message {
            ClientJsonRpcMessage::Request(request) => {
                let outbound = request.request.extensions_mut().remove::<OutboundTrace>();
                let mut context = self.connection;
                if let Some(outbound) = outbound {
                    context = outbound.context;
                    payload = outbound.payload;
                    retired = Some(outbound.retired);
                } else {
                    context.method = match request.request.method() {
                        "initialize" => Some("initialize"),
                        "server/discover" => Some("server/discover"),
                        _ => None,
                    };
                    if let Some(previous) = self.startup.replace(request.id.clone()) {
                        self.pending.lock().unwrap().remove(&previous);
                        // A timed-out discovery probe can reply while the SDK
                        // waits for the legacy initialize result.
                        self.stale_probe = Some(previous);
                    }
                }
                context.message_id = numeric_id(&request.id);
                let mut pending = self.pending.lock().unwrap();
                pending.retain(|_, entry| {
                    !entry
                        .retired
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled)
                });
                pending.insert(
                    request.id.clone(),
                    Pending {
                        context,
                        progress_token: request.request.get_meta().get_progress_token(),
                        retired: retired.clone(),
                    },
                );
                pending_id = Some((self.pending.clone(), request.id.clone()));
                (context, TraceKind::Request)
            }
            ClientJsonRpcMessage::Notification(notification) => {
                let mut context = self.connection;
                context.method = match notification.notification {
                    ClientNotification::InitializedNotification(_) => {
                        Some("notifications/initialized")
                    }
                    // The local terminal trace supplies the correlation ID for
                    // the cancellation; don't publish a second unrelated entry.
                    ClientNotification::CancelledNotification(_) => None,
                    _ => None,
                };
                (context, TraceKind::Notification)
            }
            ClientJsonRpcMessage::Response(_) | ClientJsonRpcMessage::Error(_) => {
                (self.connection, TraceKind::Response)
            }
        };
        let (done, receiver) = oneshot::channel();
        reservation.send(WriteItem {
            bytes,
            context,
            kind,
            payload,
            retired,
            done,
        });
        Ok((receiver, pending_id))
    }

    fn route(&mut self, mut message: ServerJsonRpcMessage) -> Option<ServerJsonRpcMessage> {
        let (id, kind) = match &mut message {
            ServerJsonRpcMessage::Response(response) => (&mut response.id, TraceKind::Response),
            ServerJsonRpcMessage::Error(error) => match error.id.as_mut() {
                Some(id) => (id, TraceKind::Error),
                None if self.startup.is_some() => return Some(message),
                None => return None,
            },
            ServerJsonRpcMessage::Notification(notification) => {
                if let ServerNotification::ProgressNotification(progress) =
                    &notification.notification
                {
                    let pending = self.pending.lock().unwrap();
                    if let Some(request) = pending.values().find(|request| {
                        request.progress_token.as_ref() == Some(&progress.params.progress_token)
                            && !request
                                .retired
                                .as_ref()
                                .is_some_and(CancellationToken::is_cancelled)
                    }) {
                        // Progress messages are arbitrary peer diagnostic text.
                        self.trace.record(
                            request.context,
                            TraceDirection::Received,
                            TraceKind::Progress,
                            TracePayload::None,
                        );
                    }
                }
                return None;
            }
            ServerJsonRpcMessage::Request(_) => return Some(message),
        };
        let id_number = numeric_id(id)?;
        // rmcp startup already accepts numeric-string IDs. Normalize them at
        // the boundary so correlated steady-state replies use the same rule.
        *id = NumberOrString::Number(id_number);
        let request = self.pending.lock().unwrap().remove(id);
        let Some(request) = request else {
            return (self.startup.is_some() && self.stale_probe.as_ref() != Some(id))
                .then_some(message);
        };
        if request
            .retired
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return None;
        }
        if self.startup.as_ref() == Some(id) {
            self.startup = None;
            if let ServerJsonRpcMessage::Response(response) = &message
                && let ServerResult::InitializeResult(result) = &response.result
                && result.protocol_version != ProtocolVersion::V_2025_11_25
            {
                self.trace.record(
                    request.context,
                    TraceDirection::Received,
                    TraceKind::Error,
                    TracePayload::None,
                );
                return Some(ServerJsonRpcMessage::error(
                    ErrorData::new(
                        ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
                        "Unsupported MCP revision",
                        None,
                    ),
                    Some(response.id.clone()),
                ));
            }
            self.trace.record(
                request.context,
                TraceDirection::Received,
                kind,
                TracePayload::None,
            );
        }
        Some(message)
    }

    // receive() is polled in the SDK's select loop. Keep partial frame and
    // discard state across polls so cancellation cannot lose a frame prefix.
    async fn read_frame(&mut self) -> io::Result<Frame> {
        loop {
            let available = self.read.fill_buf().await?;
            if available.is_empty() {
                return Ok(Frame::Eof);
            }
            let newline = available.iter().position(|&byte| byte == b'\n');
            let take = newline.map_or(available.len(), |position| position + 1);
            if self.discarding {
                self.discarding = newline.is_none();
                self.read.consume(take);
                continue;
            }
            if take > MAX_FRAME_BYTES - self.frame.len() {
                self.frame.clear();
                self.discarding = newline.is_none();
                self.read.consume(take);
                return Ok(Frame::Oversized);
            }
            self.frame.extend_from_slice(&available[..take]);
            self.read.consume(take);
            if newline.is_some() {
                return Ok(Frame::Complete);
            }
        }
    }
}

enum Frame {
    Eof,
    Oversized,
    Complete,
}

fn numeric_id(id: &RequestId) -> Option<i64> {
    match id {
        NumberOrString::Number(id) => Some(*id),
        NumberOrString::String(id) => id.parse().ok(),
    }
}

fn malformed_response(frame: &[u8]) -> Option<ServerJsonRpcMessage> {
    let value: serde_json::Value = serde_json::from_slice(frame).ok()?;
    if value.get("method").is_some() {
        return None;
    }
    let id = serde_json::from_value(value.get("id")?.clone()).ok()?;
    Some(ServerJsonRpcMessage::error(
        ErrorData::new(INVALID_FRAME, "Invalid MCP response", None),
        Some(id),
    ))
}

async fn write_pipe(
    mut write: impl AsyncWrite + Unpin,
    mut queue: mpsc::Receiver<WriteItem>,
    trace: Arc<TraceBuffer>,
    closing: CancellationToken,
    closed: watch::Sender<bool>,
) {
    tokio::select! {
        biased;
        _ = closing.cancelled() => {}
        _ = async {
            while let Some(item) = queue.recv().await {
                // Before a frame starts, retirement can prevent dispatch.
                // After the first byte, finish the frame so a targeted cancel
                // can follow without corrupting any sibling's messages.
                let first_write = match &item.retired {
                    Some(retired) => tokio::select! {
                        biased;
                        _ = retired.cancelled() => None,
                        written = write.write(&item.bytes) => Some(written),
                    },
                    None => Some(write.write(&item.bytes).await),
                };
                let Some(first_write) = first_write else {
                    let _ = item.done.send(Ok(()));
                    continue;
                };
                let Ok(written @ 1..) = first_write else {
                    let _ = item.done.send(Err(WireError::Disconnected));
                    break;
                };
                if item.context.method.is_some() {
                    trace.record(item.context, TraceDirection::Sent, item.kind, item.payload);
                }
                let result = async {
                    write.write_all(&item.bytes[written..]).await?;
                    write.flush().await
                }.await.map_err(|_| WireError::Disconnected);
                let _ = item.done.send(result);
                if result.is_err() {
                    break;
                }
            }
        } => {}
    }
    drop(write);
    closing.cancel();
    trace.close_payloads();
    let _ = closed.send(true);
}

pub(super) struct StderrDrain(JoinHandle<()>);

impl StderrDrain {
    pub fn start(
        mut stderr: impl AsyncRead + Unpin + Send + 'static,
        trace: Arc<TraceBuffer>,
        context: TraceContext,
    ) -> Self {
        Self(tokio::spawn(async move {
            let mut chunk = [0_u8; 8192];
            // Retain only the fact that the child emitted diagnostics. Continue
            // draining without storing, logging, or decoding the raw bytes.
            if let Ok(1..) = stderr.read(&mut chunk).await {
                trace.record(
                    context,
                    TraceDirection::Stderr,
                    TraceKind::Diagnostic,
                    TracePayload::None,
                );
                while let Ok(1..) = stderr.read(&mut chunk).await {}
            }
        }))
    }
}

impl Drop for StderrDrain {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) fn serialized_len(value: &impl Serialize, limit: usize) -> Option<usize> {
    let mut counter = FrameCounter(limit);
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(limit - counter.0)
}

struct FrameCounter(usize);

impl io::Write for FrameCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or(io::ErrorKind::InvalidData)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode(message: &ClientJsonRpcMessage) -> Result<Vec<u8>, WireError> {
    let size = serialized_len(message, MAX_FRAME_BYTES - 1).ok_or(WireError::FrameLimit)?;
    let mut frame = Vec::with_capacity(size + 1);
    serde_json::to_writer(&mut frame, message).map_err(|_| WireError::InvalidMessage)?;
    frame.push(b'\n');
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    struct BlockFirstWrite {
        waiting: Option<oneshot::Sender<()>>,
        written: Arc<Mutex<Vec<u8>>>,
    }

    impl AsyncWrite for BlockFirstWrite {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            if let Some(waiting) = self.waiting.take() {
                let _ = waiting.send(());
                return Poll::Pending;
            }
            self.written.lock().unwrap().extend_from_slice(bytes);
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn retires_blocked_frame_before_first_byte_without_affecting_sibling() {
        let (waiting, blocked) = oneshot::channel();
        let (writes, queue) = mpsc::channel(2);
        let (closed, _) = watch::channel(false);
        let closing = CancellationToken::new();
        let written = Arc::new(Mutex::new(Vec::new()));
        let trace = Arc::new(TraceBuffer::new(false));
        let writer = tokio::spawn(write_pipe(
            BlockFirstWrite {
                waiting: Some(waiting),
                written: written.clone(),
            },
            queue,
            trace.clone(),
            closing.clone(),
            closed,
        ));
        let retired = CancellationToken::new();
        let first = TraceContext::new(Uuid::new_v4(), Some("tools/call"));
        let sibling = TraceContext::new(Uuid::new_v4(), Some("tools/call"));
        let (done, first_done) = oneshot::channel();
        writes
            .send(WriteItem {
                bytes: b"first\n".to_vec(),
                context: first,
                kind: TraceKind::Request,
                payload: TracePayload::None,
                retired: Some(retired.clone()),
                done,
            })
            .await
            .ok()
            .unwrap();
        timeout(Duration::from_secs(5), blocked)
            .await
            .unwrap()
            .unwrap();
        retired.cancel();
        let (done, sibling_done) = oneshot::channel();
        writes
            .send(WriteItem {
                bytes: b"sibling\n".to_vec(),
                context: sibling,
                kind: TraceKind::Request,
                payload: TracePayload::None,
                retired: None,
                done,
            })
            .await
            .ok()
            .unwrap();
        timeout(Duration::from_secs(5), async {
            first_done.await.unwrap().unwrap();
            sibling_done.await.unwrap().unwrap();
        })
        .await
        .unwrap();
        closing.cancel();
        writer.await.unwrap();
        assert_eq!(&*written.lock().unwrap(), b"sibling\n");
        let trace = trace.snapshot();
        assert_eq!(trace.entries.len(), 1);
        assert_eq!(trace.entries[0].correlation_id, sibling.correlation_id);
    }
}
