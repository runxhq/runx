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
        "---\nname: packet-input\ndescription: canonical packet input\n---\n\n# Packet input\n",
    )?;
    fs::write(
        root.join("run.yaml"),
        format!(
            "skill: packet-input\nrunners:\n  inspect:\n    inputs:\n      plan:\n        type: json\n        required: true\n        packet: runx.test.plan.v1\n    examples:\n      - plan: {example}\n    steps:\n      - id: inspect\n        agent: {{ role: agent, task: inspect }}\n        inputs:\n          plan: $input.plan\n"
        ),
    )?;
    if let Some(packet_schema) = packet_schema {
        fs::create_dir_all(root.join("packets"))?;
        fs::write(root.join("packets/plan.schema.json"), packet_schema)?;
    }
    Ok(())
}

#[test]
fn validated_skill_package_loads_one_digest_bound_aggregate()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("SKILL.md"),
        "---\nname: aggregate\ndescription: one package truth\n---\n\n# Aggregate\n",
    )?;
    fs::write(
        temp.path().join("run.yaml"),
        "skill: aggregate\nrunners:\n  inspect:\n    steps:\n      - id: inspect\n        agent: { role: agent, task: inspect }\n",
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
fn validated_skill_package_resolves_internal_profile_to_owning_manual()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let profile_dir = temp.path().join("graph/plan");
    fs::create_dir_all(&profile_dir)?;
    let manual = "---\nname: operator\ndescription: full operator context\n---\n\n# Operator\n\nPreserve this complete manual.\n";
    fs::write(temp.path().join("SKILL.md"), manual)?;
    fs::write(
        profile_dir.join("run.yaml"),
        "skill: operator-plan\nrunners:\n  plan:\n    steps:\n      - id: plan\n        tool: { module: plan.mjs }\n",
    )?;
    fs::write(
        profile_dir.join("plan.mjs"),
        "export default (inputs) => inputs;\n",
    )?;

    let loaded = load_validated_skill_package(&profile_dir)?;

    // Package roots are canonical; macOS tempdirs resolve through /private.
    assert_eq!(loaded.package_root, temp.path().canonicalize()?);
    assert_eq!(loaded.profile_path.as_deref(), Some("graph/plan/run.yaml"));
    assert!(loaded.manifest().is_some());
    assert_eq!(loaded.package.manual_markdown, manual);
    Ok(())
}

#[test]
fn packet_input_schema_is_hydrated_into_inspection() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    write_packet_input_package(
        temp.path(),
        Some(
            r#"{"x-runx-packet-id":"runx.test.plan.v1","type":"object","required":["operation"],"properties":{"operation":{"type":"string","enum":["inspect"]}},"additionalProperties":false}"#,
        ),
        "{ operation: inspect }",
    )?;

    let inspection = inspect_skill_package(temp.path(), Some("inspect"), None)?;
    let plan = inspection
        .as_object()
        .and_then(|value| value.get("runner"))
        .and_then(runx_contracts::JsonValue::as_object)
        .and_then(|value| value.get("input_schema"))
        .and_then(runx_contracts::JsonValue::as_object)
        .and_then(|value| value.get("properties"))
        .and_then(runx_contracts::JsonValue::as_object)
        .and_then(|value| value.get("plan"))
        .and_then(runx_contracts::JsonValue::as_object)
        .ok_or("hydrated plan schema missing")?;

    assert_eq!(
        plan.get("x-runx-packet-id")
            .and_then(runx_contracts::JsonValue::as_str),
        Some("runx.test.plan.v1")
    );
    assert!(plan.contains_key("properties"));
    let inspection_json = serde_json::to_value(&inspection)?;
    assert_eq!(
        inspection_json
            .pointer("/execution_closure/package_bindings/0/input_packet_schemas/0/packet")
            .and_then(serde_json::Value::as_str),
        Some("runx.test.plan.v1")
    );
    assert!(
        inspection_json
            .pointer("/execution_closure/package_bindings/0/input_packet_schemas/0/schema_digest")
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
    assert!(missing_error.to_string().contains("missing packet schema"));

    let invalid = tempfile::tempdir()?;
    write_packet_input_package(
        invalid.path(),
        Some(
            r#"{"x-runx-packet-id":"runx.test.plan.v1","type":"object","required":["operation"],"properties":{"operation":{"const":"inspect"}},"additionalProperties":false}"#,
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
fn official_skill_packages_validate_through_the_aggregate() -> Result<(), Box<dyn std::error::Error>>
{
    let skills_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("skills");
    let mut failures = Vec::new();
    let mut directories = fs::read_dir(&skills_root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("SKILL.md").is_file())
        .collect::<Vec<_>>();
    directories.sort();

    for directory in directories {
        if let Err(error) = load_validated_skill_package(&directory) {
            let name = directory
                .file_name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            failures.push(format!("{name}: {error}"));
        }
    }

    assert!(
        failures.is_empty(),
        "official skill packages must share one aggregate contract:\n{}",
        failures.join("\n")
    );
    Ok(())
}
