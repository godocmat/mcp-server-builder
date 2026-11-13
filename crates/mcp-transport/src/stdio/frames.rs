use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use rmcp::model::{
    ClientJsonRpcMessage, ClientNotification, ClientRequest, ErrorCode, GetMeta, NumberOrString,
    ProgressToken, RequestId, ServerJsonRpcMessage, ServerNotification,
};
use rmcp::transport::Transport;
use rmcp::{ErrorData, RoleServer};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::handler::{MODERN, SUPPORTED};

#[cfg(test)]
mod tests;

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_IN_FLIGHT: usize = 16;
const MAX_ID_BYTES: usize = 1024;

pub(super) struct StdioTransport<R> {
    reader: FrameReader<R>,
    writer: mpsc::Sender<Outbound>,
    pending_rejection: Option<ServerJsonRpcMessage>,
    wire: Arc<Wire>,
    lifecycle: Lifecycle,
}

enum Outbound {
    Sdk(ServerJsonRpcMessage, oneshot::Sender<io::Result<()>>),
    Rejection(ServerJsonRpcMessage),
}

impl<R: AsyncRead> StdioTransport<R> {
    pub(super) fn new(
        reader: R,
        writer: impl AsyncWrite + Unpin + Send + 'static,
        disconnected: CancellationToken,
    ) -> (Self, JoinHandle<()>) {
        let wire = Arc::new(Wire::new(disconnected));
        let (sender, receiver) = mpsc::channel(MAX_IN_FLIGHT);
        let task = tokio::spawn(write_frames(wire.clone(), writer, receiver));
        (
            Self {
                reader: FrameReader::new(reader),
                writer: sender,
                pending_rejection: None,
                wire,
                lifecycle: Lifecycle::Undecided,
            },
            task,
        )
    }

    pub(super) fn wire(&self) -> Arc<Wire> {
        self.wire.clone()
    }
}

struct FrameReader<R> {
    reader: BufReader<R>,
    bytes: Vec<u8>,
    oversized: bool,
}

enum Frame<'a> {
    Json(&'a [u8]),
    Oversized,
    Eof,
}

impl<R: AsyncRead> FrameReader<R> {
    fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            bytes: Vec::new(),
            oversized: false,
        }
    }
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    // State lives on the reader because the SDK can drop a pending receive
    // future to service another request. Count the LF in the frame ceiling.
    async fn next(&mut self) -> io::Result<Frame<'_>> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                return Ok(Frame::Eof);
            }
            let delimiter = available.iter().position(|byte| *byte == b'\n');
            let consumed = delimiter.map_or(available.len(), |index| index + 1);
            if self.bytes.len() + consumed > MAX_FRAME_BYTES {
                self.oversized = true;
                self.bytes.clear();
            }
            if !self.oversized {
                self.bytes.extend_from_slice(&available[..consumed]);
            }
            self.reader.consume(consumed);
            if delimiter.is_some() {
                if self.oversized {
                    self.oversized = false;
                    return Ok(Frame::Oversized);
                }
                let line = &self.bytes[..self.bytes.len() - 1];
                return Ok(Frame::Json(line.strip_suffix(b"\r").unwrap_or(line)));
            }
        }
    }
}

#[derive(Default)]
struct ActiveRequests {
    next_id: i64,
    requests: HashMap<i64, ActiveRequest>,
}

struct ActiveRequest {
    id: RequestId,
    progress: Option<ProgressToken>,
    cancelled: CancellationToken,
}

enum Admission {
    Accepted(i64),
    DuplicateProgress,
    Full,
}

impl ActiveRequests {
    fn admit(&mut self, id: &RequestId, progress: Option<ProgressToken>) -> Admission {
        if progress.as_ref().is_some_and(|token| {
            self.requests
                .values()
                .any(|request| request.progress.as_ref() == Some(token))
        }) {
            return Admission::DuplicateProgress;
        }
        if self.requests.len() == MAX_IN_FLIGHT {
            return Admission::Full;
        }
        let Some(next_id) = self.next_id.checked_add(1) else {
            return Admission::Full;
        };
        self.next_id = next_id;
        self.requests.insert(
            next_id,
            ActiveRequest {
                id: id.clone(),
                progress,
                cancelled: CancellationToken::new(),
            },
        );
        Admission::Accepted(next_id)
    }

    fn cancel(&mut self, id: &RequestId) -> Option<i64> {
        let internal_id = self
            .requests
            .iter()
            .find_map(|(internal_id, request)| (request.id == *id).then_some(*internal_id))?;
        self.requests.remove(&internal_id)?.cancelled.cancel();
        Some(internal_id)
    }

    fn get(&self, id: &RequestId) -> Option<&ActiveRequest> {
        let NumberOrString::Number(internal_id) = id else {
            return None;
        };
        self.requests.get(internal_id)
    }

    fn finish(&mut self, id: i64) {
        self.requests.remove(&id);
    }

    fn contains_wire_id(&self, id: &RequestId) -> bool {
        self.requests.values().any(|request| request.id == *id)
    }

    fn progress(&self, notification: &mut ServerNotification) -> Option<CancellationToken> {
        let ServerNotification::ProgressNotification(progress) = notification else {
            return None;
        };
        let NumberOrString::Number(internal_id) = progress.params.progress_token.0 else {
            return None;
        };
        let request = self.requests.get(&internal_id)?;
        progress.params.progress_token = request.progress.clone()?;
        Some(request.cancelled.clone())
    }
}

pub(super) struct Wire {
    active: Mutex<ActiveRequests>,
    disconnected: CancellationToken,
    io_error: Mutex<Option<io::ErrorKind>>,
}

impl Wire {
    fn new(disconnected: CancellationToken) -> Self {
        Self {
            active: Mutex::default(),
            disconnected,
            io_error: Mutex::default(),
        }
    }

    fn fail(&self, error: &io::Error) {
        self.io_error.lock().unwrap().get_or_insert(error.kind());
        self.disconnected.cancel();
    }

    pub(super) fn disconnect(&self) {
        self.disconnected.cancel();
    }

    pub(super) fn io_result(&self) -> io::Result<()> {
        match *self.io_error.lock().unwrap() {
            Some(kind) => Err(io::Error::new(kind, "MCP stdio I/O failed.")),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Undecided,
    LegacyOpening,
    Legacy,
    Modern,
}

type Rejection = (ErrorData, Option<RequestId>);
type Incoming = Result<Option<ClientJsonRpcMessage>, Rejection>;

impl Lifecycle {
    fn validate_request(&mut self, request: &ClientRequest) -> Result<(), ErrorData> {
        let method = request.method();
        let meta = request.get_meta();
        let version = meta.protocol_version();
        if matches!(request, ClientRequest::InitializeRequest(_)) {
            if *self != Self::Undecided || version.is_some() {
                return Err(ErrorData::invalid_request(
                    "Legacy initialization must open the connection.",
                    None,
                ));
            }
            return Ok(());
        }
        let inline = method == "server/discover" || version.is_some() || *self == Self::Modern;
        if inline {
            if !meta.missing_required_keys(&MODERN).is_empty() {
                return Err(ErrorData::invalid_params(
                    "Expected modern protocol version and client capabilities.",
                    None,
                ));
            }
            let requested = version.as_ref().expect("validated protocol version");
            if *requested != MODERN {
                return Err(ErrorData::unsupported_protocol_version(
                    requested.clone(),
                    &SUPPORTED,
                ));
            }
            if matches!(*self, Self::LegacyOpening | Self::Legacy) {
                return Err(ErrorData::invalid_request(
                    "This connection uses the legacy MCP lifecycle.",
                    None,
                ));
            }
        } else if method != "ping" && *self != Self::Legacy {
            return Err(ErrorData::invalid_request(
                "Initialize the legacy MCP connection before using capabilities.",
                None,
            ));
        }

        match request {
            ClientRequest::DiscoverRequest(_)
            | ClientRequest::CallToolRequest(_)
            | ClientRequest::ListToolsRequest(_)
            | ClientRequest::ListResourcesRequest(_)
            | ClientRequest::ListResourceTemplatesRequest(_)
            | ClientRequest::ReadResourceRequest(_)
            | ClientRequest::GetPromptRequest(_)
            | ClientRequest::ListPromptsRequest(_) => Ok(()),
            ClientRequest::PingRequest(_) if !inline => Ok(()),
            ClientRequest::CustomRequest(_)
                if matches!(
                    method,
                    "initialize"
                        | "ping"
                        | "server/discover"
                        | "tools/list"
                        | "tools/call"
                        | "resources/list"
                        | "resources/templates/list"
                        | "resources/read"
                        | "prompts/list"
                        | "prompts/get"
                ) =>
            {
                Err(ErrorData::invalid_params("Invalid MCP parameters.", None))
            }
            _ => Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                "Unknown MCP method.",
                None,
            )),
        }
    }

    fn accept(&mut self, request: &ClientRequest) {
        match request {
            ClientRequest::InitializeRequest(_) => *self = Self::LegacyOpening,
            ClientRequest::DiscoverRequest(_) | ClientRequest::PingRequest(_) => {}
            _ if *self == Self::Undecided => *self = Self::Modern,
            _ => {}
        }
    }
}

fn bounded_identifier(id: &NumberOrString) -> bool {
    match id {
        NumberOrString::Number(_) => true,
        NumberOrString::String(value) => value.len() <= MAX_ID_BYTES,
    }
}

fn decode(bytes: &[u8], wire: &Wire, lifecycle: &mut Lifecycle) -> Incoming {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    if bytes.is_empty() {
        return Ok(None);
    }
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return Err((ErrorData::parse_error("Invalid JSON.", None), None)),
    };
    let id = value
        .get("id")
        .and_then(|id| serde_json::from_value::<RequestId>(id.clone()).ok())
        .filter(bounded_identifier);
    if id
        .as_ref()
        .is_some_and(|id| wire.active.lock().unwrap().contains_wire_id(id))
    {
        return Err((
            ErrorData::invalid_request("MCP request identifier is already active.", None),
            None,
        ));
    }
    let is_notification =
        value.get("id").is_none() && value.get("method").is_some_and(Value::is_string);
    let Ok(message) = serde_json::from_value::<ClientJsonRpcMessage>(value) else {
        return if is_notification {
            Ok(None)
        } else {
            Err((ErrorData::invalid_request("Invalid MCP message.", None), id))
        };
    };
    match message {
        ClientJsonRpcMessage::Request(mut request) => {
            if id.is_none() {
                return Err((
                    ErrorData::invalid_request("MCP request identifier is invalid.", None),
                    None,
                ));
            }
            if let Err(error) = lifecycle.validate_request(&request.request) {
                return Err((error, id));
            }
            let meta = request.request.get_meta_mut();
            let progress = meta.get_progress_token();
            if meta.contains_key("progressToken")
                && !progress
                    .as_ref()
                    .is_some_and(|token| bounded_identifier(&token.0))
            {
                return Err((
                    ErrorData::invalid_params("MCP progress token is invalid.", None),
                    id,
                ));
            }
            let has_progress = progress.is_some();
            let admission = wire.active.lock().unwrap().admit(&request.id, progress);
            match admission {
                Admission::Accepted(internal_id) => {
                    request.id = NumberOrString::Number(internal_id);
                    if has_progress {
                        meta.set_progress_token(ProgressToken(NumberOrString::Number(internal_id)));
                    }
                    lifecycle.accept(&request.request);
                    Ok(Some(ClientJsonRpcMessage::Request(request)))
                }
                Admission::DuplicateProgress => Err((
                    ErrorData::invalid_params("MCP progress token is already active.", None),
                    id,
                )),
                Admission::Full => Err((
                    ErrorData::internal_error("Too many active MCP requests.", None),
                    id,
                )),
            }
        }
        ClientJsonRpcMessage::Notification(mut notification) => {
            match &mut notification.notification {
                ClientNotification::CancelledNotification(cancelled)
                    if matches!(*lifecycle, Lifecycle::Modern | Lifecycle::Legacy) =>
                {
                    let Some(internal_id) = cancelled
                        .params
                        .request_id
                        .as_ref()
                        .and_then(|id| wire.active.lock().unwrap().cancel(id))
                    else {
                        return Ok(None);
                    };
                    cancelled.params.request_id = Some(NumberOrString::Number(internal_id));
                    cancelled.params.reason = None;
                }
                ClientNotification::InitializedNotification(_)
                    if *lifecycle == Lifecycle::LegacyOpening =>
                {
                    *lifecycle = Lifecycle::Legacy;
                }
                _ => return Ok(None),
            }
            Ok(Some(ClientJsonRpcMessage::Notification(notification)))
        }
        ClientJsonRpcMessage::Response(_) | ClientJsonRpcMessage::Error(_) => Ok(None),
    }
}

impl<R> Transport<RoleServer> for StdioTransport<R>
where
    R: AsyncRead + Unpin + Send,
{
    type Error = io::Error;

    fn send(
        &mut self,
        message: ServerJsonRpcMessage,
    ) -> impl Future<Output = io::Result<()>> + Send + 'static {
        let wire = self.wire.clone();
        let writer = self.writer.clone();
        async move {
            let (acknowledge, sent) = oneshot::channel();
            if writer
                .send(Outbound::Sdk(message, acknowledge))
                .await
                .is_err()
            {
                return wire.io_result();
            }
            sent.await.unwrap_or_else(|_| wire.io_result())
        }
    }

    async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
        loop {
            if self.pending_rejection.is_some() {
                let permit = self.writer.reserve().await.ok()?;
                if let Some(message) = self.pending_rejection.take() {
                    permit.send(Outbound::Rejection(message));
                }
            }
            if self.wire.disconnected.is_cancelled() {
                return None;
            }
            let incoming = match self.reader.next().await {
                Ok(Frame::Json(bytes)) => decode(bytes, &self.wire, &mut self.lifecycle),
                Ok(Frame::Oversized) => Err((
                    ErrorData::invalid_request("MCP frame exceeds its byte limit.", None),
                    None,
                )),
                Ok(Frame::Eof) => {
                    self.wire.disconnect();
                    return None;
                }
                Err(error) => {
                    self.wire.fail(&error);
                    return None;
                }
            };
            self.reader.bytes.clear();
            match incoming {
                Ok(Some(message)) => return Some(message),
                Ok(None) => {}
                Err((error, id)) => {
                    self.pending_rejection = Some(ServerJsonRpcMessage::error(error, id));
                }
            }
        }
    }

    async fn close(&mut self) -> io::Result<()> {
        self.wire.disconnect();
        Ok(())
    }
}

struct Publication {
    internal_id: Option<i64>,
    cancelled: Option<CancellationToken>,
}

fn prepare(wire: &Wire, message: &mut ServerJsonRpcMessage) -> Option<Publication> {
    let active = wire.active.lock().unwrap();
    let mut publication = Publication {
        internal_id: None,
        cancelled: None,
    };
    let terminal = match message {
        ServerJsonRpcMessage::Response(response) => Some(&mut response.id),
        ServerJsonRpcMessage::Error(error) => error.id.as_mut(),
        ServerJsonRpcMessage::Notification(notification) => {
            publication.cancelled = Some(active.progress(&mut notification.notification)?);
            None
        }
        ServerJsonRpcMessage::Request(_) => return None,
    };
    if let Some(id) = terminal {
        let NumberOrString::Number(internal_id) = *id else {
            return None;
        };
        let request = active.get(id)?;
        *id = request.id.clone();
        publication.internal_id = Some(internal_id);
        publication.cancelled = Some(request.cancelled.clone());
    }
    Some(publication)
}

async fn write_frames(
    wire: Arc<Wire>,
    mut writer: impl AsyncWrite + Unpin,
    mut outbound: mpsc::Receiver<Outbound>,
) {
    loop {
        let outbound = tokio::select! {
            biased;
            _ = wire.disconnected.cancelled() => return,
            outbound = outbound.recv() => outbound,
        };
        let result = match outbound {
            Some(Outbound::Sdk(mut message, acknowledge)) => {
                let result = match prepare(&wire, &mut message) {
                    Some(publication) => publish(&wire, &mut writer, &message, publication).await,
                    None => Ok(()),
                };
                let _ = acknowledge.send(
                    result
                        .as_ref()
                        .map(|_| ())
                        .map_err(|error| error.kind().into()),
                );
                result
            }
            Some(Outbound::Rejection(message)) => match encode(&message) {
                Ok(frame) => write_frame(&wire, &mut writer, &frame, None, None).await,
                Err(error) => Err(error),
            },
            None => return,
        };
        if let Err(error) = result {
            wire.fail(&error);
            return;
        }
    }
}

async fn publish(
    wire: &Wire,
    writer: &mut (impl AsyncWrite + Unpin),
    message: &ServerJsonRpcMessage,
    publication: Publication,
) -> io::Result<()> {
    let frame = match encode(message) {
        Ok(frame) => frame,
        Err(_) => {
            let Some(id) = terminal_id(message) else {
                return Ok(());
            };
            encode(&ServerJsonRpcMessage::error(
                ErrorData::internal_error("MCP response exceeds its byte limit.", None),
                Some(id.clone()),
            ))?
        }
    };
    write_frame(
        wire,
        writer,
        &frame,
        publication.cancelled.as_ref(),
        publication.internal_id,
    )
    .await
}

fn terminal_id(message: &ServerJsonRpcMessage) -> Option<&RequestId> {
    match message {
        ServerJsonRpcMessage::Response(response) => Some(&response.id),
        ServerJsonRpcMessage::Error(error) => error.id.as_ref(),
        _ => None,
    }
}

async fn write_frame(
    wire: &Wire,
    writer: &mut (impl AsyncWrite + Unpin),
    bytes: &[u8],
    request_cancelled: Option<&CancellationToken>,
    terminal: Option<i64>,
) -> io::Result<()> {
    // Until the first bytes are written, a queued cancellation can suppress
    // this frame. After that point, finish the frame or close the connection.
    tokio::select! {
        biased;
        _ = wire.disconnected.cancelled() => Ok(()),
        result = async {
            let written = tokio::select! {
                biased;
                _ = async {
                    if let Some(token) = request_cancelled {
                        token.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => return Ok(()),
                result = writer.write(bytes) => result?,
            };
            if written == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            writer.write_all(&bytes[written..]).await?;
            if let Some(internal_id) = terminal {
                wire.active.lock().unwrap().finish(internal_id);
            }
            writer.flush().await
        } => result,
    }
}

#[derive(Default)]
struct BoundedFrame(Vec<u8>);

impl io::Write for BoundedFrame {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.0.len() + data.len() >= MAX_FRAME_BYTES {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.0.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode(message: &ServerJsonRpcMessage) -> io::Result<Vec<u8>> {
    let mut frame = BoundedFrame::default();
    serde_json::to_writer(&mut frame, message)?;
    frame.0.push(b'\n');
    Ok(frame.0)
}
