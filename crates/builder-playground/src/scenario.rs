//! Replay saved scenarios against controlled adapter targets and evaluate client results.

mod adapter;
mod evaluation;

use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;

use builder_project::{ScenarioMode, ValidatedManifest, runtime_bindings, runtime_definition};
use mcp_policy::{ByteBudget, OperatorPolicy};
use mcp_runtime::Runtime;
use serde_json::Value;

pub use self::evaluation::{
    AssertionResult, RunOutcome, ScenarioEvaluation, evaluate, evaluate_with_redactor,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScenarioError {
    NotFound,
    LiveRequiresOptIn,
    InvalidRuntime,
}

impl ScenarioError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotFound => "scenario.not_found",
            Self::LiveRequiresOptIn => "scenario.live_requires_opt_in",
            Self::InvalidRuntime => "scenario.invalid_runtime",
        }
    }
}

impl fmt::Display for ScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ScenarioError {}

/// Construct the managed child's fixture-only runtime with the operator policy
/// from the reviewed launch. The caller supplies the saved scenario invocation
/// over MCP. This constructor performs no local root, network or credential lookup.
pub fn fixture_runtime(
    manifest: &ValidatedManifest,
    scenario_id: &str,
    operator: OperatorPolicy,
) -> Result<Runtime, ScenarioError> {
    let scenario = manifest
        .document()
        .scenarios
        .iter()
        .find(|scenario| scenario.id == scenario_id)
        .ok_or(ScenarioError::NotFound)?;
    if scenario.mode != ScenarioMode::Fixture {
        return Err(ScenarioError::LiveRequiresOptIn);
    }
    let adapter = adapter::FixtureAdapter {
        bindings: runtime_bindings(manifest),
        fixtures: scenario.fixtures.clone().unwrap_or_default(),
    };
    Runtime::new(runtime_definition(manifest), operator, Arc::new(adapter))
        .map_err(|_| ScenarioError::InvalidRuntime)
}

pub(crate) fn outcome_for_error(code: &str) -> RunOutcome {
    match code {
        "run.cancelled" => RunOutcome::Cancelled,
        "runtime.timeout" => RunOutcome::TimedOut,
        "policy.denied" | "auth.required" | "auth.insufficient_scope" => RunOutcome::Denied,
        "input.invalid" | "mcp.unknown_tool" | "mcp.unknown_resource" | "mcp.unknown_prompt" => {
            RunOutcome::Invalid
        }
        _ => RunOutcome::Failed,
    }
}

fn json_fits(value: &Value, byte_limit: u64) -> bool {
    fn within_depth(value: &Value, remaining: usize, nodes: &mut ByteBudget) -> bool {
        let Some(remaining) = remaining.checked_sub(1) else {
            return false;
        };
        if nodes.consume(1).is_err() {
            return false;
        }
        match value {
            Value::Array(items) => items
                .iter()
                .all(|item| within_depth(item, remaining, nodes)),
            Value::Object(fields) => fields
                .values()
                .all(|item| within_depth(item, remaining, nodes)),
            _ => true,
        }
    }

    struct Counter(ByteBudget);

    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.consume(bytes.len()).map_err(io::Error::other)?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    within_depth(value, 64, &mut ByteBudget::new(byte_limit))
        && serde_json::to_writer(Counter(ByteBudget::new(byte_limit)), value).is_ok()
}

#[cfg(test)]
mod tests;
