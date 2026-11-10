//! MCP tool discovery and transport adaptation. The local orchestrator owns
//! execution, continuation, and receipt persistence for every method kind.
use std::path::{Path, PathBuf};

use super::types::{
    McpServerExecutionOptions, McpServerOptions, McpServerSkillExecution, McpServerTool,
    McpServerToolBehavior, McpToolResult, McpTurnRuns,
};
use crate::RuntimeError;
use runx_contracts::schema::RunxSchema;
use runx_contracts::{CallAct, JsonObject, JsonValue};

impl McpServerOptions {
    pub fn from_skill_paths_with_execution(
        skill_paths: &[PathBuf],
        package_name: impl Into<String>,
        package_version: impl Into<String>,
        execution: McpServerExecutionOptions,
    ) -> Result<Self, RuntimeError> {
        let mut tools = Vec::new();
        for path in skill_paths {
            tools.extend(load_mcp_server_tools(path, &execution)?);
        }
        tools.extend(super::server_turn::turn_tools(&McpTurnRuns::Named(
            std::sync::Arc::new(super::types::NamedRuns::new(execution.clone())),
        )));
        Ok(Self {
            package_name: package_name.into(),
            package_version: package_version.into(),
            instructions: Some(
                "Each skill is a tool. A skill that pauses for agent work names its run; work that run's open turns here with turn_list, turn_status, turn_read, turn_call and turn_complete, naming the run, and cancel one of its calls while it executes with call_cancel. An approval is a person's to give, with runx resume."
                    .to_owned(),
            ),
            effects: execution.effects,
            tools,
        })
    }
}

/// A skill's tools: its selected method's, and, unless the server was given
/// one method, a tool for each other standalone method whose turns make calls.
/// Those are the methods only this connection can work.
pub(super) fn load_mcp_server_tools(
    skill_path: &Path,
    execution: &McpServerExecutionOptions,
) -> Result<Vec<McpServerTool>, RuntimeError> {
    let mut tools = vec![load_mcp_server_tool(
        skill_path,
        execution,
        execution.method.as_deref(),
    )?];
    if execution.method.is_some() {
        return Ok(tools);
    }
    let loaded = crate::load_validated_skill_package(skill_path)?;
    let Some(manifest) = loaded.manifest() else {
        return Ok(tools);
    };
    let default = manifest.default_method().map(|method| method.name.as_str());
    for method in manifest.methods.values() {
        if default == Some(method.name.as_str())
            || !crate::skill_package::method_has_sealed_test(&loaded.package, manifest, method)
            || !crate::skill_package::method_makes_calls(
                loaded.clone().into(),
                &method.name,
                &execution.env,
            )
            .map_err(|error| mcp_method_error(&loaded.package.skill.name, error))?
        {
            continue;
        }
        tools.push(load_mcp_server_tool(
            skill_path,
            execution,
            Some(&method.name),
        )?);
    }
    Ok(tools)
}

fn load_mcp_server_tool(
    skill_path: &Path,
    execution: &McpServerExecutionOptions,
    selected: Option<&str>,
) -> Result<McpServerTool, RuntimeError> {
    let loaded = crate::load_validated_skill_package(skill_path)?;
    let skill_path = loaded
        .directory
        .canonicalize()
        .map_err(|source| RuntimeError::io("canonicalizing skill path", source))?;
    let manifest = loaded.manifest().ok_or_else(|| {
        mcp_method_error(
            &loaded.package.skill.name,
            "skill package does not declare run.yaml methods",
        )
    })?;
    let method =
        crate::execution::skill_front::method_manifest::selected_method(manifest, selected)
            .map_err(|error| mcp_method_error(&loaded.package.skill.name, error))?
            .clone();
    // A tool is shown by its method's own words (surfaces.md).
    let description = manifest
        .method_description(&method, loaded.package.skill.description.as_deref())
        .map_or_else(
            || format!("runx skill {}#{}", loaded.package.skill.name, method.name),
            str::to_owned,
        );
    // A server given one method offers it under the skill's own name.
    let tool_method = selected.filter(|_| execution.method.is_none());
    let skill = loaded.package.skill.clone();
    let package_digest = loaded.package.package_digest.clone();
    let closure_digest = crate::skill_package::verify_loaded_execution_binding(
        loaded,
        &method.name,
        &execution.env,
        None,
        None,
    )
    .map_err(|error| mcp_method_error(&skill.name, error))?
    .closure_digest
    .ok_or_else(|| mcp_method_error(&skill.name, "skill requires an closure binding"))?;
    Ok(McpServerTool {
        name: crate::export::skill_tool_name(&skill.name, tool_method),
        description,
        input_schema: skill_inputs_to_json_schema(&method)?,
        result: McpServerToolBehavior::Skill(Box::new(McpServerSkillExecution {
            skill_path,
            skill_name: skill.name,
            method: method.name,
            package_digest,
            closure_digest,
            receipt_dir: execution.receipt_dir.clone(),
            env: execution.env.clone(),
            run_authority: execution.run_authority.clone(),
        })),
    })
}

fn mcp_method_error(skill_name: &str, error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: skill_name.to_owned(),
        message: error.to_string(),
    }
}

/// A skill tool takes the method's inputs, under its constraints across
/// them, and the act its caller states beside them.
fn skill_inputs_to_json_schema(
    method: &runx_parser::SkillMethodDefinition,
) -> Result<JsonObject, RuntimeError> {
    let mut schema = method.input_schema();
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
        run_authority: execution.run_authority.clone(),
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
            &execution.method,
            javascript,
            &execution.package_digest,
            &execution.closure_digest,
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
