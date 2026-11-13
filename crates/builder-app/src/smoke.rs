use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Diagnostic, DiagnosticCode, DiagnosticDetails, Reply, Severity};

pub const SMOKE_CONTRACT_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SmokeRequest {
    pub contract_version: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmokeStatus {
    pub builder_version: &'static str,
    pub contract_version: u32,
    pub correlation_id: Uuid,
}

pub fn workspace_smoke(request: SmokeRequest) -> Reply<SmokeStatus> {
    let correlation_id = Uuid::new_v4();
    if request.contract_version != SMOKE_CONTRACT_VERSION {
        tracing::warn!(%correlation_id, code = "input.invalid", "Workspace smoke request rejected");
        return Reply::Failure {
            error: Diagnostic {
                code: DiagnosticCode::InvalidInput,
                message: "Unsupported workspace smoke contract version.".into(),
                severity: Severity::Error,
                correlation_id,
                action: Some("Use a supported workspace smoke contract version.".into()),
                field: Some("/contractVersion".into()),
                capability_id: None,
                details: Some(Box::new(DiagnosticDetails::SupportedVersions {
                    supported_versions: vec![SMOKE_CONTRACT_VERSION],
                })),
            },
            diagnostics: vec![],
        };
    }

    let data = SmokeStatus {
        builder_version: env!("CARGO_PKG_VERSION"),
        contract_version: SMOKE_CONTRACT_VERSION,
        correlation_id,
    };
    tracing::info!(
        %correlation_id,
        builder_version = data.builder_version,
        contract_version = data.contract_version,
        "Workspace smoke completed"
    );
    Reply::Success {
        data,
        diagnostics: vec![],
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, from_value, json, to_value};

    use super::*;

    #[test]
    fn serializes_success_and_rejection_as_the_shared_smoke_contract() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../tests/fixtures/workspace-smoke.json")).unwrap();

        for case in ["supported", "unsupported"] {
            let fixture = &fixtures[case];
            let request: SmokeRequest = from_value(fixture["request"].clone()).unwrap();
            let mut reply = to_value(workspace_smoke(request)).unwrap();
            let correlation = if reply["ok"] == true {
                &mut reply["data"]["correlationId"]
            } else {
                &mut reply["error"]["correlationId"]
            };
            let id: Uuid = from_value(correlation.clone()).unwrap();
            assert_eq!(id.get_version_num(), 4);
            *correlation = json!(Uuid::nil());
            assert_eq!(reply, fixture["reply"], "{case}");
        }
    }

    #[test]
    fn rejects_malformed_requests() {
        for request in [
            json!({}),
            json!({ "contractVersion": "1" }),
            json!({ "contractVersion": 1, "unexpected": true }),
        ] {
            assert!(from_value::<SmokeRequest>(request).is_err());
        }
    }
}
