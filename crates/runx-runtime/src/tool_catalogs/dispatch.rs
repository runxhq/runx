use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use runx_contracts::tools::ToolInspectResult;
use runx_contracts::{JsonObject, JsonValue};
use runx_parser::SkillArtifactContract;

use crate::RuntimeError;
use crate::adapter::{InvocationOutput, InvocationStatus};
use crate::adapter_pipeline::AdapterProjection;
use crate::credentials::CredentialDelivery;
use crate::effects::{EffectAdmission, RuntimeEffectRegistry};
use crate::observations::Observations;
use crate::tool_catalogs::ToolCatalogError;

mod local;

/// The context needed to resolve and invoke one tool. Graph steps and managed
/// agents share this exact native-or-local dispatch path.
pub(crate) struct ToolDispatchRequest<'a> {
    pub tool: Cow<'a, str>,
    pub inputs: Cow<'a, JsonObject>,
    pub resolved_inputs: Cow<'a, JsonObject>,
    pub scopes: &'a [String],
    pub env: &'a BTreeMap<String, String>,
    pub skill_directory: &'a Path,
    pub credential_delivery: &'a CredentialDelivery,
    pub local_artifacts: &'a crate::services::LocalArtifactService,
    pub skill_name: &'a str,
    pub allow_explicit_manifest_path: bool,
    pub effect_admission: Option<&'a EffectAdmission>,
    /// How far the run's native file, git, command and authoring tools reach.
    pub bounds: crate::services::WorkspaceBounds,
    /// The call the tool is invoked for, when a run makes it.
    pub call: Option<&'a crate::tool_catalogs::ToolCall>,
}

/// Inspect a tool through the same native/local catalog roots used at
/// execution time, so a turn offers its real description and input contract.
pub(crate) fn inspect_catalog_tool(
    tool: &str,
    env: &BTreeMap<String, String>,
    skill_directory: &Path,
    effects: &RuntimeEffectRegistry,
) -> Result<ToolInspectResult, RuntimeError> {
    local::inspect_catalog_tool(tool, env, skill_directory, effects)
}

/// Resolve the artifact contract used to project a tool step into graph
/// context.
pub(crate) fn resolve_tool_artifacts(
    request: &ToolDispatchRequest<'_>,
    effects: &RuntimeEffectRegistry,
) -> Result<Option<SkillArtifactContract>, RuntimeError> {
    let tool = request.tool.as_ref();
    if let Some(artifacts) = crate::tool_catalogs::native::artifacts(tool, effects) {
        return Ok(Some(artifacts));
    }
    local::resolve_artifacts(request, tool)
}

/// Dispatch one tool through the native registry, then the local manifest
/// catalog. This is the only execution owner shared by graph steps and
/// managed-agent calls.
pub(crate) fn dispatch_tool(
    mut request: ToolDispatchRequest<'_>,
    effects: &RuntimeEffectRegistry,
    observed_at: &str,
    started: Instant,
) -> Result<InvocationOutput, RuntimeError> {
    let tool = request.tool.trim();
    if tool.is_empty() {
        return Ok(failure("Tool reference must not be empty.", started));
    }
    if tool.len() != request.tool.len() {
        request.tool = Cow::Owned(tool.to_owned());
    }

    invoke_resolved_tool(&mut request, effects, observed_at, started)
}

/// Run a connector's tool package as a process from exactly `manifest`, the
/// installed copy the kernel checked: never a native or effect capability,
/// and never a tool found from the calling skill. Its receipt records that a
/// trusted host process ran it with the operation's credential.
pub(crate) fn dispatch_connector_tool(
    manifest: &Path,
    request: &ToolDispatchRequest<'_>,
    started: Instant,
) -> Result<InvocationOutput, RuntimeError> {
    local::invoke_manifest(manifest, request, started)
}

fn invoke_resolved_tool(
    request: &mut ToolDispatchRequest<'_>,
    effects: &RuntimeEffectRegistry,
    observed_at: &str,
    started: Instant,
) -> Result<InvocationOutput, RuntimeError> {
    if crate::tool_catalogs::native::is_core_tool(request.tool.as_ref())
        || effects.capability(request.tool.as_ref()).is_some()
    {
        return invoke_native_tool(request, effects, observed_at, started);
    }
    Ok(match local::invoke(request, started)? {
        Some(output) => output,
        None => missing_imported_tool(request.tool.as_ref(), started),
    })
}

fn invoke_native_tool(
    request: &mut ToolDispatchRequest<'_>,
    effects: &RuntimeEffectRegistry,
    observed_at: &str,
    started: Instant,
) -> Result<InvocationOutput, RuntimeError> {
    let inputs = std::mem::replace(&mut request.inputs, Cow::Owned(JsonObject::new())).into_owned();
    let Some(invocation) =
        crate::tool_catalogs::native::invoke(crate::tool_catalogs::native::NativeToolInvocation {
            tool: request.tool.as_ref(),
            observed_at,
            inputs,
            scopes: request.scopes,
            env: request.env,
            skill_directory: request.skill_directory,
            credential_delivery: request.credential_delivery,
            local_artifacts: request.local_artifacts,
            effect_admission: request.effect_admission,
            effects,
            bounds: request.bounds,
            call: request.call,
        })
    else {
        return Err(RuntimeError::SkillFailed {
            skill_name: request.tool.to_string(),
            message: "registered native capability disappeared during dispatch".to_owned(),
        });
    };
    let mut output = match invocation.result {
        Ok(payload) => success(payload, started),
        Err(error @ RuntimeError::ProviderEffectUnknown { .. })
        | Err(error @ RuntimeError::ProviderReadbackPending { .. })
        | Err(error @ RuntimeError::EffectState { .. }) => return Err(error),
        Err(error) => failure(error.to_string(), started),
    };
    if let Some(ephemeral) = invocation.ephemeral {
        output.set_ephemeral(ephemeral);
    }
    if let Some(observation) = request.credential_delivery.public_observation() {
        output.observations.record_credential(observation)?;
    }
    output.observations.boundary = Some(invocation.execution_boundary);
    Ok(output)
}

pub(super) fn configured_tool_roots(env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    env.get("RUNX_TOOL_ROOTS")
        .map(|value| {
            std::env::split_paths(value)
                .filter(|path| !path.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn workspace_root(env: &BTreeMap<String, String>, fallback: &Path) -> PathBuf {
    crate::RunxPaths::resolve(env, fallback)
        .workspace()
        .to_path_buf()
}

pub(super) fn catalog_error(skill_name: &str, error: ToolCatalogError) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: skill_name.to_owned(),
        message: error.to_string(),
    }
}

fn success(value: JsonValue, started: Instant) -> InvocationOutput {
    AdapterProjection::from_started(started).runtime_output(
        InvocationStatus::Success,
        value,
        None,
        Observations::default(),
    )
}

pub(super) fn failure(message: impl Into<String>, started: Instant) -> InvocationOutput {
    AdapterProjection::from_started(started).failure(message.into(), Observations::default())
}

fn missing_imported_tool(tool: &str, started: Instant) -> InvocationOutput {
    failure(
        format!("Imported tool '{tool}' was not found in configured tool catalogs."),
        started,
    )
}

#[cfg(test)]
mod containment_tests;
#[cfg(test)]
mod tests;
