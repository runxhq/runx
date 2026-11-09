use std::collections::BTreeMap;
use std::path::Path;

use crate::RuntimeError;
use crate::SkillInvocation;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    AgentContextEnvelope, AgentExecutionRequirements, ExecutionLocation, JsonObject, OutputField,
    ResolutionRequest, TurnInvocation,
};

const TRUST_BOUNDARY: &str = "runtime-governed: caller-mediated resolution is the default; an in-process model loop runs only with explicit per-run managed-agent consent, and every resolution is receipt-bound";

mod profiles;

/// The act `request` asks, at `address`: the address of the step that asks
/// it, as an approval is the address of its gate.
pub(crate) fn turn_resolution_request(
    request: &SkillInvocation,
    package: &crate::LoadedSkillPackage,
    act: Option<&runx_contracts::CallAct>,
    address: String,
) -> Result<ResolutionRequest, RuntimeError> {
    let invocation = build_turn_invocation(request, package, act, address)?;
    Ok(ResolutionRequest::Turn {
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

/// The act `request` asks, under the instructions of `package`, the skill
/// package its run admitted, for the run whose invoker stated `act`.
pub(crate) fn build_turn_invocation(
    request: &SkillInvocation,
    package: &crate::LoadedSkillPackage,
    act: Option<&runx_contracts::CallAct>,
    id: String,
) -> Result<TurnInvocation, RuntimeError> {
    let agent = agent_task(request)?;
    let non_empty = |value: &str, field: &str| {
        NonEmptyString::new(value.to_owned())
            .ok_or_else(|| invalid_agent_invocation(request, format!("agent {field} is empty")))
    };
    Ok(TurnInvocation {
        id: id.into(),
        task: non_empty(&agent.task, "task")?,
        envelope: envelope(request, package, act)?,
        allowance: agent.allowance.clone(),
        requires: agent.requires.clone(),
        context_kinds: agent
            .context_kinds
            .iter()
            .map(|kind| non_empty(kind, "context kind"))
            .collect::<Result<_, _>>()?,
    })
}

fn envelope(
    request: &SkillInvocation,
    package: &crate::LoadedSkillPackage,
    act: Option<&runx_contracts::CallAct>,
) -> Result<AgentContextEnvelope, RuntimeError> {
    crate::execution_environment::resolve_declared_environment(
        &request.requirements,
        &request.env,
        &request.skill_directory,
    )?;
    let raw_output = request
        .outputs
        .as_ref()
        .filter(|fields| !fields.is_empty())
        .ok_or_else(|| {
            invalid_agent_invocation(request, "an agent step must declare at least one output")
        })?;
    // The answer holds the step's outputs as its fields, but for the one
    // `result_as` names: that one is the whole answer.
    let output = crate::output_contract::result_fields(
        output_schema_fields(raw_output)?,
        request.artifacts.as_ref(),
    );
    let output_schema = crate::packet_schemas::agent_output_contract_schema(
        &output,
        request.artifacts.as_ref(),
        &request.skill_directory,
    )?;
    let instructions = task_instructions(request, package)?;
    Ok(AgentContextEnvelope {
        run_id: request
            .env
            .get(crate::execution::runner::RUNX_RUN_ID_ENV)
            .and_then(|run_id| NonEmptyString::new(run_id.clone()))
            .unwrap_or_else(|| "rx_pending".into()),
        step_id: optional_non_empty(request.step_id.as_deref()),
        skill: skill_name(request).into(),
        act: act.cloned(),
        instructions_digest: runx_contracts::digest::sha256_prefixed(instructions.as_bytes())
            .into(),
        instructions: instructions.into(),
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

/// The instructions a turn is given: its skill's manual with only its own
/// task's section, every other task's left out. Another skill's
/// instructions reach a turn only through its `context.skills`.
fn task_instructions(
    request: &SkillInvocation,
    package: &crate::LoadedSkillPackage,
) -> Result<String, RuntimeError> {
    let task = agent_task(request)?.task.as_str();
    runx_parser::agent_task_manual(&package.package.manual_markdown, task)
        .map_err(|message| invalid_agent_invocation(request, message))?
        .ok_or_else(|| {
            invalid_agent_invocation(
                request,
                format!("agent task {task} has no \"## Agent task: {task}\" section in SKILL.md, which carries its instructions"),
            )
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
