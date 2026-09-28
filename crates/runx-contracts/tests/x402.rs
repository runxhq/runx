use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{
    RunxX402InvocationExtensionInfo, X402_UPSTREAM_COMMIT, X402_UPSTREAM_PACKAGE,
    X402_UPSTREAM_PACKAGE_VERSION, X402PaymentPayload, X402PaymentRequired, X402SettleResponse,
    sha256_prefixed,
};
use serde_json::Value;

#[test]
fn published_vectors_match_rust_and_json_schema_contracts() -> Result<(), Box<dyn std::error::Error>>
{
    let manifest = manifest()?;
    let schemas = array_field(&manifest, "schemas")?
        .iter()
        .map(|entry| {
            Ok((
                string_field(entry, "schema_id")?.to_owned(),
                string_field(entry, "file")?.to_owned(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, Box<dyn std::error::Error>>>()?;
    let mut validators = BTreeMap::new();

    for entry in array_field(&manifest, "vectors")? {
        let file = string_field(entry, "file")?;
        let schema_id = string_field(entry, "schema_id")?;
        let fixture = vector(file)?;
        let payload = fixture.get("payload").ok_or("fixture has no payload")?;
        let schema_file = schemas
            .get(schema_id)
            .ok_or_else(|| format!("unknown schema id: {schema_id}"))?;
        if !validators.contains_key(schema_id) {
            let schema: Value =
                serde_json::from_slice(&fs::read(schema_root().join(schema_file))?)?;
            validators.insert(
                schema_id.to_owned(),
                jsonschema::draft202012::options().build(&schema)?,
            );
        }
        let schema_valid = validators
            .get(schema_id)
            .ok_or("validator disappeared")?
            .is_valid(payload);
        let rust_valid = rust_valid(string_field(&fixture, "kind")?, payload);
        let expected = string_field(entry, "expectation")? == "valid";
        assert_eq!(schema_valid, expected, "schema disagrees for {file}");
        assert_eq!(rust_valid, expected, "Rust disagrees for {file}");
    }
    Ok(())
}

#[test]
fn manifest_and_pin_bind_exact_repository_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = manifest()?;
    for entry in array_field(&manifest, "schemas")? {
        let bytes = fs::read(schema_root().join(string_field(entry, "file")?))?;
        assert_eq!(string_field(entry, "sha256")?, sha256_prefixed(&bytes));
    }
    for entry in array_field(&manifest, "vectors")? {
        let bytes = fs::read(fixture_root().join(string_field(entry, "file")?))?;
        assert_eq!(string_field(entry, "sha256")?, sha256_prefixed(&bytes));
    }
    let pin_bytes = fs::read(fixture_root().join("upstream-pin.json"))?;
    assert_eq!(
        string_field(&manifest, "pin_sha256")?,
        sha256_prefixed(&pin_bytes)
    );
    assert_eq!(pin_bytes.last(), Some(&b'\n'));

    let pin: Value = serde_json::from_slice(&pin_bytes)?;
    assert_eq!(string_field(&pin, "revision")?, X402_UPSTREAM_COMMIT);
    assert_eq!(
        pin.pointer("/package/name").and_then(Value::as_str),
        Some(X402_UPSTREAM_PACKAGE)
    );
    assert_eq!(
        pin.pointer("/package/version").and_then(Value::as_str),
        Some(X402_UPSTREAM_PACKAGE_VERSION)
    );
    assert_eq!(array_field(&pin, "sources")?.len(), 7);
    Ok(())
}

#[test]
fn fixture_manifest_is_exhaustive_and_protocol_versions_do_not_drift()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest = manifest()?;
    assert_eq!(
        manifest
            .pointer("/external_protocol/version")
            .and_then(Value::as_u64),
        Some(2)
    );
    let indexed = array_field(&manifest, "vectors")?
        .iter()
        .map(|entry| Ok(string_field(entry, "file")?.to_owned()))
        .collect::<Result<BTreeSet<_>, Box<dyn std::error::Error>>>()?;
    let published = fs::read_dir(fixture_root())?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<BTreeSet<_>, _>>()?
        .into_iter()
        .filter(|name| {
            name.ends_with(".json") && name != "manifest.json" && name != "upstream-pin.json"
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(indexed, published);

    let external_schema_ids = array_field(&manifest, "schemas")?
        .iter()
        .filter_map(|entry| entry.get("schema_id").and_then(Value::as_str))
        .filter(|id| id.starts_with("https://schemas.runx.ai/external/x402/v2/"))
        .count();
    assert_eq!(external_schema_ids, 5);
    assert!(
        array_field(&manifest, "schemas")?
            .iter()
            .any(|entry| entry.get("schema_id").and_then(Value::as_str)
                == Some("runx.x402.invocation_extension.v1"))
    );
    Ok(())
}

#[test]
fn external_contract_is_tolerant_and_runx_extension_is_strict()
-> Result<(), Box<dyn std::error::Error>> {
    let external: Value = serde_json::from_slice(&fs::read(
        schema_root().join("x402-v2-payment-required.schema.json"),
    )?)?;
    assert_eq!(
        external
            .get("additionalProperties")
            .and_then(Value::as_bool),
        Some(true)
    );

    let internal: Value = serde_json::from_slice(&fs::read(
        schema_root().join("runx-x402-invocation-extension-v1.schema.json"),
    )?)?;
    for variant in internal
        .get("anyOf")
        .and_then(Value::as_array)
        .ok_or("extension schema has no variants")?
    {
        assert_eq!(
            variant.get("additionalProperties").and_then(Value::as_bool),
            Some(false)
        );
    }
    let source = include_str!("../src/x402.rs").to_ascii_lowercase();
    for forbidden in [
        "std::fs",
        "std::net",
        "std::process",
        "reqwest",
        "hyper",
        "axum",
        "sqlx",
        "aws_sdk",
        "coinbase",
        "stripe",
    ] {
        assert!(
            !source.contains(forbidden),
            "effect marker leaked: {forbidden}"
        );
    }
    Ok(())
}

fn rust_valid(kind: &str, value: &Value) -> bool {
    match kind {
        "payment-required" => serde_json::from_value::<X402PaymentRequired>(value.clone()).is_ok(),
        "payment-payload" => serde_json::from_value::<X402PaymentPayload>(value.clone()).is_ok(),
        "settle-response" => serde_json::from_value::<X402SettleResponse>(value.clone()).is_ok(),
        "runx_invocation_extension" => {
            serde_json::from_value::<RunxX402InvocationExtensionInfo>(value.clone()).is_ok()
        }
        _ => false,
    }
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
    repo_root().join("fixtures/contracts/x402-v2")
}

fn schema_root() -> PathBuf {
    repo_root().join("schemas")
}

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
}
