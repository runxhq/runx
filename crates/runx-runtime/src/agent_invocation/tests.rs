use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use runx_contracts::{
    EnvironmentRequirements, ExecutionRequirements, JsonObject, JsonValue, ProvenanceEntry,
    sha256_prefixed,
};
use runx_parser::{AgentTask, SkillArtifactContract};

use super::build_agent_act_invocation;
use super::profiles::{BUNDLED_VOICE_PROFILE_CONTENT, bundled_profile};
use crate::{CredentialDelivery, Executable, SkillInvocation};

/// The package an invocation's run would admit: its skill as it is.
fn package_of(request: &SkillInvocation) -> Result<crate::LoadedSkillPackage, crate::RuntimeError> {
    crate::load_validated_skill_package(&request.skill_directory)
}

fn temp_skill(body: &str) -> Result<tempfile::TempDir, std::io::Error> {
    let directory = tempfile::tempdir()?;
    fs::write(
        directory.path().join("SKILL.md"),
        format!(
            "---\nname: contract-test\ndescription: Contract test\n---\n\n# Contract test\n\n{body}\n"
        ),
    )?;
    Ok(directory)
}

fn invocation(skill_directory: PathBuf, outputs: Option<JsonObject>) -> SkillInvocation {
    SkillInvocation {
        historical_context: Vec::new(),
        skill_name: "contract-test".to_owned(),
        step_id: None,
        artifacts: None,
        executable: Executable::Agent(AgentTask {
            role: "planner".to_owned(),
            task: "plan".to_owned(),
            allowance: Default::default(),
            connectors: Default::default(),
        }),
        outputs,
        requirements: ExecutionRequirements::default(),
        inputs: JsonObject::new(),
        resolved_inputs: JsonObject::new(),
        current_context: Vec::new(),
        provenance: Vec::new(),
        skill_directory,
        env: BTreeMap::new(),
        credential_delivery: CredentialDelivery::none(),
    }
}

fn outputs() -> JsonObject {
    BTreeMap::from([("plan".to_owned(), JsonValue::String("object".to_owned()))])
}

#[test]
fn bundled_voice_profile_has_content_addressed_identity() {
    let profile = bundled_profile("VOICE.md", BUNDLED_VOICE_PROFILE_CONTENT);

    assert_eq!(profile.content, BUNDLED_VOICE_PROFILE_CONTENT);
    assert!(profile.sha256.as_ref().starts_with("sha256:"));
    assert_eq!(profile.root_path.as_ref(), "runx://profiles");
}

#[test]
fn bundled_voice_profile_matches_canonical_workspace_profile() -> Result<(), std::io::Error> {
    let canonical = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("skills")
        .join("VOICE.md");
    let source = fs::read_to_string(&canonical)?;

    assert_eq!(BUNDLED_VOICE_PROFILE_CONTENT, source);
    Ok(())
}

#[test]
fn agent_invocation_requires_declared_outputs() -> Result<(), Box<dyn std::error::Error>> {
    let skill = temp_skill("Produce one bounded, evidence-backed plan.")?;
    let request = invocation(skill.path().to_path_buf(), None);

    let Err(error) =
        build_agent_act_invocation(&request, &package_of(&request)?, "agent".to_owned())
    else {
        return Err("missing outputs must fail".into());
    };

    assert!(error.to_string().contains("at least one output"));
    Ok(())
}

#[test]
fn agent_invocation_pins_voice_and_output_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let skill = temp_skill("Produce one bounded, evidence-backed plan.")?;
    let request = invocation(skill.path().to_path_buf(), Some(outputs()));

    let resolved =
        build_agent_act_invocation(&request, &package_of(&request)?, "agent".to_owned())?;

    assert!(resolved.envelope.voice_profile.is_some());
    assert_eq!(
        resolved.envelope.output.as_ref().map(BTreeMap::len),
        Some(1)
    );
    Ok(())
}

#[test]
fn agent_invocation_hydrates_the_exact_declared_packet_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let skill = temp_skill("Produce one bounded, evidence-backed plan.")?;
    fs::create_dir_all(skill.path().join("packets"))?;
    fs::write(
        skill.path().join("packets/plan.schema.json"),
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "x-runx-packet-id": "runx.test.agent-plan.v1",
  "type": "object",
  "required": ["decision"],
  "properties": {"decision": {"type": "string"}},
  "additionalProperties": false
}"#,
    )?;
    let mut request = invocation(skill.path().to_path_buf(), Some(outputs()));
    request.artifacts = Some(SkillArtifactContract {
        emits: None,
        named_emits: Some(BTreeMap::from([("plan".to_owned(), "plan".to_owned())])),
        packets: Some(BTreeMap::from([(
            "plan".to_owned(),
            "runx.test.agent-plan.v1".to_owned(),
        )])),
        wrap_as: None,
        packet: None,
    });

    let resolved =
        build_agent_act_invocation(&request, &package_of(&request)?, "agent".to_owned())?;
    let output_schema = resolved
        .envelope
        .output_schema
        .as_ref()
        .ok_or("exact agent output schema is missing")?;
    assert_eq!(
        output_schema
            .as_object()
            .and_then(|schema| schema.get("type"))
            .and_then(JsonValue::as_str),
        Some("object")
    );
    assert!(
        output_schema
            .as_object()
            .is_some_and(|schema| !schema.contains_key("allOf"))
    );
    let validator =
        jsonschema::draft202012::options().build(&serde_json::to_value(output_schema)?)?;
    let invalid = JsonValue::Object(BTreeMap::from([(
        "plan".to_owned(),
        JsonValue::Object(BTreeMap::new()),
    )]));
    let valid = JsonValue::Object(BTreeMap::from([(
        "plan".to_owned(),
        JsonValue::Object(BTreeMap::from([(
            "decision".to_owned(),
            JsonValue::String("ready".to_owned()),
        )])),
    )]));

    assert!(validator.validate(&serde_json::to_value(invalid)?).is_err());
    let valid = serde_json::to_value(valid)?;
    validator
        .validate(&valid)
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[test]
fn agent_invocation_applies_a_wrapped_packet_to_the_complete_result()
-> Result<(), Box<dyn std::error::Error>> {
    let skill = temp_skill("Produce one bounded, evidence-backed plan.")?;
    fs::create_dir_all(skill.path().join("packets"))?;
    fs::write(
        skill.path().join("packets/complete.schema.json"),
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "x-runx-packet-id": "runx.test.complete-agent-result.v1",
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
}"#,
    )?;
    let mut request = invocation(skill.path().to_path_buf(), Some(outputs()));
    request.artifacts = Some(SkillArtifactContract {
        emits: None,
        named_emits: None,
        packets: None,
        wrap_as: Some("complete_result".to_owned()),
        packet: Some("runx.test.complete-agent-result.v1".to_owned()),
    });

    let resolved =
        build_agent_act_invocation(&request, &package_of(&request)?, "agent".to_owned())?;
    let output_schema = resolved
        .envelope
        .output_schema
        .as_ref()
        .ok_or("exact agent output schema is missing")?;
    assert_eq!(
        output_schema
            .as_object()
            .and_then(|schema| schema.get("type"))
            .and_then(JsonValue::as_str),
        Some("object")
    );
    assert!(
        output_schema
            .as_object()
            .is_some_and(|schema| !schema.contains_key("allOf"))
    );
    let validator =
        jsonschema::draft202012::options().build(&serde_json::to_value(output_schema)?)?;
    let invalid = JsonValue::Object(BTreeMap::from([(
        "plan".to_owned(),
        JsonValue::Object(BTreeMap::new()),
    )]));
    let valid = JsonValue::Object(BTreeMap::from([(
        "plan".to_owned(),
        JsonValue::Object(BTreeMap::from([(
            "decision".to_owned(),
            JsonValue::String("ready".to_owned()),
        )])),
    )]));

    assert!(validator.validate(&serde_json::to_value(invalid)?).is_err());
    let valid = serde_json::to_value(valid)?;
    validator
        .validate(&valid)
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[test]
fn agent_context_carries_exact_non_secret_requirements_and_environment_readiness()
-> Result<(), Box<dyn std::error::Error>> {
    let skill = temp_skill("Use the declared execution requirements without guessing.")?;
    let mut request = invocation(skill.path().to_path_buf(), Some(outputs()));
    request.requirements = ExecutionRequirements {
        scopes: vec![
            "provider:Read.Mixed".to_owned(),
            "provider:Read.Mixed".to_owned(),
            "opaque/scope?x=1".to_owned(),
        ],
        environment: EnvironmentRequirements {
            required: vec!["REGION".to_owned()],
            optional: vec!["TRACE_LABEL".to_owned()],
        },
    };
    request.env = BTreeMap::from([
        ("REGION".to_owned(), "ap-southeast-2".to_owned()),
        ("TRACE_LABEL".to_owned(), "private-value".to_owned()),
        ("UNDECLARED".to_owned(), "never-visible".to_owned()),
    ]);

    let resolved =
        build_agent_act_invocation(&request, &package_of(&request)?, "agent".to_owned())?;
    assert_eq!(
        resolved.envelope.requirements.declaration,
        request.requirements
    );
    assert_eq!(
        resolved.envelope.requirements.environment,
        vec![
            runx_contracts::EnvironmentRequirementStatus {
                name: "REGION".to_owned(),
                required: true,
                available: true,
            },
            runx_contracts::EnvironmentRequirementStatus {
                name: "TRACE_LABEL".to_owned(),
                required: false,
                available: true,
            },
        ]
    );
    let serialized = serde_json::to_string(&resolved.envelope)?;
    assert!(!serialized.contains("private-value"));
    assert!(!serialized.contains("never-visible"));
    Ok(())
}

#[test]
fn agent_step_instructions_are_the_complete_canonical_skill_document()
-> Result<(), Box<dyn std::error::Error>> {
    let skill_rule = "Keep domain policy in the skill.";
    let skill = temp_skill(skill_rule)?;
    let manual = fs::read_to_string(skill.path().join("SKILL.md"))?;
    let mut request = invocation(skill.path().to_path_buf(), Some(outputs()));
    request.step_id = Some("review".to_owned());
    request.provenance = vec![ProvenanceEntry {
        input: "source".into(),
        output: "research_packet".into(),
        from_step: Some("research".to_owned()),
        artifact_id: None,
        receipt_id: Some("rx_research".to_owned()),
    }];

    let resolved =
        build_agent_act_invocation(&request, &package_of(&request)?, "agent".to_owned())?;

    assert_eq!(resolved.envelope.instructions.as_ref(), manual);
    assert_eq!(
        resolved.envelope.instructions_sha256.as_ref(),
        sha256_prefixed(manual.as_bytes())
    );
    assert_eq!(
        resolved.envelope.step_id.as_ref().map(AsRef::as_ref),
        Some("review")
    );
    assert_eq!(resolved.envelope.provenance, request.provenance);
    Ok(())
}

#[test]
fn agent_step_fails_without_skill_instructions() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let request = invocation(directory.path().to_path_buf(), Some(outputs()));

    // An act's instructions come from its run's package, and a directory
    // without SKILL.md is no package to admit.
    let error = package_of(&request)
        .err()
        .ok_or("agent tasks without SKILL.md instructions must fail closed")?;

    assert!(error.to_string().contains("SKILL.md"));
    Ok(())
}
