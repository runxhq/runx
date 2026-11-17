use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{
    JsonObject, JsonValue, SkillValidation, SkillValidationSchema, SkillValidationVerdict,
};

use super::path::{canonical_directory, display_repo_path, invalid_skill_change};
use super::staging::{InlineCandidateStage, ValidationReceiptStage, isolated_test_env};
use crate::effects::RuntimeEffectRegistry;
use crate::{PackageTestReport, RuntimeError, TestProcesses};

/// What validating a candidate package found: its inspection, and how each
/// of its cases came out.
pub(super) struct CandidateValidation {
    pub(super) inspect: JsonObject,
    pub(super) test: PackageTestReport,
}

impl CandidateValidation {
    /// The validation of a candidate that may be written: none of its cases
    /// failed or stopped short, so nothing writes a failing package.
    pub(super) fn applicable(self) -> Result<Self, RuntimeError> {
        match self.test.status {
            "failed" => Err(invalid_skill_change(format!(
                "native test failed: {}",
                self.test.failures().join("; ")
            ))),
            "inconclusive" => Err(invalid_skill_change(
                "native test was inconclusive: a case reaches a process the test refused",
            )),
            _ => Ok(self),
        }
    }
}

/// A candidate's inspection and its test, each case run in the candidate's
/// isolated home, a process only where `processes` lets it run.
pub(super) fn validate_candidate(
    workspace: &Path,
    candidate: &Path,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
    processes: TestProcesses,
    authored: Option<&Authored>,
) -> Result<CandidateValidation, RuntimeError> {
    let loaded = crate::load_validated_skill_package(candidate)?;
    if let Some(authored) = authored {
        authored.check(&loaded.package, loaded.manifest())?;
    }
    let inspect = crate::inspect_skill_package(candidate, None, Some(env))
        .map_err(|error| invalid_skill_change(format!("skill inspection failed: {error}")))
        .and_then(|inspection| {
            serde_json::to_value(&inspection)
                .and_then(serde_json::from_value::<JsonObject>)
                .map_err(|error| invalid_skill_change(format!("skill inspection failed: {error}")))
        })?;
    let test = run_candidate_test(workspace, candidate, processes, env, effects)?;
    Ok(CandidateValidation { inspect, test })
}

/// What an authored package must say of itself, beyond loading: its
/// catalog, which states its visibility; where a package stood at its
/// target, the name it had, which a change to it keeps; and that every turn
/// the change writes reads the person's memory. The turns that stood
/// already, by method and step, are the person's to change.
pub(super) struct Authored {
    pub(super) keeps_name: Option<String>,
    pub(super) standing_turns: std::collections::BTreeSet<(String, String)>,
}

/// Each agent turn `manifest` holds, by method and step.
pub(super) fn turns(
    manifest: &runx_parser::SkillMethodManifest,
) -> Vec<(String, &runx_parser::WorkflowStep)> {
    manifest
        .methods
        .iter()
        .flat_map(|(method, definition)| {
            definition
                .workflow
                .steps
                .iter()
                .filter(|step| matches!(step.kind, runx_parser::StepKind::Agent(_)))
                .map(move |step| (method.clone(), step))
        })
        .collect()
}

/// Whether `turn` reads the person's memory: its context's entries are
/// those of a step in `steps` that calls `runx/memory#index`.
fn reads_memory(turn: &runx_parser::WorkflowStep, steps: &[runx_parser::WorkflowStep]) -> bool {
    let runx_parser::StepKind::Agent(task) = &turn.kind else {
        return true;
    };
    task.context_entries.iter().any(|entry| {
        let runx_parser::Source::Step { step, path } = entry else {
            return false;
        };
        path.first().map(String::as_str) == Some("entries")
            && steps.iter().any(|recall| {
                recall.id == *step
                    && matches!(&recall.kind, runx_parser::StepKind::Skill(target)
                        if target.skill == "runx/memory" && target.method.as_deref() == Some("index"))
            })
    })
}

impl Authored {
    fn check(
        &self,
        package: &runx_parser::ValidatedSkillPackage,
        manifest: Option<&runx_parser::SkillMethodManifest>,
    ) -> Result<(), RuntimeError> {
        if let Some(name) = &self.keeps_name
            && package.skill.name != *name
        {
            return Err(invalid_skill_change(format!(
                "a change to {name} keeps its name, and the candidate names itself {}: a rename is the person's own act",
                package.skill.name
            )));
        }
        // The package loader enforces a public catalog's readiness; an
        // authored package states its catalog rather than leaving its
        // visibility to a default.
        let Some(manifest) = manifest.filter(|manifest| manifest.catalog.is_some()) else {
            return Err(invalid_skill_change(
                "an authored package states its catalog in run.yaml, its visibility public or internal",
            ));
        };
        for (method, turn) in turns(manifest) {
            let written = !self
                .standing_turns
                .contains(&(method.clone(), turn.id.clone()));
            if written && !reads_memory(turn, &manifest.methods[&method].workflow.steps) {
                return Err(invalid_skill_change(format!(
                    "methods.{method}.steps.{}: every turn a change writes reads the person's memory, from a step on runx/memory#index at the personal and workspace levels, its entries in the turn's context.entries",
                    turn.id
                )));
            }
        }
        Ok(())
    }
}

fn run_candidate_test(
    workspace: &Path,
    candidate: &Path,
    processes: TestProcesses,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<PackageTestReport, RuntimeError> {
    let receipt_stage = ValidationReceiptStage::prepare(workspace)?;
    let mut test_env = isolated_test_env(workspace, env, receipt_stage.root());
    crate::services::merge_inferred_tool_roots(&mut test_env, candidate);
    install_candidate_closure(workspace, candidate, &test_env)?;
    crate::orchestrator::LocalOrchestrator::with_effects_and_environment(effects.clone(), test_env)
        .run_package_test(&crate::orchestrator::PackageTestRequest {
            skill_path: candidate.to_path_buf(),
            receipt_dir: Some(receipt_stage.receipt_dir.clone()),
            processes,
        })
        .map_err(|error| invalid_skill_change(format!("native test failed: {error}")))
}

/// The registry skills a candidate's closure names arrive as a publish's do:
/// each read by its record from the registry the operator selects, and
/// installed, verified, into the candidate's own project, where its test
/// reads them. A candidate naming none reads nothing.
fn install_candidate_closure(
    workspace: &Path,
    candidate: &Path,
    env: &BTreeMap<String, String>,
) -> Result<(), RuntimeError> {
    let subject = candidate.to_string_lossy();
    let request = crate::registry::RegistryPublishPackageRequest {
        subject: &subject,
        env,
        cwd: workspace,
    };
    let unbound = |error: String| {
        invalid_skill_change(format!("binding the candidate's registry closure: {error}"))
    };
    if crate::registry::unheld_publish_registry_refs(request)
        .map_err(|error| unbound(error.to_string()))?
        .is_empty()
    {
        return Ok(());
    }
    let target = crate::registry::resolve::resolve_registry_target(None, env)
        .map_err(|error| unbound(error.into_message()))?;
    let client = crate::registry::resolve::registry_client(&target.registry_url, env)
        .map_err(|error| unbound(error.into_message()))?;
    crate::registry::install_registry_closure(&client, &target.registry_url, request)
        .map_err(|error| unbound(error.to_string()))
}

pub(crate) fn validate_skill_package(
    workspace: &Path,
    requested_path: &str,
    candidate_files: Option<&[JsonValue]>,
    allow_execute_test: bool,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<SkillValidation, RuntimeError> {
    let workspace = canonical_directory(workspace, "skill workspace")?;
    let stage = candidate_files
        .map(|files| InlineCandidateStage::prepare(&workspace, files, env))
        .transpose()?;
    let candidate_path = resolve_candidate_path(&workspace, requested_path, stage.as_ref())?;
    let resolved_path = if stage.is_some() {
        "inline-candidate".to_owned()
    } else {
        display_repo_path(&workspace, &candidate_path)
    };
    Ok(validate_package_at(
        &workspace,
        &candidate_path,
        requested_path,
        &resolved_path,
        allow_execute_test,
        env,
        effects,
    ))
}

/// The released copy the registry reference `reference` names, read from
/// the registry the run reaches into runx's cache and never into the
/// workspace (N6), validated as a workspace package is. It resolves as the
/// reference resolves everywhere, and is reported by its released identity,
/// `<publisher>/<name>@<version>`.
///
/// # Errors
///
/// Returns why the reference doesn't resolve to a released copy.
pub(crate) fn validate_registry_package(
    workspace: &Path,
    reference: &str,
    allow_execute_test: bool,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<SkillValidation, RuntimeError> {
    let workspace = canonical_directory(workspace, "skill workspace")?;
    let resolved = crate::registry::resolve::resolve_skill_ref_details(
        Path::new(reference),
        &workspace,
        crate::registry::resolve::SkillResolverOptions {
            env,
            registry: None,
            expected_digest: None,
            absent_pins: crate::registry::resolve::AbsentPins::Report,
            holder: crate::registry::resolve::Holder::Cache,
        },
    )
    .map_err(|error| invalid_skill_change(format!("resolving {reference}: {error}")))?;
    let (Some(skill_id), Some(version)) = (&resolved.skill_id, &resolved.version) else {
        return Err(invalid_skill_change(format!(
            "{reference} names no released copy: a registry reference is <publisher>/<name>@<version>"
        )));
    };
    let released = format!("{skill_id}@{version}");
    let package = if resolved.runnable_path.ends_with("SKILL.md") {
        resolved
            .runnable_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| invalid_skill_change("SKILL.md has no package directory"))?
    } else {
        resolved.runnable_path.clone()
    };
    Ok(validate_package_at(
        &workspace,
        &package,
        reference,
        &released,
        allow_execute_test,
        env,
        effects,
    ))
}

/// The package at `candidate`, validated for the workspace at `workspace`:
/// its inspection, and its test, each case that reaches a process refused unless
/// the caller allows processes.
fn validate_package_at(
    workspace: &Path,
    candidate: &Path,
    requested_path: &str,
    resolved_path: &str,
    allow_execute_test: bool,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> SkillValidation {
    let inspected = crate::inspect_skill_package(candidate, None, Some(env));
    let processes = if allow_execute_test {
        TestProcesses::Run
    } else {
        TestProcesses::Refuse
    };
    let inspected = inspected.and_then(|inspection| {
        serde_json::to_value(&inspection)
            .and_then(serde_json::from_value)
            .map_err(|source| crate::SkillInspectionError::Json {
                context: "projecting the skill inspection",
                source,
            })
    });
    let result = validate_candidate(workspace, candidate, env, effects, processes, None);
    validation_result_report(requested_path, resolved_path, inspected, result)
}

fn validation_result_report(
    requested_path: &str,
    resolved_path: &str,
    inspected: Result<JsonObject, crate::SkillInspectionError>,
    result: Result<CandidateValidation, RuntimeError>,
) -> SkillValidation {
    let tested = result.and_then(|validation| {
        let test = JsonValue::from_serialize(&validation.test).map_err(|error| {
            invalid_skill_change(format!("projecting the package test report: {error}"))
        })?;
        match test {
            JsonValue::Object(test) => Ok((validation.inspect, test)),
            _ => Err(invalid_skill_change("a package test report is an object")),
        }
    });
    let (verdict, inspect, test) = match tested {
        Ok((inspect, test)) => (SkillValidationVerdict::Tested, inspect, test),
        Err(error) => (
            SkillValidationVerdict::ValidationFailed,
            failed_inspection(inspected),
            JsonObject::from([
                ("status".to_owned(), JsonValue::String("failed".to_owned())),
                ("reason".to_owned(), JsonValue::String(error.to_string())),
            ]),
        ),
    };
    SkillValidation {
        schema: SkillValidationSchema::V1,
        requested_path: requested_path.into(),
        resolved_path: resolved_path.into(),
        verdict,
        inspect,
        test,
    }
}

fn failed_inspection(inspected: Result<JsonObject, crate::SkillInspectionError>) -> JsonObject {
    inspected.unwrap_or_else(|error| {
        JsonObject::from([
            ("status".to_owned(), JsonValue::String("failed".to_owned())),
            ("error".to_owned(), JsonValue::String(error.to_string())),
        ])
    })
}

fn resolve_candidate_path(
    workspace: &Path,
    requested_path: &str,
    stage: Option<&InlineCandidateStage>,
) -> Result<std::path::PathBuf, RuntimeError> {
    if let Some(stage) = stage {
        return Ok(stage.skill_dir.clone());
    }
    let requested = Path::new(requested_path);
    let unresolved = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        workspace.join(requested)
    };
    let resolved = fs::canonicalize(&unresolved).map_err(|source| {
        RuntimeError::io(
            format!("resolving skill candidate {}", unresolved.display()),
            source,
        )
    })?;
    if resolved != workspace && !resolved.starts_with(workspace) {
        return Err(invalid_skill_change(
            "skill candidate must stay inside the workspace",
        ));
    }
    if resolved.file_name().and_then(|value| value.to_str()) == Some("SKILL.md") {
        return resolved
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| invalid_skill_change("SKILL.md has no package directory"));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    #[test]
    fn candidate_inspection_resolves_the_workspaces_pinned_installed_connector()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = tempfile::tempdir()?;
        let paths = crate::RunxPaths::resolve(&std::collections::BTreeMap::new(), repo.path());
        let installed = paths.added_connectors().join("slack/1.0.0");
        std::fs::create_dir_all(&installed)?;
        std::fs::write(
            installed.join("connector.yaml"),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../connectors/slack/connector.yaml"
            )),
        )?;
        let connector = crate::kernel::connectors::ConnectorScope::in_project(repo.path(), &paths)?
            .named("slack@1.0.0")?;
        let candidate = repo.path().join("skills/probe");
        std::fs::create_dir_all(&candidate)?;
        std::fs::write(
            candidate.join("SKILL.md"),
            "---\nname: probe\ndescription: Inspect an installed dependency.\n---\n# Probe\n",
        )?;
        std::fs::write(
            candidate.join("run.yaml"),
            format!(
                "minimum_runx: 0.12.0\nconnectors:\n  slack:\n    manifest_digest: {}\nmethods:\n  inspect:\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs: {{value: ready}}\n      - id: post\n        operation: slack/channel.post\n        idempotency_key: $run.id\n",
                connector.manifest_digest
            ),
        )?;
        let env = std::collections::BTreeMap::from([(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            repo.path().display().to_string(),
        )]);
        let inspection = crate::inspect_skill_package(&candidate, None, Some(&env))?;
        assert_eq!(inspection.name, "probe");
        Ok(())
    }

    #[test]
    fn a_candidate_is_applicable_only_when_no_case_failed_or_was_inconclusive() {
        let validation = |status: &'static str| super::CandidateValidation {
            inspect: runx_contracts::JsonObject::new(),
            test: crate::PackageTestReport {
                status,
                cases: vec![crate::PackageTestCase {
                    name: "case".to_owned(),
                    status,
                    failures: vec!["expected ready".to_owned()],
                }],
                case_count: 1,
                assertion_error_count: 1,
                receipt_ids: Vec::new(),
                workflow_case_count: 0,
            },
        };
        let refusal = validation("failed")
            .applicable()
            .err()
            .map(|error| error.to_string());
        assert!(refusal.is_some_and(|error| error.contains("case: expected ready")));
        assert!(validation("inconclusive").applicable().is_err());
        assert!(validation("passed").applicable().is_ok());
    }
}
