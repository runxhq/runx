use runx_contracts::JsonValue;
use runx_core::policy::{
    BuildAuthorityProofOptions, CredentialBindingRequest, GraphScopeAdmissionRequest,
    LocalAdmissionGrant, LocalScopeAdmissionOptions, ScopeGrantPolicy, admit_graph_step_scopes,
    build_authority_proof_metadata, build_local_scope_admission, scope_grant_allows,
    validate_credential_binding,
};
use serde::Deserialize;

const FIXTURES: &[(&str, &str)] = &[
    (
        "authority-credential-binding-allows-matching",
        include_str!(
            "../../../fixtures/kernel/policy/authority-credential-binding-allows-matching.json"
        ),
    ),
    (
        "authority-credential-binding-denies-grant-reference",
        include_str!(
            "../../../fixtures/kernel/policy/authority-credential-binding-denies-grant-reference.json"
        ),
    ),
    (
        "authority-proof-metadata-full",
        include_str!("../../../fixtures/kernel/policy/authority-proof-metadata-full.json"),
    ),
    (
        "authority-scope-admission-active-grant",
        include_str!("../../../fixtures/kernel/policy/authority-scope-admission-active-grant.json"),
    ),
    (
        "authority-scope-admission-denied-before-grant",
        include_str!(
            "../../../fixtures/kernel/policy/authority-scope-admission-denied-before-grant.json"
        ),
    ),
    (
        "authority-scope-admission-no-connected-auth",
        include_str!(
            "../../../fixtures/kernel/policy/authority-scope-admission-no-connected-auth.json"
        ),
    ),
    (
        "authority-scope-admission-no-matching-grant",
        include_str!(
            "../../../fixtures/kernel/policy/authority-scope-admission-no-matching-grant.json"
        ),
    ),
    (
        "graph-scope-allows-empty-request",
        include_str!("../../../fixtures/kernel/policy/graph-scope-allows-empty-request.json"),
    ),
    (
        "graph-scope-allows-exact-match",
        include_str!("../../../fixtures/kernel/policy/graph-scope-allows-exact-match.json"),
    ),
    (
        "graph-scope-allows-wildcard-narrowing",
        include_str!("../../../fixtures/kernel/policy/graph-scope-allows-wildcard-narrowing.json"),
    ),
    (
        "graph-scope-deduplicates-requests",
        include_str!("../../../fixtures/kernel/policy/graph-scope-deduplicates-requests.json"),
    ),
    (
        "graph-scope-denies-empty-grant",
        include_str!("../../../fixtures/kernel/policy/graph-scope-denies-empty-grant.json"),
    ),
    (
        "graph-scope-denies-partial-widening",
        include_str!("../../../fixtures/kernel/policy/graph-scope-denies-partial-widening.json"),
    ),
    (
        "graph-scope-denies-prefix-wildcard-request",
        include_str!(
            "../../../fixtures/kernel/policy/graph-scope-denies-prefix-wildcard-request.json"
        ),
    ),
    (
        "graph-scope-denies-prefix-substring",
        include_str!("../../../fixtures/kernel/policy/graph-scope-denies-prefix-substring.json"),
    ),
    (
        "graph-scope-denies-prefix-nested-segment",
        include_str!(
            "../../../fixtures/kernel/policy/graph-scope-denies-prefix-nested-segment.json"
        ),
    ),
    (
        "graph-scope-denies-widening",
        include_str!("../../../fixtures/kernel/policy/graph-scope-denies-widening.json"),
    ),
    (
        "graph-scope-omits-grant-id-when-absent",
        include_str!("../../../fixtures/kernel/policy/graph-scope-omits-grant-id-when-absent.json"),
    ),
    (
        "scope-grant-delegated-allows-one-segment",
        include_str!(
            "../../../fixtures/kernel/policy/scope-grant-delegated-allows-one-segment.json"
        ),
    ),
    (
        "scope-grant-delegated-denies-empty-namespace",
        include_str!(
            "../../../fixtures/kernel/policy/scope-grant-delegated-denies-empty-namespace.json"
        ),
    ),
    (
        "scope-grant-delegated-denies-nested-segment",
        include_str!(
            "../../../fixtures/kernel/policy/scope-grant-delegated-denies-nested-segment.json"
        ),
    ),
    (
        "scope-grant-delegated-denies-universal",
        include_str!("../../../fixtures/kernel/policy/scope-grant-delegated-denies-universal.json"),
    ),
    (
        "scope-grant-exact-only-allows-exact",
        include_str!("../../../fixtures/kernel/policy/scope-grant-exact-only-allows-exact.json"),
    ),
    (
        "scope-grant-exact-only-denies-wildcard-expansion",
        include_str!(
            "../../../fixtures/kernel/policy/scope-grant-exact-only-denies-wildcard-expansion.json"
        ),
    ),
    (
        "scope-grant-trusted-allows-universal",
        include_str!("../../../fixtures/kernel/policy/scope-grant-trusted-allows-universal.json"),
    ),
];

#[derive(Debug, Deserialize)]
struct Fixture {
    input: PolicyInput,
    expected: ExpectedOutput,
}

#[derive(Debug, Deserialize)]
struct ExpectedOutput {
    value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
enum PolicyInput {
    #[serde(rename = "policy.admitGraphStepScopes")]
    AdmitGraphStepScopes { request: GraphScopeAdmissionRequest },
    #[serde(rename = "policy.buildLocalScopeAdmission")]
    BuildLocalScopeAdmission {
        auth: Option<JsonValue>,
        #[serde(default)]
        grants: Vec<LocalAdmissionGrant>,
        #[serde(default)]
        options: LocalScopeAdmissionOptions,
    },
    #[serde(rename = "policy.buildAuthorityProofMetadata")]
    BuildAuthorityProofMetadata {
        options: Box<BuildAuthorityProofOptions>,
    },
    #[serde(rename = "policy.validateCredentialBinding")]
    ValidateCredentialBinding {
        request: Box<CredentialBindingRequest>,
    },
    #[serde(rename = "policy.scopeGrantAllows")]
    ScopeGrantAllows {
        #[serde(rename = "grantedScope")]
        granted_scope: String,
        #[serde(rename = "requestedScope")]
        requested_scope: String,
        policy: ScopeGrantPolicy,
    },
}

#[test]
fn policy_fixtures_match_rust_policy() -> Result<(), serde_json::Error> {
    for (name, source) in FIXTURES {
        let fixture: Fixture = serde_json::from_str(source)?;
        let actual = evaluate_policy_input(fixture.input)?;
        assert_eq!(actual, fixture.expected.value, "{name}");
    }
    Ok(())
}

fn evaluate_policy_input(input: PolicyInput) -> Result<serde_json::Value, serde_json::Error> {
    match input {
        PolicyInput::AdmitGraphStepScopes { request } => {
            serde_json::to_value(admit_graph_step_scopes(&request))
        }
        PolicyInput::BuildLocalScopeAdmission {
            auth,
            grants,
            options,
        } => serde_json::to_value(build_local_scope_admission(
            auth.as_ref(),
            &grants,
            &options,
        )),
        PolicyInput::BuildAuthorityProofMetadata { options } => {
            serde_json::to_value(build_authority_proof_metadata(&options))
        }
        PolicyInput::ValidateCredentialBinding { request } => {
            serde_json::to_value(validate_credential_binding(&request))
        }
        PolicyInput::ScopeGrantAllows {
            granted_scope,
            requested_scope,
            policy,
        } => serde_json::to_value(scope_grant_allows(&granted_scope, &requested_scope, policy)),
    }
}
