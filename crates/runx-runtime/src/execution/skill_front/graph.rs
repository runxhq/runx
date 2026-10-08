// Module rationale: the skill front's run driver keeps the run record,
// host resolution and sealing in one path: a run starts or continues from its
// record, takes the host's answers, runs, and seals.
use super::{
    ExecutableAdapter, PendingSkillRequest, SkillExecutionContext, SkillOutputDiagnostics,
    SkillRunError, build_domain_act_frame, invalid, needs_agent_output, sealed_output,
};

use std::collections::BTreeMap;
use std::path::PathBuf;

use runx_contracts::{
    ClosureDisposition, JsonObject, JsonValue, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor,
};
use runx_parser::{ExecutionGraph, SkillRunnerDefinition};

use crate::RuntimeError;
#[cfg(test)]
use crate::adapter::SkillInvocation;
use crate::effects::RuntimeEffectRegistry;
use crate::execution::graph::materialize_graph_parameter_inputs;
use crate::execution::orchestrator::{RunResult, SkillRunRequest};
use crate::execution::run_record::{
    Hold, Origin, RunContext, RunLog, RunRecord, StartRequest, pending_request,
};
use crate::execution::runner::{
    GraphRun, RUNX_RUN_ID_ENV, Runtime, RuntimeOptions, graph_run_context, graph_run_result,
    graph_run_skill_output, graph_run_trace, sealed_graph_run,
};
use crate::host::Host;
use crate::kernel::store::KernelStore;
use crate::receipts::{
    DomainActReceiptRequest, RunScope, RuntimeReceiptSignatureConfig, domain_act_receipt,
};
use crate::services::{ReceiptServices, WorkspaceEnv};

use super::resolution_answers::{ResolutionAnswers, read_answers};
use super::runner_manifest::write_skill_receipt;

/// How many times a run waits for an independent provider readback before
/// it is sealed deferred.
const MAX_READBACK_RETRIES: u32 = 24;

// Function rationale: one run driver: open the record, bind the host's
// answers, run or continue, and seal; each outcome branch seals exactly once.
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
    let execution_closure_digest = execution_closure_digest
        .ok_or_else(|| invalid("graph execution requires an execution-closure digest"))?;
    let log = run_log(request, workspace, &manifest.skill)?;
    let existing = log.load()?;
    let request_context = RunContext {
        current: request.current_context.clone(),
        historical: request.historical_context.clone(),
    };
    let (created_at, graph_inputs, run_context, run_authority) = match &existing {
        Some(record) => {
            let started = continuation_identity(
                record,
                request,
                &request_context,
                runner,
                package_digest,
                execution_closure_digest,
            )?;
            (
                record.created_at.clone(),
                record.inputs.clone(),
                record.context.clone(),
                started.run_authority.clone(),
            )
        }
        None if request.answers_path.is_some() => {
            return Err(invalid(format!(
                "run {} has no record to continue",
                log.run_id()
            )));
        }
        None => {
            let created_at = crate::time::now_iso8601();
            let inputs = request
                .inputs
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<JsonObject>();
            log.begin(StartRequest {
                skill: &manifest.skill,
                skill_ref: &request.skill_path.to_string_lossy(),
                runner: &runner.name,
                package_digest,
                execution_closure_digest,
                inputs: &inputs,
                context: &request_context,
                run_authority: request.run_authority.as_ref(),
                created_at: &created_at,
            })?;
            (
                created_at,
                inputs,
                request_context,
                request.run_authority.clone(),
            )
        }
    };
    if let Some(record) = existing.as_ref().filter(|record| record.sealed.is_some()) {
        return restore_sealed_run(context, &log, record.clone());
    }
    if existing.is_some() {
        crate::input_contract::materialize_complete_runner_inputs(&runner.inputs, &graph_inputs)
            .map_err(|error| error.into_runtime_error())?;
    }

    // The host's answers for this pass. Each is recorded only by the step
    // that consumes it, so an answer the run refuses never enters its record.
    let answers = match &overrides.seeded_answers {
        Some(seeded) => seeded.clone(),
        None => match &request.answers_path {
            Some(path) => read_answers(path)?,
            None => ResolutionAnswers::default(),
        },
    };
    let answered = !answers.is_empty();

    let suspension = existing
        .as_ref()
        .and_then(|record| record.suspension.clone());
    let waiting_on = pending_request(log.store(), log.run_id())?;
    if let Some(suspension) = &suspension {
        match &suspension.hold {
            Hold::Retry(retry) => {
                let Some(answer) = answers.get(&retry.id) else {
                    return Ok(needs_agent_output(
                        manifest,
                        &runner.name,
                        log.run_id(),
                        &retry.id,
                        retry.value.clone(),
                    ));
                };
                if answers.request_digest(&retry.id) != Some(retry.digest.as_str())
                    || answer
                        != &JsonValue::Object(JsonObject::from([(
                            "retry".to_owned(),
                            JsonValue::Bool(true),
                        )]))
                {
                    return Err(invalid(
                        "native retry requires the exact digest-bound acknowledgement",
                    ));
                }
            }
            // Nothing new to continue with: the run still waits on the same
            // request, which only an answer or a managed agent can settle.
            Hold::Request(_) | Hold::Child(_)
                if !answered && !request.managed_agent.is_inline() =>
            {
                if let Some(waiting) = &waiting_on {
                    return Ok(needs_agent_output(
                        manifest,
                        &runner.name,
                        log.run_id(),
                        &waiting.id,
                        waiting.value.clone(),
                    ));
                }
            }
            Hold::Request(_) | Hold::Child(_) => {}
        }
    }

    let skill_dir = context.skill_dir.to_path_buf();
    let mut env = workspace.skill_env_for_skill(&skill_dir);
    env.insert(RUNX_RUN_ID_ENV.to_owned(), log.run_id().to_owned());
    let receipt_path = workspace.receipt_path(request.receipt_dir.as_deref());
    env.insert(
        crate::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
        receipt_path.path.to_string_lossy().into_owned(),
    );
    let inline_resolver = InlineResolver {
        skill_directory: skill_dir.clone(),
        env: env.clone(),
        effects: effects.clone(),
        observed_at: created_at.clone(),
        policy: request.managed_agent.clone(),
    };
    let options = RuntimeOptions {
        current_context: run_context.current,
        historical_context: run_context.historical,
        run_authority,
        created_at: created_at.clone(),
        env,
        receipt_signature: receipts.signature_config().clone(),
        effects: effects.clone(),
        run: log.clone(),
    };
    let runtime = match &overrides.javascript {
        Some(javascript) => Runtime::with_native_services(
            ExecutableAdapter,
            options,
            javascript.clone(),
            crate::services::LocalArtifactService::default(),
        ),
        None => Runtime::new(ExecutableAdapter, options),
    };
    let graph = materialize_graph_parameter_inputs(runner.graph.clone(), &graph_inputs);
    let mut host = SkillRunGraphHost::with_inline(answers, inline_resolver);
    host.expected_pending = waiting_on.map(|waiting| PendingSkillRequest {
        id: waiting.id,
        value: waiting.value,
    });
    let run = Driven {
        context,
        log: &log,
        runtime: &runtime,
        graph: &graph,
        graph_inputs: &graph_inputs,
        created_at: &created_at,
    };
    match runtime.run_graph_with_host(&skill_dir, graph.clone(), &mut host) {
        Ok(completed) => run.seal_completed(completed),
        Err(RuntimeError::ResolutionPending { request, .. }) => {
            let value = serde_json::to_value(request.as_ref())
                .and_then(serde_json::from_value::<JsonValue>)
                .map_err(|source| RuntimeError::json("encoding a pending request", source))?;
            let id = resolution_request_id(&request).to_owned();
            Ok(needs_agent_output(
                manifest,
                &runner.name,
                log.run_id(),
                &id,
                value,
            ))
        }
        Err(RuntimeError::StepClosed {
            step_id,
            disposition,
            reason_code,
            summary,
        }) => {
            let summary = format!(
                "graph {} {} at {step_id}: {summary}",
                graph.name,
                disposition.label()
            );
            run.seal_stopped(&step_id, &reason_code, summary, Stop::Closed(disposition))
        }
        Err(RuntimeError::GraphBlocked { step_id, reason }) => run.seal_stopped(
            &step_id,
            "graph_blocked",
            format!("graph {} blocked at {step_id}: {reason}", graph.name),
            Stop::Blocked,
        ),
        Err(RuntimeError::AuthorityDenied {
            verb,
            step_id,
            reason,
        }) => run.seal_stopped(
            &step_id,
            "authority_denied",
            format!(
                "graph {} denied {verb:?} at {step_id}: {reason}",
                graph.name
            ),
            Stop::Blocked,
        ),
        Err(error @ RuntimeError::ProviderReadbackPending { .. }) => {
            let step_id = error
                .graph_step_id()
                .unwrap_or("provider-readback")
                .to_owned();
            let retries = existing.as_ref().map_or(0, |record| record.retries) + 1;
            if retries <= MAX_READBACK_RETRIES {
                let id = format!("provider-readback-{step_id}-{retries}");
                let retry = ResolutionRequest::Retry {
                    id: id.clone().into(),
                    reason: "Independent provider readback is pending".into(),
                    retry_after_seconds: 300,
                };
                log.record_retry(&step_id, &retry)?;
                let value = serde_json::to_value(&retry)
                    .and_then(serde_json::from_value::<JsonValue>)
                    .map_err(|source| {
                        RuntimeError::json("encoding native readback retry", source)
                    })?;
                return Ok(needs_agent_output(
                    manifest,
                    &runner.name,
                    log.run_id(),
                    &id,
                    value,
                ));
            }
            let summary = format!(
                "graph {} deferred at {step_id}: the provider mutation may be applied, but independent readback is unavailable; reconcile before retrying",
                graph.name
            );
            run.seal_stopped(
                &step_id,
                "provider_readback_pending",
                summary,
                Stop::Deferred(error),
            )
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
            run.seal_stopped(&step_id, &reason_code, summary, Stop::Failed(error))
        }
        Err(error) if !error.is_fatal_step_fault() => {
            let step_id =
                crate::execution::runner::failed_step_id(&graph, &error).ok_or_else(|| {
                    RuntimeError::EngineInvariant {
                        context: "sealing graph step failure",
                        message: format!("sealable graph error has no authoritative step: {error}"),
                    }
                })?;
            let summary = format!("graph {} failed at {step_id}: {error}", graph.name);
            run.seal_stopped(&step_id, "graph_step_failed", summary, Stop::Failed(error))
        }
        Err(error) => Err(error.into()),
    }
}

/// The run a request names: its pinned id, or a new one.
fn run_log(
    request: &SkillRunRequest,
    workspace: &WorkspaceEnv,
    skill: &str,
) -> Result<RunLog, SkillRunError> {
    let run_id = match (&request.run_id, &request.answers_path) {
        // A caller-pinned id is a submission identity: a repeated submission
        // continues the same run, and hosted execution keys its effects on
        // it, so a retry recovers the same mutation instead of minting one.
        (Some(run_id), _) => crate::execution::run_record::validate_run_id(run_id)?.to_owned(),
        (None, Some(_)) => {
            return Err(invalid(
                "skill continuation requires both run_id and answers",
            ));
        }
        (None, None) => crate::execution::run_record::mint_run_id(skill)?,
    };
    let store = KernelStore::for_project(workspace.paths())
        .map_err(crate::execution::run_record::RunRecordError::from)
        .map_err(RuntimeError::from)?;
    Ok(RunLog::open(store, run_id).map_err(RuntimeError::from)?)
}

/// A continuation must present the identity its run was started with. A
/// request that leaves inputs or context empty continues with the recorded
/// ones.
fn continuation_identity<'a>(
    record: &'a RunRecord,
    request: &SkillRunRequest,
    request_context: &RunContext,
    runner: &SkillRunnerDefinition,
    package_digest: &str,
    execution_closure_digest: &str,
) -> Result<&'a crate::execution::run_record::Started, SkillRunError> {
    let Origin::Run(started) = &record.origin else {
        return Err(invalid("a child run continues only through its parent"));
    };
    let refuse = |field: &str| {
        invalid(format!(
            "run identity cannot replace its original {field}: a continuation runs under the identity the run started with"
        ))
    };
    if started.runner != runner.name {
        return Err(refuse("runner"));
    }
    if started.package_digest != package_digest {
        return Err(refuse("package digest"));
    }
    if started.execution_closure_digest != execution_closure_digest {
        return Err(refuse("execution closure"));
    }
    let request_inputs = request
        .inputs
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<JsonObject>();
    if !request_inputs.is_empty() && request_inputs != record.inputs {
        return Err(refuse("inputs"));
    }
    if (!request_context.current.is_empty() || !request_context.historical.is_empty())
        && request_context != &record.context
    {
        return Err(refuse("agent context"));
    }
    if request.run_authority.is_some() && request.run_authority != started.run_authority {
        return Err(refuse("run authority"));
    }
    Ok(started)
}

/// A sealed run returns what it sealed, from its record alone.
fn restore_sealed_run(
    context: &SkillExecutionContext<'_>,
    log: &RunLog,
    record: RunRecord,
) -> Result<RunResult, SkillRunError> {
    let sealed = record
        .sealed
        .clone()
        .ok_or_else(|| invalid("the run has no seal"))?;
    let graph = materialize_graph_parameter_inputs(context.runner.graph.clone(), &record.inputs);
    let run = sealed_graph_run(graph, record)?;
    write_graph_receipts(context.request, context.workspace, context.receipts, &run)?;
    if let Some(primary) = &sealed.primary {
        write_skill_receipt(
            context.request,
            context.workspace,
            context.receipts,
            primary,
        )?;
    }
    let result = match sealed.terminal_result {
        Some(result) => result,
        None => graph_run_result(&run)?,
    };
    let output = graph_run_skill_output(&result, &run)?;
    Ok(sealed_output(
        context.manifest,
        &context.runner.name,
        log.run_id(),
        &output,
        &result,
        SkillOutputDiagnostics {
            context: Some(graph_run_context(&run)),
            trace: Some(graph_run_trace(&run)),
        },
        sealed.primary.as_ref().unwrap_or(&run.receipt),
    ))
}

/// Why a run stopped short of completion.
enum Stop {
    Blocked,
    Deferred(RuntimeError),
    /// A step sealed its own governed closure (an agent act that declined,
    /// deferred, blocked, or was superseded). The run stops with that
    /// disposition and returns the step's declared outputs.
    Closed(ClosureDisposition),
    Failed(RuntimeError),
}

/// A run being driven, and what sealing it needs.
struct Driven<'a> {
    context: &'a SkillExecutionContext<'a>,
    log: &'a RunLog,
    runtime: &'a Runtime<ExecutableAdapter>,
    graph: &'a ExecutionGraph,
    graph_inputs: &'a JsonObject,
    created_at: &'a str,
}

impl Driven<'_> {
    fn seal_completed(&self, run: GraphRun) -> Result<RunResult, SkillRunError> {
        let context = self.context;
        let result = graph_run_result(&run)?;
        // A runner that declares an `act:` block presents a clean domain-act
        // receipt; the step receipts remain as its execution trace.
        let domain = graph_domain_act_receipt(
            context.runner,
            self.graph_inputs,
            &run,
            self.log.run_id(),
            self.created_at,
            context.receipts.signature_config(),
        )?;
        self.log
            .record_sealed(&run.receipt, domain.as_ref(), None)?;
        write_graph_receipts(context.request, context.workspace, context.receipts, &run)?;
        if let Some(domain) = &domain {
            write_skill_receipt(context.request, context.workspace, context.receipts, domain)?;
        }
        let output = graph_run_skill_output(&result, &run)?;
        Ok(sealed_output(
            context.manifest,
            &context.runner.name,
            self.log.run_id(),
            &output,
            &result,
            SkillOutputDiagnostics {
                context: Some(graph_run_context(&run)),
                trace: Some(graph_run_trace(&run)),
            },
            domain.as_ref().unwrap_or(&run.receipt),
        ))
    }

    fn seal_stopped(
        &self,
        step_id: &str,
        reason_code: &str,
        summary: String,
        stop: Stop,
    ) -> Result<RunResult, SkillRunError> {
        let context = self.context;
        let mut host = SkillRunGraphHost::new(ResolutionAnswers::default());
        let failure_result = match &stop {
            Stop::Deferred(error) | Stop::Failed(error) => {
                Some(JsonValue::Object(error.public_failure_projection()))
            }
            Stop::Blocked | Stop::Closed(_) => None,
        };
        let (disposition, failure) = match stop {
            Stop::Blocked => (ClosureDisposition::Blocked, None),
            Stop::Deferred(_) => (ClosureDisposition::Deferred, None),
            Stop::Closed(disposition) => (disposition, None),
            Stop::Failed(error) => (ClosureDisposition::Failed, Some(error)),
        };
        let closure = crate::receipts::GraphClosure {
            disposition,
            reason_code: reason_code.to_owned(),
            summary,
        };
        let run = match failure {
            Some(error) => self.runtime.seal_failed_with_host(
                self.graph.clone(),
                step_id,
                error,
                closure,
                &mut host,
            )?,
            None => self.runtime.seal_stopped_with_host(
                self.graph.clone(),
                step_id,
                closure,
                &mut host,
            )?,
        };
        let result = match failure_result {
            Some(result) => result,
            None => crate::execution::runner::graph_terminal_result(&run)?,
        };
        self.log.record_sealed(&run.receipt, None, Some(&result))?;
        write_graph_receipts(context.request, context.workspace, context.receipts, &run)?;
        let output = graph_run_skill_output(&result, &run)?;
        Ok(sealed_output(
            context.manifest,
            &context.runner.name,
            self.log.run_id(),
            &output,
            &result,
            SkillOutputDiagnostics {
                context: Some(graph_run_context(&run)),
                trace: Some(graph_run_trace(&run)),
            },
            &run.receipt,
        ))
    }
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

/// The host a skill run answers its requests through: the answers the run
/// holds, then the managed agent when the run consents to one. Anything else
/// suspends the run on the request.
struct SkillRunGraphHost {
    /// The request the run was waiting on; a continuation must ask it again
    /// unchanged.
    expected_pending: Option<PendingSkillRequest>,
    answers: ResolutionAnswers,
    inline: Option<InlineResolver>,
}

impl SkillRunGraphHost {
    fn new(answers: ResolutionAnswers) -> Self {
        Self {
            answers,
            expected_pending: None,
            inline: None,
        }
    }

    fn with_inline(answers: ResolutionAnswers, inline: InlineResolver) -> Self {
        Self {
            answers,
            expected_pending: None,
            inline: Some(inline),
        }
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
        if let Some(expected) = &self.expected_pending
            && expected.id == request_id
        {
            let value = serde_json::to_value(&request)
                .and_then(serde_json::from_value::<JsonValue>)
                .map_err(|source| RuntimeError::json("binding resumed request", source))?;
            if value != expected.value {
                return Err(RuntimeError::ContinuationRejected {
                    reason: "continuation cannot replace the pending request or its context"
                        .to_owned(),
                });
            }
        }
        if let Some(answer) = self.answers.get(&request_id) {
            if self.answers.requires_request_digest(&request_id) {
                let supplied = self.answers.request_digest(&request_id).ok_or_else(|| {
                    RuntimeError::ContinuationRejected {
                        reason: format!(
                            "request_digests did not include pending request {request_id}"
                        ),
                    }
                })?;
                let current = crate::execution::run_record::request_digest(&request)?;
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
        | ResolutionRequest::AgentAct { id, .. }
        | ResolutionRequest::Retry { id, .. } => id.as_str(),
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
// build-and-seal sequence.
pub(crate) fn graph_domain_act_receipt(
    runner: &SkillRunnerDefinition,
    graph_inputs: &JsonObject,
    run: &GraphRun,
    run_id: &str,
    created_at: &str,
    signature_config: &RuntimeReceiptSignatureConfig,
) -> Result<Option<runx_contracts::Receipt>, SkillRunError> {
    let Some(act) = runner.act.as_ref() else {
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
    let verification_metadata = step_output(act.reason_step.as_deref())
        .map(|step| step.outcome.metadata.clone())
        .unwrap_or_default();
    let receipt = domain_act_receipt(DomainActReceiptRequest {
        scope: RunScope {
            run_id,
            graph_name: &run.graph.name,
        },
        succeeded: run.state.status == runx_core::state_machine::GraphStatus::Succeeded,
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

    use super::*;
    use crate::adapter::SkillAdapter;

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
            crate::execution::runner::failed_step_id(&graph, &owned).as_deref(),
            Some("apply")
        );
        assert_eq!(
            crate::execution::runner::failed_step_id(&graph, &nested),
            None
        );
        Ok(())
    }

    #[test]
    fn the_executable_adapter_refuses_an_agent_act() {
        let invocation = SkillInvocation {
            historical_context: Vec::new(),
            skill_name: "fixture".to_owned(),
            step_id: None,
            artifacts: None,
            allowed_tools: None,
            requirements: Default::default(),
            executable: crate::adapter::Executable::Agent(runx_parser::AgentTask {
                role: "reviewer".to_owned(),
                task: "review".to_owned(),
            }),
            outputs: None,
            inputs: JsonObject::new(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: PathBuf::from("."),
            env: BTreeMap::new(),
            credential_delivery: crate::credentials::CredentialDelivery::none(),
        };

        let result = ExecutableAdapter.invoke(invocation);
        assert!(
            matches!(
                &result,
                Err(RuntimeError::UnsupportedExecutable { kind }) if kind == "agent"
            ),
            "unexpected agent act result: {result:?}"
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
        use crate::execution::run_record::{RunFilter, RunState, list_runs};
        use crate::receipts::archive::{ReceiptFilter, list_receipts};
        use runx_core::state_machine::GraphStatus;
        use runx_parser::{parse_graph_yaml, validate_graph};

        for suspended in [false, true] {
            let temp = tempfile::tempdir()?;
            let receipt_dir = temp.path().join("receipts");
            let env = BTreeMap::from([(
                crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
                temp.path().join(".runx").display().to_string(),
            )]);
            let graph = validate_graph(parse_graph_yaml(
                r#"
name: managed-agent-graph-failure
steps:
  - id: compose
    skill: ./compose
"#,
            )?)?;
            let store =
                KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, temp.path()))?;
            let log = RunLog::open(store, "run_managed_agent_failure")?;
            log.begin(StartRequest {
                skill: "managed-agent-graph-failure",
                skill_ref: "skill",
                runner: "default",
                package_digest: "sha256:package",
                execution_closure_digest: "sha256:closure",
                inputs: &JsonObject::new(),
                context: &RunContext::default(),
                run_authority: None,
                created_at: "2026-07-24T00:00:00Z",
            })?;
            if suspended {
                log.record_request(
                    "compose",
                    1,
                    &ResolutionRequest::Retry {
                        id: "agent.compose.output".into(),
                        reason: "the turn is open".into(),
                        retry_after_seconds: 1,
                    },
                )?;
            }
            let mut options = RuntimeOptions::local_development(env.clone(), log.clone());
            options.created_at = "2026-07-24T00:00:00Z".to_owned();
            let runtime = Runtime::new(ExecutableAdapter, options);
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
                "agent.compose.output",
                error,
            )
            .at_graph_step("compose");
            let mut host = SkillRunGraphHost::new(ResolutionAnswers::default());
            let run = runtime.seal_failed_with_host(
                graph,
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

            log.record_sealed(&run.receipt, None, None)?;
            let runs = list_runs(&env, temp.path(), &RunFilter::default())?;
            assert_eq!(runs.runs.len(), 1);
            assert_eq!(runs.runs[0].state, RunState::Sealed);
            assert_eq!(runs.runs[0].disposition, Some(ClosureDisposition::Failed));
            assert_eq!(runs.runs[0].pending_request_id, None);

            let request = SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: None,
                skill_path: temp.path().join("skill"),
                receipt_dir: Some(receipt_dir.clone()),
                run_id: None,
                answers_path: None,
                inputs: Default::default(),
                env: Default::default(),
                cwd: temp.path().to_path_buf(),
                managed_agent: Default::default(),
            };
            let workspace = WorkspaceEnv::new(Default::default(), temp.path().to_path_buf())?;
            let receipts = ReceiptServices::from_env_or_local_development(workspace.env())?;
            write_graph_receipts(&request, &workspace, &receipts, &run)?;
            let store = LocalReceiptStore::new(&receipt_dir);
            let listing = list_receipts(
                &store,
                workspace.paths(),
                &ReceiptFilter::default(),
                crate::receipts::RuntimeReceiptSignaturePolicy::local_development(),
            )?;
            assert_eq!(listing.receipts.len(), 1);
            assert!(
                listing
                    .receipts
                    .iter()
                    .all(|receipt| receipt.status == "failed")
            );
            let with_steps = list_receipts(
                &store,
                workspace.paths(),
                &ReceiptFilter {
                    include_internal: true,
                    ..ReceiptFilter::default()
                },
                crate::receipts::RuntimeReceiptSignaturePolicy::local_development(),
            )?;
            assert_eq!(with_steps.receipts.len(), 2);
        }
        Ok(())
    }

    #[cfg(feature = "process-tool")]
    #[test]
    fn graph_process_uses_structured_credential_redaction() -> Result<(), Box<dyn std::error::Error>>
    {
        const MARKER: &str = "cli-credential-redaction-sentinel";
        let workspace = tempfile::tempdir()?;
        let mut env: BTreeMap<String, String> = std::env::vars().collect();
        env.insert(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.path().to_string_lossy().into_owned(),
        );
        env.remove(crate::paths::RUNX_PROJECT_DIR_ENV);
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
            historical_context: Vec::new(),
            skill_name: "credential-observation".to_owned(),
            step_id: None,
            artifacts: None,
            allowed_tools: None,
            requirements: Default::default(),
            executable: crate::adapter::Executable::Process(runx_parser::ProcessTool {
                command: "/bin/sh".to_owned(),
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
            }),
            outputs: None,
            inputs: JsonObject::new(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: workspace.path().to_path_buf(),
            env,
            credential_delivery: delivery,
        };

        let output = ExecutableAdapter.invoke(invocation)?;

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
            RunScope {
                run_id: "run_credential",
                graph_name: "credential_graph",
            },
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
}
