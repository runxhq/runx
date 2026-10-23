use serde::Deserialize;

use runx_contracts::{ActAssignment, BuildActAssignment};
use runx_sdk::act::assignment::build_act_assignment;

const FIXTURES: &[&str] = &[
    include_str!("../../../../fixtures/sdk-rust/act-assignment/cli-no-trigger.json"),
    include_str!("../../../../fixtures/sdk-rust/act-assignment/github-trigger.json"),
];

#[derive(Debug, Deserialize)]
struct Fixture {
    input: BuildActAssignment,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Expected {
    envelope: ActAssignment,
    intent_key: String,
    trigger_key: Option<String>,
    content_digest: String,
}

#[test]
fn sdk_act_assignment_wrappers_match_contract_fixtures() -> Result<(), serde_json::Error> {
    for fixture_json in FIXTURES {
        let fixture: Fixture = serde_json::from_str(fixture_json)?;
        let actual = build_act_assignment(fixture.input);

        assert_eq!(actual, fixture.expected.envelope);
        assert_eq!(actual.idempotency.intent_key, fixture.expected.intent_key);
        assert_eq!(
            actual.idempotency.trigger_key.as_deref(),
            fixture.expected.trigger_key.as_deref()
        );
        assert_eq!(
            actual.idempotency.content_digest,
            fixture.expected.content_digest
        );
    }
    Ok(())
}
