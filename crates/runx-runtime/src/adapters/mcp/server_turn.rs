//! A run's open agent turns served as MCP tools. The tool set is fixed,
//! `turn_list`, `turn_status`, `turn_read`, `turn_call` and `turn_complete`,
//! each taking a turn's address, so a turn a call opens mid-session is
//! reachable in every client; a turn's own tools arrive with its status, and
//! its act's context as an index each entry of which `turn_read` reads.
//! `runx mcp --run` serves
//! one run's turns; a skill server serves the same tools over the runs its own
//! skill calls opened, so a turn one of them opened is worked on the same
//! connection. Every call is made through the run: admitted against the
//! turn's allowance and recorded in the run's stream. An action on a turn that
//! is no longer open is refused.
use std::sync::Arc;

use runx_contracts::{JsonObject, JsonValue, ResolutionRequest, TurnCallOutcome};
use serde::Serialize;

use super::types::{
    McpContent, McpServerOptions, McpServerTool, McpServerToolBehavior, McpToolResult,
    McpTurnAction, McpTurnRuns, McpTurnTool,
};
use crate::effects::RuntimeEffectRegistry;
use crate::execution::skill_front::TurnSession;

impl McpServerOptions {
    /// Serve `session`'s open turns.
    pub fn for_turns(
        session: TurnSession,
        package_name: impl Into<String>,
        package_version: impl Into<String>,
    ) -> Self {
        let open = session
            .waiting()
            .map(|waiting| {
                waiting
                    .into_iter()
                    .filter_map(|request| match request {
                        ResolutionRequest::AgentAct { id, .. } => Some(id.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let instructions = format!(
            "This server works the open agent turns of runx run {}. A turn is addressed by its act's id; open now: {open}.\n\nRead a turn with turn_status: its instructions, inputs, output schema, the tools it offers, and its context as an index, each entry with a ref that turn_read reads it by. Make each call with turn_call, naming one of those tools and its input, and pin your own id for it as call_id so a call you make again returns the recorded one; runx admits, executes and records it. End a turn with turn_complete and its output, or with a failure and its reason. A call that starts a child skill may open a turn of its own, worked the same way; turn_list shows everything the run waits on.",
            session.run_id()
        );
        Self {
            package_name: package_name.into(),
            package_version: package_version.into(),
            instructions: Some(instructions),
            effects: RuntimeEffectRegistry::default(),
            tools: turn_tools(&McpTurnRuns::Bound(Arc::new(session))),
        }
    }
}

/// The fixed turn tools over `runs`. Tools that work any run the call names
/// take that run's id beside the turn's address.
pub(super) fn turn_tools(runs: &McpTurnRuns) -> Vec<McpServerTool> {
    let named = matches!(runs, McpTurnRuns::Named(_));
    let fields = |extra: &[(&str, JsonValue)], optional: &[(&str, JsonValue)]| {
        let mut fields = Vec::new();
        if named {
            fields.push(("run", string_schema()));
        }
        fields.extend(extra.iter().cloned());
        let required = fields.iter().map(|(name, _)| *name).collect::<Vec<_>>();
        fields.extend(optional.iter().cloned());
        object_schema(&fields, &required)
    };
    let turn = || ("turn", string_schema());
    [
        (
            McpTurnAction::List,
            "turn_list",
            "What the run waits on: each open turn, by the address its act's id gives it, and each ask, approval or retry beneath them.",
            fields(&[], &[]),
        ),
        (
            McpTurnAction::Status,
            "turn_status",
            "Read an open turn: its instructions, inputs and output schema, its context as an index of entries each with a ref, the tools it offers with their input schemas, and how each call it made so far ended.",
            fields(&[turn()], &[]),
        ),
        (
            McpTurnAction::Read,
            "turn_read",
            "Read a page of what a ref of an open turn names: a context entry by the ref its status lists, an input of its act as input:<name>, or one of its calls as call:<index>. A page is at most 32,000 characters from offset; next_offset reads on from what it delivered.",
            fields(
                &[turn(), ("ref", string_schema())],
                &[("offset", count_schema()), ("limit", count_schema())],
            ),
        ),
        (
            McpTurnAction::Call,
            "turn_call",
            "Make a call in an open turn: one of the tools its status lists, by name, with its input. Pin your own call_id on it, unique within the turn: a call made again with that id returns the recorded call, and one with another tool or input is refused. A call that waits on a human, or on a turn of its own, returns pending with what it waits on.",
            fields(
                &[
                    turn(),
                    ("tool", string_schema()),
                    ("input", any_object_schema()),
                ],
                &[("call_id", string_schema())],
            ),
        ),
        (
            McpTurnAction::Complete,
            "turn_complete",
            "End an open turn with its output, which must match the turn's output schema, or with a failure and its reason when the act cannot be done; the run continues. A turn whose calls have not settled cannot end.",
            fields(
                &[turn()],
                &[
                    ("output", any_object_schema()),
                    (
                        "failure",
                        JsonValue::Object(object_schema(
                            &[("reason", string_schema())],
                            &["reason"],
                        )),
                    ),
                ],
            ),
        ),
    ]
    .into_iter()
    .map(|(action, name, description, input_schema)| McpServerTool {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema,
        result: McpServerToolBehavior::Turn(McpTurnTool {
            runs: runs.clone(),
            action,
        }),
    })
    .collect()
}

/// The session a turn tool call works: the bound run, or the one it names,
/// which this server's own calls must have opened (`served` is their run-id
/// prefix). A skill server never works another host's run.
fn session(
    runs: &McpTurnRuns,
    arguments: &JsonObject,
    served: &str,
) -> Result<Arc<TurnSession>, String> {
    match runs {
        McpTurnRuns::Bound(session) => Ok(Arc::clone(session)),
        McpTurnRuns::Named(runs) => {
            let run_id = text(arguments, "run")?;
            if !run_id.starts_with(served) {
                return Err(format!(
                    "run {run_id} was not opened by this server; it works only the turns its own skill calls opened"
                ));
            }
            // One session drives a run, whichever calls work it at once.
            let mut sessions = runs
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(session) = sessions.get(run_id) {
                return Ok(Arc::clone(session));
            }
            let execution = &runs.execution;
            let workspace = crate::WorkspaceEnv::from_admitted(execution.env.clone())
                .map_err(|error| error.to_string())?;
            let session = crate::execution::orchestrator::LocalOrchestrator::with_effects(
                execution.effects.clone(),
            )
            .open_turns(&crate::execution::orchestrator::TurnRequest {
                run_id: run_id.to_owned(),
                receipt_dir: execution.receipt_dir.clone(),
                env: execution.env.clone(),
                cwd: workspace.cwd().to_path_buf(),
            })
            .map(Arc::new)
            .map_err(|error| error.to_string())?;
            sessions.insert(run_id.to_owned(), Arc::clone(&session));
            Ok(session)
        }
    }
}

/// Do what one turn tool does, through the run.
pub(super) fn execute_mcp_turn_tool(
    tool: &McpTurnTool,
    arguments: JsonObject,
    served: &str,
) -> McpToolResult {
    let result = session(&tool.runs, &arguments, served).and_then(|session| match tool.action {
        McpTurnAction::List => session
            .waiting()
            .map(|requests| structured(&Waiting::of(requests), false))
            .map_err(|error| error.to_string()),
        McpTurnAction::Status => text(&arguments, "turn").and_then(|turn| {
            session
                .status(turn)
                .map(|status| structured(&status, false))
                .map_err(|error| error.to_string())
        }),
        McpTurnAction::Read => text(&arguments, "turn").and_then(|turn| {
            let reference = text(&arguments, "ref")?;
            let offset = count(&arguments, "offset")?.unwrap_or(0);
            let limit = count(&arguments, "limit")?;
            session
                .read(turn, reference, offset, limit)
                .map(|page| structured(&page, false))
                .map_err(|error| error.to_string())
        }),
        McpTurnAction::Call => text(&arguments, "turn").and_then(|turn| {
            let tool = text(&arguments, "tool")?;
            let input = object(&arguments, "input")?;
            let call_id = arguments
                .contains_key("call_id")
                .then(|| text(&arguments, "call_id"))
                .transpose()?;
            session
                .call(turn, tool, call_id, input)
                .map(|call| {
                    let failed = matches!(
                        call.outcome,
                        TurnCallOutcome::Failed { .. } | TurnCallOutcome::Refused { .. }
                    );
                    structured(&call, failed)
                })
                .map_err(|error| error.to_string())
        }),
        McpTurnAction::Complete => text(&arguments, "turn").and_then(|turn| {
            let ended = match (arguments.get("output"), arguments.get("failure")) {
                (Some(_), None) => session.complete(turn, object(&arguments, "output")?),
                (None, Some(_)) => {
                    let failure = object(&arguments, "failure")?;
                    session.fail(turn, text(&failure, "reason")?)
                }
                _ => return Err("turn_complete takes its output or a failure, not both".to_owned()),
            };
            ended
                .map(|result| {
                    // A run that no longer waits on a turn is no longer driven.
                    if result.status != crate::execution::orchestrator::RunStatus::NeedsAgent
                        && let McpTurnRuns::Named(runs) = &tool.runs
                    {
                        runs.sessions
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .remove(session.run_id());
                    }
                    super::server::mcp_tool_result_from_run_result(result, session.receipt_dir())
                })
                .map_err(|error| error.to_string())
        }),
    });
    result.unwrap_or_else(|error| McpToolResult {
        content: vec![McpContent { text: error }],
        structured_content: None,
        is_error: true,
    })
}

/// What `turn_list` returns: each open turn as a line, its act read by
/// `turn_status`, and everything else the run waits on whole.
#[derive(Serialize)]
struct Waiting {
    turns: Vec<OpenTurn>,
    requests: Vec<ResolutionRequest>,
}

/// One open turn: its address, and what kind of act it answers.
#[derive(Serialize)]
struct OpenTurn {
    turn: String,
    skill: String,
    role: String,
    task: String,
}

impl Waiting {
    fn of(waiting: Vec<ResolutionRequest>) -> Self {
        let mut turns = Vec::new();
        let mut requests = Vec::new();
        for request in waiting {
            match request {
                ResolutionRequest::AgentAct { id, invocation } => turns.push(OpenTurn {
                    turn: id.to_string(),
                    skill: invocation.envelope.skill.to_string(),
                    role: invocation.role.to_string(),
                    task: invocation.task.to_string(),
                }),
                other => requests.push(other),
            }
        }
        Self { turns, requests }
    }
}

fn text<'a>(arguments: &'a JsonObject, field: &str) -> Result<&'a str, String> {
    arguments
        .get(field)
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{field} must be a non-empty string"))
}

/// An optional count: absent, or a whole number of zero or more.
fn count(arguments: &JsonObject, field: &str) -> Result<Option<u64>, String> {
    arguments
        .get(field)
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| format!("{field} must be a whole number of zero or more"))
        })
        .transpose()
}

fn object(arguments: &JsonObject, field: &str) -> Result<JsonObject, String> {
    arguments
        .get(field)
        .and_then(JsonValue::as_object)
        .cloned()
        .ok_or_else(|| format!("{field} must be an object"))
}

fn structured(value: &impl Serialize, is_error: bool) -> McpToolResult {
    let structured = serde_json::to_value(value)
        .and_then(serde_json::from_value::<JsonValue>)
        .ok()
        .and_then(|value| match value {
            JsonValue::Object(object) => Some(object),
            _ => None,
        });
    McpToolResult {
        content: vec![McpContent {
            text: serde_json::to_string(value).unwrap_or_default(),
        }],
        structured_content: structured,
        is_error,
    }
}

fn string_schema() -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("type".to_owned(), JsonValue::String("string".to_owned())),
        ("minLength".to_owned(), JsonValue::Number(1_u64.into())),
    ]))
}

fn count_schema() -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("type".to_owned(), JsonValue::String("integer".to_owned())),
        ("minimum".to_owned(), JsonValue::Number(0_u64.into())),
    ]))
}

fn any_object_schema() -> JsonValue {
    JsonValue::Object(JsonObject::from([(
        "type".to_owned(),
        JsonValue::String("object".to_owned()),
    )]))
}

fn object_schema(properties: &[(&str, JsonValue)], required: &[&str]) -> JsonObject {
    let properties = properties
        .iter()
        .map(|(name, schema)| ((*name).to_owned(), schema.clone()))
        .collect::<JsonObject>();
    let required = required
        .iter()
        .map(|name| JsonValue::String((*name).to_owned()))
        .collect();
    JsonObject::from([
        ("type".to_owned(), JsonValue::String("object".to_owned())),
        ("additionalProperties".to_owned(), JsonValue::Bool(false)),
        ("properties".to_owned(), JsonValue::Object(properties)),
        ("required".to_owned(), JsonValue::Array(required)),
    ])
}
