use std::path::Path;

use runx_contracts::JsonValue;

use super::super::{NativeInvocation, invalid_input, resolve_repo_root_for};
use super::inputs::{ApplyInput, BindInput, InspectInput, PlanInput, ValidateInput};
use super::outputs::{ApplyOutput, AuthoringInspectOutput, BindOutput, PlanOutput, ValidateOutput};
use super::{APPLY_TOOL, VALIDATE_TOOL};
use crate::RuntimeError;

pub(super) fn inspect_skill(
    invocation: &NativeInvocation<'_, InspectInput>,
) -> Result<AuthoringInspectOutput, RuntimeError> {
    let repo_root = resolve_repo_root_for(
        "runx.skill.inspect",
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let authoring_context = crate::services::inspect_skill_workspace(
        &repo_root,
        invocation.inputs.target_dir.as_deref(),
        invocation.effects,
    )?;
    Ok(AuthoringInspectOutput { authoring_context })
}

pub(super) fn validate_skill(
    invocation: &NativeInvocation<'_, ValidateInput>,
) -> Result<ValidateOutput, RuntimeError> {
    let repo_root = resolve_repo_root_for(
        VALIDATE_TOOL,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let candidate_files = invocation.inputs.candidate_files.as_deref();
    let requested_path = requested_skill_path(invocation, candidate_files)?;
    let (validation_root, resolved_path) = if candidate_files.is_none() {
        resolve_skill_validation_target(&repo_root, invocation.skill_directory, requested_path)?
    } else {
        (repo_root.clone(), requested_path.to_owned())
    };
    let skill_validation = crate::services::validate_skill_package(
        &validation_root,
        &resolved_path,
        candidate_files,
        invocation.inputs.allow_execute_harness,
        invocation.env,
        invocation.effects,
    )?;
    Ok(ValidateOutput { skill_validation })
}

fn requested_skill_path<'a>(
    invocation: &'a NativeInvocation<'_, ValidateInput>,
    candidate_files: Option<&[JsonValue]>,
) -> Result<&'a str, RuntimeError> {
    let supplied_path = invocation.inputs.skill_path.as_deref();
    let requested_path = supplied_path.unwrap_or(if candidate_files.is_some() {
        "inline-candidate"
    } else {
        ""
    });
    if requested_path.is_empty() && candidate_files.is_none() {
        return Err(invalid_input(
            VALIDATE_TOOL,
            "skill_path or candidate_files is required",
        ));
    }
    if supplied_path.is_some() && candidate_files.is_some() {
        return Err(invalid_input(
            VALIDATE_TOOL,
            "skill_path and candidate_files are mutually exclusive",
        ));
    }
    Ok(requested_path)
}

pub(super) fn plan_skill(
    invocation: &NativeInvocation<'_, PlanInput>,
) -> Result<PlanOutput, RuntimeError> {
    let plan = crate::services::plan_skill_architecture(
        &invocation.inputs.base_digest,
        invocation.inputs.architecture.clone(),
    )?;
    Ok(PlanOutput {
        architecture_plan: plan,
    })
}

pub(super) fn bind_skill(
    invocation: &NativeInvocation<'_, BindInput>,
) -> Result<BindOutput, RuntimeError> {
    let bundle = crate::services::bind_skill_change(
        &invocation.inputs.architecture_plan,
        invocation.inputs.change_draft.clone(),
    )?;
    Ok(BindOutput {
        change_bundle: bundle,
    })
}

fn resolve_skill_validation_target(
    workspace_root: &Path,
    skill_directory: &Path,
    requested_path: &str,
) -> Result<(std::path::PathBuf, String), RuntimeError> {
    let requested = Path::new(requested_path);
    if requested.is_absolute() || workspace_root.join(requested).exists() {
        return Ok((workspace_root.to_path_buf(), requested_path.to_owned()));
    }
    resolve_catalog_skill_target(skill_directory, requested, requested_path)
}

fn resolve_catalog_skill_target(
    skill_directory: &Path,
    requested: &Path,
    requested_path: &str,
) -> Result<(std::path::PathBuf, String), RuntimeError> {
    let catalog_root = skill_directory
        .parent()
        .ok_or_else(|| invalid_input(VALIDATE_TOOL, "owning skill has no catalog root"))?;
    let catalog_root = std::fs::canonicalize(catalog_root)
        .map_err(|source| RuntimeError::io("resolving owning skill catalog", source))?;
    let candidate = std::fs::canonicalize(skill_directory.join(requested)).map_err(|source| {
        RuntimeError::io(
            format!("resolving owning-skill reference {requested_path}"),
            source,
        )
    })?;
    if candidate != catalog_root && !candidate.starts_with(&catalog_root) {
        return Err(invalid_input(
            VALIDATE_TOOL,
            "owning-skill reference must stay inside its catalog",
        ));
    }
    let relative = candidate
        .strip_prefix(&catalog_root)
        .map_err(|_| invalid_input(VALIDATE_TOOL, "skill reference escaped its catalog"))?;
    Ok((
        catalog_root,
        relative
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    ))
}

/// What `runx.skill.apply` would change.
pub(super) fn apply_skill_effect(
    invocation: &NativeInvocation<'_, ApplyInput>,
) -> Result<Option<crate::capability::LocalEffect>, RuntimeError> {
    let repo_root = resolve_repo_root_for(
        APPLY_TOOL,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    crate::services::skill_change_effect(
        &repo_root,
        &invocation.inputs.target_dir,
        &invocation.inputs.change_bundle,
        invocation.bounds,
    )
}

pub(super) fn apply_skill(
    invocation: &NativeInvocation<'_, ApplyInput>,
) -> Result<ApplyOutput, RuntimeError> {
    let repo_root = resolve_repo_root_for(
        APPLY_TOOL,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let report = crate::services::apply_skill_change(
        &repo_root,
        &invocation.inputs.target_dir,
        &invocation.inputs.mode,
        &invocation.inputs.change_bundle,
        invocation.env,
        invocation.effects,
    )?;
    Ok(ApplyOutput {
        apply_result: report,
    })
}
