//! MCP stdio transport for a standalone server.

#[cfg(feature = "server-stdio")]
mod handler;

#[cfg(feature = "server-stdio")]
pub use handler::sdk_diagnostic_filter;

#[cfg(feature = "server-stdio")]
pub mod stdio;
