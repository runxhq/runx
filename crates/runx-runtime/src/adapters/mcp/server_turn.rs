//! An open agent turn served as MCP tools. The server is bound to the turn it
//! was started for: every call is made through the run, admitted against the
//! turn's allowance and recorded in the run's stream, and a call after the
//! run has moved on is refused.
use std::collections::BTreeMap;
use std::path::PathBuf;

use runx_contracts::{JsonObject, JsonValue};
use serde::Deserialize;

use super::types::{
    McpContent, McpServerExecutionOptions, McpServerOptions, McpServerTool, McpServerToolBehavior,
    McpToolResult, McpTurnTool,
};
use crate::RuntimeError;
use crate::effects::RuntimeEffectRegistry;
use crate::execution::orchestrator::{LocalOrchestrator, TurnRequest};
use crate::execution::runner::turn::{TurnAction, TurnCall, TurnTool, TurnToolAction};

/// The tools of the turn a run waits on, as the run describes them.
#[derive(Deserialize)]
struct DescribedTurn {
    request_id: String,
    instructions: String,
    inputs: JsonObject,
    tools: Vec<TurnTool>,
}

impl McpServerOptions {
    /// Serve the turn `run_id` waits on.
    pub fn for_turn(
        run_id: &str,
        cwd: PathBuf,
        execution: McpServerExecutionOptions,
        package_name: impl Into<String>,
        package_version: impl Into<String>,
    ) -> Result<Self, RuntimeError> {
        let orchestrator = LocalOrchestrator::with_effects_and_environment(
            execution.effects.clone(),
            execution.env.clone(),
        );
        let described = orchestrator
            .act_on_turn(&TurnRequest {
                run_id: run_id.to_owned(),
                turn: None,
                action: TurnAction::Describe,
                receipt_dir: execution.receipt_dir.clone(),
                env: execution.env.clone(),
                cwd: cwd.clone(),
            })
            .map_err(|error| RuntimeError::effect_state("describing a turn", error.to_string()))?;
        let described: DescribedTurn = serde_json::to_value(&described.output)
            .and_then(serde_json::from_value)
            .map_err(|source| RuntimeError::json("reading a turn's tools", source))?;
        let inputs = serde_json::to_string_pretty(&described.inputs)
            .map_err(|source| RuntimeError::json("encoding a turn's inputs", source))?;
        let instructions = format!(
            "{}\n\nInputs (JSON):\n{inputs}\n\nDo the work with these tools; each call is admitted and recorded by runx. End the turn by calling turn_complete with the output.",
            described.instructions
        );
        let tools = described
            .tools
            .into_iter()
            .map(|tool| McpServerTool {
                name: tool.name,
                description: tool.description,
                input_schema: tool.input_schema,
                required_scopes: Vec::new(),
                result: McpServerToolBehavior::Turn(Box::new(McpTurnTool {
                    run_id: run_id.to_owned(),
                    turn: described.request_id.clone(),
                    action: tool.action,
                    receipt_dir: execution.receipt_dir.clone(),
                    env: execution.env.clone(),
                    cwd: cwd.clone(),
                })),
            })
            .collect();
        Ok(Self {
            package_name: package_name.into(),
            package_version: package_version.into(),
            instructions: Some(instructions),
            effects: execution.effects,
            tools,
        })
    }
}

/// Make one turn tool's call through the run.
pub(super) fn execute_mcp_turn_tool(
    tool: McpTurnTool,
    arguments: JsonObject,
    effects: RuntimeEffectRegistry,
) -> McpToolResult {
    let complete = matches!(tool.action, TurnToolAction::Complete);
    let action = match tool.action {
        TurnToolAction::Call { target } => TurnAction::Call(TurnCall {
            target,
            input: arguments,
        }),
        TurnToolAction::Status => TurnAction::Status,
        TurnToolAction::Complete => TurnAction::Complete(arguments),
    };
    let result = LocalOrchestrator::with_effects_and_environment(effects, BTreeMap::new())
        .act_on_turn(&TurnRequest {
            run_id: tool.run_id,
            turn: Some(tool.turn),
            action,
            receipt_dir: tool.receipt_dir.clone(),
            env: tool.env,
            cwd: tool.cwd,
        });
    match result {
        Ok(result) if complete => {
            super::server::mcp_tool_result_from_run_result(result, tool.receipt_dir.as_deref())
        }
        Ok(result) => {
            let call = result
                .output
                .as_object()
                .and_then(|output| output.get("call"))
                .and_then(JsonValue::as_object);
            let failed = call
                .and_then(|call| call.get("status"))
                .and_then(JsonValue::as_str)
                .is_some_and(|status| matches!(status, "refused" | "failed"));
            let structured = call.cloned().or_else(|| result.output.as_object().cloned());
            let text = structured
                .as_ref()
                .and_then(|value| serde_json::to_string(value).ok())
                .unwrap_or_default();
            McpToolResult {
                content: vec![McpContent { text }],
                structured_content: structured,
                is_error: failed,
            }
        }
        Err(error) => McpToolResult {
            content: vec![McpContent {
                text: error.to_string(),
            }],
            structured_content: None,
            is_error: true,
        },
    }
}
