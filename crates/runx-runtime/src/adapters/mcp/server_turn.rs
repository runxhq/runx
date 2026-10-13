//! A run's open agent turns served as MCP tools. The tool set is fixed,
//! `turn_list`, `turn_status`, `turn_call` and `turn_complete`, each taking a
//! turn's address, so a turn a call opens mid-session is reachable in every
//! client; a turn's own tools arrive with its status. `runx turn` serves one
//! run's turns; a skill server serves the same tools over any run they name,
//! so a turn its own skill call opened is worked on the same connection. Every call is made
//! through the run: admitted against the turn's allowance and recorded in the
//! run's stream. An action on a turn that is no longer open is refused.
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
            "This server works the open agent turns of runx run {}. A turn is addressed by its act's id; open now: {open}.\n\nRead a turn with turn_status: its instructions, inputs, output schema and the tools it offers. Make each call with turn_call, naming one of those tools and its input; runx admits, executes and records it. End a turn with turn_complete and its output. A call that starts a child skill may open a turn of its own, worked the same way; turn_list shows everything the run waits on.",
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

/// The four fixed turn tools over `runs`. Tools that work any run the call
/// names take that run's id beside the turn's address.
pub(super) fn turn_tools(runs: &McpTurnRuns) -> Vec<McpServerTool> {
    let named = matches!(runs, McpTurnRuns::Named(_));
    let fields = |extra: &[(&str, JsonValue)]| {
        let mut fields = Vec::new();
        if named {
            fields.push(("run", string_schema()));
        }
        fields.extend(extra.iter().cloned());
        object_schema(&fields)
    };
    let turn = || ("turn", string_schema());
    [
        (
            McpTurnAction::List,
            "turn_list",
            "What the run waits on: each open turn's act, whose id is the turn's address, and each ask, approval or retry beneath them.",
            fields(&[]),
        ),
        (
            McpTurnAction::Status,
            "turn_status",
            "Read an open turn: its instructions, inputs and output schema, the tools it offers with their input schemas, and how each call it made so far ended.",
            fields(&[turn()]),
        ),
        (
            McpTurnAction::Call,
            "turn_call",
            "Make a call in an open turn: one of the tools its status lists, by name, with its input. A call that waits on a human, or on a turn of its own, returns pending with what it waits on.",
            fields(&[
                turn(),
                ("tool", string_schema()),
                ("input", any_object_schema()),
            ]),
        ),
        (
            McpTurnAction::Complete,
            "turn_complete",
            "End an open turn with its output, which must match the turn's output schema; the run continues. A turn whose calls have not settled cannot end.",
            fields(&[turn(), ("output", any_object_schema())]),
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

/// The session a turn tool call works: the bound run, or the one it names.
fn session(runs: &McpTurnRuns, arguments: &JsonObject) -> Result<Arc<TurnSession>, String> {
    match runs {
        McpTurnRuns::Bound(session) => Ok(Arc::clone(session)),
        McpTurnRuns::Named(execution) => {
            let run_id = text(arguments, "run")?;
            let workspace = crate::WorkspaceEnv::from_admitted(execution.env.clone())
                .map_err(|error| error.to_string())?;
            crate::execution::orchestrator::LocalOrchestrator::with_effects(
                execution.effects.clone(),
            )
            .open_turns(&crate::execution::orchestrator::TurnRequest {
                run_id: run_id.to_owned(),
                receipt_dir: execution.receipt_dir.clone(),
                env: execution.env.clone(),
                cwd: workspace.cwd().to_path_buf(),
            })
            .map(Arc::new)
            .map_err(|error| error.to_string())
        }
    }
}

/// Do what one turn tool does, through the run.
pub(super) fn execute_mcp_turn_tool(tool: &McpTurnTool, arguments: JsonObject) -> McpToolResult {
    let result = session(&tool.runs, &arguments).and_then(|session| match tool.action {
        McpTurnAction::List => session
            .waiting()
            .map(|requests| structured(&Waiting { requests }, false))
            .map_err(|error| error.to_string()),
        McpTurnAction::Status => text(&arguments, "turn").and_then(|turn| {
            session
                .status(turn)
                .map(|status| structured(&status, false))
                .map_err(|error| error.to_string())
        }),
        McpTurnAction::Call => text(&arguments, "turn").and_then(|turn| {
            let tool = text(&arguments, "tool")?;
            let input = object(&arguments, "input")?;
            session
                .call(turn, tool, input)
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
            let output = object(&arguments, "output")?;
            session
                .complete(turn, output)
                .map(|result| {
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

/// What `turn_list` returns.
#[derive(Serialize)]
struct Waiting {
    requests: Vec<ResolutionRequest>,
}

fn text<'a>(arguments: &'a JsonObject, field: &str) -> Result<&'a str, String> {
    arguments
        .get(field)
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{field} must be a non-empty string"))
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

fn any_object_schema() -> JsonValue {
    JsonValue::Object(JsonObject::from([(
        "type".to_owned(),
        JsonValue::String("object".to_owned()),
    )]))
}

fn object_schema(properties: &[(&str, JsonValue)]) -> JsonObject {
    let properties = properties
        .iter()
        .map(|(name, schema)| ((*name).to_owned(), schema.clone()))
        .collect::<JsonObject>();
    let required = properties
        .keys()
        .map(|name| JsonValue::String(name.clone()))
        .collect();
    JsonObject::from([
        ("type".to_owned(), JsonValue::String("object".to_owned())),
        ("additionalProperties".to_owned(), JsonValue::Bool(false)),
        ("properties".to_owned(), JsonValue::Object(properties)),
        ("required".to_owned(), JsonValue::Array(required)),
    ])
}
