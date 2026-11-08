#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use runx_contracts::{JsonObject, JsonValue, Receipt, ReceiptSchema, RunOutcome};
use runx_receipts::canonical_receipt_json;
use runx_runtime::{TestExpectedStatus, load_test_fixture, run_test_fixture};

#[test]
fn loads_active_test_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    for (path, expected_status, expected_seal_outcome) in [
        (
            "fixtures/skills/echo/tests/echo-skill.yaml",
            TestExpectedStatus::Sealed,
            RunOutcome::Completed,
        ),
        (
            "fixtures/graphs/sequential/tests/sequential-workflow.yaml",
            TestExpectedStatus::Sealed,
            RunOutcome::Completed,
        ),
    ] {
        let fixture = load_test_fixture(fixture_path(path))?;
        assert_eq!(fixture.expect.status, Some(expected_status));
        let receipt = fixture.expect.receipt.ok_or("missing expect.receipt")?;
        assert_eq!(receipt.schema, ReceiptSchema::V1);
        // A suspended (deferred) run carries the "deferred" state; every
        // terminal seal carries "sealed".
        let expected_state = if expected_seal_outcome == RunOutcome::Deferred {
            "deferred"
        } else {
            "sealed"
        };
        assert_eq!(receipt.state.as_deref(), Some(expected_state));
        assert_eq!(receipt.outcome, Some(expected_seal_outcome));
    }
    Ok(())
}

#[test]
fn parses_test_skill_fixture_contract() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = load_test_fixture(fixture_path("fixtures/skills/echo/tests/echo-skill.yaml"))?;

    assert_eq!(fixture.name, "echo-skill");
    assert_eq!(fixture.package, None);
    let receipt = fixture.expect.receipt.ok_or("missing expect.receipt")?;
    assert_eq!(
        receipt.subject_uri.as_deref(),
        Some("runx:run:test-echo-skill")
    );
    assert_eq!(receipt.reason, None);
    assert_eq!(receipt.child_receipt_count, Some(1));
    Ok(())
}

#[test]
fn parses_test_workflow_fixture_contract() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = load_test_fixture(fixture_path(
        "fixtures/graphs/sequential/tests/sequential-workflow.yaml",
    ))?;

    assert_eq!(fixture.name, "sequential-workflow");
    assert_eq!(fixture.package, None);
    assert_eq!(
        fixture.expect.steps,
        Some(vec!["first".to_owned(), "second".to_owned()])
    );
    let receipt = fixture.expect.receipt.ok_or("missing expect.receipt")?;
    assert_eq!(
        receipt.subject_uri.as_deref(),
        Some("runx:run:test-sequential-workflow")
    );
    let oracle: Receipt = serde_json::from_str(&std::fs::read_to_string(fixture_path(
        "fixtures/harness/oracle/sequential-workflow.receipt.json",
    ))?)?;
    assert_eq!(
        receipt.child_receipt_uris,
        Some(
            oracle
                .lineage
                .children
                .iter()
                .map(|child| child.uri.to_string())
                .collect()
        )
    );
    // An omitted list asserts nothing; an empty one asserts there is none.
    assert_eq!(receipt.verification_uris, None);
    Ok(())
}

#[test]
fn replays_active_test_skill_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let output = run_fixture("fixtures/skills/echo/tests/echo-skill.yaml")?;

    assert_eq!(output.status, TestExpectedStatus::Sealed);
    assert_eq!(
        output.receipt.subject.reference.uri,
        "runx:run:test-echo-skill"
    );
    assert_eq!(output.receipt.seal.outcome, RunOutcome::Completed);
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
fn replays_active_test_workflow_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let output = run_fixture("fixtures/graphs/sequential/tests/sequential-workflow.yaml")?;

    assert_eq!(output.status, TestExpectedStatus::Sealed);
    assert_eq!(
        output.receipt.subject.reference.uri,
        "runx:run:test-sequential-workflow"
    );
    assert_eq!(output.receipt.seal.outcome, RunOutcome::Completed);
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
    let echo = run_fixture("fixtures/skills/echo/tests/echo-skill.yaml")?;
    assert_oracle(
        "fixtures/harness/oracle/echo-skill.receipt.json",
        &canonical_receipt_json(&echo.receipt)?,
    )?;
    assert_oracle(
        "fixtures/harness/oracle/echo-skill.default.json",
        &canonical_receipt_json(&echo.step_receipts[0])?,
    )?;

    let graph = run_fixture("fixtures/graphs/sequential/tests/sequential-workflow.yaml")?;
    assert_oracle(
        "fixtures/harness/oracle/sequential-workflow.receipt.json",
        &canonical_receipt_json(&graph.receipt)?,
    )?;
    assert_oracle(
        "fixtures/harness/oracle/sequential-workflow.first.json",
        &canonical_receipt_json(&graph.step_receipts[0])?,
    )?;
    assert_oracle(
        "fixtures/harness/oracle/sequential-workflow.second.json",
        &canonical_receipt_json(&graph.step_receipts[1])?,
    )?;

    Ok(())
}

/// A seeded receipt reads through the strict wire reader: one carrying a
/// member its issuer never writes is refused before it reaches the store.
#[test]
fn test_setup_refuses_a_receipt_not_in_wire_form() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let package = workspace.path().join("echo");
    std::fs::create_dir_all(package.join("receipts"))?;
    std::fs::create_dir_all(package.join("tests"))?;
    for file in ["SKILL.md", "run.sh", "run.yaml"] {
        std::fs::copy(
            fixture_path("fixtures/skills/echo").join(file),
            package.join(file),
        )?;
    }
    let spine: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(fixture_path(
        "fixtures/contracts/run-spine/receipt-success.json",
    ))?)?;
    let mut receipt = spine["expected"].clone();
    receipt["authority"]["mandate_ref"] = serde_json::Value::Null;
    std::fs::write(
        package.join("receipts/seed.json"),
        serde_json::to_vec(&receipt)?,
    )?;
    let fixture = package.join("tests/seeded.yaml");
    std::fs::write(
        &fixture,
        "name: seeded\nsetup:\n  receipts:\n    - receipts/seed.json\ninputs:\n  message: hi\n\
         expect:\n  status: sealed\n",
    )?;

    let error = run_test_fixture(&fixture, std::env::vars().collect())
        .err()
        .ok_or("a receipt outside its wire form must be refused")?;
    assert!(
        matches!(
            error,
            runx_runtime::TestReplayError::SetupReceiptInvalid {
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
    let package = workspace.path().join("agent-task");
    std::fs::create_dir_all(package.join("tests"))?;
    for file in ["SKILL.md", "run.yaml"] {
        std::fs::copy(
            fixture_path("fixtures/skills/agent-task").join(file),
            package.join(file),
        )?;
    }
    // The run errors in its first step, a turn whose scripted call names a
    // tool the turn does not offer, so the write the case declares was never
    // sent, and the case cannot pass as though it had been.
    let fixture = package.join("tests/unsent.yaml");
    std::fs::write(
        &fixture,
        "name: unsent\ninputs:\n  prompt: hi\n\
         host:\n  turns:\n    default:\n      calls:\n        - tool: absent\n      output: {verdict: ok}\n\
         providers:\n  http:\n    - request: { method: PATCH, url: \"https://fixture.runx.invalid/item\", body: none }\n\
         \x20     response: { status: 200, headers: { content-type: application/json }, body: \"{}\" }\n\
         expect:\n  status: failure\n",
    )?;

    let error = run_test_fixture(&fixture, std::env::vars().collect())
        .err()
        .ok_or("a case that leaves a declared exchange unsent must fail")?;
    assert!(
        matches!(
            &error,
            runx_runtime::TestReplayError::Mismatch { field, actual, .. }
                if field == "providers.http"
                    && actual == "never requested: PATCH https://fixture.runx.invalid/item"
        ),
        "{error}"
    );
    Ok(())
}

fn run_fixture(
    relative_path: &str,
) -> Result<runx_runtime::TestReplayOutput, runx_runtime::TestReplayError> {
    // The tools `runx harness` runs, so a replay seals what it seals.
    run_test_fixture(fixture_path(relative_path), std::env::vars().collect())
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
