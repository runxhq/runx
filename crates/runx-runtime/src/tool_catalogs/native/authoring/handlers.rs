use std::path::Path;

use runx_contracts::{JsonValue, SkillAuthoringBrief};

use super::super::{NativeInvocation, invalid_input, resolve_root_for};
use super::inputs::{ApplyInput, BindInput, InspectInput, ValidateInput};
use super::outputs::{ApplyOutput, AuthoringInspectOutput, BindOutput, ValidateOutput};
use super::{APPLY_TOOL, VALIDATE_TOOL};
use crate::RuntimeError;

pub(super) fn inspect_skill(
    invocation: &NativeInvocation<'_, InspectInput>,
) -> Result<AuthoringInspectOutput, RuntimeError> {
    let workspace = resolve_root_for(
        "runx.skill.inspect",
        ".",
        invocation.env,
        invocation.skill_directory,
        invocation.reach,
    )?;
    let authoring_context = crate::services::inspect_skill_workspace(
        &workspace,
        invocation.inputs.target_dir.as_deref(),
        invocation.effects,
    )?;
    Ok(AuthoringInspectOutput {
        authoring_brief: SkillAuthoringBrief::from(&authoring_context),
        authoring_context,
    })
}

pub(super) fn validate_skill(
    invocation: &NativeInvocation<'_, ValidateInput>,
) -> Result<ValidateOutput, RuntimeError> {
    let workspace = resolve_root_for(
        VALIDATE_TOOL,
        ".",
        invocation.env,
        invocation.skill_directory,
        invocation.reach,
    )?;
    let candidate_files = invocation.inputs.candidate_files.as_deref();
    let requested_path = requested_skill_path(invocation, candidate_files)?;
    if candidate_files.is_none()
        && names_registry_package(&workspace, invocation.skill_directory, requested_path)
    {
        let skill_validation = crate::services::validate_registry_package(
            &workspace,
            requested_path,
            invocation.inputs.allow_execute_test,
            invocation.env,
            invocation.effects,
        )?;
        return Ok(ValidateOutput { skill_validation });
    }
    let (validation_root, resolved_path) = if candidate_files.is_none() {
        resolve_skill_validation_target(&workspace, invocation.skill_directory, requested_path)?
    } else {
        (workspace.clone(), requested_path.to_owned())
    };
    let skill_validation = crate::services::validate_skill_package(
        &validation_root,
        &resolved_path,
        candidate_files,
        invocation.inputs.allow_execute_test,
        invocation.env,
        invocation.effects,
    )?;
    Ok(ValidateOutput { skill_validation })
}

/// Whether `requested` names a registry package rather than a path: a
/// `<publisher>/<name>@<version>` reference, with or without `registry:`,
/// that names nothing in the workspace or the owning skill's catalog, as a
/// workspace copy shadows a bare reference everywhere.
fn names_registry_package(workspace_root: &Path, skill_directory: &Path, requested: &str) -> bool {
    let parsed = crate::registry::parse_registry_ref(requested);
    parsed.version.is_some()
        && crate::registry::split_skill_id(&parsed.skill_id).is_ok()
        && !Path::new(requested).is_absolute()
        && !workspace_root.join(requested).exists()
        && !skill_directory.join(requested).exists()
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

pub(super) fn bind_skill(
    invocation: &NativeInvocation<'_, BindInput>,
) -> Result<BindOutput, RuntimeError> {
    let mut draft = invocation.inputs.change_draft.clone();
    if let Some(call) = &invocation.inputs.script_call {
        keep_script(
            invocation,
            &invocation.inputs.authoring_context,
            call,
            &mut draft,
        )?;
    }
    let bundle = crate::services::bind_skill_change(
        &invocation.inputs.authoring_context,
        invocation.inputs.mode,
        draft,
    )?;
    Ok(BindOutput {
        change_bundle: bundle,
    })
}

const BIND_TOOL: &str = "runx.skill.bind";

/// The module a package kept from a turn's script call runs: the call's
/// source, named as the turn's script was.
const KEPT_MODULE: &str = "script.mjs";

/// Make `draft` keep the script call `call` read as its package's module:
/// the call's source, byte for byte, which runx writes, not the author. A
/// draft that writes the module with other bytes is refused; one that writes
/// none gets it.
fn keep_script(
    invocation: &NativeInvocation<'_, BindInput>,
    context: &runx_contracts::SkillAuthoringContext,
    call: &runx_contracts::Artifact,
    draft: &mut runx_contracts::SkillChangeDraft,
) -> Result<(), RuntimeError> {
    let refused = |message: String| RuntimeError::failed(BIND_TOOL, message);
    let artifacts =
        crate::paths::RunxPaths::resolve(invocation.env, invocation.skill_directory).artifacts();
    let read: runx_contracts::CallRead = crate::services::read_json_artifact(&artifacts, call)
        .map_err(&refused)?
        .deserialize_into()
        .map_err(|error| refused(format!("script_call is no call read: {error}")))?;
    let target = context.target_dir.as_deref().ok_or_else(|| {
        refused("a kept script needs a package: inspect its target_dir".to_owned())
    })?;
    keep_source(&read, target, draft).map_err(|failure| RuntimeError::Failed(Box::new(failure)))
}

/// Keep `read`'s source as the module of the package at `target` in
/// `draft`, byte for byte.
fn keep_source(
    read: &runx_contracts::CallRead,
    target: &str,
    draft: &mut runx_contracts::SkillChangeDraft,
) -> Result<(), crate::Failure> {
    let refused = |reason: String| crate::Failure::new(BIND_TOOL, reason);
    // Only a draft that writes a package keeps the module; one that finds
    // no skill, or needs core, writes nothing.
    if draft.decision != runx_contracts::SkillChangeDecision::Write {
        return Ok(());
    }
    if read.call.target != runx_contracts::CallTarget::Script {
        return Err(refused(format!(
            "call {} is no script call, so it keeps no module",
            read.call.address
        )));
    }
    let source = read
        .call
        .input
        .get("source")
        .and_then(runx_contracts::JsonValue::as_str)
        .ok_or_else(|| refused(format!("call {} holds no source", read.call.address)))?
        .to_owned();
    let path = format!("{target}/{KEPT_MODULE}");
    match draft
        .writes
        .iter()
        .find(|write| write.path.as_str() == path)
    {
        Some(write) if write.contents == source => {}
        Some(_) => {
            return Err(refused(format!(
                "{path} is the script call's source, byte for byte, and the draft writes it otherwise"
            ))
            .next(format!("leave {KEPT_MODULE} out of the draft: runx writes it")));
        }
        None => draft.writes.push(runx_contracts::SkillFileWrite {
            path: path.into(),
            contents: source,
        }),
    }
    Ok(())
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
    let workspace = resolve_root_for(
        APPLY_TOOL,
        ".",
        invocation.env,
        invocation.skill_directory,
        invocation.reach,
    )?;
    crate::services::skill_change_effect(
        &workspace,
        &invocation.inputs.target_dir,
        invocation.inputs.mode,
        &invocation.inputs.change_bundle,
        invocation.reach,
    )
}

pub(super) fn apply_skill(
    invocation: &NativeInvocation<'_, ApplyInput>,
) -> Result<ApplyOutput, RuntimeError> {
    let workspace = resolve_root_for(
        APPLY_TOOL,
        ".",
        invocation.env,
        invocation.skill_directory,
        invocation.reach,
    )?;
    let report = crate::services::apply_skill_change(
        &workspace,
        &invocation.inputs.target_dir,
        invocation.inputs.mode,
        &invocation.inputs.change_bundle,
        invocation.env,
        invocation.effects,
    )?;
    Ok(ApplyOutput {
        apply_result: report,
    })
}

#[cfg(test)]
mod kept_tests {
    use super::keep_source;

    fn read(source: &str) -> Result<runx_contracts::CallRead, serde_json::Error> {
        serde_json::from_value(serde_json::json!({
            "schema": "runx.call_read.v1",
            "run_id": "run_1",
            "attempt": 1,
            "call": {
                "address": "answer/0",
                "target": {"kind": "script"},
                "input": {"source": source},
                "outcome": {"status": "cancelled"},
            },
        }))
    }

    fn draft(
        writes: serde_json::Value,
    ) -> Result<runx_contracts::SkillChangeDraft, serde_json::Error> {
        serde_json::from_value(serde_json::json!({
            "schema": "runx.skill.change_draft.v1",
            "decision": "write",
            "summary": "Keep the script.",
            "writes": writes,
            "deletes": [],
        }))
    }

    /// The module of a kept script is runx's to write, the call's source
    /// byte for byte: written for a draft that leaves it out, kept for one
    /// that writes it as it is, and refused for one that writes it
    /// otherwise.
    #[test]
    fn a_kept_scripts_module_is_its_source_byte_for_byte() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = "export default async function () { return {}; }\n";
        let mut left_out = draft(serde_json::json!([]))?;
        assert!(keep_source(&read(source)?, "kept", &mut left_out).is_ok());
        assert_eq!(left_out.writes[0].path.as_str(), "kept/script.mjs");
        assert_eq!(left_out.writes[0].contents, source);
        let mut same = draft(serde_json::json!([{"path": "kept/script.mjs", "contents": source}]))?;
        assert!(keep_source(&read(source)?, "kept", &mut same).is_ok());
        assert_eq!(same.writes.len(), 1);
        let mut other = draft(
            serde_json::json!([{"path": "kept/script.mjs", "contents": "export default 1;\n"}]),
        )?;
        let refused = keep_source(&read(source)?, "kept", &mut other)
            .err()
            .map(|failure| failure.to_string())
            .unwrap_or_default();
        assert!(
            refused.contains("byte for byte") && refused.contains("Next: leave script.mjs out"),
            "{refused}"
        );
        // A save that writes no package keeps no module.
        let mut no_skill = draft(serde_json::json!([]))?;
        no_skill.decision = runx_contracts::SkillChangeDecision::NoSkill;
        assert!(keep_source(&read(source)?, "kept", &mut no_skill).is_ok());
        assert!(no_skill.writes.is_empty());
        Ok(())
    }
}
