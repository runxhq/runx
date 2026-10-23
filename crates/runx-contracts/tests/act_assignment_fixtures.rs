use serde::Deserialize;

use runx_contracts::{
    ActAssignment, BuildActAssignment, IntentKeyInput, derive_content_digest, derive_intent_key,
    derive_trigger_key,
};

const FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/contracts/act-assignment/cli-no-trigger.json"),
    include_str!("../../../fixtures/contracts/act-assignment/github-trigger.json"),
    include_str!("../../../fixtures/contracts/act-assignment/system-empty-inputs.json"),
    include_str!("../../../fixtures/contracts/act-assignment/host-normalization.json"),
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
fn act_assignment_fixtures_match_typescript() -> Result<(), serde_json::Error> {
    for fixture_json in FIXTURES {
        let fixture: Fixture = serde_json::from_str(fixture_json)?;
        let actual = fixture.input.clone().build();

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

#[test]
fn act_assignment_digest_helpers_match_fixtures() -> Result<(), serde_json::Error> {
    for fixture_json in FIXTURES {
        let fixture: Fixture = serde_json::from_str(fixture_json)?;
        let input = fixture.input;
        let expected = fixture.expected;

        assert_eq!(
            derive_intent_key(IntentKeyInput {
                skill: input.skill,
                runner: input.runner,
                source_uri: input.source_uri,
                input_overrides: input.input_overrides.clone(),
            }),
            expected.intent_key,
        );
        assert_eq!(
            derive_trigger_key(input.host.kind, input.host.trigger_uri),
            expected.trigger_key,
        );
        assert_eq!(
            derive_content_digest(input.input_overrides),
            expected.content_digest
        );
    }
    Ok(())
}
