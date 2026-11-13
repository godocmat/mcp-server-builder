//! Transport-neutral server definition and dispatch for preview and generated servers.

mod adapter;
mod dispatch;
mod execution;
mod pagination;
mod prompt;
mod registry;
mod schema;
pub mod syntax;
mod types;

pub use adapter::{
    Adapter, AdapterContext, AdapterError, AdapterErrorKind, AdapterOutput, Invocation,
    InvocationInput, PreparationContext, PreparedCall, PreparedTarget,
};
pub use execution::{BoxFuture, ExecutionContext, Progress, ProgressSink};
pub use mcp_policy::Principal;
pub use prompt::prompt_variables;
pub use registry::{DefinitionError, DefinitionErrorKind, Runtime};
pub use schema::{compile_schema, compile_tool_schema};
pub use tokio_util::sync::CancellationToken;
pub use types::*;
