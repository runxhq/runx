#![cfg(feature = "process-tool")]

use std::path::{Path, PathBuf};

use runx_contracts::{ClosureDisposition, JsonValue};
use runx_receipts::validate_receipt;
use runx_runtime::{
    HarnessExpectedStatus, HarnessReplayOutput, adapters::process_tool::ProcessToolAdapter,
    load_harness_fixture, run_harness_fixture_with_adapter,
};

#[test]
fn issue_intake_generated_fixtures_replay_to_receipts() -> Result<(), Box<dyn std::error::Error>> {
    for case_name in [
        "bounded-docs-fix",
        "feature-needs-decomposition",
        "reply-only-question",
        "request-review-before-mutation",
    ] {
        let output = run_case(case_name)?;
        assert_eq!(output.status, HarnessExpectedStatus::Sealed, "{case_name}");
        assert_eq!(output.receipt.seal.disposition, ClosureDisposition::Closed);
        validate_receipt(&output.receipt)
            .map_err(|verification| format!("{case_name}: {:?}", verification.findings))?;
        assert!(
            output.receipt.id.starts_with("sha256:"),
            "{case_name}: receipt id must be content-addressed"
        );

        let payload = skill_payload(&output)?;
        assert_object_field(&payload, "intake_report", case_name)?;
        assert_object_field(&payload, "change_set", case_name)?;
        assert_object_field(&payload, "signal", case_name)?;

        // The agent step's receipt carries the act and the bound signal.
        let [intake] = output.step_receipts.as_slice() else {
            return Err(format!("{case_name}: expected one step receipt").into());
        };
        validate_receipt(intake)
            .map_err(|verification| format!("{case_name}: {:?}", verification.findings))?;
        assert!(
            !intake.signals.is_empty(),
            "{case_name}: receipt should bind the emitted signal"
        );
        let [act] = intake.acts.as_slice() else {
            return Err(format!("{case_name}: expected one contained act").into());
        };
        assert!(
            !act.source_refs.is_empty(),
            "{case_name}: act should bind source event refs"
        );
        assert!(
            !act.artifact_refs.is_empty(),
            "{case_name}: act should bind target surface refs"
        );
    }
    Ok(())
}

#[test]
fn issue_intake_request_review_fixture_preserves_review_gate()
-> Result<(), Box<dyn std::error::Error>> {
    let output = run_case("request-review-before-mutation")?;
    let payload = skill_payload(&output)?;
    let intake_report = object_field(&payload, "intake_report")?;

    assert_eq!(
        string_field(intake_report, "action_decision")?,
        "request_review"
    );
    assert_eq!(string_field(intake_report, "review_target")?, "thread");
    assert!(
        string_field(intake_report, "review_comment")?.contains("runx is holding mutation"),
        "review comment should preserve the public stop reason"
    );
    Ok(())
}

#[test]
fn issue_intake_generated_fixtures_keep_product_skill_source_unchanged()
-> Result<(), Box<dyn std::error::Error>> {
    let skill = std::fs::read_to_string(repo_root().join("skills/issue-intake/SKILL.md"))?;
    assert!(skill.contains("name: issue-intake"));
    assert!(skill.contains("Always emit `change_set` alongside `intake_report`."));
    assert!(skill.contains("Also emit `signal` when a source event is admitted."));
    assert!(skill.contains("Close the Runx turn after intake is complete"));

    for case_name in [
        "bounded-docs-fix",
        "feature-needs-decomposition",
        "reply-only-question",
        "request-review-before-mutation",
    ] {
        let fixture = load_harness_fixture(case_path(case_name))?;
        assert_eq!(
            fixture.metadata.get("product_skill"),
            Some(&JsonValue::String("issue-intake".to_owned()))
        );
    }
    Ok(())
}

fn run_case(case_name: &str) -> Result<HarnessReplayOutput, Box<dyn std::error::Error>> {
    Ok(run_harness_fixture_with_adapter(
        case_path(case_name),
        ProcessToolAdapter,
        crate::support::local_harness_options(),
    )?)
}

fn skill_payload(output: &HarnessReplayOutput) -> Result<JsonValue, Box<dyn std::error::Error>> {
    let skill_output = output
        .skill_output
        .as_ref()
        .ok_or("agent-task fixture did not produce skill output")?;
    Ok(skill_output.value.clone())
}

fn assert_object_field(
    payload: &JsonValue,
    field: &str,
    case_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    object_field(payload, field)
        .map(|_| ())
        .map_err(|error| format!("{case_name}: {error}").into())
}

fn object_field<'a>(
    payload: &'a JsonValue,
    field: &str,
) -> Result<&'a runx_contracts::JsonObject, Box<dyn std::error::Error>> {
    let JsonValue::Object(object) = payload else {
        return Err("payload is not an object".into());
    };
    match object.get(field) {
        Some(JsonValue::Object(value)) => Ok(value),
        Some(_) => Err(format!("{field} is not an object").into()),
        None => Err(format!("{field} is missing").into()),
    }
}

fn string_field<'a>(
    object: &'a runx_contracts::JsonObject,
    field: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    match object.get(field) {
        Some(JsonValue::String(value)) => Ok(value),
        Some(_) => Err(format!("{field} is not a string").into()),
        None => Err(format!("{field} is missing").into()),
    }
}

fn case_path(case_name: &str) -> PathBuf {
    fixture_root().join(format!("{case_name}.yaml"))
}

fn fixture_root() -> PathBuf {
    repo_root().join("fixtures/runtime/skills/issue-intake/cases")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
