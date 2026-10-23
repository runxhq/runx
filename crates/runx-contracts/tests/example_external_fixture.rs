use std::path::Path;

use serde::Deserialize;

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    OperationalPolicy, OperationalPolicyAction, OperationalPolicyAdmissionRequest,
    OperationalPolicyAdmissionStatus, admit_operational_policy_request,
};

const FIXTURE_JSON: &str =
    include_str!("../../../fixtures/external/example/code-intake/api-source-thread.json");
const POLICY_JSON: &str = include_str!("../../../fixtures/operational-policy/provider-like.json");

#[derive(Debug, Deserialize)]
struct ExternalExampleFixture {
    schema: String,
    fixture_id: String,
    source: ExternalSource,
    signal: ExternalSignal,
    runtime_fixtures: Vec<String>,
    target: ExternalTarget,
}

#[derive(Debug, Deserialize)]
struct ExternalSource {
    source_id: String,
    provider: NonEmptyString,
    locator: String,
    thread_locator: String,
    thread_ts: String,
    issue_url: String,
}

#[derive(Debug, Deserialize)]
struct ExternalSignal {
    digest: String,
}

#[derive(Debug, Deserialize)]
struct ExternalTarget {
    repo: String,
    action: String,
    runner_id: String,
}

#[test]
fn example_external_fixture_is_admitted_by_operational_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture: ExternalExampleFixture = serde_json::from_str(FIXTURE_JSON)?;
    let policy: OperationalPolicy = serde_json::from_str(POLICY_JSON)?;

    assert_eq!(fixture.schema, "runx.external_dogfood_fixture.v1");
    assert_eq!(fixture.fixture_id, "example-api-source-thread");
    assert_eq!(fixture.source.provider.as_str(), "slack");
    assert_eq!(fixture.source.locator, "slack://example/C0APFMY0V8Q");
    assert_eq!(fixture.source.thread_ts, "1778834840.485629");
    assert!(fixture.source.issue_url.contains("/issues/"));
    assert_eq!(fixture.signal.digest, "sha256:example-source-482");
    assert_eq!(fixture.target.action, "issue-to-pr");

    let admission = admit_operational_policy_request(
        &policy,
        &OperationalPolicyAdmissionRequest {
            source_id: Some(fixture.source.source_id.clone()),
            target_repo: Some(fixture.target.repo.clone()),
            action: OperationalPolicyAction::IssueToPr,
            runner_id: Some(fixture.target.runner_id.clone()),
            source_thread_locator: Some(fixture.source.thread_locator.clone()),
        },
    )?;
    assert_eq!(admission.status, OperationalPolicyAdmissionStatus::Allow);
    assert_eq!(
        admission.source_id.as_deref(),
        Some(fixture.source.source_id.as_str())
    );
    assert_eq!(
        admission.target_repo.as_deref(),
        Some(fixture.target.repo.as_str())
    );
    assert_eq!(
        admission.runner_id.as_deref(),
        Some(fixture.target.runner_id.as_str())
    );
    assert!(admission.source_thread_required);
    assert!(admission.mutate_target_repo);
    assert!(admission.require_human_merge_gate);
    Ok(())
}

#[test]
fn example_external_fixture_cites_existing_runtime_fixtures()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture: ExternalExampleFixture = serde_json::from_str(FIXTURE_JSON)?;
    let root = repo_root()?;

    for runtime_fixture in &fixture.runtime_fixtures {
        assert!(
            root.join(runtime_fixture).exists(),
            "runtime fixture does not exist: {runtime_fixture}"
        );
    }

    Ok(())
}

fn repo_root() -> Result<&'static Path, Box<dyn std::error::Error>> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "runx-contracts crate is under crates/".into())
}
