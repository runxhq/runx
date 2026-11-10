//! Development-only kernel evaluator for the repository's scripts and
//! cross-language differential tests; excluded from the published crate.
//! Reads a JSON array of kernel documents on stdin and writes one result per
//! document: `{"status":"success","result":…}` or
//! `{"status":"error","code":…,"message":…}`.
//!
//! It reads the array to its end and no further, never waiting for stdin to
//! close, so a parent that holds stdin open past a complete request, as one
//! whose descriptor another process inherited does, still gets its answer.

use std::io;

use runx_core::kernel_eval::evaluate_kernel_document_str;
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let inputs: Vec<Value> = serde_json::Deserializer::from_reader(io::stdin().lock())
        .into_iter()
        .next()
        .ok_or("stdin holds no array of kernel documents")??;
    let outputs = inputs
        .iter()
        .map(
            |input| match evaluate_kernel_document_str(&input.to_string()) {
                Ok(result) => json!({ "status": "success", "result": result }),
                Err(error) => json!({
                    "status": "error",
                    "code": error.code(),
                    "message": error.to_string(),
                }),
            },
        )
        .collect::<Vec<_>>();
    serde_json::to_writer(io::stdout().lock(), &outputs)?;
    Ok(())
}
