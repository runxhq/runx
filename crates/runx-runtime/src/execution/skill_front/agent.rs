use super::{
    SkillExecutionContext, SkillOutputDiagnostics, SkillRunError, SkillSealContext,
    agent_invocation_source_type, agent_request, answer_disposition, contract_json_value,
    domain_act_frame, generated_run_id, identifier_segment, invalid, needs_agent_output,
    read_answer, sealed_output,
};

use runx_contracts::{ClosureDisposition, JsonObject, JsonValue};

use crate::RuntimeError;
use crate::adapter::{InvocationOutput, SkillInvocation};
#[cfg(feature = "agent")]
use crate::adapters::agent::AgentAdapterSourceType;
use crate::agent_contract::{
    agent_output_contract_payload, verified_agent_metadata_with_artifacts,
};
use crate::agent_invocation::agent_act_invocation_id;
#[cfg(feature = "agent")]
use crate::config::ManagedAgentConfig;
use crate::effects::RuntimeEffectRegistry;
use crate::execution::orchestrator::{RunResult, SkillRunRequest};
use crate::journal::{PausedRunCheckpoint, append_paused_run_checkpoint};
use crate::receipts::StepSealClosure;
use crate::receipts::{DomainActReceiptRequest, domain_act_receipt};
use runx_parser::{SkillRunnerDefinition, SkillRunnerManifest};

use super::runner_manifest::write_skill_receipt;

// Function rationale: one agent-front transaction resolves the
// answer source, seals either a domain act or generic answer, and emits the
// public skill output envelope.
pub(super) fn execute_agent_skill_run(
    context: &SkillExecutionContext<'_>,
    mut invocation: SkillInvocation,
) -> Result<RunResult, SkillRunError> {
    let SkillExecutionContext {
        request,
        overrides,
        effects,
        workspace,
        receipts,
        manifest,
        runner,
        package_digest,
        execution_closure_digest,
        ..
    } = *context;
    let source_type = agent_invocation_source_type(runner.source.source_type.as_str())?;
    let request_id = agent_act_invocation_id(&invocation, source_type);
    let run_id = agent_run_id(
        request,
        manifest,
        runner,
        &request_id,
        package_digest,
        execution_closure_digest,
    )?;
    invocation.env.insert(
        crate::execution::runner::RUNX_RUN_ID_ENV.to_owned(),
        run_id.clone(),
    );
    let resolution_request = agent_request(&invocation, source_type)?;
    let resolution_request_value = contract_json_value(&resolution_request)?;

    // Seeded answers (inline, single pass) take priority over the file-based
    // resume channel; absent both, the run yields to the public agent loop.
    let seeded_answer = overrides
        .seeded_answers
        .as_ref()
        .and_then(|answers| answers.get(&request_id).cloned());
    let (answer, governed_effect): (JsonValue, Option<JsonValue>) = match seeded_answer {
        Some(answer) => (answer, None),
        None => match &request.answers_path {
            Some(answers_path) => (
                read_answer(answers_path, &request_id, &resolution_request_value)?,
                None,
            ),
            None => {
                match try_inline_agent_resolution(&invocation, &request.managed_agent, effects)? {
                    #[cfg(feature = "agent")]
                    InlineAgentOutcome::Resolved { payload, effect } => (payload, effect),
                    #[cfg(feature = "agent")]
                    InlineAgentOutcome::Failed(error) => {
                        return seal_agent_failure(context, &run_id, AgentFailure::managed(&error));
                    }
                    InlineAgentOutcome::HostDrives => {
                        super::state_store::write_agent_state(
                            context,
                            &run_id,
                            super::PendingSkillRequest {
                                id: request_id.clone(),
                                value: resolution_request_value.clone(),
                            },
                        )?;
                        write_paused_agent_checkpoint(context, &run_id, &request_id)?;
                        return Ok(needs_agent_output(
                            context.manifest,
                            &context.runner.name,
                            &run_id,
                            &request_id,
                            resolution_request_value.clone(),
                        ));
                    }
                }
            }
        },
    };
    let verification_metadata = verified_agent_metadata_with_artifacts(
        &resolution_request,
        &answer,
        runner.artifacts.as_ref(),
        &invocation.skill_directory,
        workspace.env(),
    )?;
    let claim_payload = match &resolution_request {
        runx_contracts::ResolutionRequest::AgentAct { .. } => {
            agent_output_contract_payload(&answer)
        }
        _ => {
            return seal_agent_failure(
                context,
                &run_id,
                AgentFailure::contract(
                    "agent_resolution_type_invalid",
                    "agent execution resolved an incompatible request",
                    &SkillRunError::Runtime(RuntimeError::ReceiptInvalid {
                        message: "agent execution resolved a non-agent request".to_owned(),
                    }),
                ),
            );
        }
    };
    let disposition = answer_disposition(&answer)?;
    let receipt = match domain_act_frame(&invocation, &answer, governed_effect.as_ref()) {
        Some(mut frame) => {
            frame.artifact_refs.extend(
                crate::execution::prepared_skill::prepared_receipt_references(workspace.env()),
            );
            let label = disposition.label();
            let created_at = crate::time::now_iso8601();
            let graph_name = identifier_segment(&run_id);
            let step_id = identifier_segment(&runner.name);
            domain_act_receipt(DomainActReceiptRequest {
                graph_name: &graph_name,
                step_id: &step_id,
                succeeded: disposition == ClosureDisposition::Closed,
                created_at: &created_at,
                disposition,
                reason_code: format!("agent_act_{label}"),
                seal_summary: format!("agent act sealed ({label})"),
                frame,
                verification_metadata: verification_metadata.clone(),
                signature_policy: receipts.signature_config().signature_policy(),
            })?
        }
        None => SkillSealContext::from_services(&run_id, runner, receipts, workspace).seal_answer(
            &answer,
            &claim_payload,
            disposition,
            verification_metadata.clone(),
        )?,
    };
    write_skill_receipt(request, workspace, receipts, &receipt)?;

    Ok(sealed_output(
        manifest,
        &runner.name,
        &run_id,
        &agent_skill_output(answer.clone(), &receipt, verification_metadata),
        &answer,
        SkillOutputDiagnostics::default(),
        &receipt,
    ))
}

fn write_paused_agent_checkpoint(
    context: &SkillExecutionContext<'_>,
    run_id: &str,
    request_id: &str,
) -> Result<(), SkillRunError> {
    let receipt_path = context.receipts.resolve_path(
        context.workspace,
        context.request.receipt_dir.as_deref(),
        None,
    );
    let checkpoint = PausedRunCheckpoint {
        id: run_id.to_owned(),
        name: context
            .manifest
            .skill
            .clone()
            .unwrap_or_else(|| context.runner.name.clone()),
        kind: "agent".to_owned(),
        started_at: Some(crate::time::now_iso8601()),
        resume_skill_ref: Some(context.request.skill_path.to_string_lossy().into_owned()),
        selected_runner: Some(context.runner.name.clone()),
        credential_profile: context
            .request
            .local_credential
            .as_ref()
            .and_then(|credential| credential.profile.clone()),
        package_digest: Some(context.package_digest.to_owned()),
        execution_closure_digest: context.execution_closure_digest.map(str::to_owned),
        step_ids: vec![request_id.to_owned()],
        step_labels: vec![context.runner.name.clone()],
    };
    append_paused_run_checkpoint(&receipt_path.path, &checkpoint).map_err(|source| {
        RuntimeError::io(
            format!(
                "writing paused run checkpoint for {} in {}",
                checkpoint.id,
                receipt_path.path.display()
            ),
            source,
        )
    })?;
    Ok(())
}

/// Outcome of attempting the optional in-process managed-agent loop.
enum InlineAgentOutcome {
    /// The in-kernel loop ran and produced the agent answer payload, plus the last
    /// successful governed tool result (the real effect) for the domain receipt.
    #[cfg(feature = "agent")]
    Resolved {
        payload: JsonValue,
        effect: Option<JsonValue>,
    },
    /// The explicitly opted-in loop ran but failed within a bounded native
    /// boundary. The caller seals this outcome so local history cannot lose it.
    #[cfg(feature = "agent")]
    Failed(crate::adapters::agent::AgentResolverError),
    /// No in-process provider is configured; yield to the host loop.
    HostDrives,
}

/// Optionally run the managed-agent loop in-process. This requires explicit
/// per-run consent as well as a configured provider; credentials alone never
/// activate it. Otherwise the runtime yields to the host (`needs_agent`).
#[cfg(feature = "agent")]
fn try_inline_agent_resolution(
    invocation: &SkillInvocation,
    policy: &crate::execution::orchestrator::ManagedAgentPolicy,
    effects: &RuntimeEffectRegistry,
) -> Result<InlineAgentOutcome, SkillRunError> {
    use crate::adapters::agent::{AgentResolver, build_managed_agent_act_invocation};
    use crate::adapters::agent_resolver::{ManagedAgentResolver, ManagedAgentResolverOptions};
    use runx_contracts::ResolutionRequest;

    let Some((max_rounds, source_type, config)) = managed_agent_attempt(invocation, policy)? else {
        return Ok(InlineAgentOutcome::HostDrives);
    };

    let agent_act = build_managed_agent_act_invocation(invocation, source_type)?;
    let request = ResolutionRequest::AgentAct {
        id: agent_act.id.clone(),
        invocation: Box::new(agent_act),
    };
    let resolver = ManagedAgentResolver::new(
        config,
        ManagedAgentResolverOptions {
            env: invocation.env.clone(),
            skill_directory: invocation.skill_directory.clone(),
            credential_delivery: invocation.credential_delivery.clone(),
            effects: effects.clone(),
            observed_at: crate::time::now_iso8601(),
            max_rounds,
        },
    );
    let resolution = match resolver.resolve(request) {
        Ok(resolution) => resolution,
        Err(error) => return Ok(InlineAgentOutcome::Failed(error)),
    };
    Ok(InlineAgentOutcome::Resolved {
        payload: resolution.response.payload,
        effect: resolution.governed_effect,
    })
}

#[cfg(feature = "agent")]
fn managed_agent_attempt(
    invocation: &SkillInvocation,
    policy: &crate::execution::orchestrator::ManagedAgentPolicy,
) -> Result<Option<(u32, AgentAdapterSourceType, ManagedAgentConfig)>, SkillRunError> {
    let Some(max_rounds) = policy.max_rounds() else {
        return Ok(None);
    };
    let source_type = match invocation.source.source_type {
        runx_parser::SourceKind::Agent => AgentAdapterSourceType::Agent,
        runx_parser::SourceKind::AgentStep => AgentAdapterSourceType::AgentStep,
        _ => return Ok(None),
    };
    let config =
        crate::config::load_managed_agent_config(&invocation.env, &invocation.skill_directory)
            .map_err(|error| {
                SkillRunError::Invalid(format!("managed agent config error: {error}"))
            })?;
    Ok(config.map(|config| (max_rounds, source_type, config)))
}

struct AgentFailure {
    reason_code: String,
    summary: String,
    message: String,
    payload: JsonValue,
    metadata: JsonObject,
}

impl AgentFailure {
    fn contract(reason_code: &str, summary: &str, error: &impl std::fmt::Display) -> Self {
        let message = error.to_string();
        Self {
            reason_code: reason_code.to_owned(),
            summary: summary.to_owned(),
            message: message.clone(),
            payload: JsonValue::Object(JsonObject::from([
                (
                    "reason_code".to_owned(),
                    JsonValue::String(reason_code.to_owned()),
                ),
                ("message".to_owned(), JsonValue::String(message)),
            ])),
            metadata: JsonObject::new(),
        }
    }

    #[cfg(feature = "agent")]
    fn managed(error: &crate::adapters::agent::AgentResolverError) -> Self {
        let reason = error.reason_code();
        Self {
            reason_code: format!("managed_agent_{reason}"),
            summary: format!("managed agent failed ({reason})"),
            message: error.sanitized_message().to_owned(),
            payload: JsonValue::Object(error.public_failure_projection()),
            metadata: error.receipt_metadata(),
        }
    }
}

fn seal_agent_failure(
    context: &SkillExecutionContext<'_>,
    run_id: &str,
    failure: AgentFailure,
) -> Result<RunResult, SkillRunError> {
    let output = InvocationOutput::runtime_failure(
        failure.payload.clone(),
        &failure.message,
        0,
        failure.metadata.clone(),
    );
    let receipt = SkillSealContext::from_services(
        run_id,
        context.runner,
        context.receipts,
        context.workspace,
    )
    .seal_output(
        &output,
        None,
        StepSealClosure {
            disposition: ClosureDisposition::Failed,
            reason_code: failure.reason_code,
            summary: failure.summary,
        },
        Some(failure.metadata),
    )?;
    write_skill_receipt(
        context.request,
        context.workspace,
        context.receipts,
        &receipt,
    )?;
    Ok(sealed_output(
        context.manifest,
        &context.runner.name,
        run_id,
        &output,
        &failure.payload,
        SkillOutputDiagnostics::default(),
        &receipt,
    ))
}

#[cfg(not(feature = "agent"))]
fn try_inline_agent_resolution(
    _invocation: &SkillInvocation,
    _policy: &crate::execution::orchestrator::ManagedAgentPolicy,
    _effects: &RuntimeEffectRegistry,
) -> Result<InlineAgentOutcome, SkillRunError> {
    Ok(InlineAgentOutcome::HostDrives)
}

fn agent_run_id(
    request: &SkillRunRequest,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    request_id: &str,
    package_digest: &str,
    execution_closure_digest: Option<&str>,
) -> Result<String, SkillRunError> {
    match (&request.run_id, &request.answers_path) {
        (Some(run_id), _) => Ok(run_id.clone()),
        (None, Some(_)) => Err(invalid(
            "skill continuation requires both run_id and answers",
        )),
        (None, None) => generated_run_id(
            request_id,
            manifest,
            runner,
            Some(request_id),
            &request.inputs,
            package_digest,
            execution_closure_digest,
        ),
    }
}

fn agent_skill_output(
    answer: JsonValue,
    receipt: &runx_contracts::Receipt,
    verification_metadata: JsonObject,
) -> InvocationOutput {
    let succeeded = receipt.seal.disposition == ClosureDisposition::Closed;
    if succeeded {
        InvocationOutput::runtime_success(answer, 0, verification_metadata)
    } else {
        InvocationOutput::runtime_failure(
            answer,
            format!("agent act closed with {}", receipt.seal.disposition.label()),
            0,
            verification_metadata,
        )
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "agent")]
    mod managed_agent {

        use super::super::*;
        use crate::LocalReceiptStore;
        use crate::adapters::agent::{
            AgentExecutionTelemetry, AgentResolverError, AgentToolExecutionTrace,
        };
        use crate::journal::{HistoryFilter, list_local_history};
        use crate::services::{ReceiptServices, WorkspaceEnv};
        use runx_parser::{parse_runner_manifest_yaml, validate_runner_manifest};

        #[test]
        fn managed_agent_failure_is_sealed_and_visible_in_local_history()
        -> Result<(), Box<dyn std::error::Error>> {
            let temp = tempfile::tempdir()?;
            let receipt_dir = temp.path().join("receipts");
            let skill_dir = temp.path().join("skill");
            std::fs::create_dir_all(&skill_dir)?;
            let manifest = validate_runner_manifest(parse_runner_manifest_yaml(
                r#"skill: managed-agent-failure-fixture
runners:
  managed-agent-failure:
    default: true
    type: agent
"#,
            )?)?;
            let runner = manifest
                .runners
                .get("managed-agent-failure")
                .ok_or("fixture runner missing")?;
            let request = SkillRunRequest {
                skill_path: skill_dir,
                receipt_dir: Some(receipt_dir.clone()),
                run_id: None,
                answers_path: None,
                inputs: Default::default(),
                env: Default::default(),
                cwd: temp.path().to_path_buf(),
                managed_agent: Default::default(),
                local_credential: None,
            };
            let workspace = WorkspaceEnv::new(Default::default(), temp.path().to_path_buf())?;
            let receipts = ReceiptServices::from_env_or_local_development(workspace.env())?;
            let error = AgentResolverError::bounded_failure(
                "round_budget_exhausted",
                "Managed agent exceeded 3 tool-call rounds without finalizing.",
                AgentExecutionTelemetry {
                    rounds: Some(3),
                    model_calls: Some(3),
                    tool_calls: Some(3),
                    tools: Some(vec!["fs.read".to_owned()]),
                    tool_executions: Some(vec![AgentToolExecutionTrace {
                        tool: "fs.read".to_owned(),
                        status: "success".to_owned(),
                        receipt_id: None,
                        resolution_kind: None,
                    }]),
                },
            );

            let overrides = crate::execution::skill_front::SkillRunOverrides::default();
            let effects = RuntimeEffectRegistry::default();
            let context = SkillExecutionContext {
                loaded: None,
                request: &request,
                overrides: &overrides,
                effects: &effects,
                workspace: &workspace,
                receipts: &receipts,
                manifest: &manifest,
                runner,
                package_digest: "sha256:test",
                execution_closure_digest: None,
            };
            let output = seal_agent_failure(
                &context,
                "run_managed-agent-failure",
                AgentFailure::managed(&error),
            )?;

            let output_json = serde_json::to_string(&output.output)?;
            assert!(output_json.contains("\"status\":\"sealed\""));
            assert!(output_json.contains("\"reason_code\":\"round_budget_exhausted\""));
            assert!(output_json.contains("\"rounds\":3"));
            assert!(output_json.contains("\"model_calls\":3"));
            assert!(!output_json.contains("prompt"));
            assert!(!output_json.contains("credential"));
            let history = list_local_history(
                &LocalReceiptStore::new(&receipt_dir),
                temp.path(),
                &temp.path().join(".runx"),
                &HistoryFilter::default(),
            )?;
            assert_eq!(history.receipts.len(), 1);
            assert_eq!(history.receipts[0].status, "failed");
            assert!(
                history.receipts[0]
                    .summary
                    .contains("round_budget_exhausted")
            );
            assert!(history.pending_runs.is_empty());
            Ok(())
        }
    }
}
