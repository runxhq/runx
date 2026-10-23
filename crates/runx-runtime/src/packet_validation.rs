use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue, PacketId};
use runx_parser::SkillArtifactContract;

use crate::RuntimeError;
use crate::observations::PacketCheck;
use crate::packet_schemas::PacketSchemaCatalog;

/// Verify each artifact a step holds against the packet its contract
/// declares. `held` is the step's artifacts by name, each in its envelope.
pub(crate) fn verify_declared_packets(
    held: &JsonObject,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
) -> Result<BTreeMap<String, PacketCheck>, RuntimeError> {
    let bindings = packet_bindings(held, artifacts)?;
    if bindings.is_empty() {
        return Ok(BTreeMap::new());
    }

    let package_root = crate::skill_package::find_owning_package_root(skill_directory)
        .unwrap_or_else(|| skill_directory.to_path_buf());
    let schemas = PacketSchemaCatalog::for_package(
        &package_root,
        bindings.iter().map(|binding| &binding.packet),
    )
    .map_err(|error| RuntimeError::SkillFailed {
        skill_name: "agent".to_owned(),
        message: format!("packet schema catalog failed: {error}"),
    })?;
    let mut checks = BTreeMap::new();
    for binding in bindings {
        let (output, check) = verify_packet_binding(binding, &schemas)?;
        checks.insert(output, check);
    }
    Ok(checks)
}

fn verify_packet_binding(
    binding: PacketBinding,
    schemas: &PacketSchemaCatalog,
) -> Result<(String, PacketCheck), RuntimeError> {
    let schema = schemas
        .get(&binding.packet)
        .ok_or_else(|| packet_error(&binding, schemas.not_found(&binding.packet)))?;
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
            schema_digest: schema.schema.digest.clone(),
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
    packet: PacketId,
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
    packet: &PacketId,
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
        packet: packet.clone(),
        value,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use runx_contracts::{JsonValue, PacketId};
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
            packets: Some(BTreeMap::from([("plan".to_owned(), packet_id(PLAN))])),
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
            packet: Some(packet_id(WRAPPED_PLAN)),
        }
    }

    const PLAN: &str = "acme/planner.plan.v1";
    const WRAPPED_PLAN: &str = "acme/planner.wrapped_plan.v1";

    fn packet_id(id: &str) -> PacketId {
        PacketId::new(id).unwrap_or_else(|| unreachable!("{id} is a packet id"))
    }

    fn payload(value: JsonValue) -> JsonValue {
        JsonValue::Object(BTreeMap::from([("plan".to_owned(), value)]))
    }

    fn write_schema(skill: &std::path::Path) -> Result<(), std::io::Error> {
        fs::write(
            skill.join("packets/plan.v1.schema.json"),
            r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.runx.ai/packages/acme/planner/plan/v1.json",
  "x-runx-packet-id": "acme/planner.plan.v1",
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
            skill.join("packets/wrapped_plan.v1.schema.json"),
            r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.runx.ai/packages/acme/planner/wrapped-plan/v1.json",
  "x-runx-packet-id": "acme/planner.wrapped_plan.v1",
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
        )?;

        let plan = evidence.get("plan").ok_or("plan evidence is missing")?;
        assert_eq!(plan.packet, PLAN);
        assert!(plan.schema_digest.starts_with("sha256:"));
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
            )
            .is_err()
        );
        Ok(())
    }
}
