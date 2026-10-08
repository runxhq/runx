//! Native verification of resolved agent results.
//!
//! The model sees the schema emitted by `runx-contracts`; this module enforces
//! the same declaration before a result can become a successful runtime output.
//! Successful evidence is carried as structured output metadata for the generic
//! receipt sealer to bind into the signed act and seal.

use runx_contracts::{JsonObject, JsonValue, ResolutionRequest};
use runx_parser::SkillArtifactContract;
use std::collections::BTreeMap;
use std::path::Path;

use crate::RuntimeError;
use crate::agent_invocation::agent_profile_metadata;
use crate::output_contract::verified_output_metadata_with_artifacts;

#[cfg(test)]
pub(crate) fn verified_agent_metadata(
    request: &ResolutionRequest,
    payload: &JsonValue,
) -> Result<JsonObject, RuntimeError> {
    verified_agent_metadata_with_artifacts(request, payload, None, Path::new("."), &BTreeMap::new())
}

pub(crate) fn verified_agent_metadata_with_artifacts(
    request: &ResolutionRequest,
    payload: &JsonValue,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
    env: &BTreeMap<String, String>,
) -> Result<JsonObject, RuntimeError> {
    let ResolutionRequest::AgentAct { invocation, .. } = request else {
        return Err(RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: "agent result verification requires an agent-act request".to_owned(),
        });
    };
    let contract_payload = agent_output_contract_payload(payload);
    let open_output = BTreeMap::new();
    let output = invocation.envelope.output.as_ref().unwrap_or(&open_output);
    let mut metadata = verified_output_metadata_with_artifacts(
        invocation.envelope.skill.as_ref(),
        &contract_payload,
        Some(output),
        artifacts,
        skill_directory,
        env,
    )?;
    let verification = match metadata.get_mut(crate::adapter::CONTRACT_VERIFICATION_METADATA) {
        Some(JsonValue::Object(verification)) => verification,
        _ => {
            return Err(RuntimeError::ReceiptInvalid {
                message: "agent output verification produced no contract metadata".to_owned(),
            });
        }
    };
    if let Some(profile) = &invocation.envelope.voice_profile {
        verification.insert(
            "voice_profile_sha256".to_owned(),
            JsonValue::String(profile.sha256.as_ref().to_owned()),
        );
    }
    metadata.extend(agent_profile_metadata(request));
    Ok(metadata)
}

pub(crate) fn agent_output_contract_payload(payload: &JsonValue) -> JsonValue {
    let JsonValue::Object(fields) = payload else {
        return payload.clone();
    };
    let mut declared = fields.clone();
    // `closure` is Runx control metadata consumed by the receipt disposition
    // parser. It is validated by that protocol and is not a skill output.
    declared.remove("closure");
    JsonValue::Object(declared)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use runx_contracts::{
        AgentActInvocation, AgentContextEnvelope, JsonValue, OutputField, OutputType,
        ResolutionRequest,
    };

    use super::verified_agent_metadata;
    use crate::adapter::CONTRACT_VERIFICATION_METADATA;

    fn request() -> Result<ResolutionRequest, serde_json::Error> {
        let envelope: AgentContextEnvelope = serde_json::from_value(serde_json::json!({
            "run_id": "run_1",
            "skill": "slack-notify",
            "instructions": "Plan a notification",
            "instructions_sha256": "sha256:d264e5fb6c699b5793b06863a0c2d1e77beb6f01e8a7263da65a3986c3836c26",
            "inputs": {},
            "requirements": {
                "declaration": {},
                "execution_boundary": { "kind": "remote_provider" }
            },
            "current_context": [],
            "historical_context": [],
            "provenance": [],
            "output": { "notify_plan": "object" },
            "trust_boundary": "test"
        }))?;
        Ok(ResolutionRequest::AgentAct {
            id: "req_1".into(),
            invocation: Box::new(AgentActInvocation {
                id: "act_1".into(),
                role: "planner".into(),
                task: "notify".into(),
                envelope,
                allowance: Default::default(),
            }),
        })
    }

    #[test]
    fn verified_metadata_records_contract_digest() -> Result<(), Box<dyn std::error::Error>> {
        let answer = JsonValue::Object(
            [("notify_plan".to_owned(), JsonValue::Object(BTreeMap::new()))]
                .into_iter()
                .collect(),
        );

        let metadata = verified_agent_metadata(&request()?, &answer)?;

        assert!(metadata.contains_key(CONTRACT_VERIFICATION_METADATA));
        Ok(())
    }

    #[test]
    fn undeclared_agent_output_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let answer = JsonValue::Object(
            [
                ("notify_plan".to_owned(), JsonValue::Object(BTreeMap::new())),
                (
                    "message".to_owned(),
                    JsonValue::String("private".to_owned()),
                ),
            ]
            .into_iter()
            .collect(),
        );

        assert!(verified_agent_metadata(&request()?, &answer).is_err());
        Ok(())
    }

    #[test]
    fn transport_output_envelope_is_not_guessed_as_the_declared_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let answer = JsonValue::Object(
            [(
                "output".to_owned(),
                JsonValue::Object(
                    [("notify_plan".to_owned(), JsonValue::Object(BTreeMap::new()))]
                        .into_iter()
                        .collect(),
                ),
            )]
            .into_iter()
            .collect(),
        );

        assert!(verified_agent_metadata(&request()?, &answer).is_err());
        Ok(())
    }

    #[test]
    fn runx_closure_metadata_is_not_treated_as_a_declared_skill_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let answer = JsonValue::Object(
            [
                ("notify_plan".to_owned(), JsonValue::Object(BTreeMap::new())),
                (
                    "closure".to_owned(),
                    JsonValue::Object(
                        [
                            (
                                "disposition".to_owned(),
                                JsonValue::String("closed".to_owned()),
                            ),
                            (
                                "reason_code".to_owned(),
                                JsonValue::String("completed".to_owned()),
                            ),
                            (
                                "summary".to_owned(),
                                JsonValue::String("Completed".to_owned()),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        );

        assert!(verified_agent_metadata(&request()?, &answer).is_ok());
        Ok(())
    }

    #[test]
    fn fixture_contract_is_the_declared_object_field() -> Result<(), Box<dyn std::error::Error>> {
        let ResolutionRequest::AgentAct { invocation, .. } = request()? else {
            return Err("expected agent-act request".into());
        };
        assert_eq!(
            invocation.envelope.output,
            Some(
                [(
                    "notify_plan".to_owned(),
                    OutputField::Type(OutputType::Object)
                )]
                .into_iter()
                .collect()
            )
        );
        Ok(())
    }
}
