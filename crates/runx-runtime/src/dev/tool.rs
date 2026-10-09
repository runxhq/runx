// Module rationale: deterministic dev tool fixtures prepare fixture-owned
// inputs and expectations, then enter the same catalog dispatcher as live
// graph and managed-agent calls.
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

use runx_contracts::{JsonObject, JsonValue};

use super::r#loop::{
    assert_fixture_expectation, failed_fixture, prepare_fixture_workspace,
    resolve_fixture_execution_roots,
};
use super::materialize::{materialize_fixture_string, materialize_fixture_value};
use super::support::elapsed_ms;
use super::types::{DevError, DevFixtureResult, LoadedDevFixture};
use super::types::{
    DevFixtureAssertion, DevFixtureAssertionKind, DevFixtureExecutionRoots, DevFixtureStatus,
    PreparedDevFixtureWorkspace,
};
use crate::tool_catalogs::{ToolCatalogError, ToolInspectOptions, resolve_local_tool};

pub(super) fn run_tool_fixture(
    root: &Path,
    fixture: &LoadedDevFixture,
    base_env: &BTreeMap<String, String>,
) -> Result<DevFixtureResult, DevError> {
    let started = Instant::now();
    let reference = fixture.target().reference.as_str();
    let resolution = match resolve_local_tool(&ToolInspectOptions {
        root: root.to_path_buf(),
        tool_ref: reference.to_owned(),
        source: None,
        search_from_directory: root.to_path_buf(),
        tool_roots: vec![root.join("tools")],
        allow_explicit_manifest_path: false,
    }) {
        Ok(resolution) => resolution,
        Err(ToolCatalogError::NotFound(_)) => {
            return Ok(unknown_tool_ref(fixture, started, reference));
        }
        Err(error) => {
            return Err(crate::RuntimeError::SkillFailed {
                skill_name: "runx-dev".to_owned(),
                message: error.to_string(),
            }
            .into());
        }
    };
    let workspace =
        prepare_fixture_workspace(root, &fixture.path, fixture.definition.workspace.as_ref())?;
    let result = run_tool_fixture_inner(root, fixture, &resolution, &workspace, base_env, started);
    if let Some(workspace_root) = &workspace.root {
        let _ = fs::remove_dir_all(workspace_root);
    }
    result
}

fn run_tool_fixture_inner(
    root: &Path,
    fixture: &LoadedDevFixture,
    resolution: &crate::tool_catalogs::LocalToolResolution,
    workspace: &PreparedDevFixtureWorkspace,
    base_env: &BTreeMap<String, String>,
    started: Instant,
) -> Result<DevFixtureResult, DevError> {
    let tool_dir = resolution
        .manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.to_path_buf());
    let Some(execution_roots) =
        resolve_fixture_execution_roots(root, fixture.lane(), workspace.root.as_deref())
    else {
        return Ok(missing_execution_roots(fixture, started));
    };
    let env = tool_fixture_env(root, fixture, workspace, &execution_roots, base_env);
    let inputs = materialize_fixture_value(
        JsonValue::Object(fixture.definition.inputs.clone()),
        &workspace.tokens,
    );
    let JsonValue::Object(inputs) = inputs else {
        unreachable!("fixture inputs are always an object")
    };
    let credential_delivery = crate::credentials::CredentialDelivery::none();
    let local_artifacts = crate::services::LocalArtifactService::default();
    let mut output = crate::tool_catalogs::dispatch::dispatch_tool(
        crate::tool_catalogs::dispatch::ToolDispatchRequest {
            tool_ref: Cow::Borrowed(fixture.target().reference.as_str()),
            inputs: Cow::Owned(inputs),
            resolved_inputs: Cow::Owned(JsonObject::new()),
            scopes: &resolution.tool.scopes,
            env: &env,
            skill_directory: &tool_dir,
            credential_delivery: &credential_delivery,
            local_artifacts: &local_artifacts,
            skill_name: "runx-dev",
            allow_explicit_manifest_path: false,
            effect_admission: None,
        },
        &crate::effects::RuntimeEffectRegistry::empty(),
        crate::time::DEFAULT_CREATED_AT,
        started,
    )?;
    if output.succeeded() {
        let verification = crate::output_contract::verified_output_metadata_with_artifacts(
            "runx-dev",
            &output.value,
            None,
            resolution.tool.artifacts.as_ref(),
            &tool_dir,
            &env,
        )
        .and_then(|metadata| {
            crate::output_contract::attach_verified_metadata(&mut output, metadata)
        });
        if let Err(error) = verification {
            output.reject(error.to_string());
        }
    }
    Ok(tool_result_from_execution(fixture, started, output))
}

fn tool_fixture_env(
    root: &Path,
    fixture: &LoadedDevFixture,
    workspace: &PreparedDevFixtureWorkspace,
    roots: &DevFixtureExecutionRoots,
    base_env: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = base_env.clone();
    env.extend(materialize_fixture_env(
        &fixture.definition.env,
        &workspace.tokens,
    ));
    env.insert(
        "RUNX_CWD".to_owned(),
        roots.cwd.to_string_lossy().into_owned(),
    );
    env.insert(
        "RUNX_REPO_ROOT".to_owned(),
        roots.repo_root.to_string_lossy().into_owned(),
    );
    env.insert(
        "RUNX_TOOL_ROOTS".to_owned(),
        root.join("tools").to_string_lossy().into_owned(),
    );
    if let Some(workspace_root) = &workspace.root {
        env.insert(
            "RUNX_FIXTURE_ROOT".to_owned(),
            workspace_root.to_string_lossy().into_owned(),
        );
    }
    env
}

fn tool_result_from_execution(
    fixture: &LoadedDevFixture,
    started: Instant,
    execution: crate::adapter::InvocationOutput,
) -> DevFixtureResult {
    let exit_code = execution
        .exit_code()
        .unwrap_or(if execution.succeeded() { 0 } else { 1 });
    let output = Some(execution.value);
    let assertions =
        assert_fixture_expectation(&fixture.definition.expect, exit_code, output.as_ref());
    DevFixtureResult {
        name: fixture.name().to_owned(),
        lane: fixture.lane().as_str().to_owned(),
        target: fixture.target_json(),
        status: if assertions.is_empty() {
            DevFixtureStatus::Success
        } else {
            DevFixtureStatus::Failure
        },
        duration_ms: elapsed_ms(started),
        assertions,
        skip_reason: None,
        output,
        replay_path: None,
    }
}

fn unknown_tool_ref(
    fixture: &LoadedDevFixture,
    started: Instant,
    reference: &str,
) -> DevFixtureResult {
    failed_fixture(
        fixture,
        started,
        vec![DevFixtureAssertion {
            path: "target.ref".to_owned(),
            expected: Some(JsonValue::String("existing tool".to_owned())),
            actual: Some(JsonValue::String(reference.to_owned())),
            kind: DevFixtureAssertionKind::ExactMismatch,
            message: format!("Tool {reference} was not found."),
        }],
    )
}

fn missing_execution_roots(fixture: &LoadedDevFixture, started: Instant) -> DevFixtureResult {
    failed_fixture(
        fixture,
        started,
        vec![DevFixtureAssertion {
            path: "repo".to_owned(),
            expected: Some(JsonValue::String("repo or workspace fixture".to_owned())),
            actual: Some(JsonValue::String("missing".to_owned())),
            kind: DevFixtureAssertionKind::ExactMismatch,
            message: "repo-integration fixtures must declare repo or workspace contents."
                .to_owned(),
        }],
    )
}

fn materialize_fixture_env(
    object: &JsonObject,
    tokens: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    object
        .iter()
        .filter_map(|(key, value)| materialize_env_entry(key, value, tokens))
        .collect()
}

fn materialize_env_entry(
    key: &str,
    value: &JsonValue,
    tokens: &BTreeMap<String, String>,
) -> Option<(String, String)> {
    match value {
        JsonValue::Null => None,
        JsonValue::String(value) => {
            Some((key.to_owned(), materialize_fixture_string(value, tokens)))
        }
        other => Some((
            key.to_owned(),
            materialize_fixture_string(&json_display(other), tokens),
        )),
    }
}

fn json_display(value: &JsonValue) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}
