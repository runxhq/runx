use std::fs;
use std::path::PathBuf;

use runx_runtime::{inspect_skill_package, load_validated_skill_package};

fn write_packet_input_package(
    root: &std::path::Path,
    packet_schema: Option<&str>,
    example: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(
        root.join("SKILL.md"),
        "---\nname: packet-input\ndescription: canonical packet input\n---\n\n# Packet input\n\n## Agent task: inspect\n\nInspect.\n",
    )?;
    fs::write(
        root.join("run.yaml"),
        format!(
            "minimum_runx: 0.12.0\nmethods:\n  inspect:\n    inputs:\n      plan:\n        packet: acme/packet-input.plan.v1\n    examples:\n      - plan: {example}\n    steps:\n      - id: inspect\n        agent: inspect\n        inputs:\n          plan: $input.plan\n"
        ),
    )?;
    if let Some(packet_schema) = packet_schema {
        fs::create_dir_all(root.join("packets"))?;
        fs::write(root.join("packets/plan.v1.schema.json"), packet_schema)?;
    }
    Ok(())
}

#[test]
fn validated_skill_package_loads_one_digest_bound_aggregate()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("SKILL.md"),
        "---\nname: aggregate\ndescription: one package truth\n---\n\n# Aggregate\n\n## Agent task: inspect\n\nInspect.\n",
    )?;
    fs::write(
        temp.path().join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  inspect:\n    steps:\n      - id: inspect\n        agent: inspect\n",
    )?;

    let loaded = load_validated_skill_package(temp.path())?;

    assert_eq!(loaded.package.skill.name, "aggregate");
    assert_eq!(loaded.package.manual_markdown.lines().next(), Some("---"));
    assert!(loaded.package.manual_digest.starts_with("sha256:"));
    assert_eq!(loaded.package.source.files.len(), 2);
    Ok(())
}

#[cfg(unix)]
#[test]
fn validated_skill_package_rejects_symlinked_sources() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("SKILL.md"),
        "---\nname: aggregate\ndescription: aggregate fixture.\n---\n\n# Aggregate\n",
    )?;
    let outside = tempfile::NamedTempFile::new()?;
    symlink(outside.path(), temp.path().join("module.mjs"))?;

    let error = load_validated_skill_package(temp.path())
        .err()
        .ok_or("symlinked package unexpectedly loaded")?;

    assert!(error.to_string().contains("symbolic links"));
    Ok(())
}

#[test]
fn packet_input_schema_is_hydrated_into_inspection() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    write_packet_input_package(
        temp.path(),
        Some(
            r#"{"$id":"https://schemas.runx.ai/packages/acme/packet-input/plan/v1.json","x-runx-packet-id":"acme/packet-input.plan.v1","type":"object","required":["operation"],"properties":{"operation":{"type":"string","enum":["inspect"]}},"additionalProperties":false}"#,
        ),
        "{ operation: inspect }",
    )?;

    let inspection = inspect_skill_package(temp.path(), Some("inspect"), None)?;
    let plan = inspection
        .method
        .as_ref()
        .map(|method| &method.input_schema)
        .and_then(|value| value.get("properties"))
        .and_then(runx_contracts::JsonValue::as_object)
        .and_then(|value| value.get("plan"))
        .and_then(runx_contracts::JsonValue::as_object)
        .ok_or("hydrated plan schema missing")?;

    assert_eq!(
        plan.get("x-runx-packet-id")
            .and_then(runx_contracts::JsonValue::as_str),
        Some("acme/packet-input.plan.v1")
    );
    assert!(plan.contains_key("properties"));
    let inspection_json = serde_json::to_value(&inspection)?;
    assert_eq!(
        inspection_json
            .pointer("/closure/package_bindings/0/input_packet_schemas/0/packet")
            .and_then(serde_json::Value::as_str),
        Some("acme/packet-input.plan.v1")
    );
    assert!(
        inspection_json
            .pointer("/closure/package_bindings/0/input_packet_schemas/0/schema_digest")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );
    Ok(())
}

#[test]
fn packet_input_schema_must_exist_and_own_authored_examples()
-> Result<(), Box<dyn std::error::Error>> {
    let missing = tempfile::tempdir()?;
    write_packet_input_package(missing.path(), None, "{ operation: inspect }")?;
    let missing_error = load_validated_skill_package(missing.path())
        .err()
        .ok_or("missing packet schema unexpectedly loaded")?;
    assert!(
        missing_error
            .to_string()
            .contains("declared packet schema 'acme/packet-input.plan.v1' was not found")
    );

    let invalid = tempfile::tempdir()?;
    write_packet_input_package(
        invalid.path(),
        Some(
            r#"{"$id":"https://schemas.runx.ai/packages/acme/packet-input/plan/v1.json","x-runx-packet-id":"acme/packet-input.plan.v1","type":"object","required":["operation"],"properties":{"operation":{"const":"inspect"}},"additionalProperties":false}"#,
        ),
        "{ operation: mutate }",
    )?;
    let example_error = load_validated_skill_package(invalid.path())
        .err()
        .ok_or("invalid packet example unexpectedly loaded")?;
    assert!(
        example_error
            .to_string()
            .contains("examples[0]/plan/operation")
    );
    Ok(())
}

#[test]
fn bundled_core_skill_packages_validate_through_the_aggregate()
-> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bundle: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        root.join("crates/runx-runtime/bundled-skills.json"),
    )?)?;
    let skills = bundle["skills"].as_array().ok_or("bundle omitted skills")?;
    let mut failures = Vec::new();
    for entry in skills {
        let skill_id = entry["skill_id"]
            .as_str()
            .ok_or("bundle skill omitted id")?;
        let name = skill_id
            .strip_prefix("runx/")
            .ok_or("bundle skill has foreign publisher")?;
        let directory = root.join("skills").join(name);
        if let Err(error) = load_validated_skill_package(&directory) {
            failures.push(format!("{name}: {error}"));
        }
    }

    assert!(
        failures.is_empty(),
        "bundled core skill packages must share one aggregate contract:\n{}",
        failures.join("\n")
    );
    Ok(())
}
