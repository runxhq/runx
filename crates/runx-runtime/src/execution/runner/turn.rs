//! Agent turns. An agent step whose act is unanswered holds its turn open,
//! and its host works the turn: the agent calls back into the run within the
//! act's allowance. A call runs as one step under the agent step's own
//! runtime, at whatever depth the step runs, so it is admitted, executed and
//! sealed exactly as the step kind it names would be, and it is recorded in
//! the run's stream before its outcome is returned. The turn ends when the
//! host answers the act.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::connector::{AmountRule, OperationAccess, OperationName};
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    AgentActInvocation, AllowedOperation, JsonObject, JsonValue, ResolutionRequest,
    TurnCallOutcome, TurnCallResult, TurnStatus, TurnTarget,
};
use runx_parser::{
    AgentTask, AskGate, GraphStep, OperationStepTarget, SkillTarget, StepKind, ToolTarget,
};

use super::super::graph::{StepSkillLoadOptions, load_step_skill};
use super::super::run_record::{
    Answer, CallOutcome, CallRecord, PendingRequest, RecordedCall, call_step_id, request_digest,
};
use super::graph_engine::AnsweringHost;
use super::step_handlers::{
    StepRunRequest, output_error, run_step_with_inputs, run_step_with_loaded_skill_inputs,
};
use super::{Runtime, StepRun};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::host::Host;

/// The turn an unanswered agent act holds open, as its host works it.
pub trait Turn {
    /// The act the turn answers.
    fn invocation(&self) -> &AgentActInvocation;

    /// The agent step that holds the turn.
    fn step_id(&self) -> &str;

    /// The tools the turn offers its agent, named for MCP: one per allowance
    /// entry, then `ask_human`, `turn_status` and `turn_complete`.
    fn tools(&self) -> Result<Vec<TurnTool>, RuntimeError>;

    /// What the turn answers, what it may call, and how each call ended.
    fn status(&self) -> Result<TurnStatus, RuntimeError>;

    /// Make a new call. `host` answers what the call itself asks; a call it
    /// cannot answer is pending until it can.
    fn call(
        &mut self,
        host: &mut dyn Host,
        target: TurnTarget,
        input: JsonObject,
    ) -> Result<TurnCallResult, RuntimeError>;

    /// Run the pending call `index` again under `host`, which now answers
    /// what it waits on.
    fn settle(&mut self, host: &mut dyn Host, index: u32) -> Result<TurnCallResult, RuntimeError>;
}

/// One tool an open turn offers its agent.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnTool {
    pub name: String,
    pub description: String,
    pub input_schema: JsonObject,
    pub action: TurnToolAction,
}

/// What calling a turn tool does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnToolAction {
    Call(TurnTarget),
    Status,
    Complete,
}

/// The tool that ends a turn with the act's output.
pub(crate) const TURN_COMPLETE_TOOL: &str = "turn_complete";

const OPERATION_CALL_FIELDS: &[&str] = &["connector", "input", "idempotency_key", "amount"];

/// Where a turn runs: the agent step, its attempt, and its runtime.
pub(super) struct TurnScope<'a, A> {
    pub(super) runtime: &'a Runtime<A>,
    pub(super) graph_dir: &'a Path,
    pub(super) graph_name: &'a str,
    pub(super) step_id: &'a str,
    pub(super) attempt: u32,
    pub(super) agent: &'a AgentTask,
}

/// The open turn of one agent step: where it runs, the act it answers, and
/// the calls made in it so far.
pub(super) struct OpenTurn<'a, A> {
    scope: TurnScope<'a, A>,
    invocation: &'a AgentActInvocation,
    /// The act as the run records it.
    request: PendingRequest,
    /// The calls the run records for this turn, read when first needed.
    calls: OnceCell<Vec<RecordedCall>>,
}

impl<'a, A> OpenTurn<'a, A>
where
    A: SkillAdapter,
{
    /// The turn `request` opens, with the calls the run records for it.
    pub(super) fn open(
        scope: TurnScope<'a, A>,
        request: &'a ResolutionRequest,
    ) -> Result<Self, RuntimeError> {
        let ResolutionRequest::AgentAct { invocation, .. } = request else {
            return Err(RuntimeError::EngineInvariant {
                context: "opening an agent turn",
                message: "an agent step asked for something other than its act".to_owned(),
            });
        };
        Ok(Self {
            scope,
            invocation,
            request: pending_request(request)?,
            calls: OnceCell::new(),
        })
    }

    /// The turn's calls, in order.
    fn calls(&self) -> Result<&[RecordedCall], RuntimeError> {
        if let Some(calls) = self.calls.get() {
            return Ok(calls);
        }
        let read = self
            .scope
            .runtime
            .options
            .run
            .calls(self.scope.step_id, self.scope.attempt)?;
        Ok(self.calls.get_or_init(|| read))
    }

    fn calls_mut(&mut self) -> Result<&mut Vec<RecordedCall>, RuntimeError> {
        self.calls()?;
        self.calls
            .get_mut()
            .ok_or_else(|| RuntimeError::EngineInvariant {
                context: "recording a turn call",
                message: "the turn's calls were not read".to_owned(),
            })
    }

    /// The runs of the turn's completed calls, in order.
    pub(super) fn completed(&self) -> Result<Vec<&StepRun>, RuntimeError> {
        Ok(self
            .calls()?
            .iter()
            .filter_map(|call| match &call.outcome {
                CallOutcome::Completed(run) => Some(run.as_ref()),
                _ => None,
            })
            .collect())
    }

    /// The first call still waiting on an answer: its index and request.
    pub(super) fn pending(&self) -> Result<Option<(u32, &str)>, RuntimeError> {
        Ok(self.calls()?.iter().find_map(|call| match &call.outcome {
            CallOutcome::Pending(request) => Some((call.index, request.id.as_str())),
            _ => None,
        }))
    }

    /// Why the turn's allowance refuses `target`, if it does.
    fn refusal(&self, target: &TurnTarget) -> Option<String> {
        let allowance = &self.scope.agent.allowance;
        let allowed = match target {
            TurnTarget::Tool { tool } => allowance.tools.contains(tool),
            TurnTarget::Operation {
                requirement,
                operation,
            } => allowance
                .operations
                .iter()
                .any(|item| &item.requirement == requirement && &item.operation == operation),
            TurnTarget::Skill { skill } => allowance.skills.contains(skill),
            TurnTarget::Ask => true,
        };
        (!allowed).then(|| format!("{} is not in the turn's allowance", label(target)))
    }

    /// Run the call at `index` through `host`, which carries what the call
    /// already consumed. A call outside the allowance, or malformed, is
    /// refused before it runs. Otherwise it is claimed first: its identity,
    /// its input and what authorized it enter the run's record under the
    /// version this driver last read, so a second driver of the turn is
    /// refused before anything runs. A claimed call that does not settle,
    /// because its effect is unconfirmed or its driver stopped, waits on its
    /// own retry.
    fn run(
        &mut self,
        mut host: AnsweringHost<'_>,
        index: u32,
        target: TurnTarget,
        input: JsonObject,
    ) -> Result<TurnCallResult, RuntimeError> {
        let step = match self
            .refusal(&target)
            .map_or_else(|| self.call_step(index, &target, &input), Err)
        {
            Ok(step) => step,
            Err(reason) => {
                let refused = CallOutcome::Refused(reason);
                return self.settled(index, target, input, refused, &host.answers);
            }
        };
        let claim = CallOutcome::Pending(self.claim(index)?);
        let authorized = std::mem::take(&mut host.answers);
        self.record(
            RecordedCall {
                step_id: self.scope.step_id.to_owned(),
                attempt: self.scope.attempt,
                index,
                target: target.clone(),
                input: input.clone(),
                outcome: claim.clone(),
            },
            &authorized,
        )?;
        match self.run_call_step(&step, &target, &input, &mut host)? {
            Some(outcome) => self.settled(index, target, input, outcome, &host.answers),
            None => call_result(index, &target, &claim),
        }
    }

    /// The first call claimed but not settled.
    fn unsettled(&self) -> Result<Option<u32>, RuntimeError> {
        Ok(self.calls()?.iter().find_map(|call| match &call.outcome {
            CallOutcome::Pending(request)
                if request.id == retry_id(self.scope.step_id, call.index) =>
            {
                Some(call.index)
            }
            _ => None,
        }))
    }

    /// What a claimed call waits on until it settles: the host's
    /// acknowledgement to run it again with the same input.
    fn claim(&self, index: u32) -> Result<PendingRequest, RuntimeError> {
        pending_request(&ResolutionRequest::Retry {
            id: retry_id(self.scope.step_id, index).into(),
            reason: format!(
                "call {index} has not settled and may have run; acknowledge with {{\"retry\": true}} to run it again with the same input"
            )
            .into(),
            retry_after_seconds: 0,
        })
    }

    /// Record how a call ended, and return it as its agent reads it.
    fn settled(
        &mut self,
        index: u32,
        target: TurnTarget,
        input: JsonObject,
        outcome: CallOutcome,
        answers: &[Answer],
    ) -> Result<TurnCallResult, RuntimeError> {
        let result = call_result(index, &target, &outcome)?;
        self.record(
            RecordedCall {
                step_id: self.scope.step_id.to_owned(),
                attempt: self.scope.attempt,
                index,
                target,
                input,
                outcome,
            },
            answers,
        )?;
        Ok(result)
    }

    /// Record a call's latest word on its index.
    fn record(&mut self, call: RecordedCall, answers: &[Answer]) -> Result<(), RuntimeError> {
        self.scope.runtime.options.run.record_call(&CallRecord {
            step_id: self.scope.step_id,
            attempt: self.scope.attempt,
            turn: &self.request,
            index: call.index,
            target: &call.target,
            input: &call.input,
            outcome: &call.outcome,
            answers,
        })?;
        let calls = self.calls_mut()?;
        match calls.iter_mut().find(|known| known.index == call.index) {
            Some(known) => *known = call,
            None => calls.push(call),
        }
        Ok(())
    }

    /// Run a call as its step. `None`: the call has not settled.
    fn run_call_step(
        &self,
        step: &GraphStep,
        target: &TurnTarget,
        input: &JsonObject,
        host: &mut dyn Host,
    ) -> Result<Option<CallOutcome>, RuntimeError> {
        let scope = &self.scope;
        let request = StepRunRequest {
            runtime: scope.runtime,
            graph_dir: scope.graph_dir,
            graph_name: scope.graph_name,
            step,
            attempt: 1,
            inputs: input.clone(),
            provenance: Vec::new(),
            policy_approval_refs: Vec::new(),
            host,
        };
        let result = match target {
            TurnTarget::Skill { .. } => load_step_skill(
                scope.graph_dir,
                step,
                StepSkillLoadOptions {
                    env: &scope.runtime.options.env,
                },
            )
            .and_then(|skill| run_step_with_loaded_skill_inputs(request, skill)),
            _ => run_step_with_inputs(request),
        };
        Ok(Some(match result {
            Ok(run) => CallOutcome::Completed(Box::new(run)),
            Err(RuntimeError::ResolutionPending { request, .. }) => {
                CallOutcome::Pending(pending_request(&request)?)
            }
            Err(RuntimeError::AuthorityDenied { reason, .. }) => CallOutcome::Refused(reason),
            // The act may have reached its provider: its custody holds it, and
            // the call stays claimed until a retry with the same input
            // reconciles it.
            Err(RuntimeError::ProviderEffectUnknown { .. })
            | Err(RuntimeError::ProviderReadbackPending { .. }) => return Ok(None),
            Err(error) if error.is_fatal_step_fault() => return Err(error),
            Err(error) => CallOutcome::Failed(error.to_string()),
        }))
    }

    /// The step a call runs as. A malformed call is refused before it runs.
    fn call_step(
        &self,
        index: u32,
        target: &TurnTarget,
        input: &JsonObject,
    ) -> Result<GraphStep, String> {
        let step_id = call_step_id(self.scope.step_id, index);
        let (kind, scopes) = match target {
            TurnTarget::Tool { tool } => (
                StepKind::Tool(ToolTarget::Catalog(tool.as_str().to_owned())),
                self.declared_scopes(tool.as_str())?,
            ),
            TurnTarget::Operation {
                requirement,
                operation,
            } => {
                if let Some(field) = input
                    .keys()
                    .find(|field| !OPERATION_CALL_FIELDS.contains(&field.as_str()))
                {
                    return Err(format!(
                        "an operation call takes connector, input, idempotency_key and amount, not {field}"
                    ));
                }
                let target = OperationStepTarget {
                    requirement: requirement.as_str().to_owned(),
                    operation: Some(
                        OperationName::new(operation.as_str().to_owned())
                            .ok_or_else(|| format!("{operation} is not an operation name"))?,
                    ),
                    declared: self
                        .scope
                        .agent
                        .connectors
                        .get(requirement.as_str())
                        .cloned(),
                };
                (StepKind::Operation(target), Vec::new())
            }
            TurnTarget::Skill { skill } => (
                StepKind::Skill(SkillTarget {
                    reference: skill.as_str().to_owned(),
                    runner: None,
                }),
                Vec::new(),
            ),
            TurnTarget::Ask => {
                let reason = input
                    .get("reason")
                    .and_then(JsonValue::as_str)
                    .filter(|reason| !reason.trim().is_empty())
                    .ok_or("an ask names its reason")?;
                if input.len() > 1 {
                    return Err("an ask takes only its reason".to_owned());
                }
                (
                    StepKind::Ask(AskGate {
                        gate: step_id.clone(),
                        reason: reason.to_owned(),
                        gate_type: None,
                    }),
                    Vec::new(),
                )
            }
        };
        Ok(GraphStep {
            id: step_id,
            label: Some(label(target)),
            kind,
            artifacts: None,
            outputs: None,
            inputs: input.clone(),
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

    /// An operation's tool: the call fields the turn accepts, with the
    /// operation's own input schema when one connector serves it.
    fn operation_tool(
        &self,
        scope: &crate::kernel::connectors::ConnectorScope,
        allowed: &AllowedOperation,
    ) -> Result<TurnTool, RuntimeError> {
        let describing =
            |message: String| RuntimeError::effect_state("describing an operation", message);
        let declared = self
            .scope
            .agent
            .connectors
            .get(allowed.requirement.as_str())
            .ok_or_else(|| {
                describing(format!(
                    "requirement {} is not declared",
                    allowed.requirement
                ))
            })?;
        let connectors = if declared.connector_from.is_some() {
            declared
                .allowed_connectors
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>()
        } else {
            vec![allowed.requirement.as_str().to_owned()]
        };
        let mut described = Vec::new();
        for id in &connectors {
            let connector = scope
                .connector(id)
                .map_err(|error| describing(error.to_string()))?;
            let operation = connector
                .manifest
                .operations
                .iter()
                .find(|(name, _)| name.as_str() == allowed.operation.as_str())
                .map(|(_, operation)| operation.clone())
                .ok_or_else(|| {
                    describing(format!("{id} has no operation {}", allowed.operation))
                })?;
            described.push((id.clone(), operation));
        }
        let writes = described
            .iter()
            .any(|(_, operation)| operation.access() == OperationAccess::Mutate);
        let amount = described
            .iter()
            .any(|(_, operation)| operation.amount == AmountRule::Required);
        let input = match described.as_slice() {
            [(_, operation)] => operation.input_schema.clone().map_or_else(
                || JsonValue::Object(object_schema([], &[])),
                JsonValue::Object,
            ),
            _ => JsonValue::Object(JsonObject::from([(
                "type".to_owned(),
                JsonValue::String("object".to_owned()),
            )])),
        };
        let mut properties = vec![("input", input)];
        let mut required = vec!["input"];
        if writes {
            properties.push(("idempotency_key", string_schema()));
            required.push("idempotency_key");
        }
        if amount {
            properties.push((
                "amount",
                JsonValue::Object(object_schema(
                    [
                        (
                            "units",
                            JsonValue::Object(JsonObject::from([
                                ("type".to_owned(), JsonValue::String("integer".to_owned())),
                                ("minimum".to_owned(), JsonValue::Number(0_u64.into())),
                            ])),
                        ),
                        ("unit", string_schema()),
                    ],
                    &["units", "unit"],
                )),
            ));
            required.push("amount");
        }
        if declared.connector_from.is_some() {
            properties.push((
                "connector",
                JsonValue::Object(JsonObject::from([(
                    "enum".to_owned(),
                    JsonValue::Array(connectors.iter().cloned().map(JsonValue::String).collect()),
                )])),
            ));
            required.push("connector");
        }
        let mut input_schema = object_schema([], &required);
        input_schema.insert(
            "properties".to_owned(),
            JsonValue::Object(
                properties
                    .into_iter()
                    .map(|(name, schema)| (name.to_owned(), schema))
                    .collect(),
            ),
        );
        let description = described
            .iter()
            .map(|(id, operation)| format!("{id}/{}: {}", allowed.operation, operation.description))
            .collect::<Vec<_>>()
            .join("; ");
        let description = if writes {
            format!(
                "{description} A write: name it with an idempotency_key you reuse when retrying it."
            )
        } else {
            description
        };
        Ok(call_tool(
            &format!("{}__{}", allowed.requirement, allowed.operation),
            description,
            input_schema,
            TurnTarget::Operation {
                requirement: allowed.requirement.clone(),
                operation: allowed.operation.clone(),
            },
        ))
    }

    /// A child skill's tool: its description and its runner's inputs.
    fn skill_tool(&self, skill: &NonEmptyString) -> Result<TurnTool, RuntimeError> {
        let target = TurnTarget::Skill {
            skill: skill.clone(),
        };
        let step = self
            .call_step(0, &target, &JsonObject::new())
            .map_err(|reason| RuntimeError::effect_state("describing a child skill", reason))?;
        let loaded = load_step_skill(
            self.scope.graph_dir,
            &step,
            StepSkillLoadOptions {
                env: &self.scope.runtime.options.env,
            },
        )?;
        let description = crate::load_validated_skill_package(&loaded.directory)?
            .package
            .skill
            .description
            .unwrap_or_else(|| format!("Run the {} skill.", loaded.runner.name));
        Ok(call_tool(
            &format!("skill__{}", skill.as_str().trim_start_matches(['.', '/'])),
            description,
            runx_contracts::input_contract_schema(&loaded.runner.inputs),
            target,
        ))
    }

    /// A catalog tool's tool: its description and inputs as the catalog
    /// declares them.
    #[cfg(feature = "catalog")]
    fn catalog_tool(&self, tool: &NonEmptyString) -> Result<TurnTool, RuntimeError> {
        let inspected = crate::tool_catalogs::dispatch::inspect_catalog_tool(
            tool.as_str(),
            &self.scope.runtime.options.env,
            self.scope.graph_dir,
            &self.scope.runtime.options.effects,
        )?;
        Ok(call_tool(
            tool.as_str(),
            inspected
                .description
                .unwrap_or_else(|| format!("Runx tool {tool}.")),
            runx_contracts::input_contract_schema(&inspected.inputs),
            TurnTarget::Tool { tool: tool.clone() },
        ))
    }

    #[cfg(not(feature = "catalog"))]
    fn catalog_tool(&self, tool: &NonEmptyString) -> Result<TurnTool, RuntimeError> {
        Err(RuntimeError::UnsupportedAdapter {
            adapter_type: format!("catalog tool {tool}"),
        })
    }

    /// Listing a tool grants it the scopes it declares.
    #[cfg(feature = "catalog")]
    fn declared_scopes(&self, tool: &str) -> Result<Vec<String>, String> {
        crate::tool_catalogs::dispatch::inspect_catalog_tool(
            tool,
            &self.scope.runtime.options.env,
            self.scope.graph_dir,
            &self.scope.runtime.options.effects,
        )
        .map(|inspected| inspected.scopes)
        .map_err(|error| format!("tool {tool} could not be resolved: {error}"))
    }

    #[cfg(not(feature = "catalog"))]
    fn declared_scopes(&self, tool: &str) -> Result<Vec<String>, String> {
        Err(format!(
            "tool {tool} needs the tool catalog, which this runtime does not include"
        ))
    }
}

impl<A> Turn for OpenTurn<'_, A>
where
    A: SkillAdapter,
{
    fn invocation(&self) -> &AgentActInvocation {
        self.invocation
    }

    fn step_id(&self) -> &str {
        self.scope.step_id
    }

    fn tools(&self) -> Result<Vec<TurnTool>, RuntimeError> {
        let allowance = &self.scope.agent.allowance;
        let mut tools = Vec::new();
        for tool in &allowance.tools {
            tools.push(self.catalog_tool(tool)?);
        }
        if !allowance.operations.is_empty() {
            let connectors =
                crate::kernel::connectors::ConnectorScope::for_skill(self.scope.graph_dir)
                    .map_err(|error| {
                        RuntimeError::effect_state(
                            "reading the skill's connectors",
                            error.to_string(),
                        )
                    })?;
            for allowed in &allowance.operations {
                tools.push(self.operation_tool(&connectors, allowed)?);
            }
        }
        for skill in &allowance.skills {
            tools.push(self.skill_tool(skill)?);
        }
        tools.push(call_tool(
            "ask_human",
            "Ask the human to approve something; the call stays pending until they answer."
                .to_owned(),
            object_schema([("reason", string_schema())], &["reason"]),
            TurnTarget::Ask,
        ));
        tools.push(TurnTool {
            name: "turn_status".to_owned(),
            description: "Read the turn: its request, what it may call, and how each call ended."
                .to_owned(),
            input_schema: object_schema([], &[]),
            action: TurnToolAction::Status,
        });
        let envelope = &self.invocation.envelope;
        tools.push(TurnTool {
            name: TURN_COMPLETE_TOOL.to_owned(),
            description: "End the turn with its output; the run continues.".to_owned(),
            input_schema: match &envelope.output_schema {
                Some(JsonValue::Object(schema)) => schema.clone(),
                _ => runx_contracts::output_value_schema(envelope.output.as_ref())
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            },
            action: TurnToolAction::Complete,
        });
        distinct_names(&mut tools);
        Ok(tools)
    }

    fn status(&self) -> Result<TurnStatus, RuntimeError> {
        Ok(TurnStatus {
            step_id: self.scope.step_id.into(),
            request_id: self.request.id.as_str().into(),
            allowance: self.scope.agent.allowance.clone(),
            calls: self
                .calls()?
                .iter()
                .map(|call| call_result(call.index, &call.target, &call.outcome))
                .collect::<Result<_, _>>()?,
        })
    }

    fn call(
        &mut self,
        host: &mut dyn Host,
        target: TurnTarget,
        input: JsonObject,
    ) -> Result<TurnCallResult, RuntimeError> {
        // A call that may have run settles before the turn goes on, so no
        // effect is made twice behind an unconfirmed one.
        if let Some(unsettled) = self.unsettled()? {
            return Err(RuntimeError::ContinuationRejected {
                reason: format!(
                    "call {unsettled} has not settled; it waits on {}",
                    retry_id(self.scope.step_id, unsettled)
                ),
            });
        }
        let index = self
            .calls()?
            .iter()
            .map(|call| call.index + 1)
            .max()
            .unwrap_or(0);
        self.run(AnsweringHost::new(host, None), index, target, input)
    }

    fn settle(&mut self, host: &mut dyn Host, index: u32) -> Result<TurnCallResult, RuntimeError> {
        let Some(call) = self
            .calls()?
            .iter()
            .find(|call| call.index == index)
            .cloned()
        else {
            return Err(RuntimeError::ContinuationRejected {
                reason: format!("the turn has no call {index}"),
            });
        };
        let CallOutcome::Pending(waiting) = &call.outcome else {
            return call_result(index, &call.target, &call.outcome);
        };
        // The call asks again what it waits on, unchanged. A claimed call
        // runs again only on the host's exact acknowledgement, which its new
        // claim records.
        let mut answering = AnsweringHost::new(host, Some(waiting));
        if waiting.id == retry_id(self.scope.step_id, index) {
            let request = serde_json::to_value(&waiting.value)
                .and_then(serde_json::from_value)
                .map_err(|source| RuntimeError::json("reading a claimed call", source))?;
            let acknowledged = answering
                .resolve(request)?
                .is_some_and(|response| response.payload == retry_acknowledgement());
            if !acknowledged {
                return Err(RuntimeError::ContinuationRejected {
                    reason: format!(
                        "call {index} runs again only on the acknowledgement {{\"retry\": true}}"
                    ),
                });
            }
        }
        self.run(answering, index, call.target.clone(), call.input.clone())
    }
}

/// How a call ended, as its agent reads it.
fn call_result(
    index: u32,
    target: &TurnTarget,
    outcome: &CallOutcome,
) -> Result<TurnCallResult, RuntimeError> {
    let outcome = match outcome {
        CallOutcome::Completed(run) if run.outcome.succeeded() => TurnCallOutcome::Completed {
            output: run.contract.clone(),
            receipt_id: run.receipt.id.to_string().into(),
        },
        CallOutcome::Completed(run) => TurnCallOutcome::Failed {
            reason: reason(output_error(run)),
            receipt_id: Some(run.receipt.id.to_string().into()),
        },
        CallOutcome::Refused(why) => TurnCallOutcome::Refused {
            reason: reason(why.clone()),
        },
        CallOutcome::Failed(why) => TurnCallOutcome::Failed {
            reason: reason(why.clone()),
            receipt_id: None,
        },
        CallOutcome::Pending(request) => TurnCallOutcome::Pending {
            request: serde_json::to_value(&request.value)
                .and_then(serde_json::from_value)
                .map_err(|source| RuntimeError::json("reading a call's pending request", source))?,
        },
    };
    Ok(TurnCallResult {
        index,
        target: target.clone(),
        outcome,
    })
}

/// The request a claimed call waits on: `<step>/<index>/retry`.
fn retry_id(step_id: &str, index: u32) -> String {
    format!("{}/retry", call_step_id(step_id, index))
}

fn retry_acknowledgement() -> JsonValue {
    JsonValue::Object(JsonObject::from([(
        "retry".to_owned(),
        JsonValue::Bool(true),
    )]))
}

fn reason(text: String) -> NonEmptyString {
    NonEmptyString::new(text).unwrap_or_else(|| "the call gave no reason".into())
}

fn label(target: &TurnTarget) -> String {
    match target {
        TurnTarget::Tool { tool } => format!("tool {tool}"),
        TurnTarget::Operation {
            requirement,
            operation,
        } => format!("operation {requirement}/{operation}"),
        TurnTarget::Skill { skill } => format!("skill {skill}"),
        TurnTarget::Ask => "ask".to_owned(),
    }
}

fn call_tool(
    name: &str,
    description: String,
    input_schema: JsonObject,
    target: TurnTarget,
) -> TurnTool {
    TurnTool {
        name: mcp_name(name),
        description,
        input_schema,
        action: TurnToolAction::Call(target),
    }
}

/// Give each tool a distinct name: a repeat takes the first free `_<n>`
/// suffix, shortening the name to keep it within the grammar.
fn distinct_names(tools: &mut [TurnTool]) {
    let mut seen = std::collections::BTreeSet::new();
    for tool in tools {
        let base = tool.name.clone();
        let mut suffix = 1;
        while !seen.insert(tool.name.clone()) {
            suffix += 1;
            let tail = format!("_{suffix}");
            let head = base.get(..base.len().min(MCP_NAME_BYTES - tail.len()));
            tool.name = format!("{}{tail}", head.unwrap_or_default());
        }
    }
}

/// The longest MCP tool name.
const MCP_NAME_BYTES: usize = 64;

/// A name in the MCP tool-name grammar: `[A-Za-z0-9_-]`, at most 64 bytes.
fn mcp_name(raw: &str) -> String {
    let name = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    name.get(..name.len().min(MCP_NAME_BYTES))
        .unwrap_or(&name)
        .to_owned()
}

fn string_schema() -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("type".to_owned(), JsonValue::String("string".to_owned())),
        ("minLength".to_owned(), JsonValue::Number(1_u64.into())),
    ]))
}

fn object_schema<const N: usize>(
    properties: [(&str, JsonValue); N],
    required: &[&str],
) -> JsonObject {
    JsonObject::from([
        ("type".to_owned(), JsonValue::String("object".to_owned())),
        ("additionalProperties".to_owned(), JsonValue::Bool(false)),
        (
            "properties".to_owned(),
            JsonValue::Object(
                properties
                    .into_iter()
                    .map(|(name, schema)| (name.to_owned(), schema))
                    .collect(),
            ),
        ),
        (
            "required".to_owned(),
            JsonValue::Array(
                required
                    .iter()
                    .map(|name| JsonValue::String((*name).to_owned()))
                    .collect(),
            ),
        ),
    ])
}

fn pending_request(request: &ResolutionRequest) -> Result<PendingRequest, RuntimeError> {
    let value = serde_json::to_value(request)
        .and_then(serde_json::from_value::<JsonValue>)
        .map_err(|source| RuntimeError::json("encoding a pending request", source))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> TurnTool {
        TurnTool {
            name: mcp_name(name),
            description: String::new(),
            input_schema: JsonObject::new(),
            action: TurnToolAction::Status,
        }
    }

    #[test]
    fn names_stay_distinct_and_within_the_grammar() {
        let long = "x".repeat(80);
        let mut tools = [
            tool(&long),
            tool(&long),
            tool(&long),
            tool("a.b"),
            tool("a_b"),
        ];
        distinct_names(&mut tools);
        let names = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names[0], "x".repeat(64));
        assert_eq!(names[1], format!("{}_2", "x".repeat(62)));
        assert_eq!(names[2], format!("{}_3", "x".repeat(62)));
        assert_eq!(&names[3..], ["a_b", "a_b_2"]);
        assert!(names.iter().all(|name| name.len() <= MCP_NAME_BYTES));
    }
}
