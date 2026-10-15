//! MCP tool discovery and transport adaptation. The local orchestrator owns
//! execution, continuation, and receipt persistence for every runner kind.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::types::{
    McpServerExecutionOptions, McpServerOptions, McpServerSkillExecution, McpServerTool,
    McpServerToolBehavior, McpToolResult, McpTurnRuns,
};
use crate::RuntimeError;
use runx_contracts::schema::RunxSchema;
use runx_contracts::{CallAct, JsonObject, JsonValue};
use runx_parser::SkillInput;

impl McpServerOptions {
    pub fn from_skill_paths_with_execution(
        skill_paths: &[PathBuf],
        package_name: impl Into<String>,
        package_version: impl Into<String>,
        execution: McpServerExecutionOptions,
    ) -> Result<Self, RuntimeError> {
        let mut tools = skill_paths
            .iter()
            .map(|path| load_mcp_server_tool(path, &execution))
            .collect::<Result<Vec<_>, _>>()?;
        tools.extend(super::server_turn::turn_tools(&McpTurnRuns::Named(
            std::sync::Arc::new(super::types::NamedRuns::new(execution.clone())),
        )));
        Ok(Self {
            package_name: package_name.into(),
            package_version: package_version.into(),
            instructions: Some(
                "Each skill is a tool. A skill that pauses for agent work names its run; work that run's open turns here with turn_list, turn_status, turn_call and turn_complete, naming the run. An approval is a person's to give, with runx resume."
                    .to_owned(),
            ),
            effects: execution.effects,
            tools,
        })
    }
}

pub(super) fn load_mcp_server_tool(
    skill_path: &Path,
    execution: &McpServerExecutionOptions,
) -> Result<McpServerTool, RuntimeError> {
    let loaded = crate::load_validated_skill_package(skill_path)?;
    let skill_path = loaded
        .directory
        .canonicalize()
        .map_err(|source| RuntimeError::io("canonicalizing skill path", source))?;
    let manifest = loaded.manifest().ok_or_else(|| {
        mcp_runner_error(
            &loaded.package.skill.name,
            "skill package does not declare run.yaml runners",
        )
    })?;
    let runner = crate::execution::skill_front::runner_manifest::selected_runner(
        manifest,
        execution.runner.as_deref(),
    )
    .map_err(|error| mcp_runner_error(&loaded.package.skill.name, error))?
    .clone();
    let skill = loaded.package.skill.clone();
    let package_digest = loaded.package.package_digest.clone();
    let execution_closure_digest = crate::skill_package::verify_loaded_execution_binding(
        loaded,
        &runner.name,
        &execution.env,
        None,
        None,
    )
    .map_err(|error| mcp_runner_error(&skill.name, error))?
    .ok_or_else(|| mcp_runner_error(&skill.name, "skill requires an execution-closure binding"))?;
    Ok(McpServerTool {
        name: skill.name.clone(),
        description: skill
            .description
            .clone()
            .unwrap_or_else(|| format!("runx skill {}", skill.name)),
        input_schema: skill_inputs_to_json_schema(&runner.inputs)?,
        result: McpServerToolBehavior::Skill(Box::new(McpServerSkillExecution {
            skill_path,
            skill_name: skill.name,
            runner: runner.name,
            package_digest,
            execution_closure_digest,
            receipt_dir: execution.receipt_dir.clone(),
            env: execution.env.clone(),
        })),
    })
}

fn mcp_runner_error(skill_name: &str, error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: skill_name.to_owned(),
        message: error.to_string(),
    }
}

/// A skill tool takes the runner's inputs, and the act its caller states
/// beside them.
fn skill_inputs_to_json_schema(
    inputs: &BTreeMap<String, SkillInput>,
) -> Result<JsonObject, RuntimeError> {
    let mut schema = runx_contracts::input_contract_schema(inputs);
    let act = JsonValue::from_serialize(&CallAct::json_schema()).map_err(|error| {
        RuntimeError::EngineInvariant {
            context: "describing a skill tool",
            message: error.to_string(),
        }
    })?;
    if let Some(JsonValue::Object(properties)) = schema.get_mut("properties") {
        properties.insert("act".to_owned(), act);
    }
    Ok(schema)
}

pub(super) fn execute_mcp_server_skill(
    run_id: &str,
    execution: McpServerSkillExecution,
    mut inputs: JsonObject,
    javascript: crate::adapters::javascript::JavaScriptAdapter,
    effects: crate::effects::RuntimeEffectRegistry,
) -> Result<McpToolResult, RuntimeError> {
    let act = match inputs
        .remove("act")
        .map(|act| CallAct::parse(&act))
        .transpose()
    {
        Ok(act) => act,
        Err(reason) => {
            return Ok(McpToolResult {
                content: vec![super::types::McpContent { text: reason }],
                structured_content: None,
                is_error: true,
            });
        }
    };
    let workspace =
        crate::WorkspaceEnv::from_admitted(execution.env.clone()).map_err(RuntimeError::from)?;
    let request = crate::execution::orchestrator::SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act,
        skill_path: execution.skill_path,
        receipt_dir: execution.receipt_dir.clone(),
        run_id: Some(run_id.to_owned()),
        answers: None,
        inputs,
        env: execution.env,
        cwd: workspace.cwd().to_path_buf(),
        managed_agent: Default::default(),
    };
    let result = match crate::execution::orchestrator::LocalOrchestrator::with_effects(effects)
        .run_skill_with_services(
            &request,
            &execution.runner,
            javascript,
            &execution.package_digest,
            &execution.execution_closure_digest,
        ) {
        Ok(result) => result,
        Err(error) => {
            return Ok(McpToolResult {
                content: vec![super::types::McpContent {
                    text: error.to_string(),
                }],
                structured_content: None,
                is_error: true,
            });
        }
    };
    Ok(super::server::mcp_tool_result_from_run_result(
        result,
        execution.receipt_dir.as_deref(),
    ))
}

pub(super) fn identifier_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_owned()
}
