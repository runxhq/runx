use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue};
use runx_parser::SkillArtifactContract;

use crate::RuntimeError;
use crate::observations::PacketCheck;
use crate::packet_schemas::{PacketSchemaCatalog, packet_schema_directories};

/// Verify each artifact a step holds against the packet its contract
/// declares. `held` is the step's artifacts by name, each in its envelope.
pub(crate) fn verify_declared_packets(
    held: &JsonObject,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
    env: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, PacketCheck>, RuntimeError> {
    let bindings = packet_bindings(held, artifacts)?;
    if bindings.is_empty() {
        return Ok(BTreeMap::new());
    }

    let package_root = crate::skill_package::find_owning_package_root(skill_directory)
        .unwrap_or_else(|| skill_directory.to_path_buf());
    let workspace = crate::RunxPaths::resolve(env, skill_directory)
        .workspace()
        .to_path_buf();
    let schema_directories = packet_schema_directories(skill_directory, &package_root, &workspace)
        .map_err(|error| RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: format!("packet schema roots failed: {error}"),
        })?;
    let schemas = PacketSchemaCatalog::discover(schema_directories.clone()).map_err(|error| {
        RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: format!("packet schema catalog failed: {error}"),
        }
    })?;
    let mut checks = BTreeMap::new();
    for binding in bindings {
        let (output, check) = verify_packet_binding(binding, &schemas, &schema_directories)?;
        checks.insert(output, check);
    }
    Ok(checks)
}

fn verify_packet_binding(
    binding: PacketBinding,
    schemas: &PacketSchemaCatalog,
    schema_directories: &[std::path::PathBuf],
) -> Result<(String, PacketCheck), RuntimeError> {
    let schema = schemas.get(&binding.packet).ok_or_else(|| {
        let searched = schema_directories
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        packet_error(
            &binding,
            format!("declared packet schema was not found; searched {searched}"),
        )
    })?;
    let schema_document = serde_json::to_value(&schema.schema.value)
        .map_err(|source| RuntimeError::json("serializing packet schema for validation", source))?;
    let validator = jsonschema::draft202012::options()
        .build(&schema_document)
        .map_err(|error| packet_error(&binding, format!("packet schema is invalid: {error}")))?;
    let instance = serde_json::to_value(&binding.value).map_err(|source| {
        RuntimeError::json("serializing agent packet output for validation", source)
    })?;
    validator
        .validate(&instance)
        .map_err(|error| packet_error(&binding, format!("output violates schema: {error}")))?;
    Ok((
        binding.output,
        PacketCheck {
            packet: binding.packet,
            schema_sha256: schema.schema.sha256.clone(),
        },
    ))
}

fn packet_error(binding: &PacketBinding, detail: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: "agent".to_owned(),
        message: format!(
            "packet output '{}' for '{}': {detail}",
            binding.output, binding.packet
        ),
    }
}

struct PacketBinding {
    output: String,
    packet: String,
    value: JsonValue,
}

fn packet_bindings(
    held: &JsonObject,
    artifacts: Option<&SkillArtifactContract>,
) -> Result<Vec<PacketBinding>, RuntimeError> {
    let mut bindings = Vec::new();
    if let Some(artifacts) = artifacts {
        if let Some(named) = &artifacts.packets {
            for (output, packet) in named {
                bindings.push(projected_binding(held, output, packet)?);
            }
        }
        if let (Some(output), Some(packet)) = (&artifacts.wrap_as, &artifacts.packet) {
            bindings.push(projected_binding(held, output, packet)?);
        }
    }
    Ok(bindings)
}

fn projected_binding(
    projection: &JsonObject,
    output: &str,
    packet: &str,
) -> Result<PacketBinding, RuntimeError> {
    let envelope = projection
        .get(output)
        .ok_or_else(|| RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: format!("named packet output '{output}' was not returned"),
        })?;
    let value = envelope
        .as_object()
        .and_then(|object| object.get("data"))
        .cloned()
        .ok_or_else(|| RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: format!("packet output '{output}' was not projected as a data envelope"),
        })?;
    Ok(PacketBinding {
        output: output.to_owned(),
        packet: packet.to_owned(),
        value,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use runx_contracts::JsonValue;
    use runx_parser::SkillArtifactContract;

    use super::verify_declared_packets;

    /// The artifacts a step producing `value` holds under `artifacts`.
    fn held(value: &JsonValue, artifacts: &SkillArtifactContract) -> runx_contracts::JsonObject {
        crate::output_contract::project_artifact_outputs(value, Some(artifacts))
    }

    fn temp_skill() -> Result<tempfile::TempDir, std::io::Error> {
        let directory = tempfile::tempdir()?;
        fs::create_dir_all(directory.path().join("packets"))?;
        Ok(directory)
    }

    fn artifacts() -> SkillArtifactContract {
        SkillArtifactContract {
            emits: None,
            named_emits: Some(BTreeMap::from([("plan".to_owned(), "plan".to_owned())])),
            packets: Some(BTreeMap::from([(
                "plan".to_owned(),
                "runx.test.plan.v1".to_owned(),
            )])),
            wrap_as: None,
            packet: None,
        }
    }

    fn wrapped_artifacts() -> SkillArtifactContract {
        SkillArtifactContract {
            emits: None,
            named_emits: None,
            packets: None,
            wrap_as: Some("plan_packet".to_owned()),
            packet: Some("runx.test.wrapped-plan.v1".to_owned()),
        }
    }

    fn payload(value: JsonValue) -> JsonValue {
        JsonValue::Object(BTreeMap::from([("plan".to_owned(), value)]))
    }

    fn write_schema(skill: &std::path::Path) -> Result<(), std::io::Error> {
        fs::write(
            skill.join("packets/plan.schema.json"),
            r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "x-runx-packet-id": "runx.test.plan.v1",
  "type": "object",
  "required": ["decision"],
  "properties": {"decision": {"type": "string"}},
  "additionalProperties": false
}
"#,
        )?;
        Ok(())
    }

    fn write_wrapped_schema(skill: &std::path::Path) -> Result<(), std::io::Error> {
        fs::write(
            skill.join("packets/wrapped-plan.schema.json"),
            r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "x-runx-packet-id": "runx.test.wrapped-plan.v1",
  "type": "object",
  "required": ["plan"],
  "properties": {
    "plan": {
      "type": "object",
      "required": ["decision"],
      "properties": {"decision": {"type": "string"}},
      "additionalProperties": false
    }
  },
  "additionalProperties": false
}
"#,
        )?;
        Ok(())
    }

    #[test]
    fn declared_packet_schema_is_verified_and_pinned() -> Result<(), Box<dyn std::error::Error>> {
        let skill = temp_skill()?;
        write_schema(skill.path())?;
        let value = payload(JsonValue::Object(BTreeMap::from([(
            "decision".to_owned(),
            JsonValue::String("ready".to_owned()),
        )])));

        let evidence = verify_declared_packets(
            &held(&value, &artifacts()),
            Some(&artifacts()),
            skill.path(),
            &BTreeMap::new(),
        )?;

        let plan = evidence.get("plan").ok_or("plan evidence is missing")?;
        assert_eq!(plan.packet, "runx.test.plan.v1");
        assert!(plan.schema_sha256.starts_with("sha256:"));
        Ok(())
    }

    #[test]
    fn invalid_packet_output_cannot_seal() -> Result<(), Box<dyn std::error::Error>> {
        let skill = temp_skill()?;
        write_schema(skill.path())?;
        let value = payload(JsonValue::Object(BTreeMap::from([(
            "decision".to_owned(),
            JsonValue::Bool(true),
        )])));

        assert!(
            verify_declared_packets(
                &held(&value, &artifacts()),
                Some(&artifacts()),
                skill.path(),
                &BTreeMap::new(),
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn wrapped_packet_schema_validates_the_declared_output_envelope()
    -> Result<(), Box<dyn std::error::Error>> {
        let skill = temp_skill()?;
        write_wrapped_schema(skill.path())?;
        let value = payload(JsonValue::Object(BTreeMap::from([(
            "decision".to_owned(),
            JsonValue::Bool(true),
        )])));

        assert!(
            verify_declared_packets(
                &held(&value, &wrapped_artifacts()),
                Some(&wrapped_artifacts()),
                skill.path(),
                &BTreeMap::new(),
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn missing_packet_schema_cannot_seal() -> Result<(), Box<dyn std::error::Error>> {
        let skill = temp_skill()?;
        let value = payload(JsonValue::Object(BTreeMap::from([(
            "decision".to_owned(),
            JsonValue::String("ready".to_owned()),
        )])));

        assert!(
            verify_declared_packets(
                &held(&value, &artifacts()),
                Some(&artifacts()),
                skill.path(),
                &BTreeMap::new(),
            )
            .is_err()
        );
        Ok(())
    }
}
