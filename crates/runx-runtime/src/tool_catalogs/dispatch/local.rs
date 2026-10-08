#[cfg(feature = "agent")]
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use runx_contracts::JsonObject;
#[cfg(feature = "agent")]
use runx_contracts::tools::ToolInspectResult;
use runx_parser::SkillArtifactContract;

use super::{ToolDispatchRequest, catalog_error, configured_tool_roots, failure, workspace_root};
use crate::RuntimeError;
use crate::adapter::{InvocationOutput, SkillAdapter, SkillInvocation};
use crate::adapters::process_tool::ProcessToolAdapter;
#[cfg(feature = "agent")]
use crate::effects::RuntimeEffectRegistry;
use crate::tool_catalogs::{ToolCatalogError, ToolInspectOptions, resolve_local_tool};

#[cfg(feature = "agent")]
pub(super) fn inspect_catalog_tool(
    tool_ref: &str,
    env: &BTreeMap<String, String>,
    skill_directory: &Path,
    effects: &RuntimeEffectRegistry,
) -> Result<ToolInspectResult, RuntimeError> {
    crate::tool_catalogs::inspect_tool_with_effects(
        &ToolInspectOptions {
            root: workspace_root(env, skill_directory),
            tool_ref: tool_ref.to_owned(),
            source: None,
            search_from_directory: skill_directory.to_path_buf(),
            tool_roots: configured_tool_roots(env),
            allow_explicit_manifest_path: false,
        },
        effects,
    )
    .map(|report| report.tool)
    .map_err(|error| catalog_error(tool_ref, error))
}

pub(super) fn resolve_artifacts(
    request: &ToolDispatchRequest<'_>,
    tool_ref: &str,
) -> Result<Option<SkillArtifactContract>, RuntimeError> {
    match resolve_local_tool(&inspect_options(request, tool_ref)) {
        Ok(resolution) => Ok(resolution.tool.artifacts),
        Err(error) if lookup_miss(&error) => Ok(None),
        Err(error) => Err(catalog_error(request.skill_name, error)),
    }
}

pub(super) struct LocalInvocationOutput {
    pub(super) output: InvocationOutput,
    pub(super) artifacts: Option<SkillArtifactContract>,
}

pub(super) fn invoke(
    request: &ToolDispatchRequest<'_>,
    started: Instant,
) -> Result<Option<LocalInvocationOutput>, RuntimeError> {
    let resolution = match resolve_local_tool(&inspect_options(request, request.tool_ref.as_ref()))
    {
        Ok(resolution) => resolution,
        Err(error) if lookup_miss(&error) => return Ok(None),
        Err(error) => return Err(catalog_error(request.skill_name, error)),
    };
    crate::execution::prepared_skill::verify_prepared_artifact_at_use(
        request.env,
        &resolution.manifest_path,
    )?;

    let artifacts = resolution.tool.artifacts.clone();
    let declared_inputs = resolution.tool.inputs.clone();
    let requirements = resolution.tool.execution_requirements();
    if let Err(error) = crate::capability::enforce_required_scopes(
        &resolution.tool.name,
        requirements.scopes.iter().map(String::as_str),
        request.scopes,
    ) {
        return Ok(Some(LocalInvocationOutput {
            output: failure(error.to_string(), started),
            artifacts,
        }));
    }
    let tool_directory = manifest_directory(&resolution.manifest_path, request.skill_directory);
    let inputs = match crate::input_contract::materialize_tool_inputs(
        &declared_inputs,
        request.inputs.as_ref(),
        request.resolved_inputs.as_ref(),
    ) {
        Ok(inputs) => inputs,
        Err(error) => {
            return Ok(Some(LocalInvocationOutput {
                output: failure(error.to_string(), started),
                artifacts,
            }));
        }
    };
    let invocation = SkillInvocation {
        historical_context: Vec::new(),
        skill_name: resolution.tool.name,
        step_id: None,
        executable: crate::adapter::Executable::Process(resolution.tool.source),
        outputs: None,
        requirements,
        artifacts: artifacts.clone(),
        inputs,
        resolved_inputs: JsonObject::new(),
        current_context: Vec::new(),
        provenance: Vec::new(),
        skill_directory: tool_directory,
        env: request.env.clone(),
        credential_delivery: request.credential_delivery.clone(),
    };
    let output = ProcessToolAdapter.invoke(invocation)?;
    Ok(Some(LocalInvocationOutput { output, artifacts }))
}

fn inspect_options(request: &ToolDispatchRequest<'_>, tool_ref: &str) -> ToolInspectOptions {
    ToolInspectOptions {
        root: workspace_root(request.env, request.skill_directory),
        tool_ref: tool_ref.to_owned(),
        source: None,
        search_from_directory: request.skill_directory.to_path_buf(),
        tool_roots: configured_tool_roots(request.env),
        allow_explicit_manifest_path: request.allow_explicit_manifest_path,
    }
}

fn lookup_miss(error: &ToolCatalogError) -> bool {
    match error {
        ToolCatalogError::NotFound(_) => true,
        ToolCatalogError::InvalidRequest(message) => message.contains("must include a namespace"),
        ToolCatalogError::Io { .. }
        | ToolCatalogError::Json { .. }
        | ToolCatalogError::InvalidManifest { .. } => false,
    }
}

fn manifest_directory(manifest_path: &Path, fallback: &Path) -> PathBuf {
    manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| fallback.to_path_buf())
}
