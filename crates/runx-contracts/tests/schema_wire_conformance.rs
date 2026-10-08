//! Non-authoritative wire-conformance gate for the type-driven JSON Schema
//! emitter (Phase 1 of `rust-contract-pipeline-inversion`).
//!
//! For each covered contract: the Rust-emitted schema must preserve schema
//! identity (`$id`, `x-runx-schema`) and agree with the committed
//! `oss/schemas/*.json` on accept/reject for every corpus value. The schema
//! *document* shape may differ from the committed one; only the validated value
//! domain must match (dod1). Rust contract types are now the source of truth;
//! the committed schema documents are generated artifacts checked for freshness.

mod corpora;
mod covered;
mod support;

use covered::covered;
use runx_contracts::Receipt;
use serde_json::{Value, json};
use support::{SchemaDirRetriever, committed_dir};

#[test]
fn receipt_requires_an_exact_explicit_class() -> Result<(), String> {
    let valid = corpora::receipt_corpus()
        .into_iter()
        .find_map(|(label, value)| (label == "minimal valid").then_some(value))
        .ok_or("receipt corpus must contain a minimal valid case")?;
    assert!(serde_json::from_value::<Receipt>(valid.clone()).is_ok());

    let mut missing = valid.clone();
    missing
        .as_object_mut()
        .ok_or("receipt corpus value must be an object")?
        .remove("class");
    assert!(serde_json::from_value::<Receipt>(missing).is_err());

    let mut unknown = valid;
    unknown["class"] = json!("unknown");
    assert!(serde_json::from_value::<Receipt>(unknown).is_err());
    Ok(())
}

#[test]
fn emitted_schemas_conform_to_committed_value_domains() {
    let dir = committed_dir();
    let mut failures: Vec<String> = Vec::new();

    for contract in covered() {
        let name = contract.file_name;
        let raw = match std::fs::read_to_string(dir.join(name)) {
            Ok(raw) => raw,
            Err(error) => {
                failures.push(format!("{name}: cannot read committed schema: {error}"));
                continue;
            }
        };
        let committed: Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(error) => {
                failures.push(format!(
                    "{name}: committed schema is not valid JSON: {error}"
                ));
                continue;
            }
        };

        if contract.emitted.get("$id") != committed.get("$id")
            || contract.emitted.get("x-runx-schema") != committed.get("x-runx-schema")
        {
            failures.push(format!(
                "{name}: schema identity ($id / x-runx-schema) diverged"
            ));
            continue;
        }

        let Ok(committed_validator) = jsonschema::draft202012::options()
            .with_retriever(SchemaDirRetriever { dir: dir.clone() })
            .build(&committed)
        else {
            failures.push(format!(
                "{name}: committed schema is not a usable validator"
            ));
            continue;
        };
        let Ok(emitted_validator) = jsonschema::draft202012::options()
            .with_retriever(SchemaDirRetriever { dir: dir.clone() })
            .build(&contract.emitted)
        else {
            failures.push(format!("{name}: emitted schema is not a usable validator"));
            continue;
        };

        for (label, value) in &contract.corpus {
            let committed_accepts = committed_validator.is_valid(value);
            let emitted_accepts = emitted_validator.is_valid(value);
            if committed_accepts != emitted_accepts {
                failures.push(format!(
                    "{name} / {label}: committed accepts={committed_accepts}, emitted accepts={emitted_accepts}"
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "schema wire-conformance drift:\n{}",
        failures.join("\n")
    );
}
