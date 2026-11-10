//! Development-only parser evaluator for the repository's scripts; excluded
//! from the published crate. Reads one parser document on stdin and writes
//! `{"status":"success","result":…}`, or `{"status":"error","code":…,"message":…}`
//! and exits 1.
//!
//! It reads the document to its end and no further, never waiting for stdin to
//! close, so a parent that holds stdin open past a complete request, as one
//! whose descriptor another process inherited does, still gets its answer.

use std::io::{self, Write};
use std::process::ExitCode;

use runx_parser::{ParserEvalError, evaluate_parser_document_str};
use serde_json::value::RawValue;
use serde_json::{Value, json};

fn main() -> ExitCode {
    let document = serde_json::Deserializer::from_reader(io::stdin().lock())
        .into_iter::<Box<RawValue>>()
        .next();
    let source = match document {
        Some(Ok(source)) => source,
        Some(Err(error)) if error.is_io() => {
            return write(
                &json!({ "status": "error", "code": "read_stdin", "message": error.to_string() }),
                1,
            );
        }
        Some(Err(error)) => return failed(&ParserEvalError::InvalidDocument(error.to_string())),
        None => {
            return failed(&ParserEvalError::InvalidDocument(
                "stdin holds no parser document".to_owned(),
            ));
        }
    };
    match evaluate_parser_document_str(source.get()) {
        Ok(result) => write(&json!({ "status": "success", "result": result }), 0),
        Err(error) => failed(&error),
    }
}

fn failed(error: &ParserEvalError) -> ExitCode {
    write(
        &json!({ "status": "error", "code": error.code(), "message": error.to_string() }),
        1,
    )
}

fn write(envelope: &Value, code: u8) -> ExitCode {
    let mut stdout = io::stdout().lock();
    let written =
        serde_json::to_writer(&mut stdout, envelope).is_ok() && stdout.write_all(b"\n").is_ok();
    ExitCode::from(if written { code } else { 1 })
}
