use runx_core::policy::{ScopeGrantPolicy, scope_grant_allows};
use serde::Deserialize;

const FIXTURES: &[(&str, &str)] = &[
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
        PolicyInput::ScopeGrantAllows {
            granted_scope,
            requested_scope,
            policy,
        } => serde_json::to_value(scope_grant_allows(&granted_scope, &requested_scope, policy)),
    }
}
