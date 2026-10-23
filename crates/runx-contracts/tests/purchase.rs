use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{JsonValue, QuotePurchaseResult, sha256_prefixed};
use serde_json::Value;

#[test]
fn vectors_cover_identity_replay_terms_binding_and_outcomes()
-> Result<(), Box<dyn std::error::Error>> {
    let direct = vector("quote-direct-admission.json")?;
    let replay = vector("quote-same-term-replay.json")?;
    assert_eq!(
        direct.pointer("/payload/value/purchase/purchase_id"),
        replay.pointer("/payload/value/purchase/purchase_id")
    );
    assert_eq!(
        direct.pointer("/payload/value/purchase/input_digest"),
        replay.pointer("/payload/value/purchase/input_digest")
    );
    assert_eq!(
        direct.pointer("/payload/value/purchase/package_digest"),
        replay.pointer("/payload/value/purchase/package_digest")
    );

    let independent = vector("quote-independent-purchase.json")?;
    assert_eq!(
        direct.pointer("/payload/value/purchase/input_digest"),
        independent.pointer("/payload/input_digest")
    );
    assert_eq!(
        direct.pointer("/payload/value/purchase/package_digest"),
        independent.pointer("/payload/package_digest")
    );
    assert_ne!(
        direct.pointer("/payload/value/purchase/idempotency/key"),
        independent.pointer("/payload/idempotency/key")
    );

    for (file, code) in [
        ("quote-terms-changed.json", "terms_changed"),
        ("quote-replay-conflict.json", "replay_conflict"),
        ("quote-expired.json", "quote_expired"),
    ] {
        assert_eq!(
            vector(file)?.pointer("/payload/code"),
            Some(&Value::String(code.to_owned()))
        );
    }

    assert_eq!(
        vector("get-fulfilment-won.json")?.pointer("/payload/value/purchase/outcome_gate"),
        Some(&Value::String("fulfilment_won".to_owned()))
    );
    assert_eq!(
        vector("get-refund-won.json")?.pointer("/payload/value/purchase/outcome_gate"),
        Some(&Value::String("refund_won".to_owned()))
    );
    assert_eq!(
        vector("cancel-before-settlement.json")?.pointer("/payload/value/purchase/payment_state"),
        Some(&Value::String("unpaid".to_owned()))
    );
    assert_eq!(
        vector("cancel-after-settlement.json")?.pointer("/payload/value/purchase/payment_state"),
        Some(&Value::String("settled".to_owned()))
    );
    Ok(())
}

#[test]
fn manifest_digests_bind_exact_published_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = manifest()?;
    for schema in array_field(&manifest, "schemas")? {
        let schema_file = string_field(schema, "schema_file")?;
        let schema_bytes = fs::read(repo_root().join("schemas").join(schema_file))?;
        assert_digest(schema, "schema_digest", &schema_bytes)?;
        let published: Value = serde_json::from_slice(&schema_bytes)?;
        assert_eq!(
            published.get("x-runx-schema").and_then(Value::as_str),
            Some(string_field(schema, "schema_id")?)
        );
    }
    for vector in array_field(&manifest, "vectors")? {
        let file = string_field(vector, "file")?;
        assert_digest(
            vector,
            "vector_digest",
            &fs::read(fixture_root().join(file))?,
        )?;
    }
    Ok(())
}

#[test]
fn published_vectors_validate_against_published_schemas() -> Result<(), Box<dyn std::error::Error>>
{
    let manifest = manifest()?;
    let schema_files = array_field(&manifest, "schemas")?
        .iter()
        .map(|entry| {
            Ok((
                string_field(entry, "schema_id")?.to_owned(),
                string_field(entry, "schema_file")?.to_owned(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, Box<dyn std::error::Error>>>()?;

    let indexed = array_field(&manifest, "vectors")?
        .iter()
        .map(|entry| Ok(string_field(entry, "file")?.to_owned()))
        .collect::<Result<BTreeSet<_>, Box<dyn std::error::Error>>>()?;
    let published = fs::read_dir(fixture_root())?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<BTreeSet<_>, _>>()?
        .into_iter()
        .filter(|name| name.ends_with(".json") && name != "manifest.json")
        .collect::<BTreeSet<_>>();
    assert_eq!(
        indexed, published,
        "manifest must exhaustively index vectors"
    );

    let mut validators = BTreeMap::new();
    for entry in array_field(&manifest, "vectors")? {
        let schema_id = string_field(entry, "schema_id")?;
        let schema_file = schema_files
            .get(schema_id)
            .ok_or_else(|| format!("vector names unknown schema {schema_id}"))?;
        if !validators.contains_key(schema_id) {
            let schema: Value =
                serde_json::from_slice(&fs::read(repo_root().join("schemas").join(schema_file))?)?;
            validators.insert(
                schema_id.to_owned(),
                jsonschema::draft202012::options().build(&schema)?,
            );
        }
        let fixture = vector(string_field(entry, "file")?)?;
        let payload = fixture
            .pointer(string_field(entry, "payload_pointer")?)
            .ok_or("fixture payload pointer is missing")?;
        let valid = validators
            .get(schema_id)
            .ok_or("validator disappeared")?
            .is_valid(payload);
        let expected = string_field(entry, "expectation")? == "valid";
        assert_eq!(
            valid,
            expected,
            "{} disagrees with {schema_id}",
            string_field(entry, "file")?
        );
    }
    Ok(())
}

#[test]
fn challenge_is_rail_neutral_and_digest_bound() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = vector("quote-direct-admission.json")?;
    let result: QuotePurchaseResult = serde_json::from_value(
        fixture
            .get("payload")
            .cloned()
            .ok_or("quote fixture has no payload")?,
    )?;
    let QuotePurchaseResult::Admitted { value } = result else {
        return Err("quote fixture must be admitted".into());
    };
    let option = value
        .payment_options
        .first()
        .ok_or("quote has no payment options")?;
    let payload = serde_json::to_value(&option.payload)?;
    let bytes = serde_json::to_vec(&payload)?;
    assert_eq!(option.payload_digest.as_str(), sha256_prefixed(&bytes));

    let challenge = serde_json::to_value(option)?;
    let keys = challenge
        .as_object()
        .ok_or("challenge must serialize as an object")?
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from([
            "media_type",
            "payload",
            "payload_digest",
            "protocol_version",
            "quote_expires_at",
            "quote_ref",
            "settlement_family",
        ])
    );
    assert!(matches!(option.payload, JsonValue::Object(_)));
    let source = include_str!("../src/purchase.rs").to_ascii_lowercase();
    for forbidden in ["aws-sdk", "coinbase", "stripe", "wallet", "facilitator"] {
        assert!(
            !source.contains(forbidden),
            "rail/provider term leaked into contract: {forbidden}"
        );
    }
    Ok(())
}

fn assert_digest(
    entry: &Value,
    field: &str,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(string_field(entry, field)?, sha256_prefixed(bytes));
    assert_eq!(
        bytes.last(),
        Some(&b'\n'),
        "published JSON must end in newline"
    );
    Ok(())
}

fn manifest() -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(
        fixture_root().join("manifest.json"),
    )?)?)
}

fn vector(file: &str) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(
        fixture_root().join(file),
    )?)?)
}

fn array_field<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a Vec<Value>, Box<dyn std::error::Error>> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing array field {field}").into())
}

fn string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field {field}").into())
}

fn fixture_root() -> PathBuf {
    repo_root().join("fixtures/contracts/purchase")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .components()
        .collect()
}
