// Module rationale: deterministic dev tool fixtures prepare fixture-owned
// inputs and expectations, then enter the same catalog dispatcher as live
// graph and managed-agent calls.
#[cfg(feature = "catalog")]
use std::borrow::Cow;
use std::collections::BTreeMap;
#[cfg(feature = "catalog")]
use std::fs;
use std::path::Path;
#[cfg(feature = "catalog")]
use std::time::Instant;

#[cfg(feature = "catalog")]
use runx_contracts::{JsonObject, JsonValue};

#[cfg(feature = "catalog")]
use super::r#loop::{
    assert_fixture_expectation, failed_fixture, prepare_fixture_workspace,
    resolve_fixture_execution_roots,
};
#[cfg(feature = "catalog")]
use super::materialize::{materialize_fixture_string, materialize_fixture_value};
#[cfg(feature = "catalog")]
use super::support::elapsed_ms;
use super::types::{DevError, DevFixtureResult, LoadedDevFixture};
#[cfg(feature = "catalog")]
use super::types::{
    DevFixtureAssertion, DevFixtureAssertionKind, DevFixtureExecutionRoots, DevFixtureStatus,
    PreparedDevFixtureWorkspace,
};
#[cfg(feature = "catalog")]
use crate::tool_catalogs::{ToolCatalogError, ToolInspectOptions, resolve_local_tool};

pub(super) fn run_tool_fixture(
    root: &Path,
    fixture: &LoadedDevFixture,
    base_env: &BTreeMap<String, String>,
) -> Result<DevFixtureResult, DevError> {
    #[cfg(not(feature = "catalog"))]
    {
        let _ = (root, fixture, base_env);
        Err(crate::RuntimeError::SkillFailed {
            skill_name: "runx-dev".to_owned(),
            message: "tool fixtures require the runx-runtime catalog feature".to_owned(),
        }
        .into())
    }

    #[cfg(feature = "catalog")]
    {
        run_tool_fixture_with_catalog(root, fixture, base_env)
    }
}

#[cfg(feature = "catalog")]
fn run_tool_fixture_with_catalog(
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

#[cfg(feature = "catalog")]
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

#[cfg(feature = "catalog")]
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

#[cfg(feature = "catalog")]
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

#[cfg(feature = "catalog")]
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

#[cfg(feature = "catalog")]
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

#[cfg(feature = "catalog")]
fn materialize_fixture_env(
    object: &JsonObject,
    tokens: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    object
        .iter()
        .filter_map(|(key, value)| materialize_env_entry(key, value, tokens))
        .collect()
}

#[cfg(feature = "catalog")]
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

#[cfg(feature = "catalog")]
fn json_display(value: &JsonValue) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}
