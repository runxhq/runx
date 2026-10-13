//! Development-only kernel evaluator for the repository's scripts and
//! cross-language differential tests; excluded from the published crate.
//! Reads a JSON array of kernel documents on stdin and writes one result per
//! document: `{"status":"success","result":…}` or
//! `{"status":"error","code":…,"message":…}`.

use std::io;

use runx_core::kernel_eval::evaluate_kernel_document_str;
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let inputs: Vec<Value> = serde_json::from_reader(io::stdin().lock())?;
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
