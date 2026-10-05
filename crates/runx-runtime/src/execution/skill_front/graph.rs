// Module rationale: graph skill-front execution keeps nested skill
// resolution, graph state projection, and receipt handoff together until the
// graph runner/front boundary is split.
#[cfg(test)]
use super::contract_json_value;
use super::{
    GRAPH_SKILL_STATE_SCHEMA, PendingSkillRequest, SkillExecutionContext, SkillOutputDiagnostics,
    SkillRunError, SkillSourceAdapter, build_domain_act_frame, generated_run_id,
    identifier_segment, invalid, needs_agent_output, sealed_output,
};

use std::collections::BTreeMap;
use std::path::PathBuf;

use runx_contracts::{
    ClosureDisposition, JsonObject, JsonValue, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor,
};
use runx_core::state_machine::GraphStatus;
use runx_parser::{ExecutionGraph, SkillRunnerDefinition, SkillRunnerManifest};
use runx_receipts::ReceiptTreeConfig;
use serde::{Deserialize, Serialize};

use crate::RuntimeError;
#[cfg(test)]
use crate::adapter::SkillInvocation;
use crate::credentials::CredentialDelivery;
use crate::effects::RuntimeEffectRegistry;
use crate::execution::graph::materialize_graph_parameter_inputs;
use crate::execution::orchestrator::{RunResult, SkillRunRequest};
use crate::execution::runner::{
    GraphCheckpoint, GraphRun, RUNX_RUN_ID_ENV, Runtime, RuntimeOptions, graph_run_context,
    graph_run_result, graph_run_skill_output, graph_run_trace,
};
use crate::host::Host;
use crate::journal::{PausedRunCheckpoint, append_paused_run_checkpoint};
use crate::receipts::{DomainActReceiptRequest, RuntimeReceiptSignatureConfig, domain_act_receipt};
use crate::services::{ReceiptServices, WorkspaceEnv};

use super::resolution_answers::{ResolutionAnswers, read_answers};
use super::runner_manifest::{credential_delivery_from_invocation, write_skill_receipt};
use super::state_store::{read_graph_state, try_read_graph_state, write_graph_state};

// Function rationale: graph-backed skill execution keeps
// checkpoint hydration, host resolution, and final receipt sealing in one path.
pub(super) fn execute_graph_skill_run(
    context: &SkillExecutionContext<'_>,
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
    let graph = runner
        .source
        .graph
        .clone()
        .ok_or_else(|| invalid("graph runner is missing source.graph"))?;
    let request_graph_inputs = request
        .inputs
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<JsonObject>();
    let run_id = graph_run_id(
        request,
        manifest,
        runner,
        package_digest,
        execution_closure_digest,
    )?;
    let execution_closure_digest = execution_closure_digest
        .ok_or_else(|| invalid("graph execution requires an execution-closure digest"))?;
    // Only caller-pinned identities probe local state. Generated-ID graph runs
    // keep their existing hot path; a pinned retry reuses its exact checkpoint.
    let seeded = overrides.seeded_answers.clone();
    let explicit_resume = request.answers_path.is_some() && seeded.is_none();
    let mut resumed_state = if explicit_resume {
        Some(read_graph_state(
            request,
            workspace,
            receipts,
            &run_id,
            &runner.name,
            package_digest,
            execution_closure_digest,
        )?)
    } else if request.run_id.is_some() && seeded.is_none() {
        try_read_graph_state(
            request,
            workspace,
            receipts,
            &run_id,
            &runner.name,
            package_digest,
            execution_closure_digest,
        )?
    } else {
        None
    };
    if let Some(state) = resumed_state.as_ref() {
        let inputs_drifted = if explicit_resume {
            // Resume reconstructs the original inputs from the checkpoint;
            // preparation may still inject a stable repo_root. Check supplied
            // values without requiring the omitted originals a second time.
            request_graph_inputs
                .iter()
                .any(|(key, value)| state.graph_inputs.get(key) != Some(value))
        } else {
            state.graph_inputs != request_graph_inputs
        };
        if inputs_drifted {
            return Err(invalid(
                "graph run identity cannot replace its original inputs",
            ));
        }
        if state.completed_receipt_id.is_none()
            && (state.sealed_graph_receipt.is_some()
                || state.terminal_result.is_some()
                || matches!(
                    state.checkpoint.state.status,
                    GraphStatus::Succeeded | GraphStatus::Failed
                ))
        {
            return Err(invalid(
                "completed graph checkpoint lacks its primary receipt identity",
            ));
        }
        if state.completed_receipt_id.is_some() {
            let completed = resumed_state
                .take()
                .ok_or_else(|| invalid("completed graph state is missing"))?;
            return restore_sealed_graph_skill_run(context, graph, completed, &run_id);
        }
    }
    let skill_dir = crate::skill_package::resolve_skill_package_directory(&request.skill_path)?;
    let mut env = workspace.skill_env_for_skill(&skill_dir);
    env.insert(RUNX_RUN_ID_ENV.to_owned(), run_id.clone());
    let receipt_path = receipts.resolve_path(workspace, request.receipt_dir.as_deref(), None);
    env.insert(
        crate::receipts::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
        receipt_path.path.to_string_lossy().into_owned(),
    );
    let credential_delivery = match &overrides.credential_delivery {
        Some(delivery) => delivery.clone(),
        None => {
            credential_delivery_from_invocation(workspace.env(), request.local_credential.as_ref())?
        }
    };
    let created_at = crate::time::now_iso8601();
    let inline_resolver = InlineResolver {
        skill_directory: skill_dir.clone(),
        env: env.clone(),
        credential_delivery: credential_delivery.clone(),
        effects: effects.clone(),
        observed_at: created_at.clone(),
        policy: request.managed_agent.clone(),
    };
    let options = RuntimeOptions {
        created_at: created_at.clone(),
        env,
        receipt_signature: receipts.signature_config().clone(),
        effects: effects.clone(),
        credential_delivery,
    };
    let runtime = match &overrides.javascript {
        Some(javascript) => Runtime::with_native_services(
            SkillSourceAdapter::with_javascript(javascript.clone()),
            options,
            javascript.clone(),
            crate::services::LocalArtifactService::default(),
        ),
        None => Runtime::new(SkillSourceAdapter::default(), options),
    };
    // Seeded answers run a single fresh pass with the answers pre-loaded into the
    // host (they drive the graph to completion, or block -> needs_agent when a
    // step has no seeded answer). The file-based `answers_path` remains the
    // resume-from-checkpoint channel.
    let incoming_answers = match &seeded {
        Some(seeded) => seeded.clone(),
        None => match &request.answers_path {
            Some(path) => read_answers(path)?,
            None => ResolutionAnswers::default(),
        },
    };
    let mut answers = resumed_state
        .as_ref()
        .map(|state| state.resolution_answers.clone())
        .unwrap_or_default();
    answers.merge(incoming_answers)?;
    let graph_inputs = resumed_state
        .as_ref()
        .map(|state| {
            if state.graph_inputs.is_empty() {
                request_graph_inputs.clone()
            } else {
                state.graph_inputs.clone()
            }
        })
        .unwrap_or_else(|| request_graph_inputs.clone());
    if resumed_state.is_some() {
        crate::input_contract::materialize_complete_runner_inputs(&runner.inputs, &graph_inputs)
            .map_err(|error| error.into_runtime_error())?;
    }
    let graph = materialize_graph_parameter_inputs(graph, &graph_inputs);
    let mut host = SkillRunGraphHost::with_inline(answers.clone(), inline_resolver);
    let mut checkpoint = if let Some(state) = resumed_state.take() {
        state.checkpoint
    } else {
        runtime.run_graph_until_steps_with_host(&skill_dir, &graph, 0, &mut host)?
    };

    loop {
        let previous_checkpoint = checkpoint.clone();
        match runtime
            .resume_graph_until_steps_with_host(&skill_dir, &graph, checkpoint, 1, &mut host)
        {
            Ok(next_checkpoint) => {
                if next_checkpoint.state.status == GraphStatus::Succeeded {
                    let completed_checkpoint = next_checkpoint.clone();
                    let mut final_host = SkillRunGraphHost::new(ResolutionAnswers::default());
                    let run = runtime.seal_completed_graph_checkpoint_with_host(
                        graph.clone(),
                        next_checkpoint,
                        &mut final_host,
                    )?;
                    let result = graph_run_result(&run)?;
                    let public_context = graph_run_context(&run);
                    let trace = graph_run_trace(&run);
                    // A graph that declares an `act:` block seals a clean domain-act
                    // receipt as its primary receipt; the step receipts above remain
                    // as its execution trace.
                    let domain = graph_domain_act_receipt(
                        runner,
                        &graph_inputs,
                        &run,
                        &run_id,
                        &created_at,
                        receipts.signature_config(),
                    )?;
                    let receipt = domain.as_ref().unwrap_or(&run.receipt);
                    write_graph_state(
                        request,
                        workspace,
                        receipts,
                        &run_id,
                        &GraphSkillRunState {
                            schema: GRAPH_SKILL_STATE_SCHEMA.to_owned(),
                            run_id: run_id.clone(),
                            runner_name: runner.name.clone(),
                            package_digest: package_digest.to_owned(),
                            execution_closure_digest: execution_closure_digest.to_owned(),
                            graph_inputs: graph_inputs.clone(),
                            resolution_answers: answers.clone(),
                            pending_request: None,
                            completed_receipt_id: Some(receipt.id.to_string()),
                            completed_graph_receipt_id: Some(run.receipt.id.to_string()),
                            terminal_result: None,
                            sealed_graph_receipt: Some(run.receipt.clone()),
                            sealed_primary_receipt: domain.clone(),
                            checkpoint: completed_checkpoint,
                        },
                    )?;
                    write_graph_receipts(request, workspace, receipts, &run)?;
                    if let Some(domain_receipt) = &domain {
                        write_skill_receipt(request, workspace, receipts, domain_receipt)?;
                    }
                    let output = graph_run_skill_output(&result, &run)?;
                    return Ok(sealed_output(
                        manifest,
                        &runner.name,
                        &run_id,
                        &output,
                        &result,
                        SkillOutputDiagnostics {
                            context: Some(public_context),
                            trace: Some(trace),
                        },
                        receipt,
                    ));
                }
                write_graph_state(
                    request,
                    workspace,
                    receipts,
                    &run_id,
                    &GraphSkillRunState {
                        schema: GRAPH_SKILL_STATE_SCHEMA.to_owned(),
                        run_id: run_id.clone(),
                        runner_name: runner.name.clone(),
                        package_digest: package_digest.to_owned(),
                        execution_closure_digest: execution_closure_digest.to_owned(),
                        graph_inputs: graph_inputs.clone(),
                        resolution_answers: answers.clone(),
                        pending_request: None,
                        completed_receipt_id: None,
                        completed_graph_receipt_id: None,
                        terminal_result: None,
                        sealed_graph_receipt: None,
                        sealed_primary_receipt: None,
                        checkpoint: next_checkpoint.clone(),
                    },
                )?;
                checkpoint = next_checkpoint;
            }
            Err(RuntimeError::ResolutionPending {
                checkpoint: paused, ..
            }) if host.pending_request().is_some() => {
                let (request_id, request_value) = host
                    .pending_request()
                    .ok_or_else(|| invalid("graph blocked without pending request"))?;
                let pending_request = PendingSkillRequest {
                    id: request_id.to_owned(),
                    value: request_value.clone(),
                };
                write_graph_state(
                    request,
                    workspace,
                    receipts,
                    &run_id,
                    &GraphSkillRunState {
                        schema: GRAPH_SKILL_STATE_SCHEMA.to_owned(),
                        run_id: run_id.clone(),
                        runner_name: runner.name.clone(),
                        package_digest: package_digest.to_owned(),
                        execution_closure_digest: execution_closure_digest.to_owned(),
                        graph_inputs: graph_inputs.clone(),
                        resolution_answers: answers.clone(),
                        pending_request: Some(pending_request.clone()),
                        completed_receipt_id: None,
                        completed_graph_receipt_id: None,
                        terminal_result: None,
                        sealed_graph_receipt: None,
                        sealed_primary_receipt: None,
                        checkpoint: *paused
                            .ok_or_else(|| invalid("graph pause omitted its checkpoint"))?,
                    },
                )?;
                write_paused_graph_checkpoint(PausedGraphCheckpoint {
                    request,
                    workspace,
                    receipts,
                    manifest,
                    runner,
                    graph: &graph,
                    package_digest,
                    execution_closure_digest: Some(execution_closure_digest),
                    run_id: &run_id,
                    request_id: &pending_request.id,
                })?;
                return Ok(needs_agent_output(
                    manifest,
                    &runner.name,
                    &run_id,
                    &pending_request.id,
                    pending_request.value,
                ));
            }
            Err(RuntimeError::ResolutionPending {
                step_id, reason, ..
            }) => {
                return Err(invalid(format!(
                    "graph step {step_id:?} suspended without a pending host request: {reason}"
                )));
            }
            Err(RuntimeError::GraphBlocked { step_id, reason }) => {
                return seal_terminal_graph_skill_run(TerminalGraphSkillRun {
                    request,
                    workspace,
                    receipts,
                    manifest,
                    runner_name: &runner.name,
                    graph: graph.clone(),
                    checkpoint: previous_checkpoint,
                    run_id: &run_id,
                    runtime: &runtime,
                    package_digest,
                    execution_closure_digest,
                    graph_inputs: &graph_inputs,
                    answers: &answers,
                    step_id: &step_id,
                    reason_code: "graph_blocked",
                    summary: format!("graph {} blocked at {step_id}: {reason}", graph.name),
                    cause: GraphTerminalCause::Blocked,
                });
            }
            Err(RuntimeError::AuthorityDenied {
                verb,
                step_id,
                reason,
            }) => {
                return seal_terminal_graph_skill_run(TerminalGraphSkillRun {
                    request,
                    workspace,
                    receipts,
                    manifest,
                    runner_name: &runner.name,
                    graph: graph.clone(),
                    checkpoint: previous_checkpoint,
                    run_id: &run_id,
                    runtime: &runtime,
                    package_digest,
                    execution_closure_digest,
                    graph_inputs: &graph_inputs,
                    answers: &answers,
                    step_id: &step_id,
                    reason_code: "authority_denied",
                    summary: format!(
                        "graph {} denied {verb:?} at {step_id}: {reason}",
                        graph.name
                    ),
                    cause: GraphTerminalCause::Blocked,
                });
            }
            Err(error @ RuntimeError::ProviderReadbackPending { .. }) => {
                let step_id = error
                    .graph_step_id()
                    .unwrap_or("provider-readback")
                    .to_owned();
                let summary = format!(
                    "graph {} deferred at {step_id}: the provider mutation may be applied, but independent readback is unavailable; reconcile before retrying",
                    graph.name
                );
                return seal_terminal_graph_skill_run(TerminalGraphSkillRun {
                    request,
                    workspace,
                    receipts,
                    manifest,
                    runner_name: &runner.name,
                    graph: graph.clone(),
                    checkpoint: previous_checkpoint,
                    run_id: &run_id,
                    runtime: &runtime,
                    package_digest,
                    execution_closure_digest,
                    graph_inputs: &graph_inputs,
                    answers: &answers,
                    step_id: &step_id,
                    reason_code: "provider_readback_pending",
                    summary,
                    cause: GraphTerminalCause::Deferred(error),
                });
            }
            #[cfg(feature = "agent")]
            Err(RuntimeError::ManagedAgentResolution {
                step_id,
                request_id,
                source,
            }) => {
                let reason_code = format!("managed_agent_{}", source.reason_code());
                let summary = format!(
                    "graph {} managed agent failed at {step_id} ({})",
                    graph.name,
                    source.reason_code()
                );
                let error = RuntimeError::ManagedAgentResolution {
                    step_id: step_id.clone(),
                    request_id,
                    source,
                };
                return seal_terminal_graph_skill_run(TerminalGraphSkillRun {
                    request,
                    workspace,
                    receipts,
                    manifest,
                    runner_name: &runner.name,
                    graph: graph.clone(),
                    checkpoint: previous_checkpoint,
                    run_id: &run_id,
                    runtime: &runtime,
                    package_digest,
                    execution_closure_digest,
                    graph_inputs: &graph_inputs,
                    answers: &answers,
                    step_id: &step_id,
                    reason_code: &reason_code,
                    summary,
                    cause: GraphTerminalCause::Failed(error),
                });
            }
            Err(error) if !error.is_fatal_step_fault() => {
                let step_id = terminal_error_step_id(&graph, &error).ok_or_else(|| {
                    RuntimeError::EngineInvariant {
                        context: "sealing graph step failure",
                        message: format!("sealable graph error has no authoritative step: {error}"),
                    }
                })?;
                let summary = format!("graph {} failed at {step_id}: {error}", graph.name);
                return seal_terminal_graph_skill_run(TerminalGraphSkillRun {
                    request,
                    workspace,
                    receipts,
                    manifest,
                    runner_name: &runner.name,
                    graph: graph.clone(),
                    checkpoint: previous_checkpoint,
                    run_id: &run_id,
                    runtime: &runtime,
                    package_digest,
                    execution_closure_digest,
                    graph_inputs: &graph_inputs,
                    answers: &answers,
                    step_id: &step_id,
                    reason_code: "graph_step_failed",
                    summary,
                    cause: GraphTerminalCause::Failed(error),
                });
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn restore_sealed_graph_skill_run(
    context: &SkillExecutionContext<'_>,
    graph: ExecutionGraph,
    state: GraphSkillRunState,
    run_id: &str,
) -> Result<RunResult, SkillRunError> {
    let terminal_result = state.terminal_result.clone();
    let graph_receipt_id = state
        .completed_graph_receipt_id
        .as_deref()
        .ok_or_else(|| invalid("completed graph checkpoint lacks its sealed receipt identity"))?;
    let receipt_id = state
        .completed_receipt_id
        .as_deref()
        .ok_or_else(|| invalid("completed graph checkpoint lacks its primary receipt identity"))?;
    let sealed_graph_receipt = state
        .sealed_graph_receipt
        .as_ref()
        .ok_or_else(|| invalid("completed graph state lacks its sealed receipt"))?;
    if sealed_graph_receipt.id.as_str() != graph_receipt_id {
        return Err(invalid("completed graph state changed its sealed receipt"));
    }
    let sealed_primary_receipt = state.sealed_primary_receipt.as_ref();
    if sealed_primary_receipt
        .map(|receipt| receipt.id.as_str())
        .unwrap_or(graph_receipt_id)
        != receipt_id
    {
        return Err(invalid("completed graph state changed its primary receipt"));
    }
    let receipt_path = context.receipts.resolve_path(
        context.workspace,
        context.request.receipt_dir.as_deref(),
        None,
    );
    let policy = context.receipts.signature_config().signature_policy();
    let graph = materialize_graph_parameter_inputs(graph, &state.graph_inputs);
    let checkpoint = state.checkpoint;
    if checkpoint.graph_name != graph.name
        || (terminal_result.is_none() && checkpoint.state.status != GraphStatus::Succeeded)
        || (terminal_result.is_some() && checkpoint.state.status == GraphStatus::Succeeded)
    {
        return Err(invalid(
            "completed graph checkpoint does not match its runner",
        ));
    }
    crate::receipts::tree::validate_runtime_receipt_tree_refs_with_policy(
        sealed_graph_receipt,
        checkpoint.steps.iter().flat_map(|step| {
            step.nested_receipts
                .iter()
                .chain(std::iter::once(&step.receipt))
        }),
        ReceiptTreeConfig::default(),
        policy,
    )
    .map_err(|_| invalid("completed graph receipt tree does not match its checkpoint"))?;
    let run = GraphRun {
        graph,
        state: checkpoint.state,
        steps: checkpoint.steps,
        sync_points: checkpoint.sync_points,
        receipt: sealed_graph_receipt.clone(),
        journal: checkpoint.journal,
    };
    let primary = sealed_primary_receipt.unwrap_or(sealed_graph_receipt);
    if sealed_graph_receipt.subject.reference.uri.as_str()
        != format!("hrn_{}_graph", run.graph.name)
        || (terminal_result.is_some()
            && (sealed_graph_receipt.seal.disposition == ClosureDisposition::Closed
                || sealed_primary_receipt.is_some()))
        || (terminal_result.is_none()
            && (sealed_graph_receipt.seal.disposition != ClosureDisposition::Closed
                || primary.seal.disposition != ClosureDisposition::Closed))
    {
        return Err(invalid(
            "completed graph receipt does not match its sealed run",
        ));
    }
    let expected_primary = if terminal_result.is_none() && context.runner.source.act.is_some() {
        format!("hrn_{}_turn", identifier_segment(run_id))
    } else {
        sealed_graph_receipt.subject.reference.uri.to_string()
    };
    if primary.subject.reference.uri.as_str() != expected_primary {
        return Err(invalid(
            "completed primary receipt does not match its runner",
        ));
    }
    write_graph_receipts(context.request, context.workspace, context.receipts, &run)?;
    if let Some(primary) = sealed_primary_receipt {
        write_skill_receipt(
            context.request,
            context.workspace,
            context.receipts,
            primary,
        )?;
    }
    let graph_receipt = context
        .receipts
        .read_local_receipt(graph_receipt_id, &receipt_path.path)
        .map_err(|error| invalid(format!("completed graph receipt is invalid: {error}")))?;
    let primary_receipt = if receipt_id == graph_receipt_id {
        graph_receipt.clone()
    } else {
        context
            .receipts
            .read_local_receipt(receipt_id, &receipt_path.path)
            .map_err(|error| invalid(format!("completed primary receipt is invalid: {error}")))?
    };
    let run = GraphRun {
        receipt: graph_receipt,
        ..run
    };
    let result = match terminal_result {
        Some(result) => result,
        None => graph_run_result(&run)?,
    };
    let output = graph_run_skill_output(&result, &run)?;
    Ok(sealed_output(
        context.manifest,
        &context.runner.name,
        run_id,
        &output,
        &result,
        SkillOutputDiagnostics {
            context: Some(graph_run_context(&run)),
            trace: Some(graph_run_trace(&run)),
        },
        &primary_receipt,
    ))
}

fn terminal_error_step_id(graph: &ExecutionGraph, error: &RuntimeError) -> Option<String> {
    error.graph_step_id().map(str::to_owned).or_else(|| {
        let RuntimeError::SkillFailed { skill_name, .. } = error else {
            return None;
        };
        graph
            .steps
            .iter()
            .any(|step| step.id == *skill_name)
            .then(|| skill_name.clone())
    })
}

struct PausedGraphCheckpoint<'a> {
    request: &'a SkillRunRequest,
    workspace: &'a WorkspaceEnv,
    receipts: &'a ReceiptServices,
    manifest: &'a SkillRunnerManifest,
    runner: &'a SkillRunnerDefinition,
    graph: &'a ExecutionGraph,
    package_digest: &'a str,
    execution_closure_digest: Option<&'a str>,
    run_id: &'a str,
    request_id: &'a str,
}

fn write_paused_graph_checkpoint(input: PausedGraphCheckpoint<'_>) -> Result<(), SkillRunError> {
    let receipt_path =
        input
            .receipts
            .resolve_path(input.workspace, input.request.receipt_dir.as_deref(), None);
    let checkpoint = PausedRunCheckpoint {
        id: input.run_id.to_owned(),
        name: input
            .manifest
            .skill
            .clone()
            .unwrap_or_else(|| input.graph.name.clone()),
        kind: "graph".to_owned(),
        started_at: Some(crate::time::now_iso8601()),
        resume_skill_ref: Some(input.request.skill_path.to_string_lossy().into_owned()),
        selected_runner: Some(input.runner.name.clone()),
        credential_profile: input
            .request
            .local_credential
            .as_ref()
            .and_then(|credential| credential.profile.clone()),
        package_digest: Some(input.package_digest.to_owned()),
        execution_closure_digest: input.execution_closure_digest.map(str::to_owned),
        step_ids: vec![input.request_id.to_owned()],
        step_labels: vec![input.request_id.to_owned()],
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

enum GraphTerminalCause {
    Blocked,
    Deferred(RuntimeError),
    Failed(RuntimeError),
}

struct TerminalGraphSkillRun<'a> {
    request: &'a SkillRunRequest,
    workspace: &'a WorkspaceEnv,
    receipts: &'a ReceiptServices,
    manifest: &'a SkillRunnerManifest,
    runner_name: &'a str,
    graph: ExecutionGraph,
    checkpoint: GraphCheckpoint,
    run_id: &'a str,
    runtime: &'a Runtime<SkillSourceAdapter>,
    package_digest: &'a str,
    execution_closure_digest: &'a str,
    graph_inputs: &'a JsonObject,
    answers: &'a ResolutionAnswers,
    step_id: &'a str,
    reason_code: &'a str,
    summary: String,
    cause: GraphTerminalCause,
}

fn seal_terminal_graph_skill_run(
    context: TerminalGraphSkillRun<'_>,
) -> Result<RunResult, SkillRunError> {
    let mut final_host = SkillRunGraphHost::new(ResolutionAnswers::default());
    let failure_result = match &context.cause {
        GraphTerminalCause::Deferred(error) | GraphTerminalCause::Failed(error) => {
            Some(JsonValue::Object(error.public_failure_projection()))
        }
        GraphTerminalCause::Blocked => None,
    };
    let run = match context.cause {
        GraphTerminalCause::Blocked => context.runtime.seal_blocked_graph_checkpoint_with_host(
            context.graph,
            context.checkpoint,
            context.step_id,
            context.reason_code,
            context.summary,
            &mut final_host,
        )?,
        GraphTerminalCause::Deferred(_) => {
            context.runtime.seal_deferred_graph_checkpoint_with_host(
                context.graph,
                context.checkpoint,
                context.step_id,
                context.reason_code,
                context.summary,
                &mut final_host,
            )?
        }
        GraphTerminalCause::Failed(error) => {
            context.runtime.seal_failed_graph_checkpoint_with_host(
                context.graph,
                context.checkpoint,
                context.step_id,
                error,
                crate::receipts::GraphClosure {
                    disposition: ClosureDisposition::Failed,
                    reason_code: context.reason_code.to_owned(),
                    summary: context.summary,
                },
                &mut final_host,
            )?
        }
    };
    let result = failure_result.unwrap_or(graph_run_result(&run)?);
    let public_context = graph_run_context(&run);
    let trace = graph_run_trace(&run);
    let output = graph_run_skill_output(&result, &run)?;
    write_graph_state(
        context.request,
        context.workspace,
        context.receipts,
        context.run_id,
        &GraphSkillRunState {
            schema: GRAPH_SKILL_STATE_SCHEMA.to_owned(),
            run_id: context.run_id.to_owned(),
            runner_name: context.runner_name.to_owned(),
            package_digest: context.package_digest.to_owned(),
            execution_closure_digest: context.execution_closure_digest.to_owned(),
            graph_inputs: context.graph_inputs.clone(),
            resolution_answers: context.answers.clone(),
            pending_request: None,
            completed_receipt_id: Some(run.receipt.id.to_string()),
            completed_graph_receipt_id: Some(run.receipt.id.to_string()),
            terminal_result: Some(result.clone()),
            sealed_graph_receipt: Some(run.receipt.clone()),
            sealed_primary_receipt: None,
            checkpoint: GraphCheckpoint {
                graph_name: run.graph.name.clone(),
                state: run.state.clone(),
                steps: run.steps.clone(),
                sync_points: run.sync_points.clone(),
                journal: run.journal.clone(),
                suspended_step: None,
            },
        },
    )?;
    write_graph_receipts(context.request, context.workspace, context.receipts, &run)?;
    Ok(sealed_output(
        context.manifest,
        context.runner_name,
        context.run_id,
        &output,
        &result,
        SkillOutputDiagnostics {
            context: Some(public_context),
            trace: Some(trace),
        },
        &run.receipt,
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct GraphSkillRunState {
    pub(super) schema: String,
    pub(super) run_id: String,
    pub(super) runner_name: String,
    pub(super) package_digest: String,
    pub(super) execution_closure_digest: String,
    #[serde(default)]
    pub(super) graph_inputs: JsonObject,
    #[serde(default)]
    pub(super) resolution_answers: ResolutionAnswers,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) pending_request: Option<PendingSkillRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) completed_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) completed_graph_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) terminal_result: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) sealed_graph_receipt: Option<runx_contracts::Receipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) sealed_primary_receipt: Option<runx_contracts::Receipt>,
    pub(super) checkpoint: GraphCheckpoint,
}

#[derive(Default)]
/// In-process managed-agent resolver for graph agent steps. An agent step inside
/// a graph that has no seeded answer would otherwise host-drive (yield
/// `needs_agent`); explicit per-run consent plus a configured provider resolves
/// it inline. Provider configuration alone never enables managed execution.
struct InlineResolver {
    // Both fields feed the agent resolver path in `try_resolve` under the `agent`
    // feature; without it `try_resolve` is a no-op, so they are written at
    // construction but never read.
    #[cfg_attr(not(feature = "agent"), allow(dead_code))]
    skill_directory: PathBuf,
    #[cfg_attr(not(feature = "agent"), allow(dead_code))]
    env: BTreeMap<String, String>,
    #[cfg_attr(not(feature = "agent"), allow(dead_code))]
    credential_delivery: CredentialDelivery,
    #[cfg_attr(not(feature = "agent"), allow(dead_code))]
    effects: RuntimeEffectRegistry,
    #[cfg_attr(not(feature = "agent"), allow(dead_code))]
    observed_at: String,
    #[cfg_attr(not(feature = "agent"), allow(dead_code))]
    policy: crate::execution::orchestrator::ManagedAgentPolicy,
}

impl InlineResolver {
    #[cfg(feature = "agent")]
    fn try_resolve(&self, request: &ResolutionRequest) -> Result<Option<JsonValue>, RuntimeError> {
        use crate::adapters::agent::AgentResolver;
        use crate::adapters::agent_resolver::{ManagedAgentResolver, ManagedAgentResolverOptions};

        let Some(max_rounds) = self.policy.max_rounds() else {
            return Ok(None);
        };
        let fail = |message: String| RuntimeError::SkillFailed {
            skill_name: "managed-agent".to_owned(),
            message,
        };
        let Some(config) =
            crate::config::load_managed_agent_config(&self.env, &self.skill_directory)
                .map_err(|error| fail(format!("managed agent config error: {error}")))?
        else {
            return Ok(None);
        };
        let resolver = ManagedAgentResolver::new(
            config,
            ManagedAgentResolverOptions {
                env: self.env.clone(),
                skill_directory: self.skill_directory.clone(),
                credential_delivery: self.credential_delivery.clone(),
                effects: self.effects.clone(),
                observed_at: self.observed_at.clone(),
                max_rounds,
            },
        );
        let request_id = resolution_request_id(request).to_owned();
        let step_id = match request {
            ResolutionRequest::AgentAct { invocation, .. } => {
                invocation.envelope.skill.as_ref().to_owned()
            }
            _ => "managed-agent".to_owned(),
        };
        let resolution = resolver
            .resolve(request.clone())
            .map_err(|error| RuntimeError::managed_agent_resolution(step_id, request_id, error))?;
        Ok(Some(resolution.response.payload))
    }

    #[cfg(not(feature = "agent"))]
    fn try_resolve(&self, _request: &ResolutionRequest) -> Result<Option<JsonValue>, RuntimeError> {
        Ok(None)
    }
}

struct SkillRunGraphHost {
    answers: ResolutionAnswers,
    pending: Vec<(String, JsonValue)>,
    inline: Option<InlineResolver>,
}

impl SkillRunGraphHost {
    fn new(answers: ResolutionAnswers) -> Self {
        Self {
            answers,
            pending: Vec::new(),
            inline: None,
        }
    }

    fn with_inline(answers: ResolutionAnswers, inline: InlineResolver) -> Self {
        Self {
            answers,
            pending: Vec::new(),
            inline: Some(inline),
        }
    }

    fn pending_request(&self) -> Option<(&str, &JsonValue)> {
        self.pending
            .first()
            .map(|(request_id, request)| (request_id.as_str(), request))
    }
}

impl Host for SkillRunGraphHost {
    fn report(&mut self, _event: runx_contracts::ExecutionEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError> {
        let request_id = resolution_request_id(&request).to_owned();
        if let Some(answer) = self.answers.get(&request_id) {
            if self.answers.requires_request_digest(&request_id) {
                let supplied = self.answers.request_digest(&request_id).ok_or_else(|| {
                    RuntimeError::ContinuationRejected {
                        reason: format!(
                            "request_digests did not include pending request {request_id}"
                        ),
                    }
                })?;
                let request_value = serde_json::to_value(&request)
                    .and_then(serde_json::from_value::<JsonValue>)
                    .map_err(|source| {
                        RuntimeError::json("normalizing graph request for digest binding", source)
                    })?;
                let bytes = serde_json::to_vec(&request_value).map_err(|source| {
                    RuntimeError::json("serializing graph request for digest binding", source)
                })?;
                let current = runx_contracts::sha256_prefixed(&bytes);
                if supplied != current {
                    return Err(RuntimeError::ContinuationRejected {
                        reason: format!(
                            "request digest mismatch for {request_id}: supplied {supplied}, current {current}"
                        ),
                    });
                }
            }
            return Ok(Some(ResolutionResponse {
                actor: if self.answers.is_human_approval(&request_id) {
                    ResolutionResponseActor::Human
                } else {
                    ResolutionResponseActor::Agent
                },
                payload: answer.clone(),
            }));
        }
        // An agent step with no seeded answer runs the configured provider inline
        // rather than host-driving, so a graph turn (agent step -> governed action
        // step) completes in one pass. No provider configured -> falls through to
        // the host as before.
        if matches!(request, ResolutionRequest::AgentAct { .. })
            && let Some(inline) = &self.inline
            && let Some(payload) = inline.try_resolve(&request)?
        {
            return Ok(Some(ResolutionResponse {
                actor: ResolutionResponseActor::Agent,
                payload,
            }));
        }
        let request_value = serde_json::to_value(&request)
            .and_then(serde_json::from_value)
            .map_err(|source| RuntimeError::json("serializing graph resolution request", source))?;
        self.pending.push((request_id, request_value));
        Ok(None)
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }
}

fn resolution_request_id(request: &ResolutionRequest) -> &str {
    match request {
        ResolutionRequest::Input { id, .. }
        | ResolutionRequest::Approval { id, .. }
        | ResolutionRequest::AgentAct { id, .. } => id.as_str(),
    }
}

fn graph_run_id(
    request: &SkillRunRequest,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    package_digest: &str,
    execution_closure_digest: Option<&str>,
) -> Result<String, SkillRunError> {
    match (&request.run_id, &request.answers_path) {
        // A caller-supplied identity is durable across retries; hosted
        // execution keys receipts and provider-effect state on it so a repeated
        // attempt recovers the same mutation instead of minting a new one.
        (Some(run_id), _) => Ok(run_id.clone()),
        (None, Some(_)) => Err(invalid(
            "skill continuation requires both run_id and answers",
        )),
        (None, None) => generated_run_id(
            &runner.name,
            manifest,
            runner,
            None,
            &request.inputs,
            package_digest,
            execution_closure_digest,
        ),
    }
}

fn write_graph_receipts(
    request: &SkillRunRequest,
    workspace: &WorkspaceEnv,
    receipts: &ReceiptServices,
    run: &GraphRun,
) -> Result<(), SkillRunError> {
    for step in &run.steps {
        for receipt in &step.nested_receipts {
            write_skill_receipt(request, workspace, receipts, receipt)?;
        }
        write_skill_receipt(request, workspace, receipts, &step.receipt)?;
    }
    write_skill_receipt(request, workspace, receipts, &run.receipt)
}

/// When a graph runner declares an `act:` block, seal the turn's primary receipt
/// as its domain act: the reason comes from the agent voice step's output, the
/// effect from the deterministic action step's real `/v1` response, and the
/// structure/authority from the declared `act:` block plus the trusted graph
/// inputs. The graph's per-step receipts remain as the execution trace; this
/// standalone domain receipt is what the turn presents and what chains by
/// lineage. Transport (the http step, status, token) never enters it.
// Function rationale: assembling the domain-act receipt is one frame
// build/mint/seal sequence; splitting it would separate the authority mint from the
// frame it seals into.
pub(crate) fn graph_domain_act_receipt(
    runner: &SkillRunnerDefinition,
    graph_inputs: &JsonObject,
    run: &GraphRun,
    run_id: &str,
    created_at: &str,
    signature_config: &RuntimeReceiptSignatureConfig,
) -> Result<Option<runx_contracts::Receipt>, SkillRunError> {
    let Some(act) = runner.source.act.as_ref() else {
        return Ok(None);
    };
    let step_output = |step_id: Option<&str>| {
        step_id.and_then(|id| run.steps.iter().find(|step| step.step_id == id))
    };
    // Reason: the agent voice step's structured output (e.g. {line: "..."}).
    let reason_source = step_output(act.reason_step.as_deref())
        .map(|step| JsonValue::Object(step.contract.clone()))
        .unwrap_or(JsonValue::Null);
    // Effect: the action step's declared contract. Adapter transport is not a
    // second semantic surface and is discarded after receipt sealing.
    let governed_effect = step_output(act.effect_step.as_deref())
        .filter(|step| step.outcome.succeeded())
        .map(|step| JsonValue::Object(step.contract.clone()));
    let authority_grant_refs = graph_credential_grant_refs(run);
    let Some(mut frame) = build_domain_act_frame(
        act,
        graph_inputs,
        &reason_source,
        governed_effect.as_ref(),
        authority_grant_refs,
    ) else {
        return Ok(None);
    };
    for reference in run
        .steps
        .iter()
        .flat_map(|step| step.receipt.acts.iter())
        .flat_map(|receipt_act| receipt_act.artifact_refs.iter())
        .filter(|reference| reference.uri.as_str().contains("operator_context"))
    {
        if !frame
            .artifact_refs
            .iter()
            .any(|existing| existing.uri == reference.uri)
        {
            frame.artifact_refs.push(reference.clone());
        }
    }
    // Compute path: when the act declares `mint_authority`, the runtime mints the
    // child term and proves the subset against the graph charter off the model
    // path, overriding the (empty, since the parser holds them mutually exclusive)
    // pre-built attenuation fields. Fail-loud: a request exceeding the charter
    // fails the turn rather than sealing a false or missing attenuation.
    if let Some((terms, attenuation)) = mint_charter_attenuation(
        act,
        runner
            .source
            .graph
            .as_ref()
            .and_then(|graph| graph.charter_from.as_deref()),
        graph_inputs,
        created_at,
    )? {
        frame.authority_terms = terms;
        frame.authority_attenuation = Some(attenuation);
    }
    let graph_name = identifier_segment(run_id);
    let verification_metadata = step_output(act.reason_step.as_deref())
        .map(|step| step.outcome.metadata.clone())
        .unwrap_or_default();
    let receipt = domain_act_receipt(DomainActReceiptRequest {
        graph_name: &graph_name,
        step_id: "turn",
        succeeded: run.state.status == GraphStatus::Succeeded,
        created_at,
        disposition: ClosureDisposition::Closed,
        reason_code: "agent_act_closed".to_owned(),
        seal_summary: "governed graph turn sealed".to_owned(),
        frame,
        verification_metadata,
        signature_policy: signature_config.signature_policy(),
    })?;
    Ok(Some(receipt))
}

/// Mint the charter -> member attenuation for a graph turn that declares
/// `mint_authority`. The parent charter is the AuthorityTerm carried by the graph
/// runner's `charter_from` input; the requested narrowing is the AttenuationRequest
/// carried by `requested_scope_from`. The child term and subset proof are computed
/// and verified by the core mint primitive, so the runtime never trusts a pre-built
/// proof here and a request exceeding the charter fails the turn loudly.
// Function rationale: minting is one linear resolve-charter,
// build-request, mint-and-prove sequence on the trust boundary; splitting it would
// separate the charter from the proof that bounds it.
pub(crate) fn mint_charter_attenuation(
    act: &runx_parser::ActDeclaration,
    charter_key: Option<&str>,
    graph_inputs: &JsonObject,
    created_at: &str,
) -> Result<
    Option<(
        Vec<runx_contracts::AuthorityTerm>,
        runx_contracts::AuthorityAttenuation,
    )>,
    SkillRunError,
> {
    use runx_core::policy::{AttenuationRequest, ScopeBoundsComparator, mint_attenuated};
    use runx_parser::MintScopeSource;

    let Some(directive) = act.mint_authority.as_ref() else {
        return Ok(None);
    };
    let charter_key = charter_key.ok_or_else(|| {
        invalid("mint_authority requires the graph runner to declare charter_from")
    })?;
    let charter: runx_contracts::AuthorityTerm = decode_graph_input(graph_inputs, charter_key)
        .ok_or_else(|| {
            invalid(format!(
                "mint_authority charter input '{charter_key}' did not resolve to an authority term"
            ))
        })?;
    let request: AttenuationRequest = match directive.source {
        MintScopeSource::RequestedScope => {
            let key = act.requested_scope_from.as_deref().ok_or_else(|| {
                invalid("mint_authority requested_scope requires requested_scope_from")
            })?;
            decode_graph_input(graph_inputs, key).ok_or_else(|| {
                invalid(format!(
                    "mint_authority requested_scope input '{key}' did not resolve to an attenuation request"
                ))
            })?
        }
        MintScopeSource::StaticScopes => {
            return Err(invalid(
                "mint_authority source static_scopes is not yet wired in the runtime; use requested_scope",
            ));
        }
    };
    let (child, proof) = mint_attenuated(
        &charter,
        &request,
        &ScopeBoundsComparator,
        created_at.into(),
    )
    .map_err(|error| {
        invalid(format!(
            "mint_authority requested child is not a subset of the charter ({error:?})"
        ))
    })?;
    let attenuation = runx_contracts::AuthorityAttenuation {
        parent_authority_ref: Some(proof.parent_authority_ref.clone()),
        subset_proof: Some(proof),
    };
    Ok(Some((vec![child], attenuation)))
}

/// Decode a trusted graph input value into a typed contract struct.
pub(crate) fn decode_graph_input<T: serde::de::DeserializeOwned>(
    inputs: &JsonObject,
    key: &str,
) -> Option<T> {
    inputs
        .get(key)
        .and_then(|value| serde_json::to_value(value).ok())
        .and_then(|value| serde_json::from_value(value).ok())
}

/// Gather the credential grant refs the turn actually held, read from the
/// `Credential` verification refs sealed on each step receipt. These become the
/// domain act's `authority.grant_refs`, so the receipt records the authority it
/// carried, not only the declared scope.
pub(crate) fn graph_credential_grant_refs(run: &GraphRun) -> Vec<runx_contracts::Reference> {
    let mut refs: Vec<runx_contracts::Reference> = Vec::new();
    for step in &run.steps {
        for act in &step.receipt.acts {
            for binding in &act.criterion_bindings {
                for reference in &binding.verification_refs {
                    if reference.reference_type == runx_contracts::ReferenceType::Credential
                        && !refs.contains(reference)
                    {
                        refs.push(reference.clone());
                    }
                }
            }
        }
    }
    refs
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use runx_parser::{SkillSource, SourceKind};

    use super::*;
    use crate::adapter::SkillAdapter;
    #[cfg(feature = "mcp")]
    use crate::adapters::mcp::{McpAdapter, McpToolCallRequest, McpTransport, McpTransportError};

    #[test]
    fn terminal_skill_failure_uses_only_an_exact_graph_step_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let graph = runx_parser::validate_graph(runx_parser::parse_graph_yaml(
            "name: authoring\nsteps:\n  - id: apply\n    skill: ./apply\n",
        )?)?;
        let owned = RuntimeError::SkillFailed {
            skill_name: "apply".to_owned(),
            message: "candidate harness failed".to_owned(),
        };
        let nested = RuntimeError::SkillFailed {
            skill_name: "runx.skill.apply".to_owned(),
            message: "candidate harness failed".to_owned(),
        };

        assert_eq!(
            terminal_error_step_id(&graph, &owned).as_deref(),
            Some("apply")
        );
        assert_eq!(terminal_error_step_id(&graph, &nested), None);
        Ok(())
    }

    #[test]
    fn mint_authority_seals_a_subset_proven_child() -> Result<(), SkillRunError> {
        use runx_contracts::{
            AuthorityBounds, AuthorityResourceFamily, AuthorityTerm, AuthorityVerb, Reference,
            ReferenceType,
        };
        use runx_core::policy::{AttenuationRequest, ensure_subset_proof};

        // Deterministic fixture instant for the minted child's `granted_at`; the
        // test asserts on the subset proof, not the timestamp.
        let created_at = "2026-05-18T00:00:00Z";

        let principal = Reference::with_uri(ReferenceType::Principal, "runx:principal:agency");
        let member = Reference::with_uri(ReferenceType::Principal, "runx:principal:writer");
        let resource = Reference::with_uri(ReferenceType::Repository, "runx:repository:docs");
        let bounds = AuthorityBounds {
            filesystem_roots: vec!["/repo".into()],
            ..AuthorityBounds::default()
        };
        let charter = AuthorityTerm {
            term_id: "charter".into(),
            principal_ref: principal.clone(),
            resource_ref: resource.clone(),
            resource_family: AuthorityResourceFamily::Workspace,
            verbs: vec![AuthorityVerb::Read, AuthorityVerb::Write],
            bounds: bounds.clone(),
            conditions: Vec::new(),
            approvals: Vec::new(),
            capabilities: Vec::new(),
            expires_at: None,
            issued_by_ref: principal,
            credential_ref: None,
        };
        let make_request = |verbs: Vec<AuthorityVerb>| AttenuationRequest {
            principal_ref: member.clone(),
            resource_ref: resource.clone(),
            resource_family: AuthorityResourceFamily::Workspace,
            verbs,
            capabilities: Vec::new(),
            bounds: bounds.clone(),
            expires_at: None,
        };

        let act: runx_parser::ActDeclaration = serde_json::from_value(serde_json::json!({
            "mint_authority": {"source": "requested_scope"},
            "requested_scope_from": "requested"
        }))
        .map_err(|error| invalid(format!("act fixture: {error}")))?;

        // Valid narrowing: a read-only child of a read+write charter, same resource.
        let mut inputs = JsonObject::new();
        inputs.insert("charter".to_owned(), contract_json_value(&charter)?);
        inputs.insert(
            "requested".to_owned(),
            contract_json_value(&make_request(vec![AuthorityVerb::Read]))?,
        );
        let (terms, attenuation) =
            mint_charter_attenuation(&act, Some("charter"), &inputs, created_at)?
                .ok_or_else(|| invalid("expected minted attenuation"))?;
        assert_eq!(terms.len(), 1, "exactly one minted child term");
        let proof = attenuation
            .subset_proof
            .as_ref()
            .ok_or_else(|| invalid("minted attenuation must carry a subset proof"))?;
        // The receipt verifier accepts the computed proof.
        ensure_subset_proof(Some(proof), &terms[0], &charter)
            .map_err(|error| invalid(format!("verifier rejected minted proof: {error:?}")))?;

        // Fail-closed: widening verbs beyond the charter errors and seals nothing.
        let mut widen = JsonObject::new();
        widen.insert("charter".to_owned(), contract_json_value(&charter)?);
        widen.insert(
            "requested".to_owned(),
            contract_json_value(&make_request(vec![
                AuthorityVerb::Read,
                AuthorityVerb::Delete,
            ]))?,
        );
        assert!(
            mint_charter_attenuation(&act, Some("charter"), &widen, created_at,).is_err(),
            "a request that widens beyond the charter must fail closed"
        );

        // Fail-closed: an unresolved charter input errors rather than sealing a root.
        assert!(
            mint_charter_attenuation(&act, Some("absent"), &inputs, created_at,).is_err(),
            "an unresolved charter must fail closed"
        );

        Ok(())
    }

    #[test]
    fn graph_source_registry_fails_closed_on_unregistered_source() {
        let mut raw = JsonObject::new();
        raw.insert("type".to_owned(), JsonValue::String("a2a".to_owned()));
        let invocation = SkillInvocation {
            skill_name: "fixture-a2a".to_owned(),
            step_id: None,
            artifacts: None,
            allowed_tools: None,
            requirements: Default::default(),
            source: SkillSource {
                act: None,
                source_type: SourceKind::A2a,
                command: None,
                module: None,
                javascript_export: None,
                pages: None,
                args: Vec::new(),
                cwd: None,
                timeout_seconds: None,
                input_mode: None,
                environment: Default::default(),
                server: None,
                tool: None,
                arguments: None,
                agent_card_url: None,
                agent_identity: None,
                agent: None,
                task: None,
                outputs: None,
                graph: None,
                external_adapter: None,
                thread_outbox_provider: None,
                raw,
            },
            inputs: JsonObject::new(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: PathBuf::from("."),
            env: BTreeMap::new(),
            credential_delivery: crate::credentials::CredentialDelivery::none(),
        };

        let result = SkillSourceAdapter::default().invoke(invocation);
        assert!(
            matches!(
                &result,
                Err(RuntimeError::UnsupportedSource { source_kind }) if source_kind == "a2a"
            ),
            "unexpected unregistered graph source result: {result:?}"
        );
    }

    #[cfg(feature = "agent")]
    #[test]
    fn managed_agent_graph_failure_is_typed_sealed_and_visible_in_history()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::LocalReceiptStore;
        use crate::adapters::agent::{
            AgentExecutionTelemetry, AgentResolverError, AgentToolExecutionTrace,
        };
        use crate::journal::{HistoryFilter, list_local_history};
        use runx_core::state_machine::{GraphStatus, create_sequential_graph_state};
        use runx_parser::{parse_graph_yaml, validate_graph};

        for suspended in [false, true] {
            let temp = tempfile::tempdir()?;
            let receipt_dir = temp.path().join("receipts");
            let graph = validate_graph(parse_graph_yaml(
                r#"
name: managed-agent-graph-failure
steps:
  - id: compose
    skill: ./compose
"#,
            )?)?;
            let mut checkpoint = GraphCheckpoint {
                graph_name: graph.name.clone(),
                state: create_sequential_graph_state(
                    graph.name.clone(),
                    &crate::execution::graph::step_definitions(&graph),
                ),
                steps: Vec::new(),
                sync_points: Vec::new(),
                journal: crate::ExecutionJournal::default(),
                suspended_step: None,
            };
            if suspended {
                checkpoint.state.steps[0].status =
                    runx_core::state_machine::GraphStepStatus::Running;
                checkpoint.state.steps[0].attempts = 1;
                checkpoint.suspended_step =
                    Some(Box::new(crate::execution::runner::SuspendedGraphStep {
                        step_id: "compose".to_owned(),
                        child: None,
                    }));
            }
            let runtime = Runtime::new(
                SkillSourceAdapter::default(),
                RuntimeOptions::local_development(std::env::vars().collect()),
            );
            let error = AgentResolverError::bounded_failure(
                "round_budget_exhausted",
                "Managed agent exceeded 3 tool-call rounds without finalizing.",
                AgentExecutionTelemetry {
                    rounds: Some(3),
                    model_calls: Some(4),
                    tool_calls: Some(3),
                    tools: Some(vec!["data.read".to_owned()]),
                    tool_executions: Some(vec![AgentToolExecutionTrace {
                        tool: "data.read".to_owned(),
                        status: "success".to_owned(),
                        receipt_id: Some("rct_child".to_owned()),
                        resolution_kind: None,
                    }]),
                },
            );
            let runtime_error = RuntimeError::managed_agent_resolution(
                "referenced-agent-runner",
                "agent_task.compose.output",
                error,
            )
            .at_graph_step("compose");
            let mut host = SkillRunGraphHost::new(ResolutionAnswers::default());
            let run = runtime.seal_failed_graph_checkpoint_with_host(
                graph,
                checkpoint,
                "compose",
                runtime_error,
                crate::receipts::GraphClosure {
                    disposition: ClosureDisposition::Failed,
                    reason_code: "managed_agent_round_budget_exhausted".to_owned(),
                    summary: "managed agent failed at compose".to_owned(),
                },
                &mut host,
            )?;

            assert_eq!(run.state.status, GraphStatus::Failed);
            assert_eq!(run.receipt.seal.disposition, ClosureDisposition::Failed);
            assert_eq!(run.steps.len(), 1);
            assert_eq!(run.state.steps[0].attempts, 1);
            let step = &run.steps[0];
            assert_eq!(step.attempt, 1);
            assert_eq!(step.receipt.seal.disposition, ClosureDisposition::Failed);
            let metadata = step
                .receipt
                .metadata
                .as_ref()
                .ok_or("managed-agent failure metadata missing from child receipt")?;
            let encoded = serde_json::to_string(metadata)?;
            assert!(encoded.contains("\"reason_code\":\"round_budget_exhausted\""));
            assert!(encoded.contains("\"model_calls\":4"));
            assert!(encoded.contains("\"tool_calls\":3"));
            assert!(!encoded.contains("prompt"));
            assert!(!encoded.contains("credential"));
            assert!(!encoded.contains("raw_output"));

            let request = SkillRunRequest {
                skill_path: temp.path().join("skill"),
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
            write_graph_receipts(&request, &workspace, &receipts, &run)?;
            let history = list_local_history(
                &LocalReceiptStore::new(&receipt_dir),
                temp.path(),
                &temp.path().join(".runx"),
                &HistoryFilter::default(),
            )?;
            assert_eq!(history.receipts.len(), 1);
            assert!(
                history
                    .receipts
                    .iter()
                    .all(|receipt| receipt.status == "failed")
            );
            assert!(history.pending_runs.is_empty());
            let diagnostic_history = list_local_history(
                &LocalReceiptStore::new(&receipt_dir),
                temp.path(),
                &temp.path().join(".runx"),
                &HistoryFilter {
                    include_internal: true,
                    ..HistoryFilter::default()
                },
            )?;
            assert_eq!(diagnostic_history.receipts.len(), 2);
        }
        Ok(())
    }

    #[cfg(feature = "cli-tool")]
    #[test]
    fn graph_cli_tool_uses_structured_credential_redaction()
    -> Result<(), Box<dyn std::error::Error>> {
        const MARKER: &str = "cli-credential-redaction-sentinel";
        let secret = "cli-credential-redaction-sentinel-\"quoted\\slash\ncontrol";
        let delivery = crate::credentials::CredentialDelivery::from_local_descriptor(
            "twitter",
            "oauth1_user",
            "TWITTER_TOKEN",
            "ref:twitter:primary",
            vec!["twitter:read".to_owned()],
            secret,
        )?;
        let encoded = serde_json::to_string(&JsonValue::Object(JsonObject::from([
            (secret.to_owned(), JsonValue::String(secret.to_owned())),
            (
                "nested".to_owned(),
                JsonValue::Array(vec![JsonValue::String(secret.to_owned())]),
            ),
        ])))?;
        let invocation = SkillInvocation {
            skill_name: "credential-observation".to_owned(),
            step_id: None,
            artifacts: None,
            allowed_tools: None,
            requirements: Default::default(),
            source: SkillSource {
                act: None,
                source_type: SourceKind::CliTool,
                command: Some("/bin/sh".to_owned()),
                module: None,
                javascript_export: None,
                pages: None,
                args: vec![
                    "-c".to_owned(),
                    "test -n \"$TWITTER_TOKEN\" && printf '%s' \"$1\"".to_owned(),
                    "runx-credential-redaction".to_owned(),
                    encoded,
                ],
                cwd: None,
                timeout_seconds: Some(5),
                input_mode: None,
                environment: Default::default(),
                server: None,
                tool: None,
                arguments: None,
                agent_card_url: None,
                agent_identity: None,
                agent: None,
                task: None,
                outputs: None,
                graph: None,
                external_adapter: None,
                thread_outbox_provider: None,
                raw: JsonObject::new(),
            },
            inputs: JsonObject::new(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: std::env::current_dir()?,
            env: std::env::vars().collect(),
            credential_delivery: delivery,
        };

        let output = SkillSourceAdapter::default().invoke(invocation)?;

        assert!(output.succeeded());
        assert!(!serde_json::to_string(&output.value)?.contains(MARKER));
        let projection = crate::execution::output_projection::project_step_claim(JsonObject::new());
        let value = serde_json::to_string(&output.value)?;
        assert!(projection.outputs.is_empty());
        assert!(value.contains("[redacted-credential]"));
        assert!(!value.contains(MARKER));
        assert!(!format!("{output:?}").contains(MARKER));
        let metadata = serde_json::to_string(&output.metadata)?;
        assert!(metadata.contains("credential_delivery_observations"));
        assert!(metadata.contains("runx:credential:local:"));
        assert!(!metadata.contains(MARKER));

        let receipt = crate::receipts::step_receipt(
            "credential_graph",
            "credential_cli",
            1,
            &output,
            &JsonObject::new(),
            "2026-07-15T00:00:00Z",
        )?;
        assert!(
            receipt.seal.criteria[0]
                .verification_refs
                .iter()
                .any(|reference| {
                    reference.reference_type == runx_contracts::ReferenceType::Credential
                })
        );
        assert!(!serde_json::to_string(&receipt)?.contains(MARKER));
        Ok(())
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn graph_mcp_uses_structured_credential_redaction() -> Result<(), Box<dyn std::error::Error>> {
        assert_mcp_credential_redaction(StructuredCredentialTransport, "structured-result")
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn graph_mcp_text_json_uses_structured_credential_redaction()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_mcp_credential_redaction(JsonTextCredentialTransport, "text-json-result")
    }

    #[cfg(feature = "mcp")]
    const MCP_SECRET: &str = "mcp-credential-redaction-sentinel-\"quoted\\slash\ncontrol";
    #[cfg(feature = "mcp")]
    const MCP_MARKER: &str = "mcp-credential-redaction-sentinel";

    #[cfg(feature = "mcp")]
    #[derive(Clone, Debug)]
    struct StructuredCredentialTransport;

    #[cfg(feature = "mcp")]
    impl McpTransport for StructuredCredentialTransport {
        fn call_tool(&self, _request: McpToolCallRequest) -> Result<JsonValue, McpTransportError> {
            Ok(mcp_secret_document())
        }
    }

    #[cfg(feature = "mcp")]
    #[derive(Clone, Debug)]
    struct JsonTextCredentialTransport;

    #[cfg(feature = "mcp")]
    impl McpTransport for JsonTextCredentialTransport {
        fn call_tool(&self, _request: McpToolCallRequest) -> Result<JsonValue, McpTransportError> {
            let text = serde_json::to_string(&mcp_secret_document())
                .map_err(|_| McpTransportError::failed("fixture serialization failed"))?;
            Ok(JsonValue::Object(JsonObject::from([(
                "content".to_owned(),
                JsonValue::Array(vec![JsonValue::Object(JsonObject::from([
                    ("type".to_owned(), JsonValue::String("text".to_owned())),
                    ("text".to_owned(), JsonValue::String(text)),
                ]))]),
            )])))
        }
    }

    #[cfg(feature = "mcp")]
    fn mcp_secret_document() -> JsonValue {
        JsonValue::Object(JsonObject::from([(
            "structuredContent".to_owned(),
            JsonValue::Object(JsonObject::from([
                (
                    MCP_SECRET.to_owned(),
                    JsonValue::String(MCP_SECRET.to_owned()),
                ),
                (
                    "nested".to_owned(),
                    JsonValue::Array(vec![JsonValue::String(MCP_SECRET.to_owned())]),
                ),
            ])),
        )]))
    }

    #[cfg(feature = "mcp")]
    fn assert_mcp_credential_redaction<T: McpTransport>(
        transport: T,
        step_id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let delivery = crate::credentials::CredentialDelivery::from_local_descriptor(
            "example",
            "api_key",
            "EXAMPLE_TOKEN",
            "ref:example:primary",
            vec!["example:read".to_owned()],
            MCP_SECRET,
        )?;
        let invocation = SkillInvocation {
            skill_name: "mcp-structured-redaction".to_owned(),
            step_id: None,
            artifacts: None,
            allowed_tools: None,
            requirements: Default::default(),
            source: SkillSource {
                act: None,
                source_type: SourceKind::Mcp,
                command: None,
                module: None,
                javascript_export: None,
                pages: None,
                args: Vec::new(),
                cwd: None,
                timeout_seconds: Some(5),
                input_mode: None,
                environment: Default::default(),
                server: Some(runx_parser::SkillMcpServer {
                    command: std::env::current_exe()?.to_string_lossy().into_owned(),
                    args: Vec::new(),
                    cwd: None,
                }),
                tool: Some("reflect".to_owned()),
                arguments: None,
                agent_card_url: None,
                agent_identity: None,
                agent: None,
                task: None,
                outputs: None,
                graph: None,
                external_adapter: None,
                thread_outbox_provider: None,
                raw: JsonObject::new(),
            },
            inputs: JsonObject::new(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: std::env::current_dir()?,
            env: std::env::vars().collect(),
            credential_delivery: delivery,
        };

        let output = McpAdapter::new(transport).invoke(invocation)?;
        assert!(output.succeeded());
        assert!(!format!("{:?}", output.value).contains(MCP_MARKER));
        let projection = crate::execution::output_projection::project_step_claim(JsonObject::new());
        let value = serde_json::to_string(&output.value)?;
        assert!(projection.outputs.is_empty());
        assert!(value.contains("[redacted-credential]"));
        assert!(
            output
                .value
                .as_object()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(!value.contains(MCP_MARKER));
        assert!(!format!("{output:?}").contains(MCP_MARKER));

        let receipt = crate::receipts::step_receipt(
            "mcp-redaction",
            step_id,
            1,
            &output,
            &JsonObject::new(),
            "2026-07-22T00:00:00Z",
        )?;
        assert!(!serde_json::to_string(&receipt)?.contains(MCP_MARKER));
        Ok(())
    }

    #[cfg(feature = "external-adapter")]
    #[test]
    fn graph_source_registry_routes_external_adapter() {
        let mut raw = JsonObject::new();
        raw.insert(
            "type".to_owned(),
            JsonValue::String("external-adapter".to_owned()),
        );
        let invocation = SkillInvocation {
            skill_name: "fixture-external".to_owned(),
            step_id: None,
            artifacts: None,
            allowed_tools: None,
            requirements: Default::default(),
            source: SkillSource {
                act: None,
                source_type: SourceKind::ExternalAdapter,
                command: None,
                module: None,
                javascript_export: None,
                pages: None,
                args: Vec::new(),
                cwd: None,
                timeout_seconds: None,
                input_mode: None,
                environment: Default::default(),
                server: None,
                tool: None,
                arguments: None,
                agent_card_url: None,
                agent_identity: None,
                agent: None,
                task: None,
                outputs: None,
                graph: None,
                external_adapter: None,
                thread_outbox_provider: None,
                raw,
            },
            inputs: JsonObject::new(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: PathBuf::from("."),
            env: BTreeMap::new(),
            credential_delivery: crate::credentials::CredentialDelivery::none(),
        };

        let result = SkillSourceAdapter::default().invoke(invocation);
        assert!(
            matches!(&result, Err(RuntimeError::SkillFailed { .. })),
            "external-adapter source should route to the external adapter and fail on the \
             missing manifest, not fall through as UnsupportedSource; got: {result:?}"
        );
    }

    #[cfg(feature = "thread-outbox-provider")]
    #[test]
    fn graph_source_registry_routes_thread_outbox_provider() {
        let mut raw = JsonObject::new();
        raw.insert(
            "type".to_owned(),
            JsonValue::String("thread-outbox-provider".to_owned()),
        );
        let invocation = SkillInvocation {
            skill_name: "fixture-thread-outbox-provider".to_owned(),
            step_id: None,
            artifacts: None,
            allowed_tools: None,
            requirements: Default::default(),
            source: SkillSource {
                act: None,
                source_type: SourceKind::ThreadOutboxProvider,
                command: None,
                module: None,
                javascript_export: None,
                pages: None,
                args: Vec::new(),
                cwd: None,
                timeout_seconds: None,
                input_mode: None,
                environment: Default::default(),
                server: None,
                tool: None,
                arguments: None,
                agent_card_url: None,
                agent_identity: None,
                agent: None,
                task: None,
                outputs: None,
                graph: None,
                external_adapter: None,
                thread_outbox_provider: None,
                raw,
            },
            inputs: JsonObject::new(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: PathBuf::from("."),
            env: BTreeMap::new(),
            credential_delivery: crate::credentials::CredentialDelivery::none(),
        };

        let result = SkillSourceAdapter::default().invoke(invocation);
        assert!(
            matches!(&result, Err(RuntimeError::SkillFailed { .. })),
            "thread-outbox-provider source should route to the Rust provider front and fail on \
             missing config, not fall through as UnsupportedSource; got: {result:?}"
        );
    }
}
