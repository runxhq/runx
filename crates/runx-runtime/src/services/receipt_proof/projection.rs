use runx_contracts::{JsonNumber, JsonObject, JsonValue};

use super::ProofProjection;
use crate::RuntimeError;
use crate::receipts::inspection::ReceiptInspection;

pub(super) fn proof_packet(
    receipt_ids: Vec<String>,
    projection: ProofProjection,
    store_label: &str,
    complete: bool,
    trees_valid: bool,
) -> JsonObject {
    let intact = complete && trees_valid;
    let decision = if intact {
        "verified"
    } else {
        "needs_more_evidence"
    };
    let ProofProjection {
        matched,
        details,
        trees,
        findings,
    } = projection;
    let verification = verification_object(intact, trees, findings);
    JsonObject::from([
        (
            "schema".to_owned(),
            JsonValue::String("runx.receipt.proof.v1".to_owned()),
        ),
        (
            "decision".to_owned(),
            JsonValue::String(decision.to_owned()),
        ),
        (
            "requested_receipt_ids".to_owned(),
            JsonValue::Array(receipt_ids.into_iter().map(JsonValue::String).collect()),
        ),
        ("matched_receipts".to_owned(), JsonValue::Array(matched)),
        ("receipt_details".to_owned(), JsonValue::Array(details)),
        ("verification".to_owned(), JsonValue::Object(verification)),
        (
            "store".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "label".to_owned(),
                JsonValue::String(store_label.to_owned()),
            )])),
        ),
    ])
}

fn verification_object(
    intact: bool,
    trees: Vec<JsonValue>,
    findings: Vec<JsonValue>,
) -> JsonObject {
    JsonObject::from([
        ("checked".to_owned(), JsonValue::Bool(true)),
        ("intact".to_owned(), JsonValue::Bool(intact)),
        ("trees".to_owned(), JsonValue::Array(trees)),
        ("findings".to_owned(), JsonValue::Array(findings)),
    ])
}

pub(super) fn tree_findings(findings: &[runx_receipts::ReceiptFinding]) -> Vec<JsonValue> {
    findings
        .iter()
        .map(|finding| {
            JsonValue::Object(JsonObject::from([
                (
                    "code".to_owned(),
                    JsonValue::String(format!("{:?}", finding.code)),
                ),
                ("path".to_owned(), JsonValue::String(finding.path.clone())),
                (
                    "message".to_owned(),
                    JsonValue::String(finding.message.clone()),
                ),
            ]))
        })
        .collect()
}

pub(super) fn matched_receipt(inspection: &ReceiptInspection) -> JsonValue {
    JsonValue::Object(JsonObject::from([
        (
            "receipt_id".to_owned(),
            JsonValue::String(inspection.id.clone()),
        ),
        (
            "skill_ref".to_owned(),
            JsonValue::String(inspection.subject_ref.clone()),
        ),
        (
            "status".to_owned(),
            JsonValue::String(inspection.status.clone()),
        ),
        (
            "created_at".to_owned(),
            JsonValue::String(inspection.created_at.clone()),
        ),
        (
            "verification_status".to_owned(),
            JsonValue::String(inspection.verification.status.clone()),
        ),
    ]))
}

pub(super) fn tree_projection(
    receipt_id: &str,
    receipt_count: usize,
    valid: bool,
    findings: Vec<JsonValue>,
) -> JsonValue {
    JsonValue::Object(JsonObject::from([
        (
            "root_receipt_id".to_owned(),
            JsonValue::String(receipt_id.to_owned()),
        ),
        (
            "receipt_count".to_owned(),
            JsonValue::Number(JsonNumber::U64(receipt_count as u64)),
        ),
        ("valid".to_owned(), JsonValue::Bool(valid)),
        ("findings".to_owned(), JsonValue::Array(findings)),
    ]))
}

pub(super) fn to_json_value(value: &impl serde::Serialize) -> Result<JsonValue, RuntimeError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|source| RuntimeError::json("serializing receipt proof projection", source))?;
    serde_json::from_slice(&bytes)
        .map_err(|source| RuntimeError::json("projecting receipt proof JSON", source))
}
