//! Shared application services and DTOs for the CLI and desktop shells.

mod diagnostic;
mod reply;
mod smoke;

pub use diagnostic::{Diagnostic, DiagnosticCode, DiagnosticDetails, Severity};
pub use reply::Reply;
pub use smoke::{SMOKE_CONTRACT_VERSION, SmokeRequest, SmokeStatus, workspace_smoke};
