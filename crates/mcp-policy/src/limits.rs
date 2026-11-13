use std::fmt;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Optional bounds from a grant, approved configuration, caller or request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LimitOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<NonZeroU64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<NonZeroU64>,
}

impl<'de> Deserialize<'de> for LimitOverrides {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Fields {
            #[serde(default, deserialize_with = "crate::deserialize_present")]
            timeout_ms: Option<NonZeroU64>,
            #[serde(default, deserialize_with = "crate::deserialize_present")]
            max_output_bytes: Option<NonZeroU64>,
        }
        let Fields {
            timeout_ms,
            max_output_bytes,
        } = crate::deserialize_policy(deserializer)?;
        Ok(Self {
            timeout_ms,
            max_output_bytes,
        })
    }
}

/// Effective per-call limits. The timeout starts at admission, including on rechecks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionLimits {
    timeout: Duration,
    max_output_bytes: u64,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self::for_capability(None)
    }
}

impl ExecutionLimits {
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
    pub const MAX_TIMEOUT: Duration = Duration::from_secs(30);
    pub const MAX_INPUT_BYTES: u64 = 1024 * 1024;
    pub const MAX_OUTPUT_BYTES: u64 = 1024 * 1024;

    pub fn for_capability(timeout_ms: Option<NonZeroU64>) -> Self {
        let timeout = timeout_ms.map_or(Self::DEFAULT_TIMEOUT, |value| {
            Duration::from_millis(value.get()).min(Self::MAX_TIMEOUT)
        });
        Self {
            timeout,
            max_output_bytes: Self::MAX_OUTPUT_BYTES,
        }
    }

    #[must_use]
    pub fn tighten(self, bounds: LimitOverrides) -> Self {
        Self {
            timeout: bounds.timeout_ms.map_or(self.timeout, |value| {
                self.timeout.min(Duration::from_millis(value.get()))
            }),
            max_output_bytes: bounds
                .max_output_bytes
                .map_or(self.max_output_bytes, |value| {
                    self.max_output_bytes.min(value.get())
                }),
        }
    }

    #[must_use]
    pub fn intersect(self, other: Self) -> Self {
        Self {
            timeout: self.timeout.min(other.timeout),
            max_output_bytes: self.max_output_bytes.min(other.max_output_bytes),
        }
    }

    pub fn timeout(self) -> Duration {
        self.timeout
    }

    pub fn max_output_bytes(self) -> u64 {
        self.max_output_bytes
    }

    pub fn output_budget(self) -> ByteBudget {
        ByteBudget::new(self.max_output_bytes)
    }

    pub fn input_budget() -> ByteBudget {
        ByteBudget::new(Self::MAX_INPUT_BYTES)
    }
}

/// Carry an existing per-request ceiling into a target or policy recheck.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionCeiling {
    pub limits: ExecutionLimits,
    pub deadline: Instant,
}

/// Count cumulative bytes before passing a chunk to a handler or publishing a result.
#[derive(Debug)]
pub struct ByteBudget {
    limit: u64,
    used: u64,
}

impl ByteBudget {
    pub fn new(max_bytes: u64) -> Self {
        Self {
            limit: max_bytes,
            used: 0,
        }
    }

    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used)
    }

    /// Apply a smaller cap after a recheck while retaining the bytes already consumed.
    pub fn tighten(&mut self, max_bytes: u64) -> Result<(), ByteLimitExceeded> {
        self.limit = self.limit.min(max_bytes);
        if self.used > self.limit {
            return Err(ByteLimitExceeded);
        }
        Ok(())
    }

    /// An oversized chunk consumes no budget. The caller must stop the oversized operation.
    pub fn consume(&mut self, bytes: usize) -> Result<(), ByteLimitExceeded> {
        let bytes = u64::try_from(bytes).map_err(|_| ByteLimitExceeded)?;
        let used = self.used.checked_add(bytes).ok_or(ByteLimitExceeded)?;
        if used > self.limit {
            return Err(ByteLimitExceeded);
        }
        self.used = used;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteLimitExceeded;

impl fmt::Display for ByteLimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("execution.byte_limit_exceeded")
    }
}

impl std::error::Error for ByteLimitExceeded {}

/// Report effect state independently of whether a call was cancelled or timed out.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Effect {
    None,
    Committed,
    Unknown,
}
