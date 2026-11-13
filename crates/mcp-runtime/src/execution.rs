use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::time::Instant;

use mcp_policy::{DenyReason, Effect, LimitOverrides};
use serde::Serialize;
use uuid::Uuid;

use crate::{CancellationToken, ErrorKind, Principal, RuntimeError};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub progress: f64,
    pub total: Option<f64>,
    pub message: Option<String>,
}

/// The transport associates these events with the originating request's progress token.
pub trait ProgressSink: Send + Sync {
    fn send(&self, event: Progress) -> BoxFuture<'_, ()>;
}

pub struct ExecutionContext<'a> {
    pub(crate) principal: &'a Principal<'a>,
    pub(crate) cancellation: CancellationToken,
    pub(crate) correlation_id: Uuid,
    pub(crate) admitted_at: Instant,
    pub(crate) limits: LimitOverrides,
    pub(crate) progress: Option<&'a dyn ProgressSink>,
}

impl<'a> ExecutionContext<'a> {
    pub fn new(principal: &'a Principal<'a>, cancellation: CancellationToken) -> Self {
        Self {
            principal,
            cancellation,
            correlation_id: Uuid::new_v4(),
            admitted_at: Instant::now(),
            limits: LimitOverrides::default(),
            progress: None,
        }
    }

    #[must_use]
    pub fn with_progress(mut self, progress: &'a dyn ProgressSink) -> Self {
        self.progress = Some(progress);
        self
    }

    #[must_use]
    pub fn with_correlation_id(mut self, correlation_id: Uuid) -> Self {
        self.correlation_id = correlation_id;
        self
    }

    #[must_use]
    pub fn with_limits(mut self, limits: LimitOverrides) -> Self {
        self.limits = limits;
        self
    }

    pub fn correlation_id(&self) -> Uuid {
        self.correlation_id
    }

    pub(crate) fn error(&self, kind: ErrorKind, effect: Effect) -> RuntimeError {
        RuntimeError {
            kind,
            correlation_id: self.correlation_id,
            effect,
            denial: None,
        }
    }

    pub(crate) fn denial(&self, denial: DenyReason) -> RuntimeError {
        let kind = match denial {
            DenyReason::DeadlineExceeded | DenyReason::AuthorizationExpired => ErrorKind::TimedOut,
            _ => ErrorKind::Denied,
        };
        RuntimeError {
            denial: Some(denial),
            ..self.error(kind, Effect::None)
        }
    }
}

pub(crate) fn serialized_len<T: Serialize + ?Sized>(value: &T, limit: usize) -> Option<usize> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(buffer.len())
                .filter(|bytes| *bytes <= self.limit)
                .ok_or_else(|| io::Error::other("result exceeds byte limit"))?;
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.bytes)
}

/// Bound nested values before passing them to a recursive serializer or validator.
pub(crate) fn json_depth_is_bounded(value: &serde_json::Value, remaining: usize) -> bool {
    let Some(depth) = remaining.checked_sub(1) else {
        return false;
    };
    match value {
        serde_json::Value::Array(items) => {
            items.iter().all(|item| json_depth_is_bounded(item, depth))
        }
        serde_json::Value::Object(fields) => fields
            .values()
            .all(|item| json_depth_is_bounded(item, depth)),
        _ => true,
    }
}
