//! Development-only parser evaluator for the repository's scripts; excluded
//! from the published crate. Reads one parser document on stdin and writes
//! `{"status":"success","result":…}`, or `{"status":"error","code":…,"message":…}`
//! and exits 1.

use std::io::{self, Read, Write};
use std::process::ExitCode;

use runx_parser::evaluate_parser_document_str;
use serde_json::{Value, json};

fn main() -> ExitCode {
    let mut source = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut source) {
        return write(
            &json!({ "status": "error", "code": "read_stdin", "message": error.to_string() }),
            1,
        );
    }
    match evaluate_parser_document_str(&source) {
        Ok(result) => write(&json!({ "status": "success", "result": result }), 0),
        Err(error) => write(
            &json!({ "status": "error", "code": error.code(), "message": error.to_string() }),
            1,
        ),
    }
}

fn write(envelope: &Value, code: u8) -> ExitCode {
    let mut stdout = io::stdout().lock();
    let written =
        serde_json::to_writer(&mut stdout, envelope).is_ok() && stdout.write_all(b"\n").is_ok();
    ExitCode::from(if written { code } else { 1 })
}
