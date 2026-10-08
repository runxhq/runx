// Module rationale: policy wire types stay colocated so serde surface changes are reviewed together.
use runx_contracts::policy_proof::{
    AuthorityKind, AuthorityProof, CredentialEnvelope, ScopeAdmission,
};
use runx_contracts::{ExecutionBoundaryObservation, JsonValue};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LocalExecutionPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict_process_tool_inline_code: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LocalAdmissionGrant {
    pub grant_id: String,
    pub provider: String,
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<LocalAdmissionGrantStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_kind: Option<AuthorityKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_locator: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAdmissionGrantStatus {
    Active,
    Revoked,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalScopeAdmissionOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denied_before_grant_resolution: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_auth_checked_at: Option<String>,
    /// Honor a universal `*` grant scope. Defaults to `false` (fail closed):
    /// only a trusted caller resolving first-party grants may set this true.
    #[serde(default)]
    pub wildcard_scopes_trusted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "kebab-case",
    tag = "status",
    rename_all_fields = "camelCase"
)]
pub enum CredentialBindingDecision {
    Allow { reasons: Vec<String> },
    Deny { reasons: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorityProofApprovalGate {
    pub id: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub gate_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorityProofApproval {
    pub gate: AuthorityProofApprovalGate,
    pub approved: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildAuthorityProofOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_auth_checked_at: Option<String>,
    pub skill_name: String,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<LocalAdmissionGrant>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_admission: Option<ScopeAdmission>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialEnvelope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    pub execution_boundary: ExecutionBoundaryObservation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<AuthorityProofApproval>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialBindingRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<LocalAdmissionGrant>,
    pub scope_admission: ScopeAdmission,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialEnvelope>,
}

// AuthorityProof is intentionally policy-owned. It is emitted by
// policy.buildAuthorityProofMetadata, depends on policy admission decisions, and
// is guarded as a contract by schema validation in runx-contracts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AuthorityProofMetadata {
    pub authority_proof: AuthorityProof,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GraphScopeGrant {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphScopeAdmissionRequest {
    pub step_id: String,
    pub requested_scopes: Vec<String>,
    pub grant: GraphScopeGrant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum AdmissionDecision {
    Allow {
        reasons: Vec<String>,
    },
    AllowMarked {
        reasons: Vec<String>,
        norm_refs: Vec<String>,
    },
    Deny {
        reasons: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum GraphScopeAdmissionDecision {
    Allow {
        reasons: Vec<String>,
        step_id: String,
        requested_scopes: Vec<String>,
        granted_scopes: Vec<String>,
        #[serde(rename = "grantId", skip_serializing_if = "Option::is_none")]
        grant_id: Option<String>,
    },
    Deny {
        reasons: Vec<String>,
        step_id: String,
        requested_scopes: Vec<String>,
        granted_scopes: Vec<String>,
        #[serde(rename = "grantId", skip_serializing_if = "Option::is_none")]
        grant_id: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use runx_contracts::policy_proof::AuthorityKind;

    use super::{
        AdmissionDecision, GraphScopeAdmissionDecision, LocalAdmissionGrant,
        LocalAdmissionGrantStatus,
    };

    // AllowMarked has no kernel fixture producing it, so its wire shape is
    // pinned directly here.
    #[test]
    fn admission_decision_round_trips_allow_marked() -> Result<(), serde_json::Error> {
        let decision = AdmissionDecision::AllowMarked {
            reasons: vec!["allowed with visible norm mark".to_owned()],
            norm_refs: vec!["acme:norm:reply-before-escalation".to_owned()],
        };

        let json = serde_json::to_string(&decision)?;
        let decoded: AdmissionDecision = serde_json::from_str(&json)?;

        assert_eq!(
            json,
            r#"{"status":"allow-marked","reasons":["allowed with visible norm mark"],"normRefs":["acme:norm:reply-before-escalation"]}"#,
        );
        assert_eq!(decoded, decision);
        Ok(())
    }

    #[test]
    fn grant_deserializes_snake_case_targeting_fields() -> Result<(), serde_json::Error> {
        let json = r#"{"grant_id":"grant_1","provider":"github","scopes":["issues:write"],"status":"active","scope_family":"github","authority_kind":"constructive","target_repo":"runxhq/runx","target_locator":"issue/1"}"#;

        let grant: LocalAdmissionGrant = serde_json::from_str(json)?;

        assert_eq!(grant.grant_id, "grant_1");
        assert_eq!(grant.scopes, vec!["issues:write"]);
        assert_eq!(grant.status, Some(LocalAdmissionGrantStatus::Active));
        assert_eq!(grant.authority_kind, Some(AuthorityKind::Constructive));
        Ok(())
    }

    #[test]
    fn graph_scope_decision_serializes_camel_case_and_empty_arrays() -> Result<(), serde_json::Error>
    {
        let decision = GraphScopeAdmissionDecision::Allow {
            reasons: vec!["graph step requested no scopes".to_owned()],
            step_id: "deploy".to_owned(),
            requested_scopes: Vec::new(),
            granted_scopes: Vec::new(),
            grant_id: Some("grant_1".to_owned()),
        };

        let json = serde_json::to_string(&decision)?;

        assert_eq!(
            json,
            r#"{"status":"allow","reasons":["graph step requested no scopes"],"stepId":"deploy","requestedScopes":[],"grantedScopes":[],"grantId":"grant_1"}"#,
        );
        Ok(())
    }
}
