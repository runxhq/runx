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
/// fields: `input`; `idempotency_key` for a write; `amount` where the
/// operation takes one; and `connector` when the run chooses it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnCall {
    pub target: TurnTarget,
    #[serde(default)]
    pub input: JsonObject,
}

const OPERATION_CALL_FIELDS: &[&str] = &["connector", "input", "idempotency_key", "amount"];

/// What a driver does with an open turn.
#[derive(Clone, Debug, PartialEq)]
pub enum TurnAction {
    /// Make a call within the allowance.
    Call(TurnCall),
    /// Read the turn: its request, allowance and calls.
    Status,
    /// List the tools the turn offers, with their input schemas.
    Describe,
    /// End the turn with the act's output; the run continues.
    Complete(JsonObject),
}

/// One tool an open turn offers its agent, named for MCP.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnTool {
    pub name: String,
    pub description: String,
    pub input_schema: JsonObject,
    pub action: TurnToolAction,
}

/// What calling a turn tool does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TurnToolAction {
    Call { target: TurnTarget },
    Status,
    Complete,
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
                    "an operation call takes connector, input, idempotency_key and amount, not {field}"
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

/// The tools `turn` offers: one per allowance entry, an ask, its status, and
/// its completion. Names are unique and fit the MCP tool-name grammar.
pub(crate) fn tools<A>(
    context: &TurnCallContext<'_, A>,
    turn: &OpenTurn,
) -> Result<Vec<TurnTool>, RuntimeError> {
    let allowance = &turn.invocation.allowance;
    let mut tools = Vec::new();
    for tool in &allowance.tools {
        let (description, input_schema) = catalog_tool_contract(context, tool.as_str())?;
        tools.push(call_tool(
            tool.as_str(),
            description,
            input_schema,
            TurnTarget::Tool {
                tool: tool.as_str().to_owned(),
            },
        ));
    }
    let scope = crate::kernel::connectors::ConnectorScope::for_skill(context.graph_dir).map_err(
        |error| RuntimeError::effect_state("reading the skill's connectors", error.to_string()),
    )?;
    for allowed in &allowance.operations {
        let target = TurnTarget::Operation {
            requirement: allowed.requirement.as_str().to_owned(),
            operation: allowed.operation.as_str().to_owned(),
        };
        let (description, input_schema) = operation_contract(context, &scope, allowed)?;
        tools.push(call_tool(
            &format!("{}__{}", allowed.requirement, allowed.operation),
            description,
            input_schema,
            target,
        ));
    }
    for skill in &allowance.skills {
        let target = TurnTarget::Skill {
            skill: skill.as_str().to_owned(),
        };
        let step = call_step(
            context,
            "describe",
            0,
            &TurnCall {
                target: target.clone(),
                input: JsonObject::new(),
            },
        )
        .map_err(|reason| RuntimeError::effect_state("describing a child skill", reason))?;
        let loaded = load_step_skill(
            context.graph_dir,
            &step,
            StepSkillLoadOptions {
                env: &context.runtime.options.env,
            },
        )?;
        let description = crate::load_validated_skill_package(&loaded.directory)?
            .package
            .skill
            .description
            .unwrap_or_else(|| format!("Run the {} skill.", loaded.runner.name));
        tools.push(call_tool(
            &format!("skill__{}", skill.as_str().trim_start_matches(['.', '/'])),
            description,
            runx_contracts::input_contract_schema(&loaded.runner.inputs),
            target,
        ));
    }
    tools.push(call_tool(
        "ask_human",
        "Ask the human to approve something; the call stays pending until they answer.".to_owned(),
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
    tools.push(TurnTool {
        name: "turn_complete".to_owned(),
        description: "End the turn with its output; the run continues.".to_owned(),
        input_schema: match &turn.invocation.envelope.output_schema {
            Some(JsonValue::Object(schema)) => schema.clone(),
            _ => runx_contracts::output_value_schema(turn.invocation.envelope.output.as_ref())
                .as_object()
                .cloned()
                .unwrap_or_default(),
        },
        action: TurnToolAction::Complete,
    });
    let mut seen = std::collections::BTreeSet::new();
    for tool in &mut tools {
        let base = tool.name.clone();
        let mut suffix = 1;
        while !seen.insert(tool.name.clone()) {
            suffix += 1;
            tool.name = mcp_name(&format!("{base}_{suffix}"));
        }
    }
    Ok(tools)
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
        action: TurnToolAction::Call { target },
    }
}

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
    name.get(..name.len().min(64)).unwrap_or(&name).to_owned()
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

/// An operation tool's input: the call fields the turn accepts, with the
/// operation's own input schema when one connector serves it.
fn operation_contract<A>(
    context: &TurnCallContext<'_, A>,
    scope: &crate::kernel::connectors::ConnectorScope,
    allowed: &runx_contracts::AllowedOperation,
) -> Result<(String, JsonObject), RuntimeError> {
    let declared = context
        .manifest
        .connectors
        .get(allowed.requirement.as_str())
        .ok_or_else(|| {
            RuntimeError::effect_state(
                "describing an operation",
                format!("requirement {} is not declared", allowed.requirement),
            )
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
        let connector = scope.connector(id).map_err(|error| {
            RuntimeError::effect_state("describing an operation", error.to_string())
        })?;
        let operation = connector
            .manifest
            .operations
            .iter()
            .find(|(name, _)| name.as_str() == allowed.operation.as_str())
            .map(|(_, operation)| operation.clone())
            .ok_or_else(|| {
                RuntimeError::effect_state(
                    "describing an operation",
                    format!("{id} has no operation {}", allowed.operation),
                )
            })?;
        described.push((id.clone(), operation));
    }
    let writes = described.iter().any(|(_, operation)| {
        operation.access() == runx_contracts::connector::OperationAccess::Mutate
    });
    let amount = described
        .iter()
        .any(|(_, operation)| operation.amount == runx_contracts::connector::AmountRule::Required);
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
    let mut schema = object_schema([], &required);
    schema.insert(
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
    Ok((description, schema))
}

/// A catalog tool's description and input schema, as the catalog declares them.
#[cfg(feature = "catalog")]
fn catalog_tool_contract<A>(
    context: &TurnCallContext<'_, A>,
    tool: &str,
) -> Result<(String, JsonObject), RuntimeError> {
    let inspected = crate::tool_catalogs::dispatch::inspect_catalog_tool(
        tool,
        &context.runtime.options.env,
        context.graph_dir,
        &context.runtime.options.effects,
    )?;
    Ok((
        inspected
            .description
            .unwrap_or_else(|| format!("Runx tool {tool}.")),
        runx_contracts::input_contract_schema(&inspected.inputs),
    ))
}

#[cfg(not(feature = "catalog"))]
fn catalog_tool_contract<A>(
    _: &TurnCallContext<'_, A>,
    tool: &str,
) -> Result<(String, JsonObject), RuntimeError> {
    Err(RuntimeError::UnsupportedAdapter {
        adapter_type: format!("catalog tool {tool}"),
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

/// The tools the turn offers, for a host that serves them.
pub(crate) fn description(
    run_id: &str,
    turn: &OpenTurn,
    tools: &[TurnTool],
) -> Result<JsonValue, RuntimeError> {
    to_json(&TurnDescription {
        schema: "runx.turn.tools.v1",
        run_id,
        step_id: &turn.step_id,
        request_id: &turn.request.id,
        instructions: turn.invocation.envelope.instructions.as_str(),
        inputs: &turn.invocation.envelope.inputs,
        tools,
    })
}

#[derive(Serialize)]
struct TurnDescription<'a> {
    schema: &'static str,
    run_id: &'a str,
    step_id: &'a str,
    request_id: &'a str,
    instructions: &'a str,
    inputs: &'a JsonObject,
    tools: &'a [TurnTool],
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
