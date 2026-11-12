use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use runx_contracts::{
    EnvironmentRequirements, JsonObject, JsonValue, ProvenanceEntry, sha256_prefixed,
};
use runx_parser::{AgentTask, OutputArtifacts};

use super::build_turn_invocation;
use crate::{CredentialDelivery, Executable, SkillInvocation};

/// The package an invocation's run would admit: its skill as it is.
fn package_of(request: &SkillInvocation) -> Result<crate::LoadedSkillPackage, crate::RuntimeError> {
    crate::load_validated_skill_package(&request.skill_directory)
}

fn packet_id(id: &str) -> Result<runx_contracts::PacketId, String> {
    runx_contracts::PacketId::new(id).ok_or_else(|| format!("{id} is not a packet id"))
}

fn temp_skill(body: &str) -> Result<tempfile::TempDir, std::io::Error> {
    let directory = tempfile::tempdir()?;
    fs::write(
        directory.path().join("SKILL.md"),
        format!(
            "---\nname: contract-test\ndescription: Contract test\n---\n\n# Contract test\n\n## Agent task: plan\n\n{body}\n"
        ),
    )?;
    Ok(directory)
}

fn invocation(skill_directory: PathBuf, outputs: Option<JsonObject>) -> SkillInvocation {
    SkillInvocation {
        reach: runx_contracts::Reach::Inputs,
        historical_context: Vec::new(),
        skill_name: "contract-test".to_owned(),
        step_id: None,
        artifacts: None,
        executable: Executable::Agent(AgentTask {
            task: "plan".to_owned(),
            allowance: Default::default(),
            connectors: Default::default(),
            requires: Default::default(),
            context_kinds: Default::default(),
            context_entries: Default::default(),
        }),
        outputs,
        environment: EnvironmentRequirements::default(),
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
    BTreeMap::from([(
        "plan".to_owned(),
        JsonValue::Object(JsonObject::from([(
            "type".to_owned(),
            JsonValue::String("object".to_owned()),
        )])),
    )])
}

#[test]
fn voice_profiles_are_selected_by_the_workspace_not_the_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let skill = temp_skill("Derive context only from the supplied evidence.")?;
    fs::write(
        skill.path().join("VOICE.md"),
        "A package's unrelated writing style.",
    )?;
    let mut request = invocation(skill.path().to_path_buf(), Some(outputs()));
    request.env.insert(
        "RUNX_CWD".to_owned(),
        workspace.path().to_string_lossy().into_owned(),
    );
    let resolve = |request: &SkillInvocation| {
        build_turn_invocation(request, &package_of(request)?, None, "agent".to_owned())
    };
    assert!(resolve(&request)?.envelope.voice_profile.is_none());
    fs::write(
        workspace.path().join("VOICE.md"),
        "The operator's workspace style.",
    )?;
    let selected = resolve(&request)?
        .envelope
        .voice_profile
        .ok_or("workspace voice is selected")?;
    assert_eq!(selected.content, "The operator's workspace style.");
    assert_eq!(
        selected.digest.as_str(),
        sha256_prefixed(selected.content.as_bytes())
    );
    fs::write(
        workspace.path().join("selected.md"),
        "The explicitly selected style.",
    )?;
    request.env.insert(
        "RUNX_VOICE_PROFILE_PATH".to_owned(),
        "selected.md".to_owned(),
    );
    assert_eq!(
        resolve(&request)?
            .envelope
            .voice_profile
            .ok_or("explicit voice is selected")?
            .content,
        "The explicitly selected style."
    );
    Ok(())
}

#[test]
fn agent_invocation_requires_declared_outputs() -> Result<(), Box<dyn std::error::Error>> {
    let skill = temp_skill("Produce one bounded, evidence-backed plan.")?;
    let request = invocation(skill.path().to_path_buf(), None);

    let Err(error) =
        build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())
    else {
        return Err("missing outputs must fail".into());
    };

    assert!(error.to_string().contains("at least one output"));
    Ok(())
}

#[test]
fn agent_invocation_pins_declared_output_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let skill = temp_skill("Produce one bounded, evidence-backed plan.")?;
    let request = invocation(skill.path().to_path_buf(), Some(outputs()));

    let resolved =
        build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())?;

    assert!(resolved.envelope.voice_profile.is_none());
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
        skill.path().join("packets/plan.v1.schema.json"),
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.runx.ai/packages/acme/contract-test/plan/v1.json",
  "x-runx-packet-id": "acme/contract-test.plan.v1",
  "type": "object",
  "required": ["decision"],
  "properties": {"decision": {"type": "string"}},
  "additionalProperties": false
}"#,
    )?;
    let mut request = invocation(skill.path().to_path_buf(), Some(outputs()));
    request.artifacts = Some(OutputArtifacts {
        result_as: None,
        packets: BTreeMap::from([("plan".to_owned(), packet_id("acme/contract-test.plan.v1")?)]),
    });

    let resolved =
        build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())?;
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
fn agent_invocation_preserves_inline_and_packet_constraints()
-> Result<(), Box<dyn std::error::Error>> {
    for wrapped in [false, true] {
        let skill =
            temp_skill("Assess the supplied evidence without weakening either output contract.")?;
        fs::create_dir_all(skill.path().join("packets"))?;
        let packet = packet_id("acme/contract-test.lossless.v1")?;
        let packet_fields = serde_json::json!({
            "type": "object",
            "required": ["decision"],
            "properties": {"decision": {"$ref": "#/$defs/decision"}},
            "additionalProperties": true
        });
        let mut packet_schema = if wrapped {
            serde_json::json!({"type":"object", "required":["plan"], "properties":{"plan":packet_fields}, "additionalProperties":false})
        } else {
            packet_fields
        };
        packet_schema["$schema"] =
            serde_json::json!("https://json-schema.org/draft/2020-12/schema");
        packet_schema["$id"] = serde_json::json!(packet.schema_url());
        packet_schema["x-runx-packet-id"] = serde_json::json!(packet.as_str());
        packet_schema["$defs"] = serde_json::json!({"decision":{"type":"string","const":"ready"}});
        fs::write(
            skill.path().join("packets/lossless.v1.schema.json"),
            packet_schema.to_string(),
        )?;
        let declared = serde_json::from_value(serde_json::json!({
            "plan": {
                "type":"object", "required":["decision","caller_work"],
                "properties": {
                    "decision":{"type":"string","enum":["ready","blocked"]},
                    "caller_work":{"type":"array","minItems":1,"items":{"type":"string"}}
                }, "additionalProperties":false
            }
        }))?;
        let mut request = invocation(skill.path().to_path_buf(), Some(declared));
        request.artifacts = Some(OutputArtifacts {
            result_as: wrapped.then(|| "complete_result".to_owned()),
            packets: BTreeMap::from([(
                (if wrapped { "complete_result" } else { "plan" }).to_owned(),
                packet,
            )]),
        });
        let resolved =
            build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())?;
        let schema = serde_json::to_value(
            resolved
                .envelope
                .output_schema
                .as_ref()
                .ok_or("missing schema")?,
        )?;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["plan"].is_object());
        let validator = jsonschema::draft202012::options().build(&schema)?;
        let turn = runx_contracts::ResolutionRequest::Turn {
            id: "assess".into(),
            invocation: Box::new(resolved),
        };
        for (value, valid) in [
            (
                serde_json::json!({"plan":{"decision":"ready","caller_work":["inspect"]}}),
                true,
            ),
            (
                serde_json::json!({"plan":{"decision":"ready","caller_work":"scalar"}}),
                false,
            ),
            (serde_json::json!({"plan":{"decision":"ready"}}), false),
            (
                serde_json::json!({"plan":{"decision":"blocked","caller_work":["inspect"]}}),
                false,
            ),
            (
                serde_json::json!({"plan":{"decision":"ready","caller_work":["inspect"],"extra":true}}),
                false,
            ),
        ] {
            let offered = validator.validate(&value).is_ok();
            let value: JsonValue = serde_json::from_value(value)?;
            let enforced = crate::agent_contract::agent_contract_checks_with_artifacts(
                &turn,
                &value,
                request.artifacts.as_ref(),
                skill.path(),
            )
            .is_ok();
            assert_eq!(offered, valid, "offered wrapped={wrapped}: {value:?}");
            assert_eq!(enforced, valid, "enforced wrapped={wrapped}: {value:?}");
        }
    }
    Ok(())
}

#[test]
fn agent_invocation_applies_a_wrapped_packet_to_the_complete_result()
-> Result<(), Box<dyn std::error::Error>> {
    let skill = temp_skill("Produce one bounded, evidence-backed plan.")?;
    fs::create_dir_all(skill.path().join("packets"))?;
    fs::write(
        skill.path().join("packets/complete_result.v1.schema.json"),
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.runx.ai/packages/acme/contract-test/complete-result/v1.json",
  "x-runx-packet-id": "acme/contract-test.complete_result.v1",
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
    request.artifacts = Some(OutputArtifacts {
        result_as: Some("complete_result".to_owned()),
        packets: BTreeMap::from([(
            "complete_result".to_owned(),
            packet_id("acme/contract-test.complete_result.v1")?,
        )]),
    });

    let resolved =
        build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())?;
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
    request.environment = EnvironmentRequirements {
        required: vec!["REGION".to_owned()],
        optional: vec!["TRACE_LABEL".to_owned()],
    };
    request.env = BTreeMap::from([
        ("REGION".to_owned(), "ap-southeast-2".to_owned()),
        ("TRACE_LABEL".to_owned(), "private-value".to_owned()),
        ("UNDECLARED".to_owned(), "never-visible".to_owned()),
    ]);

    let resolved =
        build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())?;
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
fn agent_step_instructions_are_the_manual_with_only_its_own_task()
-> Result<(), Box<dyn std::error::Error>> {
    let skill_rule = "Keep domain policy in the skill.";
    let skill = temp_skill(&format!(
        "{skill_rule}\n\n## Agent task: publish\n\nPublish what the plan approved."
    ))?;
    let manual = fs::read_to_string(skill.path().join("SKILL.md"))?.replace(
        "## Agent task: publish\n\nPublish what the plan approved.\n",
        "",
    );
    assert!(manual.contains(skill_rule) && manual.contains("## Agent task: plan"));
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
        build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())?;

    assert_eq!(resolved.envelope.instructions.as_ref(), manual);
    assert_eq!(
        resolved.envelope.instructions_digest.as_ref(),
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
fn an_agent_step_refuses_a_manual_with_no_section_for_its_task()
-> Result<(), Box<dyn std::error::Error>> {
    let skill = temp_skill("Plan carefully.")?;
    fs::write(
        skill.path().join("SKILL.md"),
        "---\nname: contract-test\ndescription: Contract test\n---\n\n# Contract test\n\n## Agent task: review\n\nReview.\n",
    )?;
    let request = invocation(skill.path().to_path_buf(), Some(outputs()));
    let error = build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())
        .err()
        .ok_or("a turn was given a manual with no section for its task")?;
    assert!(
        error
            .to_string()
            .contains("agent task plan has no \"## Agent task: plan\" section"),
        "{error}"
    );
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

#[test]
fn an_act_carries_the_projects_conventions_and_never_its_memory()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let root = fs::canonicalize(workspace.path())?;
    fs::write(root.join("MEMORY.md"), "Remember the launch date.\n")?;
    fs::write(root.join("notes.md"), "A memory named for runx.\n")?;
    fs::write(root.join("CONVENTIONS.md"), "Prefer typed boundaries.\n")?;
    let skill = temp_skill("Plan from the project's conventions.")?;
    let mut request = invocation(skill.path().to_path_buf(), Some(outputs()));
    request.env = BTreeMap::from([
        ("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned()),
        // A memory profile once named this way is read no more.
        ("RUNX_MEMORY_PROFILE_PATH".to_owned(), "notes.md".to_owned()),
    ]);

    let invocation =
        build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())?;
    let profiles = invocation
        .envelope
        .context
        .ok_or("the act carries its conventions")?;
    assert_eq!(profiles.memory, None, "memory is its host's");
    assert_eq!(
        profiles.conventions.map(|conventions| conventions.content),
        Some("Prefer typed boundaries.\n".to_owned())
    );
    Ok(())
}

#[test]
fn the_act_carries_the_projects_instructions_from_the_repository_root_down()
-> Result<(), Box<dyn std::error::Error>> {
    let repository = tempfile::tempdir()?;
    let root = fs::canonicalize(repository.path())?;
    fs::create_dir_all(root.join(".git"))?;
    let workspace = root.join("apps/site");
    fs::create_dir_all(&workspace)?;
    fs::write(root.join("AGENTS.md"), "Root rules.\n")?;
    fs::write(root.join("apps/CLAUDE.md"), "App rules.\n")?;
    fs::write(workspace.join("AGENTS.md"), "Site rules.\n")?;
    // A link within its own directory is read as its target.
    #[cfg(unix)]
    std::os::unix::fs::symlink("AGENTS.md", workspace.join("CLAUDE.md"))?;
    let skill = temp_skill("Plan from the project's rules.")?;
    let mut request = invocation(skill.path().to_path_buf(), Some(outputs()));
    request.env = BTreeMap::from([(
        "RUNX_CWD".to_owned(),
        workspace.to_string_lossy().into_owned(),
    )]);

    let invocation =
        build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())?;
    let instructions = invocation
        .envelope
        .context
        .map(|profiles| profiles.instructions)
        .unwrap_or_default();
    let read = instructions
        .iter()
        .map(|profile| (profile.path.as_str().to_owned(), profile.content.clone()))
        .collect::<Vec<_>>();
    let mut expected = vec![
        ("AGENTS.md".to_owned(), "Root rules.\n".to_owned()),
        ("apps/CLAUDE.md".to_owned(), "App rules.\n".to_owned()),
        ("apps/site/AGENTS.md".to_owned(), "Site rules.\n".to_owned()),
    ];
    if cfg!(unix) {
        expected.push(("apps/site/CLAUDE.md".to_owned(), "Site rules.\n".to_owned()));
    }
    assert_eq!(read, expected);
    assert!(
        instructions
            .iter()
            .all(|profile| profile.root_path.as_str() == root.to_string_lossy()),
        "each path is relative to the repository root"
    );

    // A link leaving its directory is refused.
    #[cfg(unix)]
    {
        fs::remove_file(workspace.join("CLAUDE.md"))?;
        std::os::unix::fs::symlink("../CLAUDE.md", workspace.join("CLAUDE.md"))?;
        let refused =
            build_turn_invocation(&request, &package_of(&request)?, None, "agent".to_owned())
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default();
        assert!(refused.contains("links outside its directory"), "{refused}");
    }
    Ok(())
}
