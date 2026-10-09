//! Agent turns. While a run waits on an agent act, the agent calls back into
//! the run within the act's allowance. A call runs as one step under the
//! run's own runtime, so it is admitted, executed and sealed exactly as the
//! step kind it names would be, and it is recorded in the run's stream before
//! its outcome is returned. The agent ends the turn by answering the act.

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::connector::OperationName;
use runx_contracts::{AgentActInvocation, JsonObject, JsonValue, ResolutionRequest};
use runx_parser::{
    AskGate, GraphStep, OperationStepTarget, SkillRunnerManifest, SkillTarget, StepKind, ToolTarget,
};
use serde::{Deserialize, Serialize};

use super::super::graph::{StepSkillLoadOptions, load_step_skill};
use super::super::run_record::{
    CallOutcome, CallRecord, Hold, PendingRequest, RecordedCall, RunRecord, call_step_id,
    request_digest,
};
use super::Runtime;
use super::graph_engine::AnsweringHost;
use super::step_handlers::{
    StepRunRequest, run_step_with_inputs, run_step_with_loaded_skill_inputs,
};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::host::Host;

/// What a call names: one entry of the turn's allowance, or a question for
/// the human, which needs none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TurnTarget {
    Tool {
        tool: String,
    },
    Operation {
        requirement: String,
        operation: String,
    },
    Skill {
        skill: String,
    },
    /// The call's input is `{reason}`; the human approves or declines it.
    Ask,
}

/// One call: what it names and its input. An operation's input is its call
/// fields: `input`, and `idempotency_key` for a write, and `amount` where the
/// operation takes one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnCall {
    pub target: TurnTarget,
    #[serde(default)]
    pub input: JsonObject,
}

const OPERATION_CALL_FIELDS: &[&str] = &["input", "idempotency_key", "amount"];

/// What a driver does with an open turn: make a call, or read where it stands.
#[derive(Clone, Debug, PartialEq)]
pub enum TurnAction {
    Call(TurnCall),
    Status,
}

/// The agent act a run waits on, with the calls made in it so far.
#[derive(Clone, Debug)]
pub(crate) struct OpenTurn {
    pub step_id: String,
    pub attempt: u32,
    pub request: PendingRequest,
    pub invocation: AgentActInvocation,
    pub calls: Vec<RecordedCall>,
}

impl OpenTurn {
    /// The turn `record` waits on; `None` when it waits on anything else.
    pub(crate) fn of(record: &RunRecord) -> Result<Option<Self>, RuntimeError> {
        let Some(suspension) = &record.suspension else {
            return Ok(None);
        };
        let Hold::Request(request) = &suspension.hold else {
            return Ok(None);
        };
        let parsed: ResolutionRequest = serde_json::to_value(&request.value)
            .and_then(serde_json::from_value)
            .map_err(|source| RuntimeError::json("reading the run's pending request", source))?;
        let ResolutionRequest::AgentAct { invocation, .. } = parsed else {
            return Ok(None);
        };
        Ok(Some(Self {
            calls: record
                .calls
                .iter()
                .filter(|call| {
                    call.step_id == suspension.step_id && call.attempt == suspension.attempt
                })
                .cloned()
                .collect(),
            step_id: suspension.step_id.clone(),
            attempt: suspension.attempt,
            request: request.clone(),
            invocation: *invocation,
        }))
    }

    /// The index the next new call takes.
    fn next_index(&self) -> u32 {
        self.calls
            .iter()
            .map(|call| call.index + 1)
            .max()
            .unwrap_or(0)
    }

    /// Why the turn's allowance refuses `target`, if it does.
    fn refusal(&self, target: &TurnTarget) -> Option<String> {
        let allowance = &self.invocation.allowance;
        let allowed = match target {
            TurnTarget::Tool { tool } => allowance.tools.iter().any(|item| item == tool),
            TurnTarget::Operation {
                requirement,
                operation,
            } => allowance.operations.iter().any(|item| {
                item.requirement == requirement.as_str() && item.operation == operation.as_str()
            }),
            TurnTarget::Skill { skill } => allowance.skills.iter().any(|item| item == skill),
            TurnTarget::Ask => true,
        };
        (!allowed).then(|| format!("{} is not in the turn's allowance", target.label()))
    }
}

impl TurnTarget {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Tool { tool } => format!("tool {tool}"),
            Self::Operation {
                requirement,
                operation,
            } => format!("operation {requirement}/{operation}"),
            Self::Skill { skill } => format!("skill {skill}"),
            Self::Ask => "ask".to_owned(),
        }
    }
}

/// What a call runs against: the run's runtime and its skill.
pub(crate) struct TurnCallContext<'a, A> {
    pub runtime: &'a Runtime<A>,
    pub graph_dir: &'a Path,
    pub graph_name: &'a str,
    pub manifest: &'a SkillRunnerManifest,
}

/// Make a new call in `turn`, record it, and return how it ended.
pub(crate) fn call<A>(
    context: &TurnCallContext<'_, A>,
    turn: &OpenTurn,
    call: &TurnCall,
    host: &mut dyn Host,
) -> Result<(u32, CallOutcome), RuntimeError>
where
    A: SkillAdapter,
{
    let index = turn.next_index();
    let outcome = run_and_record(context, turn, index, call, host)?;
    Ok((index, outcome))
}

/// Run again each of `turn`'s pending calls whose request the host has now
/// answered. Each is recorded under its own index, settling it.
pub(crate) fn settle_answered<A>(
    context: &TurnCallContext<'_, A>,
    turn: &OpenTurn,
    answered: impl Fn(&str) -> bool,
    host: &mut dyn Host,
) -> Result<Vec<(u32, CallOutcome)>, RuntimeError>
where
    A: SkillAdapter,
{
    let mut settled = Vec::new();
    for recorded in &turn.calls {
        let CallOutcome::Pending(request) = &recorded.outcome else {
            continue;
        };
        if !answered(&request.id) {
            continue;
        }
        let call = TurnCall {
            target: recorded.target.clone(),
            input: recorded.input.clone(),
        };
        let outcome = run_and_record(context, turn, recorded.index, &call, host)?;
        settled.push((recorded.index, outcome));
    }
    Ok(settled)
}

fn run_and_record<A>(
    context: &TurnCallContext<'_, A>,
    turn: &OpenTurn,
    index: u32,
    call: &TurnCall,
    host: &mut dyn Host,
) -> Result<CallOutcome, RuntimeError>
where
    A: SkillAdapter,
{
    let mut answering = AnsweringHost {
        host,
        answers: Vec::new(),
    };
    let outcome = match turn.refusal(&call.target) {
        Some(reason) => CallOutcome::Refused(reason),
        None => match call_step(context, &turn.step_id, index, call) {
            Ok(step) => run_call_step(context, &step, call, &mut answering)?,
            Err(reason) => CallOutcome::Refused(reason),
        },
    };
    context.runtime.options.run.record_call(&CallRecord {
        step_id: &turn.step_id,
        attempt: turn.attempt,
        turn: &turn.request,
        index,
        target: &call.target,
        input: &call.input,
        outcome: &outcome,
        answers: &answering.answers,
    })?;
    Ok(outcome)
}

fn run_call_step<A>(
    context: &TurnCallContext<'_, A>,
    step: &GraphStep,
    call: &TurnCall,
    host: &mut dyn Host,
) -> Result<CallOutcome, RuntimeError>
where
    A: SkillAdapter,
{
    let request = StepRunRequest {
        runtime: context.runtime,
        graph_dir: context.graph_dir,
        graph_name: context.graph_name,
        step,
        attempt: 1,
        inputs: call.input.clone(),
        provenance: Vec::new(),
        policy_approval_refs: Vec::new(),
        host,
    };
    let result = match &call.target {
        TurnTarget::Skill { .. } => load_step_skill(
            context.graph_dir,
            step,
            StepSkillLoadOptions {
                env: &context.runtime.options.env,
            },
        )
        .and_then(|skill| run_step_with_loaded_skill_inputs(request, skill)),
        _ => run_step_with_inputs(request),
    };
    Ok(match result {
        Ok(run) => CallOutcome::Completed(Box::new(run)),
        Err(RuntimeError::ResolutionPending { request, .. }) => {
            CallOutcome::Pending(pending_request(&request)?)
        }
        Err(RuntimeError::AuthorityDenied { reason, .. }) => CallOutcome::Refused(reason),
        // The act may have reached its provider: its custody holds it, and
        // the call says so instead of failing the turn.
        Err(error @ RuntimeError::ProviderEffectUnknown { .. })
        | Err(error @ RuntimeError::ProviderReadbackPending { .. }) => {
            CallOutcome::Failed(error.to_string())
        }
        Err(error) if error.is_fatal_step_fault() => return Err(error),
        Err(error) => CallOutcome::Failed(error.to_string()),
    })
}

/// The step a call runs as. A malformed call is refused before it runs.
fn call_step<A>(
    context: &TurnCallContext<'_, A>,
    turn_step: &str,
    index: u32,
    call: &TurnCall,
) -> Result<GraphStep, String> {
    let (kind, scopes) = match &call.target {
        TurnTarget::Tool { tool } => (
            StepKind::Tool(ToolTarget::Catalog(tool.clone())),
            declared_scopes(context, tool)?,
        ),
        TurnTarget::Operation {
            requirement,
            operation,
        } => {
            if let Some(field) = call
                .input
                .keys()
                .find(|field| !OPERATION_CALL_FIELDS.contains(&field.as_str()))
            {
                return Err(format!(
                    "an operation call takes input, idempotency_key and amount, not {field}"
                ));
            }
            let target = OperationStepTarget {
                requirement: requirement.clone(),
                operation: Some(
                    OperationName::new(operation.clone())
                        .ok_or_else(|| format!("{operation} is not an operation name"))?,
                ),
                declared: context.manifest.connectors.get(requirement).cloned(),
            };
            (StepKind::Operation(target), Vec::new())
        }
        TurnTarget::Skill { skill } => (
            StepKind::Skill(SkillTarget {
                reference: skill.clone(),
                runner: None,
            }),
            Vec::new(),
        ),
        TurnTarget::Ask => {
            let reason = call
                .input
                .get("reason")
                .and_then(JsonValue::as_str)
                .filter(|reason| !reason.trim().is_empty())
                .ok_or("an ask names its reason")?;
            if call.input.len() > 1 {
                return Err("an ask takes only its reason".to_owned());
            }
            (
                StepKind::Ask(AskGate {
                    gate: call_step_id(turn_step, index),
                    reason: reason.to_owned(),
                    gate_type: None,
                }),
                Vec::new(),
            )
        }
    };
    Ok(GraphStep {
        id: call_step_id(turn_step, index),
        label: Some(call.target.label()),
        kind,
        artifacts: None,
        outputs: None,
        inputs: call.input.clone(),
        context: BTreeMap::new(),
        context_edges: Vec::new(),
        context_skills: Vec::new(),
        scopes,
        when: None,
        idempotency_key: None,
        on_uncertain: None,
        duplicates: None,
    })
}

/// Listing a tool grants it the scopes it declares.
#[cfg(feature = "catalog")]
fn declared_scopes<A>(context: &TurnCallContext<'_, A>, tool: &str) -> Result<Vec<String>, String> {
    crate::tool_catalogs::dispatch::inspect_catalog_tool(
        tool,
        &context.runtime.options.env,
        context.graph_dir,
        &context.runtime.options.effects,
    )
    .map(|inspected| inspected.scopes)
    .map_err(|error| format!("tool {tool} could not be resolved: {error}"))
}

#[cfg(not(feature = "catalog"))]
fn declared_scopes<A>(_: &TurnCallContext<'_, A>, tool: &str) -> Result<Vec<String>, String> {
    Err(format!(
        "tool {tool} needs the tool catalog, which this runtime does not include"
    ))
}

fn pending_request(request: &ResolutionRequest) -> Result<PendingRequest, RuntimeError> {
    let value = serde_json::to_value(request)
        .and_then(serde_json::from_value::<JsonValue>)
        .map_err(|source| RuntimeError::json("encoding a call's pending request", source))?;
    let id = match request {
        ResolutionRequest::Input { id, .. }
        | ResolutionRequest::Approval { id, .. }
        | ResolutionRequest::AgentAct { id, .. }
        | ResolutionRequest::Retry { id, .. } => id.to_string(),
    };
    Ok(PendingRequest {
        id,
        value,
        digest: request_digest(request)?,
    })
}

/// The turn as the agent sees it: what it answers, what it may call, and how
/// each call it made ended.
pub(crate) fn status(run_id: &str, turn: &OpenTurn) -> Result<JsonValue, RuntimeError> {
    to_json(&TurnStatus {
        schema: "runx.turn.status.v1",
        run_id,
        step_id: &turn.step_id,
        request_id: &turn.request.id,
        allowance: &turn.invocation.allowance,
        calls: turn
            .calls
            .iter()
            .map(|call| CallView::of(call.index, &call.target, &call.outcome))
            .collect(),
    })
}

/// One call's outcome as the agent sees it.
pub(crate) fn outcome_value(
    index: u32,
    target: &TurnTarget,
    outcome: &CallOutcome,
) -> Result<JsonValue, RuntimeError> {
    to_json(&CallView::of(index, target, outcome))
}

#[derive(Serialize)]
struct TurnStatus<'a> {
    schema: &'static str,
    run_id: &'a str,
    step_id: &'a str,
    request_id: &'a str,
    allowance: &'a runx_contracts::TurnAllowance,
    calls: Vec<CallView<'a>>,
}

#[derive(Serialize)]
struct CallView<'a> {
    index: u32,
    target: &'a TurnTarget,
    #[serde(flatten)]
    outcome: OutcomeView<'a>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum OutcomeView<'a> {
    /// The call ran; a step that ran and failed reports `failed` with its
    /// receipt.
    Completed {
        output: &'a JsonObject,
        receipt_id: String,
    },
    Failed {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        receipt_id: Option<String>,
    },
    Refused {
        reason: &'a str,
    },
    Pending {
        request_id: &'a str,
        request: &'a JsonValue,
    },
}

impl<'a> CallView<'a> {
    fn of(index: u32, target: &'a TurnTarget, outcome: &'a CallOutcome) -> Self {
        let outcome = match outcome {
            CallOutcome::Completed(run) if run.outcome.succeeded() => OutcomeView::Completed {
                output: &run.contract,
                receipt_id: run.receipt.id.to_string(),
            },
            CallOutcome::Completed(run) => OutcomeView::Failed {
                reason: None,
                receipt_id: Some(run.receipt.id.to_string()),
            },
            CallOutcome::Refused(reason) => OutcomeView::Refused { reason },
            CallOutcome::Failed(reason) => OutcomeView::Failed {
                reason: Some(reason),
                receipt_id: None,
            },
            CallOutcome::Pending(request) => OutcomeView::Pending {
                request_id: &request.id,
                request: &request.value,
            },
        };
        Self {
            index,
            target,
            outcome,
        }
    }
}

fn to_json(value: &impl Serialize) -> Result<JsonValue, RuntimeError> {
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|source| RuntimeError::json("encoding a turn", source))
}
