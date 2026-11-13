//! Bounded, sanitized in-memory evidence for a local playground connection.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

use mcp_policy::{RedactionError, Redactor};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

const MAX_ENTRIES: usize = 500;
const MAX_TRACE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_ENTRY_BYTES: usize = Redactor::DEFAULT_ENTRY_BYTES;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TraceDirection {
    Sent,
    Received,
    Stderr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TraceKind {
    Request,
    Response,
    Error,
    Notification,
    Progress,
    Cancelled,
    TimedOut,
    Diagnostic,
    InvalidFrame,
    FrameLimit,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceEntry {
    pub sequence: u64,
    pub offset_ms: u64,
    pub direction: TraceDirection,
    pub kind: TraceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<i64>,
    pub correlation_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSummary {
    pub entries: usize,
    pub dropped_entries: u64,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSnapshot {
    pub entries: Vec<TraceEntry>,
    pub summary: TraceSummary,
}

#[derive(Clone, Copy)]
pub(crate) struct TraceContext {
    pub correlation_id: Uuid,
    pub method: Option<&'static str>,
    pub message_id: Option<i64>,
}

impl TraceContext {
    pub fn new(correlation_id: Uuid, method: Option<&'static str>) -> Self {
        Self {
            correlation_id,
            method,
            message_id: None,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) enum TracePayload {
    #[default]
    None,
    Sanitized(Value),
    Omitted,
}

impl From<Result<Value, RedactionError>> for TracePayload {
    fn from(result: Result<Value, RedactionError>) -> Self {
        match result {
            Ok(value) => Self::Sanitized(value),
            Err(_) => Self::Omitted,
        }
    }
}

pub(crate) struct TraceBuffer {
    started: Instant,
    capture_payloads: bool,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    entries: VecDeque<(TraceEntry, usize)>,
    bytes: usize,
    sequence: u64,
    dropped_entries: u64,
    truncated: bool,
    closed: bool,
}

impl TraceBuffer {
    pub fn new(capture_payloads: bool) -> Self {
        Self {
            started: Instant::now(),
            capture_payloads,
            state: Mutex::new(State::default()),
        }
    }

    pub fn captures_payloads(&self) -> bool {
        self.capture_payloads
    }

    pub fn record(
        &self,
        context: TraceContext,
        direction: TraceDirection,
        kind: TraceKind,
        payload: TracePayload,
    ) {
        let mut state = self.state.lock().unwrap();
        state.sequence = state.sequence.saturating_add(1);
        let mut entry = TraceEntry {
            sequence: state.sequence,
            offset_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            direction,
            kind,
            method: context.method,
            message_id: context.message_id,
            correlation_id: context.correlation_id,
            payload: None,
            truncated: false,
        };
        if self.capture_payloads && !state.closed {
            match payload {
                TracePayload::Sanitized(payload) => entry.payload = Some(payload),
                TracePayload::Omitted => entry.truncated = true,
                TracePayload::None => {}
            }
        }
        let mut bytes = entry_size(&entry);
        if bytes > MAX_ENTRY_BYTES {
            entry.payload = None;
            entry.truncated = true;
            bytes = entry_size(&entry);
        }
        while state.entries.len() >= MAX_ENTRIES || state.bytes + bytes > MAX_TRACE_BYTES {
            let Some((_, evicted_bytes)) = state.entries.pop_front() else {
                break;
            };
            state.bytes -= evicted_bytes;
            state.dropped_entries = state.dropped_entries.saturating_add(1);
        }
        state.truncated |= entry.truncated;
        state.bytes += bytes;
        state.entries.push_back((entry, bytes));
    }

    pub fn snapshot(&self) -> TraceSnapshot {
        let state = self.state.lock().unwrap();
        TraceSnapshot {
            entries: state
                .entries
                .iter()
                .map(|(entry, _)| entry.clone())
                .collect(),
            summary: state.summary(),
        }
    }

    pub fn summary(&self) -> TraceSummary {
        self.state.lock().unwrap().summary()
    }

    pub fn close_payloads(&self) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        state.closed = true;
        for (entry, bytes) in &mut state.entries {
            if entry.payload.take().is_some() {
                *bytes = entry_size(entry);
            }
        }
        state.bytes = state.entries.iter().map(|(_, bytes)| bytes).sum();
    }
}

impl State {
    fn summary(&self) -> TraceSummary {
        TraceSummary {
            entries: self.entries.len(),
            dropped_entries: self.dropped_entries,
            truncated: self.truncated || self.dropped_entries > 0,
        }
    }
}

fn entry_size(entry: &TraceEntry) -> usize {
    // Reserve a delimiter for the enclosing JSON array.
    serde_json::to_vec(entry)
        .expect("trace entries serialize")
        .len()
        + 1
}
