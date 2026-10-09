//! An open agent turn served as MCP tools. The server holds the turn's
//! session: every call is made through the run, admitted against the turn's
//! allowance and recorded in the run's stream, and an action after the run
//! has moved on is refused.
use std::sync::Arc;

use runx_contracts::{JsonObject, JsonValue, TurnCallOutcome};
use serde::Serialize;

use super::types::{
    McpContent, McpServerOptions, McpServerTool, McpServerToolBehavior, McpToolResult, McpTurnTool,
};
use crate::effects::RuntimeEffectRegistry;
use crate::execution::runner::turn::TurnToolAction;
use crate::execution::skill_front::TurnSession;

impl McpServerOptions {
    /// Serve `turn`'s tools.
    pub fn for_turn(
        turn: TurnSession,
        package_name: impl Into<String>,
        package_version: impl Into<String>,
    ) -> Self {
        let envelope = &turn.invocation().envelope;
        let inputs = serde_json::to_string_pretty(&envelope.inputs).unwrap_or_default();
        let instructions = format!(
            "{}\n\nInputs (JSON):\n{inputs}\n\nDo the work with these tools; each call is admitted and recorded by runx. End the turn by calling turn_complete with the output.",
            envelope.instructions
        );
        let turn = Arc::new(turn);
        let tools = turn
            .tools()
            .iter()
            .map(|tool| McpServerTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
                required_scopes: Vec::new(),
                result: McpServerToolBehavior::Turn(McpTurnTool {
                    turn: Arc::clone(&turn),
                    action: tool.action.clone(),
                }),
            })
            .collect();
        Self {
            package_name: package_name.into(),
            package_version: package_version.into(),
            instructions: Some(instructions),
            effects: RuntimeEffectRegistry::default(),
            tools,
        }
    }
}

/// Do what one turn tool does, through the run.
pub(super) fn execute_mcp_turn_tool(tool: &McpTurnTool, arguments: JsonObject) -> McpToolResult {
    let result = match &tool.action {
        TurnToolAction::Call(target) => tool.turn.call(target.clone(), arguments).map(|call| {
            let failed = matches!(
                call.outcome,
                TurnCallOutcome::Failed { .. } | TurnCallOutcome::Refused { .. }
            );
            structured(&call, failed)
        }),
        TurnToolAction::Status => tool.turn.status().map(|status| structured(&status, false)),
        TurnToolAction::Complete => tool
            .turn
            .complete(arguments)
            .map(|result| super::server::mcp_tool_result_from_run_result(result, None)),
    };
    result.unwrap_or_else(|error| McpToolResult {
        content: vec![McpContent {
            text: error.to_string(),
        }],
        structured_content: None,
        is_error: true,
    })
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
