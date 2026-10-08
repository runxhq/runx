use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(feature = "agent")]
use runx_contracts::tools::ToolInspectResult;
use runx_contracts::{JsonObject, JsonValue};
use runx_parser::SkillArtifactContract;

use crate::RuntimeError;
use crate::adapter::{InvocationOutput, InvocationStatus};
use crate::adapter_pipeline::AdapterProjection;
use crate::credentials::CredentialDelivery;
use crate::effects::{EffectAdmission, RuntimeEffectRegistry};
use crate::tool_catalogs::ToolCatalogError;

mod local;

/// The context needed to resolve and invoke one tool. Graph steps and managed
/// agents share this exact native-or-local dispatch path.
pub(crate) struct ToolDispatchRequest<'a> {
    pub tool_ref: Cow<'a, str>,
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
}

/// Inspect a model-callable tool through the same native/local catalog roots
/// used at execution time. Managed agents receive the real tool description
/// and input contract instead of guessing from a bare ref.
#[cfg(feature = "agent")]
pub(crate) fn inspect_catalog_tool(
    tool_ref: &str,
    env: &BTreeMap<String, String>,
    skill_directory: &Path,
    effects: &RuntimeEffectRegistry,
) -> Result<ToolInspectResult, RuntimeError> {
    local::inspect_catalog_tool(tool_ref, env, skill_directory, effects)
}

/// Resolve the artifact contract used to project a tool step into graph
/// context.
pub(crate) fn resolve_tool_artifacts(
    request: &ToolDispatchRequest<'_>,
    effects: &RuntimeEffectRegistry,
) -> Result<Option<SkillArtifactContract>, RuntimeError> {
    let tool_ref = request.tool_ref.as_ref();
    if let Some(artifacts) = crate::tool_catalogs::native::artifacts(tool_ref, effects) {
        return Ok(Some(artifacts));
    }
    local::resolve_artifacts(request, tool_ref)
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
    let tool_ref = request.tool_ref.trim();
    if tool_ref.is_empty() {
        return Ok(failure("Tool reference must not be empty.", started));
    }
    if tool_ref.len() != request.tool_ref.len() {
        request.tool_ref = Cow::Owned(tool_ref.to_owned());
    }

    let (output, artifacts) = invoke_resolved_tool(&mut request, effects, observed_at, started)?;
    Ok(finalize_output(output, artifacts.as_ref()))
}

fn invoke_resolved_tool(
    request: &mut ToolDispatchRequest<'_>,
    effects: &RuntimeEffectRegistry,
    observed_at: &str,
    started: Instant,
) -> Result<(InvocationOutput, Option<SkillArtifactContract>), RuntimeError> {
    if crate::tool_catalogs::native::is_core_tool(request.tool_ref.as_ref())
        || effects.capability(request.tool_ref.as_ref()).is_some()
    {
        let artifacts = crate::tool_catalogs::native::artifacts(request.tool_ref.as_ref(), effects);
        let output = invoke_native_tool(request, effects, observed_at, started)?;
        return Ok((output, artifacts));
    }
    let Some(invocation) = local::invoke(request, started)? else {
        return Ok((
            missing_imported_tool(request.tool_ref.as_ref(), started),
            None,
        ));
    };
    Ok((invocation.output, invocation.artifacts))
}

fn finalize_output(
    mut output: InvocationOutput,
    artifacts: Option<&SkillArtifactContract>,
) -> InvocationOutput {
    if output.succeeded() {
        crate::output_contract::apply_artifact_outputs(&mut output.value, artifacts);
        if let Some(ephemeral) = output.ephemeral.as_value_mut() {
            crate::output_contract::apply_artifact_outputs(ephemeral, artifacts);
        }
    }
    output
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
            tool_ref: request.tool_ref.as_ref(),
            observed_at,
            inputs,
            scopes: request.scopes,
            env: request.env,
            skill_directory: request.skill_directory,
            credential_delivery: request.credential_delivery,
            local_artifacts: request.local_artifacts,
            effect_admission: request.effect_admission,
            effects,
        })
    else {
        return Err(RuntimeError::SkillFailed {
            skill_name: request.tool_ref.to_string(),
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
        output.record_credential_observation(observation)?;
    }
    output
        .metadata
        .extend(crate::process_invocation::boundary_metadata(
            invocation.execution_boundary,
        )?);
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
        JsonObject::new(),
    )
}

pub(super) fn failure(message: impl Into<String>, started: Instant) -> InvocationOutput {
    AdapterProjection::from_started(started).failure(message.into(), JsonObject::new())
}

fn missing_imported_tool(tool_ref: &str, started: Instant) -> InvocationOutput {
    failure(
        format!("Imported tool '{tool_ref}' was not found in configured tool catalogs."),
        started,
    )
}

#[cfg(test)]
mod containment_tests;
#[cfg(test)]
mod tests;
