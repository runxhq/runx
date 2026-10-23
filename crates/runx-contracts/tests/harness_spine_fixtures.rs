use serde::Deserialize;

use runx_contracts::{Receipt, Signal};

use crate::fixture_support::roundtrip;

const FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/contracts/harness-spine/receipt-abnormal.json"),
    include_str!("../../../fixtures/contracts/harness-spine/receipt-success.json"),
    include_str!("../../../fixtures/contracts/harness-spine/signal-digest-links.json"),
];

#[derive(Debug, Deserialize)]
struct Fixture {
    fixture_kind: FixtureKind,
    expected: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixtureKind {
    Receipt,
    Signal,
}

#[test]
fn harness_spine_fixtures_roundtrip() -> Result<(), serde_json::Error> {
    for fixture_json in FIXTURES {
        let fixture: Fixture = serde_json::from_str(fixture_json)?;
        assert_roundtrip(fixture)?;
    }
    Ok(())
}

#[test]
fn receipt_rejects_unknown_fields() -> Result<(), serde_json::Error> {
    let mut fixture: Fixture = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/harness-spine/receipt-success.json"
    ))?;
    fixture.expected["unexpected"] = serde_json::json!(true);

    let result = serde_json::from_value::<Receipt>(fixture.expected);

    assert!(result.is_err());
    Ok(())
}

fn assert_roundtrip(fixture: Fixture) -> Result<(), serde_json::Error> {
    match fixture.fixture_kind {
        FixtureKind::Receipt => roundtrip::<Receipt>(fixture.expected),
        FixtureKind::Signal => roundtrip::<Signal>(fixture.expected),
    }
}
