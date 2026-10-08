use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue};
use runx_parser::SkillArtifactContract;

use crate::RuntimeError;
use crate::packet_schemas::{PacketSchemaCatalog, packet_schema_directories};

pub(crate) fn verify_declared_packets(
    payload: &JsonValue,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
    env: &BTreeMap<String, String>,
) -> Result<JsonObject, RuntimeError> {
    let bindings = packet_bindings(payload, artifacts)?;
    if bindings.is_empty() {
        return Ok(JsonObject::new());
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
    let mut evidence = JsonObject::new();
    for binding in bindings {
        let (output, verified) = verify_packet_binding(binding, &schemas, &schema_directories)?;
        evidence.insert(output, verified);
    }
    Ok(evidence)
}

fn verify_packet_binding(
    binding: PacketBinding,
    schemas: &PacketSchemaCatalog,
    schema_directories: &[std::path::PathBuf],
) -> Result<(String, JsonValue), RuntimeError> {
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
    let verified = JsonValue::Object(
        [
            ("packet".to_owned(), JsonValue::String(binding.packet)),
            (
                "schema_sha256".to_owned(),
                JsonValue::String(schema.schema.sha256.clone()),
            ),
        ]
        .into_iter()
        .collect(),
    );
    Ok((binding.output, verified))
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
    payload: &JsonValue,
    artifacts: Option<&SkillArtifactContract>,
) -> Result<Vec<PacketBinding>, RuntimeError> {
    let mut bindings = Vec::new();
    if let Some(artifacts) = artifacts {
        let projected = crate::output_contract::project_artifact_outputs(payload, Some(artifacts));
        if let Some(named) = &artifacts.packets {
            for (output, packet) in named {
                bindings.push(projected_binding(&projected, output, packet)?);
            }
        }
        if let (Some(output), Some(packet)) = (&artifacts.wrap_as, &artifacts.packet) {
            bindings.push(projected_binding(&projected, output, packet)?);
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
    use crate::output_contract::project_artifact_outputs;

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

        let evidence =
            verify_declared_packets(&value, Some(&artifacts()), skill.path(), &BTreeMap::new())?;

        let Some(plan) = evidence.get("plan").and_then(JsonValue::as_object) else {
            return Err("plan evidence is missing".into());
        };
        assert_eq!(
            plan.get("packet").and_then(JsonValue::as_str),
            Some("runx.test.plan.v1")
        );
        assert!(
            plan.get("schema_sha256")
                .and_then(JsonValue::as_str)
                .is_some_and(|value| value.starts_with("sha256:"))
        );
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
            verify_declared_packets(&value, Some(&artifacts()), skill.path(), &BTreeMap::new(),)
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
                &value,
                Some(&wrapped_artifacts()),
                skill.path(),
                &BTreeMap::new(),
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn packet_validation_is_identical_after_runtime_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let skill = temp_skill()?;
        write_wrapped_schema(skill.path())?;
        let value = payload(JsonValue::Object(BTreeMap::from([(
            "decision".to_owned(),
            JsonValue::String("ready".to_owned()),
        )])));
        let artifacts = wrapped_artifacts();
        let mut projected_payload = value.clone();
        let projection = project_artifact_outputs(&value, Some(&artifacts));
        let JsonValue::Object(projected_object) = &mut projected_payload else {
            return Err("projected payload must be an object".into());
        };
        projected_object.extend(projection);

        verify_declared_packets(&value, Some(&artifacts), skill.path(), &BTreeMap::new())?;
        verify_declared_packets(
            &projected_payload,
            Some(&artifacts),
            skill.path(),
            &BTreeMap::new(),
        )?;
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
            verify_declared_packets(&value, Some(&artifacts()), skill.path(), &BTreeMap::new(),)
                .is_err()
        );
        Ok(())
    }
}
