use serde::Deserialize;

use runx_contracts::{HostRunResult, HostRunState};
use runx_sdk::host::{decode_host_result, decode_host_state, host_result_status};

#[derive(Debug, Deserialize)]
struct ResultFixture {
    expected: HostRunResult,
}

#[derive(Debug, Deserialize)]
struct StateFixture {
    expected: HostRunState,
}

#[derive(Debug, Deserialize)]
struct EmbeddedRuntimeBoundaryFixture {
    target: EmbeddedRuntimeBoundaryTarget,
    semantics: Vec<String>,
    host_result: HostRunResult,
}

#[derive(Debug, Deserialize)]
struct EmbeddedRuntimeBoundaryTarget {
    allowed_package_imports: Vec<String>,
    forbidden_package_imports: Vec<String>,
    boundary: String,
    sdk_role: String,
    trusted_executor: String,
    typescript_role: String,
}

#[test]
fn sdk_decodes_host_run_result_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    let fixture: ResultFixture = serde_json::from_str(include_str!(
        "../../../fixtures/sdk-rust/host-protocol/result-host-run-completed.json"
    ))?;
    let expected_json = serde_json::to_string(&fixture.expected)?;
    let decoded = decode_host_result(&expected_json)?;

    assert_eq!(host_result_status(&decoded), "completed");
    assert_eq!(decoded, fixture.expected);
    Ok(())
}

#[test]
fn sdk_decodes_host_state_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    let fixture: StateFixture = serde_json::from_str(include_str!(
        "../../../fixtures/sdk-rust/host-protocol/inspect-host-state-needs-agent.json"
    ))?;
    let expected_json = serde_json::to_string(&fixture.expected)?;
    let decoded = decode_host_state(&expected_json)?;

    assert_eq!(decoded, fixture.expected);
    Ok(())
}

#[test]
fn sdk_decodes_embedded_runtime_service_fixture_without_typescript_fallback()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture: EmbeddedRuntimeBoundaryFixture = serde_json::from_str(include_str!(
        "../../../fixtures/embedded-sdk-migration/runtime-service-boundary.json"
    ))?;
    let expected_json = serde_json::to_string(&fixture.host_result)?;
    let decoded = decode_host_result(&expected_json)?;

    assert_eq!(host_result_status(&decoded), "needs_agent");
    assert_eq!(decoded, fixture.host_result);
    assert_eq!(fixture.target.boundary, "runx-runtime-service");
    assert_eq!(fixture.target.trusted_executor, "runx-runtime");
    assert_eq!(fixture.target.typescript_role, "client_only");
    assert_eq!(fixture.target.sdk_role, "runx-sdk-cli-backed");
    assert!(fixture.semantics.contains(&"host_continuation".to_owned()));
    assert!(fixture.semantics.contains(&"auth_resolution".to_owned()));
    // The runtime resolves a connector's credential itself; a paused run
    // never asks its host for one.
    assert!(!expected_json.contains("credential"));
    assert!(
        !fixture
            .target
            .allowed_package_imports
            .iter()
            .any(|package| package == "@runxhq/runtime-local" || package == "@runxhq/adapters"),
        "embedded target must not allow hidden TypeScript runtime-local/adapters fallback"
    );
    assert!(
        fixture
            .target
            .forbidden_package_imports
            .contains(&"@runxhq/runtime-local".to_owned())
    );
    assert!(
        fixture
            .target
            .forbidden_package_imports
            .contains(&"@runxhq/adapters".to_owned())
    );
    Ok(())
}
