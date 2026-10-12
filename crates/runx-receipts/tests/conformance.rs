//! Cross-binding conformance oracle (A+ contract-coherence detector).
//!
//! Asserts the three receipt bindings stay coherent:
//!   1. The Rust emitter's canonical instances validate against the published
//!      JSON Schema (`schemas/receipt.schema.json`) — emitter-validates-against-schema.
//!   2. The Rust canonicalizer reproduces the checked-in canonical-json oracle
//!      byte-for-byte (full + body), so the c14n contract cannot drift silently.
//!
//! The TS validator accepts the same instances; that arm is enforced by
//! `packages/contracts/src/index.test.ts` against the same fixtures and the same
//! `receipt.schema.json` (generated from the TS contract). Any divergence between
//! Rust types, the JSON Schema, and the emitter fails one of these gates.

// Test oracle: asserting via expect/unwrap and panicking on an unknown fixture
// is the intended failure mode, so the workspace expect/unwrap/panic bans are
// lifted for this test target.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use jsonschema::Validator;
use runx_contracts::Receipt;
use runx_receipts::{
    canonical_receipt_body_digest, canonical_receipt_body_json, canonical_receipt_digest,
    canonical_receipt_json,
};
use serde::Deserialize;
use serde_json::Value;

const RECEIPT_SCHEMA: &str = include_str!("../../../schemas/receipt.schema.json");
const SUCCESS_RECEIPT: &str =
    include_str!("../../../fixtures/contracts/harness-spine/receipt-success.json");
const ABNORMAL_RECEIPT: &str =
    include_str!("../../../fixtures/contracts/harness-spine/receipt-abnormal.json");
const RECEIPT_ORACLE: &str =
    include_str!("../../../fixtures/contracts/canonical-json/runx-receipt-c14n-v1.oracles.json");

#[derive(Debug, Deserialize)]
struct Fixture {
    expected: Receipt,
}

#[derive(Debug, Deserialize)]
struct OracleFixture {
    canonicalization: String,
    cases: Vec<OracleCase>,
}

#[derive(Debug, Deserialize)]
struct OracleCase {
    name: String,
    fixture: String,
    full_canonical_json: String,
    full_sha256: String,
    body_canonical_json: String,
    body_sha256: String,
}

fn schema() -> Validator {
    let schema_value: Value = serde_json::from_str(RECEIPT_SCHEMA).expect("receipt schema parses");
    jsonschema::validator_for(&schema_value).expect("receipt schema compiles")
}

fn fixture_receipt(json: &str) -> Receipt {
    serde_json::from_str::<Fixture>(json)
        .expect("fixture parses")
        .expected
}

fn fixture_json_by_path(path: &str) -> &'static str {
    match path {
        "harness-spine/receipt-success.json" => SUCCESS_RECEIPT,
        "harness-spine/receipt-abnormal.json" => ABNORMAL_RECEIPT,
        other => panic!("unknown conformance fixture path: {other}"),
    }
}

#[test]
fn conformance_emitter_instances_validate_against_published_schema() {
    let validator = schema();
    for json in [SUCCESS_RECEIPT, ABNORMAL_RECEIPT] {
        let receipt = fixture_receipt(json);
        // Serialize through the Rust contract type exactly as the emitter does,
        // then validate that instance against the published JSON Schema.
        let instance = serde_json::to_value(&receipt).expect("receipt serializes");
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|error| format!("{}: {error}", error.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "Rust-emitted receipt {} must validate against receipt.schema.json: {errors:?}",
            receipt.id
        );
    }
}

#[test]
fn conformance_canonical_json_is_byte_identical_to_oracle() {
    let oracle: OracleFixture =
        serde_json::from_str(RECEIPT_ORACLE).expect("canonical-json oracle parses");
    assert_eq!(oracle.canonicalization, "runx.receipt.c14n.v1");
    assert!(!oracle.cases.is_empty(), "oracle must carry cases");

    for case in oracle.cases {
        let receipt = fixture_receipt(fixture_json_by_path(&case.fixture));
        assert_eq!(
            canonical_receipt_json(&receipt).expect("full canonical json"),
            case.full_canonical_json,
            "{} full canonical JSON drifted",
            case.name
        );
        assert_eq!(
            canonical_receipt_digest(&receipt).expect("full digest"),
            case.full_sha256,
            "{} full digest drifted",
            case.name
        );
        assert_eq!(
            canonical_receipt_body_json(&receipt).expect("body canonical json"),
            case.body_canonical_json,
            "{} body canonical JSON drifted",
            case.name
        );
        assert_eq!(
            canonical_receipt_body_digest(&receipt).expect("body digest"),
            case.body_sha256,
            "{} body digest drifted",
            case.name
        );
    }
}
