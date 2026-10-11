#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use runx_contracts::{ClosureDisposition, JsonObject, JsonValue, ReceiptSchema};
use runx_receipts::canonical_receipt_json;
use runx_runtime::{
    HarnessExpectedStatus, HarnessFixtureKind, HarnessOptions, InvocationOutput, SkillAdapter,
    SkillInvocation, load_harness_fixture, run_harness_fixture_with_adapter,
};

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";

#[test]
fn loads_active_harness_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    for (path, expected_status, expected_disposition) in [
        (
            "fixtures/harness/echo-skill.yaml",
            HarnessExpectedStatus::Sealed,
            ClosureDisposition::Closed,
        ),
        (
            "fixtures/harness/sequential-graph.yaml",
            HarnessExpectedStatus::Sealed,
            ClosureDisposition::Closed,
        ),
    ] {
        let fixture = load_harness_fixture(fixture_path(path))?;
        assert_eq!(fixture.expect.status, Some(expected_status));
        let receipt = fixture.expect.receipt.ok_or("missing expect.receipt")?;
        assert_eq!(receipt.schema, ReceiptSchema::V1);
        // A suspended (deferred) run carries the "deferred" state; every
        // terminal seal carries "sealed".
        let expected_state = if expected_disposition == ClosureDisposition::Deferred {
            "deferred"
        } else {
            "sealed"
        };
        assert_eq!(receipt.state.as_deref(), Some(expected_state));
        assert_eq!(receipt.disposition, Some(expected_disposition));
    }
    Ok(())
}

#[test]
fn parses_harness_skill_fixture_contract() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = load_harness_fixture(fixture_path("fixtures/harness/echo-skill.yaml"))?;

    assert_eq!(fixture.name, "echo-skill");
    assert_eq!(fixture.kind, HarnessFixtureKind::Skill);
    assert_eq!(fixture.target, "../skills/echo");
    let receipt = fixture.expect.receipt.ok_or("missing expect.receipt")?;
    assert_eq!(
        receipt.subject_ref.as_deref(),
        Some("runx:run:harness-echo-skill")
    );
    assert_eq!(receipt.reason_code.as_deref(), Some("graph_closed"));
    assert_eq!(receipt.child_receipt_count, Some(1));
    Ok(())
}

#[test]
fn parses_harness_graph_fixture_contract() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = load_harness_fixture(fixture_path("fixtures/harness/sequential-graph.yaml"))?;

    assert_eq!(fixture.name, "sequential-graph");
    assert_eq!(fixture.kind, HarnessFixtureKind::Graph);
    assert_eq!(fixture.target, "../graphs/sequential/graph.yaml");
    assert_eq!(
        fixture.expect.steps,
        Some(vec!["first".to_owned(), "second".to_owned()])
    );
    let receipt = fixture.expect.receipt.ok_or("missing expect.receipt")?;
    assert_eq!(
        receipt.subject_ref.as_deref(),
        Some("runx:run:harness-graph")
    );
    assert_eq!(
        receipt.child_receipt_refs,
        Some(vec![
            "runx:receipt:sha256:29ef9a7302fa318b112fc44ae50a6b27784b9d61fdb28290df0931d0843a6021"
                .to_owned(),
            "runx:receipt:sha256:0b231dbaf8767a447711e2131c4106a9d44aef64fa37efc7f72ec45b9e2b4055"
                .to_owned()
        ])
    );
    // An omitted list asserts nothing; an empty one asserts there is none.
    assert_eq!(receipt.verification_refs, None);
    Ok(())
}

#[test]
fn replays_active_harness_skill_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let output = run_fixture_with_test_adapter("fixtures/harness/echo-skill.yaml")?;

    assert_eq!(output.status, HarnessExpectedStatus::Sealed);
    assert_eq!(
        output.receipt.subject.reference.uri,
        "runx:run:harness-echo-skill"
    );
    assert_eq!(output.receipt.seal.disposition, ClosureDisposition::Closed);
    assert_eq!(output.step_receipts.len(), 1);
    let skill_output = output.skill_output.ok_or("missing skill_output")?;
    assert_eq!(
        skill_output.value,
        JsonValue::Object(JsonObject::from([(
            "message".to_owned(),
            JsonValue::String("hello from harness".to_owned()),
        )]))
    );
    Ok(())
}

#[test]
fn replays_active_harness_graph_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let output = run_fixture_with_test_adapter("fixtures/harness/sequential-graph.yaml")?;

    assert_eq!(output.status, HarnessExpectedStatus::Sealed);
    assert_eq!(
        output.receipt.subject.reference.uri,
        "runx:run:harness-graph"
    );
    assert_eq!(output.receipt.seal.disposition, ClosureDisposition::Closed);
    assert_eq!(output.step_receipts.len(), 2);
    // Neither step makes a governed call, so neither seals an act.
    assert!(
        output
            .step_receipts
            .iter()
            .all(|receipt| receipt.acts.is_empty())
    );
    assert_eq!(
        output
            .steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    Ok(())
}

#[test]
fn replay_receipts_match_checked_in_canonical_oracles() -> Result<(), Box<dyn std::error::Error>> {
    let echo = run_fixture_with_test_adapter("fixtures/harness/echo-skill.yaml")?;
    assert_oracle(
        "fixtures/harness/oracle/echo-skill.receipt.json",
        &canonical_receipt_json(&echo.receipt)?,
    )?;
    assert_oracle(
        "fixtures/harness/oracle/echo-skill.default.json",
        &canonical_receipt_json(&echo.step_receipts[0])?,
    )?;

    let graph = run_fixture_with_test_adapter("fixtures/harness/sequential-graph.yaml")?;
    assert_oracle(
        "fixtures/harness/oracle/sequential-graph.receipt.json",
        &canonical_receipt_json(&graph.receipt)?,
    )?;
    assert_oracle(
        "fixtures/harness/oracle/sequential-graph.first.json",
        &canonical_receipt_json(&graph.step_receipts[0])?,
    )?;
    assert_oracle(
        "fixtures/harness/oracle/sequential-graph.second.json",
        &canonical_receipt_json(&graph.step_receipts[1])?,
    )?;

    Ok(())
}

/// A seeded receipt reads through the strict wire reader: one carrying a
/// member its issuer never writes is refused before it reaches the store.
#[test]
fn harness_setup_refuses_a_receipt_not_in_wire_form() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let package = workspace.path().join("echo");
    std::fs::create_dir_all(package.join("receipts"))?;
    for file in ["SKILL.md", "run.sh", "run.yaml"] {
        std::fs::copy(
            fixture_path("fixtures/skills/echo").join(file),
            package.join(file),
        )?;
    }
    let spine: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(fixture_path(
        "fixtures/contracts/harness-spine/receipt-success.json",
    ))?)?;
    let mut receipt = spine["expected"].clone();
    receipt["lineage"]["previous"] = serde_json::Value::Null;
    std::fs::write(
        package.join("receipts/seed.json"),
        serde_json::to_vec(&receipt)?,
    )?;
    let fixture = workspace.path().join("seeded.yaml");
    std::fs::write(
        &fixture,
        "name: seeded\nkind: skill\ntarget: echo\ninputs:\n  message: hi\n\
         setup:\n  receipts:\n    - receipts/seed.json\nexpect:\n  status: sealed\n",
    )?;

    let error = run_harness_fixture_with_adapter(&fixture, TestAdapter, fixture_runtime_options()?)
        .err()
        .ok_or("a receipt outside its wire form must be refused")?;
    assert!(
        matches!(
            error,
            runx_runtime::HarnessReplayError::SetupReceiptInvalid {
                source: runx_receipts::ReceiptDecodeError::NotWireForm { .. },
                ..
            }
        ),
        "{error}"
    );
    Ok(())
}

#[test]
fn a_case_whose_run_errors_still_proves_only_the_requests_it_made()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let package = workspace.path().join("echo");
    std::fs::create_dir_all(&package)?;
    for file in ["SKILL.md", "run.sh", "run.yaml"] {
        std::fs::copy(
            fixture_path("fixtures/skills/echo").join(file),
            package.join(file),
        )?;
    }
    // The run ends before any step, so the write the case declares was never
    // sent, and the case cannot pass as though it had been.
    let fixture = workspace.path().join("unsent.yaml");
    std::fs::write(
        &fixture,
        "name: unsent\nkind: skill\ntarget: echo\nrunner: absent\ninputs:\n  message: hi\n\
         caller:\n  http_exchanges:\n    - request: { method: PATCH, url: \"https://fixture.runx.invalid/item\", body: none }\n\
         \x20     response: { status: 200, headers: { content-type: application/json }, body: \"{}\" }\n\
         expect:\n  status: failure\n",
    )?;

    let error = run_harness_fixture_with_adapter(&fixture, TestAdapter, fixture_runtime_options()?)
        .err()
        .ok_or("a case that leaves a declared exchange unsent must fail")?;
    assert!(
        matches!(
            &error,
            runx_runtime::HarnessReplayError::Mismatch { field, actual, .. }
                if field == "caller.http_exchanges"
                    && actual == "never requested: PATCH https://fixture.runx.invalid/item"
        ),
        "{error}"
    );
    Ok(())
}

fn run_fixture_with_test_adapter(
    relative_path: &str,
) -> Result<runx_runtime::HarnessReplayOutput, runx_runtime::HarnessReplayError> {
    run_harness_fixture_with_adapter(
        fixture_path(relative_path),
        TestAdapter,
        fixture_runtime_options()?,
    )
}

fn fixture_runtime_options() -> Result<HarnessOptions, runx_runtime::RuntimeError> {
    let mut options = HarnessOptions::new(std::env::vars().collect())?;
    options.clock = runx_runtime::RunClock::Fixed(FIXTURE_CREATED_AT.to_owned());
    Ok(options)
}

struct TestAdapter;

impl SkillAdapter for TestAdapter {
    fn invoke(
        &self,
        request: SkillInvocation,
    ) -> Result<InvocationOutput, runx_runtime::RuntimeError> {
        let message = request
            .inputs
            .get("message")
            .and_then(|value| match value {
                JsonValue::String(value) => Some(value.as_str()),
                _ => None,
            })
            .unwrap_or_default()
            .to_owned();
        // Emit a structured `{ "message": ... }` claim so the producing step's
        // declared artifact contract (for example json-output's `result` packet)
        // is addressable by downstream context edges under the contract model.
        let mut claim = JsonObject::default();
        claim.insert("message".to_owned(), JsonValue::String(message));
        Ok(InvocationOutput::runtime_success(
            JsonValue::Object(claim),
            0,
        ))
    }
}

fn fixture_path(relative_path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative_path)
}

fn assert_oracle(relative_path: &str, actual: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = fixture_path(relative_path);
    if std::env::var("RUNX_REGEN_FIXTURES").is_ok() {
        std::fs::write(&path, format!("{actual}\n"))?;
        return Ok(());
    }
    let expected = std::fs::read_to_string(path)?;
    assert_eq!(expected, format!("{actual}\n"), "{relative_path}");
    Ok(())
}
