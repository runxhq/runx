use runx_contracts::{ClosureDisposition, JsonValue, Receipt, ReceiptSchema};
use runx_receipts::{
    ReceiptProofContextProvider, canonical_receipt_body_digest, canonical_receipt_digest,
    verify_receipt_proof,
};

use crate::execution::harness::fixtures::{HarnessExpectedStatus, ReceiptExpectation};
use crate::execution::harness::json_assertions::assert_json_expectation;
use crate::execution::harness::runner::{HarnessReplayError, HarnessReplayOutput};
use crate::receipts::{RuntimeReceiptProofContextProvider, RuntimeReceiptSignaturePolicy};

mod replayed_answers;

use replayed_answers::assert_caller_answers_replayed;

#[derive(Clone, Debug, PartialEq)]
pub struct HarnessReplayReceipt {
    pub receipt_id: String,
    pub subject_ref: String,
    pub state: String,
    pub disposition: ClosureDisposition,
    pub reason_code: String,
    pub act_ids: Vec<String>,
    pub decision_ids: Vec<String>,
    pub child_receipt_refs: Vec<String>,
    pub verification_refs: Vec<String>,
}

pub(super) fn assert_expectations(
    output: &HarnessReplayOutput,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<(), HarnessReplayError> {
    if let Some(expected_status) = &output.fixture.expect.status {
        // A run that stopped short for a request the case did not answer
        // names it.
        let (expected, actual) = (status_name(expected_status), status_name(&output.status));
        if expected != actual {
            let actual = match output.unanswered.as_slice() {
                [] => actual.to_owned(),
                open => format!("{actual} (unanswered: {})", open.join(", ")),
            };
            assert_equal("expect.status", expected, actual)?;
        }
    }
    if let Some(expected_receipt) = &output.fixture.expect.receipt {
        assert_receipt_expectation(expected_receipt, &output.receipt, signature_policy)?;
    }
    if let Some(expected) = &output.fixture.expect.steps {
        let actual = output
            .steps
            .iter()
            .map(|step| step.step_id.clone())
            .collect::<Vec<_>>();
        assert_list("expect.steps", expected, &actual)?;
    }
    assert_caller_answers_replayed(output)?;
    if let Some(expectation) = &output.fixture.expect.output {
        let actual = output
            .skill_output
            .as_ref()
            .map_or(JsonValue::Null, |skill_output| skill_output.value.clone());
        assert_json_expectation(expectation, &actual, "expect.output")?;
    }
    for (step_id, expectation) in &output.fixture.expect.step_outputs {
        let actual = output
            .steps
            .iter()
            .find(|step| step.step_id == *step_id)
            .map_or(JsonValue::Null, |step| {
                JsonValue::Object(step.contract.clone())
            });
        assert_json_expectation(
            expectation,
            &actual,
            &format!("expect.step_outputs.{step_id}"),
        )?;
    }
    Ok(())
}

pub(super) fn status_from_disposition(disposition: &ClosureDisposition) -> HarnessExpectedStatus {
    match disposition {
        ClosureDisposition::Closed => HarnessExpectedStatus::Sealed,
        ClosureDisposition::Deferred => HarnessExpectedStatus::NeedsAgent,
        ClosureDisposition::Blocked => HarnessExpectedStatus::PolicyDenied,
        ClosureDisposition::TimedOut
        | ClosureDisposition::Declined
        | ClosureDisposition::Failed
        | ClosureDisposition::Killed
        | ClosureDisposition::Superseded => HarnessExpectedStatus::Failure,
    }
}

pub(crate) fn assert_receipt_expectation(
    expected: &ReceiptExpectation,
    actual: &Receipt,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<(), HarnessReplayError> {
    assert_receipt_proof(actual, signature_policy)?;
    assert_equal(
        "expect.receipt.schema",
        schema_name(&expected.schema),
        schema_name(&actual.schema),
    )?;
    if let Some(expected_id) = &expected.receipt_id {
        assert_equal("expect.receipt.receipt_id", expected_id, &actual.id)?;
    }
    let summary = summarize_receipt(actual);
    assert_receipt_identity(expected, &summary)?;
    assert_receipt_lists(expected, &summary)?;
    if let Some(expected_count) = expected.child_receipt_count {
        assert_equal(
            "expect.receipt.child_receipt_count",
            expected_count.to_string(),
            summary.child_receipt_refs.len().to_string(),
        )?;
    }
    assert_receipt_digests(expected, actual)
}

fn assert_receipt_identity(
    expected: &ReceiptExpectation,
    summary: &HarnessReplayReceipt,
) -> Result<(), HarnessReplayError> {
    if let Some(expected_subject_ref) = &expected.subject_ref {
        assert_equal(
            "expect.receipt.subject_ref",
            expected_subject_ref,
            &summary.subject_ref,
        )?;
    }
    if let Some(expected_state) = &expected.state {
        assert_equal(
            "expect.receipt.state",
            expected_state.as_str(),
            summary.state.as_str(),
        )?;
    }
    if let Some(expected_disposition) = &expected.disposition {
        assert_equal(
            "expect.receipt.disposition",
            disposition_name(expected_disposition),
            disposition_name(&summary.disposition),
        )?;
    }
    if let Some(expected_reason_code) = &expected.reason_code {
        assert_equal(
            "expect.receipt.reason_code",
            expected_reason_code,
            &summary.reason_code,
        )?;
    }
    Ok(())
}

fn assert_receipt_lists(
    expected: &ReceiptExpectation,
    summary: &HarnessReplayReceipt,
) -> Result<(), HarnessReplayError> {
    for (field, expected, actual) in [
        (
            "expect.receipt.act_ids",
            &expected.act_ids,
            &summary.act_ids,
        ),
        (
            "expect.receipt.decision_ids",
            &expected.decision_ids,
            &summary.decision_ids,
        ),
        (
            "expect.receipt.child_receipt_refs",
            &expected.child_receipt_refs,
            &summary.child_receipt_refs,
        ),
        (
            "expect.receipt.verification_refs",
            &expected.verification_refs,
            &summary.verification_refs,
        ),
    ] {
        if let Some(expected) = expected {
            assert_list(field, expected, actual)?;
        }
    }
    Ok(())
}

fn assert_receipt_digests(
    expected: &ReceiptExpectation,
    actual: &Receipt,
) -> Result<(), HarnessReplayError> {
    if let Some(expected_body_digest) = &expected.body_digest {
        let body_digest = canonical_receipt_body_digest(actual).map_err(receipt_digest_error)?;
        assert_equal(
            "expect.receipt.body_digest",
            expected_body_digest,
            body_digest,
        )?;
    }
    if let Some(expected_digest) = &expected.receipt_digest {
        let receipt_digest = canonical_receipt_digest(actual).map_err(receipt_digest_error)?;
        assert_equal(
            "expect.receipt.receipt_digest",
            expected_digest,
            receipt_digest,
        )?;
    }
    Ok(())
}

fn assert_receipt_proof(
    receipt: &Receipt,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<(), HarnessReplayError> {
    let proof_contexts = RuntimeReceiptProofContextProvider::new(signature_policy);
    let context = proof_contexts.proof_context(receipt);
    let verification = verify_receipt_proof(receipt, &context);
    if verification.valid {
        Ok(())
    } else {
        Err(HarnessReplayError::ReceiptProofInvalid {
            receipt_id: receipt.id.to_string(),
            findings: format!("{:?}", verification.findings),
        })
    }
}

fn receipt_digest_error(error: runx_receipts::ReceiptError) -> HarnessReplayError {
    HarnessReplayError::ReceiptDigest {
        message: error.to_string(),
    }
}

fn summarize_receipt(receipt: &Receipt) -> HarnessReplayReceipt {
    let state = if matches!(receipt.seal.disposition, ClosureDisposition::Deferred) {
        "deferred".to_owned()
    } else {
        "sealed".to_owned()
    };
    HarnessReplayReceipt {
        receipt_id: receipt.id.to_string(),
        subject_ref: receipt.subject.reference.uri.clone().into_string(),
        state,
        disposition: receipt.seal.disposition.clone(),
        reason_code: receipt.seal.reason_code.to_string(),
        act_ids: receipt.acts.iter().map(|act| act.id.to_string()).collect(),
        decision_ids: receipt
            .decisions
            .iter()
            .map(|decision| decision.decision_id.as_str().to_owned())
            .collect(),
        child_receipt_refs: receipt
            .lineage
            .children
            .iter()
            .map(|reference| reference.uri.clone().into_string())
            .collect(),
        verification_refs: receipt
            .seal
            .criteria
            .iter()
            .flat_map(|binding| binding.verification_refs.iter())
            .map(|reference| reference.uri.clone().into_string())
            .collect(),
    }
}

/// A list asserted whole, in order: an empty expected list asserts there is
/// none.
fn assert_list(
    field: &'static str,
    expected: &[String],
    actual: &[String],
) -> Result<(), HarnessReplayError> {
    if expected == actual {
        return Ok(());
    }
    assert_equal(field, format!("{expected:?}"), format!("{actual:?}"))
}

fn assert_equal(
    field: impl Into<String>,
    expected: impl AsRef<str>,
    actual: impl AsRef<str>,
) -> Result<(), HarnessReplayError> {
    let expected = expected.as_ref();
    let actual = actual.as_ref();
    if expected == actual {
        return Ok(());
    }
    Err(HarnessReplayError::Mismatch {
        field: field.into(),
        expected: expected.to_owned(),
        actual: actual.to_owned(),
    })
}

fn schema_name(schema: &ReceiptSchema) -> &'static str {
    match schema {
        ReceiptSchema::V1 => "runx.receipt.v1",
    }
}

fn disposition_name(disposition: &ClosureDisposition) -> &'static str {
    match disposition {
        ClosureDisposition::Closed => "closed",
        ClosureDisposition::Deferred => "deferred",
        ClosureDisposition::Superseded => "superseded",
        ClosureDisposition::Declined => "declined",
        ClosureDisposition::Blocked => "blocked",
        ClosureDisposition::Failed => "failed",
        ClosureDisposition::Killed => "killed",
        ClosureDisposition::TimedOut => "timed_out",
    }
}

pub(crate) fn status_name(status: &HarnessExpectedStatus) -> &'static str {
    match status {
        HarnessExpectedStatus::Sealed => "sealed",
        HarnessExpectedStatus::Failure => "failure",
        HarnessExpectedStatus::NeedsAgent => "needs_agent",
        HarnessExpectedStatus::PolicyDenied => "policy_denied",
        HarnessExpectedStatus::Escalated => "escalated",
    }
}

#[cfg(test)]
mod tests {
    use super::assert_list;

    #[test]
    fn an_empty_expected_list_asserts_there_is_none() {
        assert!(assert_list("expect.receipt.act_ids", &[], &[]).is_ok());
        assert!(assert_list("expect.receipt.act_ids", &[], &["act_step".to_owned()]).is_err());
        assert!(
            assert_list(
                "expect.receipt.act_ids",
                &["act_step".to_owned()],
                &["act_step".to_owned()]
            )
            .is_ok()
        );
    }
}
