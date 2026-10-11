use runx_contracts::{JsonObject, JsonValue};

use super::input::QueryRequest;
use crate::RuntimeError;
use crate::receipts::archive::ReceiptListing;

struct QueryProjection {
    store_label: String,
    filter: JsonValue,
    receipt_ids: Vec<String>,
    receipts: JsonValue,
    receipt_details: JsonValue,
    verification: JsonValue,
}

impl QueryProjection {
    fn into_packet(self) -> JsonObject {
        JsonObject::from([
            (
                "schema".to_owned(),
                JsonValue::String("runx.receipt.query.v1".to_owned()),
            ),
            (
                "source".to_owned(),
                JsonValue::String("native_receipt_store".to_owned()),
            ),
            (
                "store_label".to_owned(),
                JsonValue::String(self.store_label),
            ),
            ("filter".to_owned(), self.filter),
            (
                "receipt_ids".to_owned(),
                JsonValue::Array(
                    self.receipt_ids
                        .into_iter()
                        .map(JsonValue::String)
                        .collect(),
                ),
            ),
            ("receipts".to_owned(), self.receipts),
            ("receipt_details".to_owned(), self.receipt_details),
            ("verification".to_owned(), self.verification),
        ])
    }
}

pub(super) fn render_query(
    request: QueryRequest,
    history: Option<ReceiptListing>,
    proof: Option<JsonObject>,
    resolved_store_label: &str,
    proof_limit_exceeded: bool,
    proof_count: usize,
) -> Result<JsonObject, RuntimeError> {
    let history_receipts = history
        .as_ref()
        .map(|history| to_json(&history.receipts))
        .transpose()?
        .unwrap_or_else(|| JsonValue::Array(Vec::new()));
    let history_ids = history
        .as_ref()
        .map(|history| {
            history
                .receipts
                .iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let exact = request.exact_ids.is_some();
    let receipts = if exact {
        proof_array(&proof, "matched_receipts")
    } else {
        history_receipts
    };
    let matched_ids = if exact {
        matched_proof_ids(&proof)
    } else {
        history_ids
    };
    let receipt_details = proof_array(&proof, "receipt_details");
    let verification = verification(&proof, proof_limit_exceeded, proof_count);
    let store_label = history
        .as_ref()
        .map(|history| history.store_label.clone())
        .or_else(|| proof_store_label(&proof))
        .unwrap_or_else(|| resolved_store_label.to_owned());

    Ok(QueryProjection {
        store_label,
        filter: request.filter_json(),
        receipt_ids: matched_ids,
        receipts,
        receipt_details,
        verification,
    }
    .into_packet())
}

fn verification(proof: &Option<JsonObject>, limit_exceeded: bool, proof_count: usize) -> JsonValue {
    if limit_exceeded {
        return JsonValue::Object(JsonObject::from([
            ("checked".to_owned(), JsonValue::Bool(true)),
            ("intact".to_owned(), JsonValue::Null),
            ("trees".to_owned(), JsonValue::Array(Vec::new())),
            (
                "findings".to_owned(),
                JsonValue::Array(vec![JsonValue::Object(JsonObject::from([
                    (
                        "code".to_owned(),
                        JsonValue::String("receipt.proof.limit".to_owned()),
                    ),
                    (
                        "message".to_owned(),
                        JsonValue::String(format!(
                            "matched {proof_count} receipts; native tree proof is bounded to 100"
                        )),
                    ),
                ]))]),
            ),
        ]));
    }
    proof
        .as_ref()
        .and_then(|value| value.get("verification"))
        .cloned()
        .unwrap_or_else(unchecked_verification)
}

fn unchecked_verification() -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("checked".to_owned(), JsonValue::Bool(false)),
        ("intact".to_owned(), JsonValue::Null),
        ("trees".to_owned(), JsonValue::Array(Vec::new())),
        ("findings".to_owned(), JsonValue::Array(Vec::new())),
    ]))
}

fn proof_array(proof: &Option<JsonObject>, field: &str) -> JsonValue {
    proof
        .as_ref()
        .and_then(|value| value.get(field))
        .cloned()
        .unwrap_or_else(|| JsonValue::Array(Vec::new()))
}

fn matched_proof_ids(proof: &Option<JsonObject>) -> Vec<String> {
    proof
        .as_ref()
        .and_then(|value| value.get("matched_receipts"))
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            value
                .as_object()
                .and_then(|value| value.get("receipt_id"))
                .and_then(JsonValue::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn proof_store_label(proof: &Option<JsonObject>) -> Option<String> {
    proof
        .as_ref()
        .and_then(|value| value.get("store"))
        .and_then(JsonValue::as_object)
        .and_then(|value| value.get("label"))
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
}

fn to_json(value: &impl serde::Serialize) -> Result<JsonValue, RuntimeError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|source| RuntimeError::json("serializing native receipt query", source))?;
    serde_json::from_slice(&bytes)
        .map_err(|source| RuntimeError::json("projecting native receipt query", source))
}
