//! Native verification of resolved agent results.
//!
//! The model sees the schema emitted by `runx-contracts`; this module enforces
//! the same declaration before a result can become a successful runtime output.
//! The checks it ran are the runtime's observations, which the receipt sealer
//! binds into the signed act and seal.

use runx_contracts::{JsonValue, ResolutionRequest};
use runx_parser::SkillArtifactContract;
use std::collections::BTreeMap;
use std::path::Path;

use crate::RuntimeError;
use crate::observations::ContractChecks;
use crate::output_contract::output_contract_checks;

#[cfg(test)]
pub(crate) fn agent_contract_checks(
    request: &ResolutionRequest,
    payload: &JsonValue,
) -> Result<ContractChecks, RuntimeError> {
    agent_contract_checks_with_artifacts(request, payload, None, Path::new("."))
}

pub(crate) fn agent_contract_checks_with_artifacts(
    request: &ResolutionRequest,
    payload: &JsonValue,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
) -> Result<ContractChecks, RuntimeError> {
    let ResolutionRequest::AgentAct { invocation, .. } = request else {
        return Err(RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: "agent result verification requires an agent-act request".to_owned(),
        });
    };
    let contract_payload = agent_output_contract_payload(payload);
    let open_output = BTreeMap::new();
    let output = invocation.envelope.output.as_ref().unwrap_or(&open_output);
    let mut checks = output_contract_checks(
        invocation.envelope.skill.as_ref(),
        &contract_payload,
        Some(output),
        artifacts,
        skill_directory,
    )?;
    checks.voice_profile_digest = invocation
        .envelope
        .voice_profile
        .as_ref()
        .map(|profile| profile.digest.as_ref().to_owned());
    Ok(checks)
}

pub(crate) fn agent_output_contract_payload(payload: &JsonValue) -> JsonValue {
    let JsonValue::Object(fields) = payload else {
        return payload.clone();
    };
    let mut declared = fields.clone();
    // `closure` is Runx control metadata consumed by the receipt outcome
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

    use super::agent_contract_checks;

    fn request() -> Result<ResolutionRequest, serde_json::Error> {
        let envelope: AgentContextEnvelope = serde_json::from_value(serde_json::json!({
            "run_id": "run_1",
            "skill": "message-send",
            "instructions": "Plan a notification",
            "instructions_digest": "sha256:d264e5fb6c699b5793b06863a0c2d1e77beb6f01e8a7263da65a3986c3836c26",
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
    fn an_agent_answer_is_checked_against_its_contract_digest()
    -> Result<(), Box<dyn std::error::Error>> {
        let answer = JsonValue::Object(
            [("notify_plan".to_owned(), JsonValue::Object(BTreeMap::new()))]
                .into_iter()
                .collect(),
        );

        let checks = agent_contract_checks(&request()?, &answer)?;

        assert!(
            checks
                .output_contract_digest
                .is_some_and(|digest| digest.starts_with("sha256:"))
        );
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

        assert!(agent_contract_checks(&request()?, &answer).is_err());
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

        assert!(agent_contract_checks(&request()?, &answer).is_err());
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
                                "outcome".to_owned(),
                                JsonValue::String("completed".to_owned()),
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

        assert!(agent_contract_checks(&request()?, &answer).is_ok());
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
