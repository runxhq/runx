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

mod data_source;
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
    pub javascript: &'a crate::adapters::javascript::JavaScriptAdapter,
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
/// context. Data-source routing resolves to the concrete adapter first because
/// the router itself deliberately has no manifest.
pub(crate) fn resolve_tool_artifacts(
    request: &ToolDispatchRequest<'_>,
    effects: &RuntimeEffectRegistry,
) -> Result<Option<SkillArtifactContract>, RuntimeError> {
    let tool_ref = request.tool_ref.as_ref();
    if let Some(artifacts) = crate::tool_catalogs::native::artifacts(tool_ref, effects) {
        return Ok(Some(artifacts));
    }

    let resolved_ref = match data_source::target(
        tool_ref,
        request.inputs.as_ref(),
        request.env,
        request.skill_directory,
    ) {
        Ok(Some(target)) => Cow::Owned(target.tool_ref),
        Ok(None) | Err(_) => Cow::Borrowed(tool_ref),
    };
    local::resolve_artifacts(request, resolved_ref.as_ref())
}

/// Dispatch one tool through data-source routing, then the native registry and
/// finally the local manifest catalog. This is the only execution owner shared
/// by graph steps and managed-agent calls.
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

    let data_operation = match prepare_data_operation(&mut request, observed_at) {
        Ok(operation) => operation,
        Err(error) => return Ok(failure(error.to_string(), started)),
    };
    let native_binding = match route_data_source(&mut request, data_operation.as_ref()) {
        Ok(binding) => binding,
        Err(message) => return Ok(failure(message, started)),
    };
    let (output, artifacts) = invoke_resolved_tool(
        &mut request,
        native_binding,
        data_operation.is_some(),
        effects,
        observed_at,
        started,
    )?;
    finalize_output(output, artifacts, data_operation.as_ref(), effects)
}

fn prepare_data_operation(
    request: &mut ToolDispatchRequest<'_>,
    observed_at: &str,
) -> Result<Option<crate::tool_catalogs::native::PreparedDataOperation>, RuntimeError> {
    let Some(operation) = crate::tool_catalogs::native::prepare_data_operation(
        request.tool_ref.as_ref(),
        request.inputs.as_ref(),
        observed_at,
    ) else {
        return Ok(None);
    };
    let mut operation = operation?;
    request.inputs = Cow::Owned(std::mem::take(&mut operation.inputs));
    Ok(Some(operation))
}

fn route_data_source(
    request: &mut ToolDispatchRequest<'_>,
    data_operation: Option<&crate::tool_catalogs::native::PreparedDataOperation>,
) -> Result<Option<JsonObject>, String> {
    let Some(target) = data_source::target(
        request.tool_ref.as_ref(),
        request.inputs.as_ref(),
        request.env,
        request.skill_directory,
    )?
    else {
        return Ok(None);
    };
    let data_source::Target {
        tool_ref,
        binding,
        operation,
    } = target;
    let native_binding = if tool_ref == request.tool_ref {
        Some(binding)
    } else {
        if let Some(operation) = data_operation {
            operation.apply_adapter_inputs(request.inputs.to_mut());
        }
        request.inputs.to_mut().insert(
            "operation".to_owned(),
            JsonValue::String(operation.to_owned()),
        );
        request
            .inputs
            .to_mut()
            .insert("data_source_binding".to_owned(), JsonValue::Object(binding));
        None
    };
    request.tool_ref = Cow::Owned(tool_ref);
    Ok(native_binding)
}

fn invoke_resolved_tool(
    request: &mut ToolDispatchRequest<'_>,
    native_data_source_binding: Option<JsonObject>,
    runtime_prepared_inputs: bool,
    effects: &RuntimeEffectRegistry,
    observed_at: &str,
    started: Instant,
) -> Result<(InvocationOutput, Option<SkillArtifactContract>), RuntimeError> {
    if crate::tool_catalogs::native::is_core_tool(request.tool_ref.as_ref())
        || effects.capability(request.tool_ref.as_ref()).is_some()
    {
        let artifacts = crate::tool_catalogs::native::artifacts(request.tool_ref.as_ref(), effects);
        let output = invoke_native_tool(
            request,
            native_data_source_binding,
            effects,
            observed_at,
            started,
        )?;
        return Ok((output, artifacts));
    }
    let contract = if runtime_prepared_inputs {
        local::InvocationContract::DataAdapter
    } else {
        local::InvocationContract::DeclaredTool
    };
    let Some(invocation) = local::invoke(request, contract, started)? else {
        return Ok((
            missing_imported_tool(request.tool_ref.as_ref(), started),
            None,
        ));
    };
    Ok((invocation.output, invocation.artifacts))
}

fn finalize_output(
    mut output: InvocationOutput,
    invocation_artifacts: Option<SkillArtifactContract>,
    data_operation: Option<&crate::tool_catalogs::native::PreparedDataOperation>,
    effects: &RuntimeEffectRegistry,
) -> Result<InvocationOutput, RuntimeError> {
    let artifacts = if let Some(operation) = data_operation {
        if output.succeeded()
            && let Err(error) = operation.validate_result(&output.value)
        {
            reject_invalid_provider_output(&mut output, error);
            return Ok(output);
        }
        crate::tool_catalogs::native::artifacts(operation.tool_ref(), effects)
    } else {
        invocation_artifacts
    };
    if output.succeeded() {
        crate::output_contract::apply_artifact_outputs(&mut output.value, artifacts.as_ref());
        if let Some(ephemeral) = output.ephemeral.as_value_mut() {
            crate::output_contract::apply_artifact_outputs(ephemeral, artifacts.as_ref());
        }
    }
    Ok(output)
}

fn invoke_native_tool(
    request: &mut ToolDispatchRequest<'_>,
    data_source_binding: Option<JsonObject>,
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
            data_source_binding,
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

fn reject_invalid_provider_output(output: &mut InvocationOutput, error: RuntimeError) {
    output.reject(error.to_string());
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
    crate::config::resolve_runx_workspace_base(env, fallback)
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

pub(super) fn string_input<'a>(object: &'a JsonObject, key: &str) -> Option<&'a str> {
    match object.get(key) {
        Some(JsonValue::String(value)) if !value.trim().is_empty() => Some(value.trim()),
        _ => None,
    }
}

#[cfg(test)]
mod containment_tests;
#[cfg(test)]
mod tests;
