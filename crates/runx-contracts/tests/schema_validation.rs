use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const AUTHORITY_PROOF_SCHEMA: &str = include_str!("../../../schemas/authority-proof.schema.json");
const RESOLUTION_REQUEST_SCHEMA: &str =
    include_str!("../../../schemas/resolution-request.schema.json");
const AUTHORITY_PROOF_FIXTURES: &[(&str, &str)] = &[(
    "authority-proof-metadata-full",
    include_str!("../../../fixtures/kernel/policy/authority-proof-metadata-full.json"),
)];

const CONTRACT_FIXTURE_SCHEMA_MAPPINGS: &[FixtureSchemaMapping] = &[
    FixtureSchemaMapping::new(
        "fixtures/contracts/act-assignment/cli-no-trigger.json",
        "/expected/envelope",
        "act-assignment.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/act-assignment/github-trigger.json",
        "/expected/envelope",
        "act-assignment.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/act-assignment/host-normalization.json",
        "/expected/envelope",
        "act-assignment.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/act-assignment/system-empty-inputs.json",
        "/expected/envelope",
        "act-assignment.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/credential-delivery/response.json",
        "/expected",
        "credential-delivery-response.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/credential-delivery/observation.json",
        "/expected",
        "credential-delivery-observation.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/credential-delivery/profile.json",
        "/expected",
        "credential-delivery-profile.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/credential-delivery/request.json",
        "/expected",
        "credential-delivery-request.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/harness-spine/receipt-abnormal.json",
        "/expected",
        "receipt.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/harness-spine/receipt-success.json",
        "/expected",
        "receipt.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/harness-spine/signal-fingerprint-links.json",
        "/expected",
        "signal.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/harness-spine/verification-act.json",
        "/expected",
        "act.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/host-protocol/resolution-agent-act-request.json",
        "/expected",
        "resolution-request.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/host-protocol/resolution-approval-request.json",
        "/expected",
        "resolution-request.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/host-protocol/resolution-input-request.json",
        "/expected",
        "resolution-request.schema.json",
    ),
    FixtureSchemaMapping::new(
        "fixtures/contracts/host-protocol/resolution-response.json",
        "/expected",
        "resolution-response.schema.json",
    ),
];

const CONTRACT_FIXTURE_EXEMPT_KINDS: &[&str] =
    &["event", "governed_act_ref", "run_result", "run_state"];

#[test]
fn authority_proof_outputs_validate_against_generated_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let validator = schema_validator(AUTHORITY_PROOF_SCHEMA)?;
    for (name, fixture_json) in AUTHORITY_PROOF_FIXTURES {
        let fixture: Value = serde_json::from_str(fixture_json)?;
        let authority_proof = fixture
            .pointer("/expected/value/authority_proof")
            .ok_or_else(|| format!("{name} missing expected.value.authority_proof"))?;
        assert_valid(&validator, authority_proof, name)?;
    }
    Ok(())
}

#[test]
fn mapped_contract_fixtures_validate_against_generated_schemas()
-> Result<(), Box<dyn std::error::Error>> {
    let mut validators = BTreeMap::new();
    for mapping in CONTRACT_FIXTURE_SCHEMA_MAPPINGS {
        let validator = validators
            .entry(mapping.schema_file)
            .or_insert(schema_file_validator(mapping.schema_file)?);
        let fixture = read_json_fixture(mapping.fixture_path)?;
        let payload = fixture.pointer(mapping.payload_pointer).ok_or_else(|| {
            format!(
                "{} missing {}",
                mapping.fixture_path, mapping.payload_pointer
            )
        })?;
        assert_valid(
            validator,
            payload,
            &format!(
                "{}{} against {}",
                mapping.fixture_path, mapping.payload_pointer, mapping.schema_file
            ),
        )?;
    }
    Ok(())
}

#[test]
fn contract_fixture_schema_mapping_has_only_declared_exemptions()
-> Result<(), Box<dyn std::error::Error>> {
    let mapped = CONTRACT_FIXTURE_SCHEMA_MAPPINGS
        .iter()
        .map(|mapping| mapping.fixture_path)
        .collect::<BTreeSet<_>>();
    let exempt_kinds = CONTRACT_FIXTURE_EXEMPT_KINDS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    for directory in [
        "fixtures/contracts/act-assignment",
        "fixtures/contracts/harness-spine",
        "fixtures/contracts/host-protocol",
    ] {
        for fixture_path in json_files_in(directory)? {
            let fixture_path = fixture_path_string(&fixture_path)?;
            if mapped.contains(fixture_path.as_str()) {
                continue;
            }
            let fixture = read_json_fixture(&fixture_path)?;
            let fixture_kind = fixture
                .get("fixture_kind")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{fixture_path} missing fixture_kind"))?;
            assert!(
                exempt_kinds.contains(fixture_kind),
                "{fixture_path} fixture_kind '{fixture_kind}' has no schema mapping or explicit exemption"
            );
        }
    }
    Ok(())
}

#[test]
fn host_approval_gate_is_rejected_inside_authority_proof() -> Result<(), Box<dyn std::error::Error>>
{
    let validator = schema_validator(AUTHORITY_PROOF_SCHEMA)?;
    let mut fixture: Value = serde_json::from_str(AUTHORITY_PROOF_FIXTURES[0].1)?;
    let authority_proof = fixture
        .pointer_mut("/expected/value/authority_proof")
        .ok_or("authority-proof fixture missing expected.value.authority_proof")?;
    authority_proof["approval_gate"] = json!({
        "id": "workspace-write",
        "reason": "Allow workspace write",
        "type": "filesystem_write",
        "summary": { "path": "docs/guide.md" }
    });

    assert_invalid(
        &validator,
        authority_proof,
        "host gate must not masquerade as authority proof gate",
    );
    Ok(())
}

#[test]
fn authority_proof_approval_gate_is_rejected_inside_host_resolution_request()
-> Result<(), Box<dyn std::error::Error>> {
    let validator = schema_validator(RESOLUTION_REQUEST_SCHEMA)?;
    let resolution_request = json!({
        "id": "req_approval",
        "kind": "approval",
        "gate": {
            "gate_id": "approval_1",
            "gate_type": "human",
            "decision": "approved",
            "reason": "mutating github action"
        }
    });

    assert_invalid(
        &validator,
        &resolution_request,
        "authority-proof gate must not masquerade as host resolution request gate",
    );
    Ok(())
}

fn schema_validator(schema: &str) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let schema: Value = serde_json::from_str(schema)?;
    Ok(jsonschema::draft202012::options().build(&schema)?)
}

fn schema_file_validator(
    schema_file: &str,
) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let schema = fs::read_to_string(repo_root().join("schemas").join(schema_file))?;
    schema_validator(&schema)
}

fn read_json_fixture(path: &str) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(&fs::read_to_string(
        repo_root().join(path),
    )?)?)
}

fn json_files_in(directory: &str) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut files = fs::read_dir(repo_root().join(directory))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    files.sort();
    Ok(files)
}

fn fixture_path_string(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let relative = path.strip_prefix(repo_root())?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .components()
        .collect()
}

struct FixtureSchemaMapping {
    fixture_path: &'static str,
    payload_pointer: &'static str,
    schema_file: &'static str,
}

impl FixtureSchemaMapping {
    const fn new(
        fixture_path: &'static str,
        payload_pointer: &'static str,
        schema_file: &'static str,
    ) -> Self {
        Self {
            fixture_path,
            payload_pointer,
            schema_file,
        }
    }
}

fn assert_valid(
    validator: &jsonschema::Validator,
    value: &Value,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let errors = validation_errors(validator, value);
    if !errors.is_empty() {
        return Err(format!("{label} failed schema validation:\n{}", errors.join("\n")).into());
    }
    Ok(())
}

fn assert_invalid(validator: &jsonschema::Validator, value: &Value, label: &str) {
    assert!(
        !validation_errors(validator, value).is_empty(),
        "{label}: value unexpectedly passed schema validation"
    );
}

fn validation_errors(validator: &jsonschema::Validator, value: &Value) -> Vec<String> {
    validator
        .iter_errors(value)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect()
}
