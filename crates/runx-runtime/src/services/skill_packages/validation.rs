use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue, SkillArchitectureDecision};

use super::path::{canonical_directory, display_repo_path, invalid_skill_change};
use super::staging::{InlineCandidateStage, ValidationReceiptStage, isolated_harness_env};
use crate::RuntimeError;
use crate::effects::RuntimeEffectRegistry;

mod resources;

use resources::{candidate_resource_usage, validate_architecture_resources};

pub(super) fn validate_candidate(
    repo_root: &Path,
    candidate: &Path,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
    run_harness: bool,
    architecture: Option<&SkillArchitectureDecision>,
) -> Result<JsonObject, RuntimeError> {
    let loaded = crate::load_validated_skill_package(candidate)?;
    let resources = candidate_resource_usage(&loaded.package);
    if let Some(architecture) = architecture {
        validate_architecture_resources(architecture, &resources)?;
    }
    let inspection = crate::inspect_skill_package(candidate, None, Some(env))
        .map_err(|error| invalid_skill_change(format!("skill inspection failed: {error}")))?;
    let harness = if run_harness {
        run_candidate_harness(repo_root, candidate, env, effects)?
    } else {
        skipped_harness()
    };
    Ok(JsonObject::from([
        ("inspect".to_owned(), inspection),
        ("harness".to_owned(), harness),
        ("resources".to_owned(), resources.as_json()),
    ]))
}

fn run_candidate_harness(
    repo_root: &Path,
    candidate: &Path,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<JsonValue, RuntimeError> {
    let receipt_stage = ValidationReceiptStage::prepare(repo_root)?;
    let mut harness_env = isolated_harness_env(repo_root, env, receipt_stage.root());
    crate::services::merge_inferred_tool_roots(&mut harness_env, candidate);
    let harness = crate::orchestrator::LocalOrchestrator::with_effects_and_environment(
        effects.clone(),
        harness_env,
    )
    .run_package_harness(&crate::orchestrator::PackageHarnessRequest {
        skill_path: candidate.to_path_buf(),
        receipt_dir: Some(receipt_stage.receipt_dir.clone()),
    })
    .map_err(|error| invalid_skill_change(format!("native harness failed: {error}")))?;
    if harness.status == "failed" {
        return Err(invalid_skill_change(format!(
            "native harness failed: {}",
            harness.assertion_errors.join("; ")
        )));
    }
    let encoded = serde_json::to_string(&harness)
        .map_err(|source| RuntimeError::json("serializing package harness report", source))?;
    serde_json::from_str(&encoded)
        .map_err(|source| RuntimeError::json("projecting package harness report", source))
}

fn skipped_harness() -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("attempted".to_owned(), JsonValue::Bool(false)),
        ("status".to_owned(), JsonValue::String("skipped".to_owned())),
        (
            "reason".to_owned(),
            JsonValue::String("needs_consequential_harness_approval".to_owned()),
        ),
    ]))
}

pub(crate) fn validate_skill_package(
    repo_root: &Path,
    requested_ref: &str,
    candidate_files: Option<&[JsonValue]>,
    allow_execute_harness: bool,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<JsonObject, RuntimeError> {
    let repo_root = canonical_directory(repo_root, "skill workspace")?;
    let stage = candidate_files
        .map(|files| InlineCandidateStage::prepare(&repo_root, files, env))
        .transpose()?;
    let candidate_path = resolve_candidate_path(&repo_root, requested_ref, stage.as_ref())?;
    let inspected = crate::inspect_skill_package(&candidate_path, None, Some(env));
    let run_harness = allow_execute_harness || safe_harness_execution(inspected.as_ref().ok());
    let resolved_ref = if stage.is_some() {
        "inline-candidate".to_owned()
    } else {
        display_repo_path(&repo_root, &candidate_path)
    };

    let result = validate_candidate(&repo_root, &candidate_path, env, effects, run_harness, None);
    Ok(validation_result_report(
        requested_ref,
        &resolved_ref,
        inspected,
        run_harness,
        result,
    ))
}

fn validation_result_report(
    requested_ref: &str,
    resolved_ref: &str,
    inspected: Result<JsonValue, crate::SkillInspectionError>,
    run_harness: bool,
    result: Result<JsonObject, RuntimeError>,
) -> JsonObject {
    match result {
        Ok(validation) => {
            let harness = validation
                .get("harness")
                .cloned()
                .unwrap_or(JsonValue::Null);
            let verdict = if harness
                .as_object()
                .and_then(|value| value.get("status"))
                .and_then(JsonValue::as_str)
                == Some("skipped")
            {
                "needs_consequential_harness_approval"
            } else {
                "tested"
            };
            skill_validation_report(
                requested_ref,
                resolved_ref,
                verdict,
                validation
                    .get("inspect")
                    .cloned()
                    .unwrap_or(JsonValue::Null),
                harness,
            )
        }
        Err(error) => skill_validation_report(
            requested_ref,
            resolved_ref,
            "validation_failed",
            failed_inspection(inspected),
            JsonValue::Object(JsonObject::from([
                ("attempted".to_owned(), JsonValue::Bool(run_harness)),
                ("status".to_owned(), JsonValue::String("failed".to_owned())),
                ("reason".to_owned(), JsonValue::String(error.to_string())),
            ])),
        ),
    }
}

fn failed_inspection(inspected: Result<JsonValue, crate::SkillInspectionError>) -> JsonValue {
    inspected.unwrap_or_else(|error| {
        JsonValue::Object(JsonObject::from([
            ("status".to_owned(), JsonValue::String("failed".to_owned())),
            ("error".to_owned(), JsonValue::String(error.to_string())),
        ]))
    })
}

fn resolve_candidate_path(
    repo_root: &Path,
    requested_ref: &str,
    stage: Option<&InlineCandidateStage>,
) -> Result<std::path::PathBuf, RuntimeError> {
    if let Some(stage) = stage {
        return Ok(stage.skill_dir.clone());
    }
    let requested = Path::new(requested_ref);
    let unresolved = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        repo_root.join(requested)
    };
    let resolved = fs::canonicalize(&unresolved).map_err(|source| {
        RuntimeError::io(
            format!("resolving skill candidate {}", unresolved.display()),
            source,
        )
    })?;
    if resolved != repo_root && !resolved.starts_with(repo_root) {
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

fn safe_harness_execution(inspection: Option<&JsonValue>) -> bool {
    inspection
        .and_then(JsonValue::as_object)
        .and_then(|inspection| inspection.get("capabilities"))
        .and_then(JsonValue::as_object)
        .and_then(|capabilities| capabilities.get("execution"))
        .and_then(JsonValue::as_str)
        .is_none_or(|execution| matches!(execution, "read" | "plan"))
}

fn skill_validation_report(
    requested_ref: &str,
    resolved_ref: &str,
    verdict: &str,
    inspection: JsonValue,
    harness: JsonValue,
) -> JsonObject {
    JsonObject::from([
        (
            "schema".to_owned(),
            JsonValue::String("runx.skill.validation.v1".to_owned()),
        ),
        (
            "requested_ref".to_owned(),
            JsonValue::String(requested_ref.to_owned()),
        ),
        (
            "resolved_ref".to_owned(),
            JsonValue::String(resolved_ref.to_owned()),
        ),
        ("verdict".to_owned(), JsonValue::String(verdict.to_owned())),
        ("inspect".to_owned(), inspection),
        ("harness".to_owned(), harness),
    ])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use runx_contracts::SkillArchitectureDecision;
    use serde_json::json;

    use super::resources::{CandidateResourceUsage, validate_architecture_resources};

    #[test]
    fn candidate_inspection_resolves_the_workspaces_pinned_installed_connector()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = tempfile::tempdir()?;
        let installed = repo.path().join("connectors/slack/1.0.0");
        std::fs::create_dir_all(&installed)?;
        std::fs::write(
            installed.join("connector.yaml"),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../connectors/slack/connector.yaml"
            )),
        )?;
        let connector = crate::kernel::connectors::ConnectorScope::at(repo.path())?
            .in_project(repo.path().join("connectors"))
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
                "skill: probe\nconnectors:\n  slack:\n    manifest_digest: {}\n    operations: [channel.post]\nrunners:\n  inspect:\n    default: true\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs: {{value: ready}}\n",
                connector.manifest_digest
            ),
        )?;
        let env = std::collections::BTreeMap::from([(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            repo.path().display().to_string(),
        )]);
        let report = super::validate_candidate(
            repo.path(),
            &candidate,
            &env,
            &crate::RuntimeEffectRegistry::default(),
            false,
            None,
        )?;
        assert!(
            report
                .get("inspect")
                .and_then(runx_contracts::JsonValue::as_object)
                .is_some()
        );
        Ok(())
    }

    fn architecture(domain_module: bool) -> SkillArchitectureDecision {
        serde_json::from_value(json!({
            "schema": "runx.skill.architecture_decision.v1",
            "disposition": "build",
            "identity": {
                "proposed_name": "demo",
                "action": "create",
                "visibility": "public",
                "rationale": "Demo is the exact operator-facing identity."
            },
            "direct_use": {
                "trigger_requests": ["Make one bounded demo decision."],
                "non_trigger_requests": ["Publish this decision."],
                "default_outcome": "Return one bounded decision.",
                "routine_host_work": ["Inspect the supplied objective."],
                "runx_boundary": "Bind result evidence in a receipt.",
                "terminal_result": "A reviewable demo decision.",
                "blocker_behavior": "Block once and name missing evidence.",
                "native_escape": "Return gathered evidence for native continuation."
            },
            "chain_use": {
                "accepted_inputs": ["A supplied objective or prior evidence packet."],
                "result": "A reusable demo decision.",
                "reused_evidence": ["Prior objective evidence."],
                "reused_effects": [],
                "must_not_repeat": ["Do not rediscover supplied evidence."]
            },
            "objective": "Create a bounded operator skill.",
            "operator_value": "Return one reviewable result.",
            "knowledge_contract": {
                "purpose": "Perform the bounded operation.",
                "evidence_required": ["A supplied objective."],
                "decision_logic": ["Preserve supplied evidence."],
                "stop_conditions": ["Stop when evidence is incomplete."],
                "recovery": ["Resume with the missing evidence."]
            },
            "required_behaviors": [{
                "id": "operate",
                "outcome": "Return one bounded result.",
                "lane": if domain_module { "domain_module" } else { "agent" },
                "domain_module_justification": if domain_module {
                    Some("The transformation cannot be expressed by a native tool.")
                } else {
                    None
                }
            }],
            "native_reuse": {
                "inspected_capabilities": ["runx.skill.inspect"],
                "selected_capabilities": [],
                "missing_capabilities": []
            },
            "effects": [{
                "effect": "none",
                "authority_scopes": [],
                "approval": "none",
                "provider_boundary": false
            }],
            "skill_chain": { "context_skills": [], "routes": [] },
            "resource_budget": {
                "max_files": 8,
                "max_executable_lines": if domain_module { 100 } else { 0 },
                "max_process_spawns": if domain_module { 1 } else { 0 },
                "network_allowed": false
            },
            "preservation_obligations": ["Keep the operating manual substantive."],
            "deletions": [],
            "proof_plan": [
                {
                    "name": "cold-selection",
                    "kind": "selection_trial",
                    "expected": "The natural request selects demo and a publish request does not."
                },
                {
                    "name": "standalone-result",
                    "kind": "standalone_operator_journey",
                    "expected": "The direct request returns a decision."
                },
                {
                    "name": "composed-reuse",
                    "kind": "composed_operator_journey",
                    "expected": "Prior evidence is reused without rediscovery."
                }
            ]
        }))
        .expect("architecture fixture must be valid")
    }

    #[test]
    fn skill_authoring_architecture_resource_validation_enforces_every_runtime_budget() {
        let declarative = architecture(false);

        let process = CandidateResourceUsage {
            process_spawns: 1,
            ..CandidateResourceUsage::default()
        };
        assert!(validate_architecture_resources(&declarative, &process).is_err());

        let network = CandidateResourceUsage {
            network: true,
            ..CandidateResourceUsage::default()
        };
        assert!(validate_architecture_resources(&declarative, &network).is_err());

        let unplanned_domain_module = CandidateResourceUsage {
            domain_modules: true,
            ..CandidateResourceUsage::default()
        };
        assert!(validate_architecture_resources(&declarative, &unplanned_domain_module).is_err());

        let planned_domain_module = CandidateResourceUsage {
            process_spawns: 1,
            domain_modules: true,
            ..CandidateResourceUsage::default()
        };
        assert!(
            validate_architecture_resources(&architecture(true), &planned_domain_module).is_ok()
        );
    }
}
