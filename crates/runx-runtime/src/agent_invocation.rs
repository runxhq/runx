use std::collections::BTreeMap;
use std::path::Path;

use crate::RuntimeError;
use crate::SkillInvocation;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    AgentActInvocation, AgentContextEnvelope, AgentExecutionRequirements, ExecutionLocation,
    JsonObject, OutputField, ResolutionRequest,
};

const TRUST_BOUNDARY: &str = "runtime-governed: caller-mediated resolution is the default; an in-process model loop runs only with explicit per-run managed-agent consent, and every resolution is receipt-bound";

mod profiles;

/// The act `request` asks, at `address`: the address of the step that asks
/// it, as an approval is the address of its gate.
pub(crate) fn agent_act_resolution_request(
    request: &SkillInvocation,
    address: String,
) -> Result<ResolutionRequest, RuntimeError> {
    let invocation = build_agent_act_invocation(request, address)?;
    Ok(ResolutionRequest::AgentAct {
        id: invocation.id.clone(),
        invocation: Box::new(invocation),
    })
}

fn agent_task(request: &SkillInvocation) -> Result<&runx_parser::AgentTask, RuntimeError> {
    request
        .executable
        .agent()
        .ok_or_else(|| invalid_agent_invocation(request, "the invocation is not an agent act"))
}

pub(crate) fn build_agent_act_invocation(
    request: &SkillInvocation,
    id: String,
) -> Result<AgentActInvocation, RuntimeError> {
    let agent = agent_task(request)?;
    let non_empty = |value: &str, field: &str| {
        NonEmptyString::new(value.to_owned())
            .ok_or_else(|| invalid_agent_invocation(request, format!("agent {field} is empty")))
    };
    Ok(AgentActInvocation {
        id: id.into(),
        role: non_empty(&agent.role, "role")?,
        task: non_empty(&agent.task, "task")?,
        envelope: envelope(request)?,
        allowance: agent.allowance.clone(),
    })
}

fn envelope(request: &SkillInvocation) -> Result<AgentContextEnvelope, RuntimeError> {
    crate::execution_environment::resolve_declared_environment(
        &request.requirements,
        &request.env,
        &request.skill_directory,
    )?;
    let manual = load_skill_instructions(&request.skill_directory)?;
    let raw_output = request
        .outputs
        .as_ref()
        .filter(|fields| !fields.is_empty())
        .ok_or_else(|| {
            invalid_agent_invocation(request, "an agent step must declare at least one output")
        })?;
    let output = output_schema_fields(raw_output)?;
    let output_schema = crate::packet_schemas::agent_output_contract_schema(
        &output,
        request.artifacts.as_ref(),
        &request.skill_directory,
        &request.env,
    )?;
    Ok(AgentContextEnvelope {
        run_id: request
            .env
            .get(crate::execution::runner::RUNX_RUN_ID_ENV)
            .and_then(|run_id| NonEmptyString::new(run_id.clone()))
            .unwrap_or_else(|| "rx_pending".into()),
        step_id: optional_non_empty(request.step_id.as_deref()),
        skill: skill_name(request).into(),
        instructions_sha256: manual.digest.into(),
        instructions: envelope_instructions(request, &manual.markdown)?.into(),
        inputs: request.inputs.clone(),
        requirements: AgentExecutionRequirements {
            declaration: request.requirements.clone(),
            environment: crate::execution_environment::environment_requirement_statuses(
                &request.requirements.environment,
                &request.env,
            ),
            execution_boundary: runx_contracts::ExecutionBoundaryObservation {
                kind: runx_contracts::ExecutionBoundaryKind::RemoteProvider,
            },
        },
        current_context: request.current_context.clone(),
        historical_context: request.historical_context.clone(),
        provenance: request.provenance.clone(),
        context: profiles::resolve_context_profiles(request)?,
        voice_profile: Some(profiles::resolve_voice_profile(request)?),
        execution_location: Some(execution_location(&request.skill_directory, &request.env)),
        output: Some(output),
        output_schema: Some(output_schema),
        trust_boundary: TRUST_BOUNDARY.into(),
    })
}

fn envelope_instructions(
    request: &SkillInvocation,
    skill_instructions: &str,
) -> Result<String, RuntimeError> {
    if skill_instructions.trim().is_empty() {
        return Err(invalid_agent_invocation(
            request,
            "an agent step requires operating instructions in SKILL.md",
        ));
    }
    Ok(skill_instructions.to_owned())
}

struct SkillManualInstructions {
    markdown: String,
    digest: String,
}

fn load_skill_instructions(
    skill_directory: &Path,
) -> Result<SkillManualInstructions, RuntimeError> {
    let loaded = crate::load_validated_skill_package(skill_directory)?;
    Ok(SkillManualInstructions {
        markdown: loaded.package.manual_markdown,
        digest: loaded.package.manual_digest,
    })
}

fn invalid_agent_invocation(request: &SkillInvocation, message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: if request.skill_name.is_empty() {
            "agent".to_owned()
        } else {
            request.skill_name.clone()
        },
        message: message.into(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;

fn optional_non_empty(value: Option<&str>) -> Option<NonEmptyString> {
    value.and_then(NonEmptyString::new)
}

fn output_schema_fields(raw: &JsonObject) -> Result<BTreeMap<String, OutputField>, RuntimeError> {
    runx_contracts::parse_output_contract(raw).map_err(|source| RuntimeError::ReceiptInvalid {
        message: format!("agent output contract is invalid: {source}"),
    })
}

fn execution_location(skill_directory: &Path, env: &BTreeMap<String, String>) -> ExecutionLocation {
    let tool_roots = parse_configured_tool_roots(env);
    ExecutionLocation {
        skill_directory: skill_directory.to_string_lossy().into_owned().into(),
        tool_roots: if tool_roots.is_empty() {
            None
        } else {
            Some(tool_roots.into_iter().map(Into::into).collect())
        },
    }
}

fn parse_configured_tool_roots(env: &BTreeMap<String, String>) -> Vec<String> {
    let Some(value) = env.get("RUNX_TOOL_ROOTS") else {
        return Vec::new();
    };
    std::env::split_paths(value)
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

fn skill_name(request: &SkillInvocation) -> String {
    if request.skill_name.is_empty() {
        return "skill".to_owned();
    }
    request.skill_name.clone()
}
