//! MCP server and client transport boundary.

#[cfg(any(feature = "server-http", feature = "server-stdio"))]
mod handler;

#[cfg(any(feature = "server-http", feature = "server-stdio"))]
pub use handler::sdk_diagnostic_filter;

#[cfg(feature = "server-http")]
pub mod http;

#[cfg(feature = "server-stdio")]
pub mod stdio;
