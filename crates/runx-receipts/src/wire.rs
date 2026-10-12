//! The receipt wire form.
//!
//! A receipt has exactly one wire form: what it serializes to. Every array is
//! written, even when empty, and an absent optional value is omitted, never
//! `null`. A reader that filled in a missing array or dropped a `null` would
//! hash different bytes from a verifier that reads the document as written,
//! and the two would disagree about the same receipt. So a document is read
//! as a receipt only when it is that one form.

use serde_json::Value;
use thiserror::Error;

use crate::Receipt;

#[derive(Debug, Error)]
pub enum ReceiptDecodeError {
    #[error("receipt is not JSON: {message}")]
    Json { message: String },
    #[error("document is not a runx.receipt.v1 receipt: {message}")]
    Shape { message: String },
    #[error("receipt is not in its wire form at {path}: {reason}")]
    NotWireForm { path: String, reason: &'static str },
}

/// Read a receipt from a JSON document in its wire form.
pub fn decode_receipt(document: &[u8]) -> Result<Receipt, ReceiptDecodeError> {
    let value =
        serde_json::from_slice::<Value>(document).map_err(|error| ReceiptDecodeError::Json {
            message: error.to_string(),
        })?;
    decode_receipt_value(value)
}

/// Read a receipt from a parsed JSON document in its wire form.
pub fn decode_receipt_value(value: Value) -> Result<Receipt, ReceiptDecodeError> {
    let receipt = serde_json::from_value::<Receipt>(value.clone()).map_err(|error| {
        ReceiptDecodeError::Shape {
            message: error.to_string(),
        }
    })?;
    let written = serde_json::to_value(&receipt).map_err(|error| ReceiptDecodeError::Shape {
        message: error.to_string(),
    })?;
    match first_difference(&value, &written, String::new()) {
        None => Ok(receipt),
        Some((path, reason)) => Err(ReceiptDecodeError::NotWireForm {
            path: if path.is_empty() {
                "/".to_owned()
            } else {
                path
            },
            reason,
        }),
    }
}

/// The first JSON pointer where `read` and `written` differ, and how.
fn first_difference(read: &Value, written: &Value, path: String) -> Option<(String, &'static str)> {
    match (read, written) {
        (Value::Object(read), Value::Object(written)) => {
            if let Some(key) = read.keys().find(|key| !written.contains_key(*key)) {
                return Some((pointer(&path, key), "a value the receipt does not write"));
            }
            if let Some(key) = written.keys().find(|key| !read.contains_key(*key)) {
                return Some((
                    pointer(&path, key),
                    "a value the receipt always writes is missing",
                ));
            }
            read.iter().find_map(|(key, value)| {
                first_difference(value, &written[key], pointer(&path, key))
            })
        }
        (Value::Array(read), Value::Array(written)) if read.len() == written.len() => read
            .iter()
            .zip(written)
            .enumerate()
            .find_map(|(index, (read, written))| {
                first_difference(read, written, format!("{path}/{index}"))
            }),
        (read, written) if read == written => None,
        _ => Some((path, "a value written in another form")),
    }
}

fn pointer(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{ReceiptDecodeError, decode_receipt_value};

    const RECEIPT: &str =
        include_str!("../../../fixtures/contracts/harness-spine/receipt-success.json");

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn written() -> Result<Value, serde_json::Error> {
        let fixture: Value = serde_json::from_str(RECEIPT)?;
        Ok(fixture["expected"].clone())
    }

    /// The JSON pointer a wire-form refusal names, or why there was none.
    fn refused(edit: impl FnOnce(&mut Value)) -> Result<String, String> {
        let mut document = written().map_err(|error| error.to_string())?;
        edit(&mut document);
        match decode_receipt_value(document) {
            Err(ReceiptDecodeError::NotWireForm { path, .. }) => Ok(path),
            other => Err(format!("expected a wire-form refusal, got {other:?}")),
        }
    }

    #[test]
    fn a_receipt_reads_in_its_wire_form() -> TestResult {
        let receipt = decode_receipt_value(written()?)?;
        assert_eq!(serde_json::to_value(&receipt)?, written()?);
        Ok(())
    }

    #[test]
    fn an_omitted_array_is_refused_not_filled_in() -> TestResult {
        let mut document = written()?;
        if let Some(seal) = document["seal"].as_object_mut() {
            seal.remove("criteria");
        }
        assert!(matches!(
            decode_receipt_value(document),
            Err(ReceiptDecodeError::Shape { .. })
        ));
        Ok(())
    }

    #[test]
    fn a_null_for_an_absent_value_is_refused() -> TestResult {
        assert_eq!(
            refused(|document| {
                document["authority"]["mandate_ref"] = Value::Null;
            })?,
            "/authority/mandate_ref"
        );
        Ok(())
    }

    #[test]
    fn a_value_the_receipt_does_not_write_is_refused() -> TestResult {
        assert_eq!(
            refused(|document| {
                document["authority"]["enforcement"] = json!({ "execution_boundary": null });
            })?,
            "/authority/enforcement/execution_boundary"
        );
        Ok(())
    }
}
