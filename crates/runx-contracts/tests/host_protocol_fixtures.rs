use serde::Deserialize;

use runx_contracts::{ExecutionEvent, ResolutionRequest, ResolutionResponse};

use crate::fixture_support::roundtrip;

const FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/contracts/host-protocol/event-admitted.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-auth_resolved.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-completed.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-executing.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-inputs_resolved.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-resolution_requested.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-resolution_resolved.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-skill_loaded.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-step_completed.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-step_started.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-step_waiting_resolution.json"),
    include_str!("../../../fixtures/contracts/host-protocol/event-warning.json"),
    include_str!("../../../fixtures/contracts/host-protocol/resolution-approval-request.json"),
    include_str!("../../../fixtures/contracts/host-protocol/resolution-turn-request.json"),
    include_str!("../../../fixtures/contracts/host-protocol/resolution-input-request.json"),
    include_str!("../../../fixtures/contracts/host-protocol/resolution-response.json"),
];

#[derive(Debug, Deserialize)]
struct Fixture {
    fixture_kind: FixtureKind,
    expected: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixtureKind {
    Event,
    ResolutionRequest,
    ResolutionResponse,
}

#[test]
fn host_protocol_fixtures_match_typescript_wire_shapes() -> Result<(), serde_json::Error> {
    for fixture_json in FIXTURES {
        let fixture: Fixture = serde_json::from_str(fixture_json)?;
        assert_roundtrip(fixture)?;
    }
    Ok(())
}

fn assert_roundtrip(fixture: Fixture) -> Result<(), serde_json::Error> {
    match fixture.fixture_kind {
        FixtureKind::Event => roundtrip::<ExecutionEvent>(fixture.expected),
        FixtureKind::ResolutionRequest => roundtrip::<ResolutionRequest>(fixture.expected),
        FixtureKind::ResolutionResponse => roundtrip::<ResolutionResponse>(fixture.expected),
    }
}
