use runx_contracts::{
    OperationalPolicy, OperationalPolicyAction, OperationalPolicySchema,
    lint_operational_policy_contract, project_operational_policy_readback,
    validate_operational_policy_contract,
};

const PROVIDER_LIKE: &str = include_str!("../../../fixtures/operational-policy/provider-like.json");
const MINIMAL_SINGLE_REPO: &str =
    include_str!("../../../fixtures/operational-policy/minimal-single-repo.json");
const INVALID_UNKNOWN_AUTOMATION: &str =
    include_str!("../../../fixtures/operational-policy/invalid-unknown-automation.json");
const INVALID_OWNER_ROUTE_MISMATCH: &str =
    include_str!("../../../fixtures/operational-policy/invalid-owner-route-mismatch.json");
const INVALID_SOURCE_THREAD_MISSING: &str =
    include_str!("../../../fixtures/operational-policy/invalid-source-thread-missing.json");
const INVALID_NO_AVAILABLE_AUTOMATION: &str =
    include_str!("../../../fixtures/operational-policy/invalid-no-available-automation.json");
const INVALID_NOT_SCAFLD_TARGET: &str =
    include_str!("../../../fixtures/operational-policy/invalid-not-scafld-target.json");
const INVALID_SCHEMA_LITERAL: &str =
    include_str!("../../../fixtures/operational-policy/invalid-schema-literal.json");
const INVALID_SECRET_FIELD: &str =
    include_str!("../../../fixtures/operational-policy/invalid-secret-field.json");

#[test]
fn positive_operational_policy_fixtures_are_valid() -> Result<(), Box<dyn std::error::Error>> {
    for fixture in [PROVIDER_LIKE, MINIMAL_SINGLE_REPO] {
        let policy: OperationalPolicy = serde_json::from_str(fixture)?;

        validate_operational_policy_contract(&policy)?;
        assert!(lint_operational_policy_contract(&policy)?.is_empty());
        assert_eq!(policy.schema, OperationalPolicySchema::V1);
        assert_eq!(
            policy.schema_version.to_string(),
            "runx.operational_policy.v1"
        );
    }
    Ok(())
}

#[test]
fn semantic_fixture_findings_are_stable() -> Result<(), Box<dyn std::error::Error>> {
    for (fixture, code) in [
        (INVALID_UNKNOWN_AUTOMATION, "unknown_automation"),
        (INVALID_OWNER_ROUTE_MISMATCH, "owner_route_target_mismatch"),
        (INVALID_SOURCE_THREAD_MISSING, "source_thread_required"),
        (
            INVALID_NO_AVAILABLE_AUTOMATION,
            "target_action_without_automation",
        ),
        (INVALID_NOT_SCAFLD_TARGET, "mutation_without_scafld"),
    ] {
        let policy: OperationalPolicy = serde_json::from_str(fixture)?;
        let findings = lint_operational_policy_contract(&policy)?;

        assert!(findings.iter().any(|finding| finding.code == code));
    }
    Ok(())
}

#[test]
fn schema_invalid_fixtures_are_rejected() {
    assert!(serde_json::from_str::<OperationalPolicy>(INVALID_SCHEMA_LITERAL).is_err());
    assert!(serde_json::from_str::<OperationalPolicy>(INVALID_SECRET_FIELD).is_err());
}

#[test]
fn invalid_created_at_is_rejected_like_typescript_schema() -> Result<(), Box<dyn std::error::Error>>
{
    let mut policy: OperationalPolicy = serde_json::from_str(PROVIDER_LIKE)?;

    policy.created_at = Some("2026-05-19 00:00:00".into());
    let missing_t = validate_operational_policy_contract(&policy);

    policy.created_at = Some("2026-05-19T00:00:00+10:00".into());
    let offset = validate_operational_policy_contract(&policy);

    assert!(missing_t.is_err());
    assert!(offset.is_err());
    Ok(())
}

#[test]
fn readback_redacts_source_locators() -> Result<(), Box<dyn std::error::Error>> {
    let policy: OperationalPolicy = serde_json::from_str(PROVIDER_LIKE)?;
    let readback = project_operational_policy_readback(&policy)?;
    let json = serde_json::to_string(&readback)?;

    assert!(readback.valid);
    assert_eq!(readback.sources[0].locator_count, 1);
    assert!(json.contains(r#""locator_count":1"#));
    assert!(!json.contains("slack://example"));
    Ok(())
}

#[test]
fn typed_action_names_match_contract_literals() {
    assert_eq!(
        OperationalPolicyAction::IssueToPr.to_string(),
        "issue-to-pr"
    );
    assert_eq!(OperationalPolicyAction::PrFixUp.to_string(), "pr-fix-up");
    assert_eq!(
        OperationalPolicyAction::MergeAssist.to_string(),
        "merge-assist"
    );
}
