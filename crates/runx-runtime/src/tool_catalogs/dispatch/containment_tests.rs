use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use runx_contracts::{JsonObject, JsonValue};

use super::{ToolDispatchRequest, dispatch_tool};
use crate::{
    CredentialDelivery, InvocationOutput, InvocationStatus, RuntimeEffectRegistry, RuntimeError,
};

fn invoke(
    tool_ref: &str,
    inputs: JsonObject,
    workspace: &Path,
    credential_delivery: CredentialDelivery,
) -> Result<InvocationOutput, RuntimeError> {
    let scopes: Vec<String> = crate::tool_catalogs::native::required_scopes(tool_ref)
        .map(|scopes| scopes.iter().map(|scope| (*scope).to_owned()).collect())
        .unwrap_or_default();
    invoke_with_scopes(tool_ref, inputs, workspace, credential_delivery, &scopes)
}

fn invoke_with_scopes(
    tool_ref: &str,
    inputs: JsonObject,
    workspace: &Path,
    credential_delivery: CredentialDelivery,
    scopes: &[String],
) -> Result<InvocationOutput, RuntimeError> {
    let env = BTreeMap::from([(
        "RUNX_CWD".to_owned(),
        workspace.to_string_lossy().into_owned(),
    )]);
    let local_artifacts = crate::services::LocalArtifactService::default();
    dispatch_tool(
        ToolDispatchRequest {
            tool_ref: Cow::Borrowed(tool_ref),
            inputs: Cow::Owned(inputs),
            resolved_inputs: Cow::Owned(JsonObject::new()),
            scopes,
            env: &env,
            skill_directory: workspace,
            credential_delivery: &credential_delivery,
            local_artifacts: &local_artifacts,
            skill_name: "architecture-containment",
            allow_explicit_manifest_path: false,
            effect_admission: None,
        },
        &RuntimeEffectRegistry::default(),
        "2026-01-01T00:00:00Z",
        Instant::now(),
    )
}

#[test]
fn native_capability_refuses_undeclared_owned_scope() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let output = invoke_with_scopes(
        "fs.read",
        JsonObject::from([(
            "path".to_owned(),
            JsonValue::String("missing.txt".to_owned()),
        )]),
        workspace.path(),
        CredentialDelivery::none(),
        &[],
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(
        output.failure_message().is_some_and(
            |message| message.contains("missing required scope declaration(s): fs.read")
        )
    );
    Ok(())
}

#[test]
fn scoped_native_command_does_not_invent_human_approval() -> Result<(), Box<dyn std::error::Error>>
{
    let workspace = tempfile::tempdir()?;
    let scopes = vec!["process.exec".to_owned()];
    let output = invoke_with_scopes(
        "command.execute",
        JsonObject::from([(
            "command".to_owned(),
            JsonValue::String("/usr/bin/true".to_owned()),
        )]),
        workspace.path(),
        CredentialDelivery::none(),
        &scopes,
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    Ok(())
}

#[test]
fn native_filesystem_reads_from_explicit_absolute_roots() -> Result<(), Box<dyn std::error::Error>>
{
    let workspace = tempfile::tempdir()?;
    let external = tempfile::tempdir()?;
    std::fs::write(
        external.path().join("operator-context.txt"),
        "bounded context",
    )?;
    let output = invoke(
        "fs.read",
        JsonObject::from([
            (
                "repo_root".to_owned(),
                JsonValue::String(external.path().to_string_lossy().into_owned()),
            ),
            (
                "path".to_owned(),
                JsonValue::String("operator-context.txt".to_owned()),
            ),
        ]),
        workspace.path(),
        CredentialDelivery::none(),
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    assert_eq!(
        output
            .value
            .as_object()
            .and_then(|value| value.get("contents"))
            .and_then(JsonValue::as_str),
        Some("bounded context")
    );
    Ok(())
}

#[test]
fn native_command_boundary_keeps_generic_commands_credential_free()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let delivery = CredentialDelivery::process_env(
        "example",
        "api_key",
        "EXAMPLE_TOKEN",
        "local:example:test",
        "credential-sentinel",
    )?;
    let output = invoke(
        "command.execute",
        JsonObject::from([(
            "command".to_owned(),
            JsonValue::String("/usr/bin/true".to_owned()),
        )]),
        workspace.path(),
        delivery,
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(
        output
            .failure_message()
            .is_some_and(|message| message.contains("not supported"))
    );
    Ok(())
}

#[test]
fn native_command_executes_exact_argv_under_process_supervision()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let output = invoke(
        "command.execute",
        JsonObject::from([(
            "command".to_owned(),
            JsonValue::String("/usr/bin/true".to_owned()),
        )]),
        workspace.path(),
        CredentialDelivery::none(),
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    assert_eq!(
        output.observations().boundary(),
        Some(runx_contracts::ExecutionBoundaryKind::TrustedHostProcess)
    );
    let payload = output.value;
    let execution = payload
        .as_object()
        .and_then(|value| value.get("command_execution"))
        .and_then(JsonValue::as_object)
        .ok_or("missing command execution packet")?;
    let decision = execution.get("decision").or_else(|| {
        execution
            .get("data")
            .and_then(JsonValue::as_object)
            .and_then(|data| data.get("decision"))
    });
    assert_eq!(decision, Some(&JsonValue::String("completed".to_owned())));
    Ok(())
}
