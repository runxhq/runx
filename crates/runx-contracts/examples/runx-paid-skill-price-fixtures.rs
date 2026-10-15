//! Price vectors for paid skill offers: every implementation that prices a
//! purchase agrees with these, computed by Rust.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use runx_contracts::canonical_json::canonical_stable_json;
use runx_contracts::{JsonObject, PaidSkillPrice};
use serde_json::{Value, json};

const FIXTURE: &str = "paid-skill-price-v1.vectors.json";

struct Options {
    out_dir: PathBuf,
    check: bool,
}

fn main() -> io::Result<()> {
    let options = parse_args()?;
    let document = fixture_document()?;
    reconcile_file(
        &options.out_dir.join(FIXTURE),
        canonical_bytes(&document)?,
        options.check,
    )?;
    reject_orphans(&options.out_dir, options.check)
}

fn parse_args() -> io::Result<Options> {
    let mut out_dir = None;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out_dir = args.next().map(PathBuf::from),
            "--check" => check = true,
            other => return Err(io::Error::other(format!("unsupported argument: {other}"))),
        }
    }
    Ok(Options {
        out_dir: out_dir.ok_or_else(|| io::Error::other("--out is required"))?,
        check,
    })
}

fn fixture_document() -> io::Result<Value> {
    let duration = json!({
        "model": "input_choice",
        "selectors": ["duration_seconds"],
        "cases": [
            {"when": [3600], "amount_minor": 5},
            {"when": [86400], "amount_minor": 20},
            {"when": [604800], "amount_minor": 100}
        ]
    });
    let pair = json!({
        "model": "input_choice",
        "selectors": ["from", "to", "rush"],
        "cases": [
            {"when": ["en", "fr", false], "amount_minor": 3},
            {"when": ["en", "fr", true], "amount_minor": 9},
            {"when": ["fr", "en", false], "amount_minor": 4}
        ]
    });
    let bytes = json!({
        "model": "artifact_bytes",
        "artifact_input": "artifact",
        "base_amount_minor": 25,
        "unit_bytes": 1_048_576,
        "unit_amount_minor": 5,
        "max_bytes": 104_857_600
    });
    let prices = vec![
        price_case(
            "fixed",
            json!({"model": "fixed", "amount_minor": 125}),
            vec![call("any-inputs", json!({"note": "x"}), None)],
        )?,
        price_case(
            "input-choice-duration",
            duration,
            vec![
                call("hour", json!({"duration_seconds": 3600}), None),
                call("week", json!({"duration_seconds": 604800}), None),
                call("whole-float", json!({"duration_seconds": 86400.0}), None),
                call(
                    "string-is-not-integer",
                    json!({"duration_seconds": "3600"}),
                    None,
                ),
                call("unpriced", json!({"duration_seconds": 60}), None),
                call("missing", json!({}), None),
            ],
        )?,
        price_case(
            "input-choice-pair",
            pair,
            vec![
                call(
                    "en-fr",
                    json!({"from": "en", "to": "fr", "rush": false}),
                    None,
                ),
                call(
                    "en-fr-rush",
                    json!({"from": "en", "to": "fr", "rush": true}),
                    None,
                ),
                call(
                    "fr-en-rush",
                    json!({"from": "fr", "to": "en", "rush": true}),
                    None,
                ),
            ],
        )?,
        price_case(
            "artifact-bytes",
            bytes,
            vec![
                call(
                    "stored-reference",
                    json!({"artifact": {"artifact_ref": format!("runx:artifact:sha256:{}", "a".repeat(64)), "media_type": "application/pdf"}}),
                    Some(2_097_152),
                ),
                call(
                    "uppercase-reference",
                    json!({"artifact": {"artifact_ref": format!("runx:artifact:sha256:{}", "A".repeat(64))}}),
                    Some(1),
                ),
                call(
                    "bare-reference",
                    json!({"artifact": format!("runx:artifact:sha256:{}", "a".repeat(64))}),
                    Some(1),
                ),
                call("one-byte", json!({}), Some(1)),
                call("one-unit", json!({}), Some(1_048_576)),
                call("one-unit-and-a-byte", json!({}), Some(1_048_577)),
                call("at-max", json!({}), Some(104_857_600)),
                call("over-max", json!({}), Some(104_857_601)),
                call("empty", json!({}), Some(0)),
                call("size-unknown", json!({}), None),
            ],
        )?,
        price_case(
            "zero-amount",
            json!({"model": "fixed", "amount_minor": 0}),
            vec![],
        )?,
        price_case(
            "unknown-model",
            json!({"model": "quote", "amount_minor": 5}),
            vec![],
        )?,
        price_case(
            "duplicate-case",
            json!({"model": "input_choice", "selectors": ["a"], "cases": [
                {"when": [1], "amount_minor": 1},
                {"when": [1.0], "amount_minor": 2}
            ]}),
            vec![],
        )?,
        price_case(
            "case-arity",
            json!({"model": "input_choice", "selectors": ["a", "b"], "cases": [
                {"when": [1], "amount_minor": 1}
            ]}),
            vec![],
        )?,
        price_case(
            "fractional-choice",
            json!({"model": "input_choice", "selectors": ["a"], "cases": [
                {"when": [1.5], "amount_minor": 1}
            ]}),
            vec![],
        )?,
        price_case(
            "artifact-overflow",
            json!({
                "model": "artifact_bytes",
                "artifact_input": "artifact",
                "base_amount_minor": 1,
                "unit_bytes": 1,
                "unit_amount_minor": 9_007_199_254_740_991_u64,
                "max_bytes": 2
            }),
            vec![],
        )?,
    ];

    Ok(json!({
        "prices": prices,
        "schema": "runx.paid_skill_price.fixtures.v1",
    }))
}

struct Call {
    name: &'static str,
    inputs: Value,
    artifact_size_bytes: Option<u64>,
}

fn call(name: &'static str, inputs: Value, artifact_size_bytes: Option<u64>) -> Call {
    Call {
        name,
        inputs,
        artifact_size_bytes,
    }
}

fn price_case(name: &str, price: Value, calls: Vec<Call>) -> io::Result<Value> {
    let parsed = serde_json::from_value::<PaidSkillPrice>(price.clone());
    let Ok(parsed) = parsed else {
        if !calls.is_empty() {
            return Err(io::Error::other(format!(
                "price fixture {name} is invalid but has calls"
            )));
        }
        return Ok(json!({"expectation": "invalid", "name": name, "price": price}));
    };
    let calls = calls
        .into_iter()
        .map(|call| {
            let inputs = serde_json::from_value::<JsonObject>(call.inputs.clone())
                .map_err(io::Error::other)?;
            let expected = match parsed.amount_minor(&inputs, call.artifact_size_bytes) {
                Ok(amount) => json!({"amount_minor": amount.get()}),
                Err(error) => json!({"error": error.code()}),
            };
            let mut entry = json!({
                "expected": expected,
                "inputs": call.inputs,
                "name": call.name,
                "priced_artifact_ref": parsed.priced_artifact_ref(&inputs),
            });
            if let (Some(size), Some(object)) = (call.artifact_size_bytes, entry.as_object_mut()) {
                object.insert("artifact_size_bytes".to_owned(), json!(size));
            }
            Ok(entry)
        })
        .collect::<io::Result<Vec<_>>>()?;
    Ok(json!({"calls": calls, "expectation": "valid", "name": name, "price": price}))
}

/// Sorted keys at every depth, as the contract fixture checks require.
fn canonical_bytes(value: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = canonical_stable_json(value)
        .map_err(io::Error::other)?
        .into_bytes();
    bytes.push(b'\n');
    Ok(bytes)
}

fn reconcile_file(path: &Path, expected: Vec<u8>, check: bool) -> io::Result<()> {
    if check {
        let actual = fs::read(path).map_err(|error| {
            io::Error::other(format!(
                "missing generated fixture {}: {error}",
                path.display()
            ))
        })?;
        if actual != expected {
            return Err(io::Error::other(format!(
                "generated fixture is stale: {}",
                path.display()
            )));
        }
    } else {
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| io::Error::other("fixture path has no parent"))?,
        )?;
        fs::write(path, expected)?;
    }
    Ok(())
}

fn reject_orphans(out_dir: &Path, check: bool) -> io::Result<()> {
    let expected = BTreeSet::from([FIXTURE]);
    if !out_dir.exists() {
        return if check {
            Err(io::Error::other(format!(
                "missing generated fixture directory: {}",
                out_dir.display()
            )))
        } else {
            Ok(())
        };
    }
    for entry in fs::read_dir(out_dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if path.extension().and_then(|value| value.to_str()) != Some("json")
            || expected.contains(name)
        {
            continue;
        }
        if check {
            return Err(io::Error::other(format!(
                "orphan paid-skill-price fixture: {}",
                path.display()
            )));
        }
        fs::remove_file(path)?;
    }
    Ok(())
}
