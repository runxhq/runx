use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{
    JsonValue, SkillApplyResult, SkillApplyResultSchema, SkillApplyVerdict,
    SkillArchitectureDecision, SkillArchitecturePlan, SkillArchitecturePlanSchema,
    SkillChangeBundle, SkillChangeBundleSchema, SkillChangeDecision, SkillChangeDraft,
    SkillValidationResult, sha256_prefixed,
};

use super::path::{
    canonical_directory, display_relative, invalid_skill_change, normalize_child_path,
};
use super::snapshot::{PackageSnapshot, package_metrics, package_snapshot};
use super::staging::CandidateStage;
use super::validation::validate_candidate;
use crate::RuntimeError;
use crate::effects::RuntimeEffectRegistry;
use crate::filesystem::apply_text_bundle_verified;
use crate::paths::RunxPaths;

mod admission;
mod application;
#[cfg(test)]
mod commit_tests;

use admission::{
    validate_architecture, validate_architecture_decision, validate_change_contract,
    validate_change_shape, validate_digest, validate_resource_budget,
};
use application::{
    AdmittedSkillChange, SkillApplicationLock, admit_skill_change, no_change_result,
    record_application, recorded_application_matches, validate_stable_target, validation_result,
};

pub(crate) fn architecture_digest(
    base_digest: &str,
    architecture: &SkillArchitectureDecision,
) -> Result<String, RuntimeError> {
    let architecture: JsonValue = serde_json::to_value(architecture)
        .and_then(serde_json::from_value)
        .map_err(|source| RuntimeError::json("serializing skill architecture decision", source))?;
    let value = JsonValue::Object(runx_contracts::JsonObject::from([
        (
            "base_digest".to_owned(),
            JsonValue::String(base_digest.to_owned()),
        ),
        ("architecture".to_owned(), architecture),
    ]));
    let canonical = runx_contracts::canonical_stable_json(&value)
        .map_err(|error| invalid_skill_change(format!("canonicalizing architecture: {error}")))?;
    Ok(sha256_prefixed(canonical.as_bytes()))
}

pub(crate) fn plan_skill_architecture(
    base_digest: &str,
    architecture: SkillArchitectureDecision,
) -> Result<SkillArchitecturePlan, RuntimeError> {
    validate_digest("base_digest", base_digest)?;
    validate_architecture_decision(&architecture)?;
    let plan_digest = architecture_digest(base_digest, &architecture)?;
    Ok(SkillArchitecturePlan {
        schema: SkillArchitecturePlanSchema::V1,
        base_digest: base_digest.to_owned().into(),
        plan_digest: plan_digest.into(),
        architecture,
    })
}

pub(crate) fn bind_skill_change(
    plan: &SkillArchitecturePlan,
    draft: SkillChangeDraft,
) -> Result<SkillChangeBundle, RuntimeError> {
    validate_digest("base_digest", plan.base_digest.as_str())?;
    validate_digest("plan_digest", plan.plan_digest.as_str())?;
    validate_architecture_decision(&plan.architecture)?;
    let expected_plan = architecture_digest(plan.base_digest.as_str(), &plan.architecture)?;
    if plan.plan_digest.as_str() != expected_plan {
        return Err(invalid_skill_change(
            "architecture plan digest does not bind its base and decision",
        ));
    }
    let bundle = SkillChangeBundle {
        schema: SkillChangeBundleSchema::V1,
        decision: draft.decision,
        base_digest: plan.base_digest.clone(),
        plan_digest: plan.plan_digest.clone(),
        architecture: plan.architecture.clone(),
        summary: draft.summary,
        non_goals: draft.non_goals,
        writes: draft.writes,
        deletes: draft.deletes,
        expected_outputs: draft.expected_outputs,
    };
    validate_change_shape(&bundle)?;
    validate_architecture(&bundle)?;
    Ok(bundle)
}

pub(crate) fn apply_skill_change(
    repo_root: &Path,
    target_dir: &str,
    mode: &str,
    change: &SkillChangeBundle,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<SkillApplyResult, RuntimeError> {
    let target = ApplyTarget::resolve(repo_root, target_dir, env)?;
    validate_change_contract(change, mode, &target.before.digest)?;
    let admitted = admit_skill_change(&target.relative, &target.path, mode, change)?;
    if change.decision != SkillChangeDecision::Write {
        return Ok(no_change_result(
            &target.relative,
            change,
            target.before.digest,
        ));
    }
    if let Some(result) = reapplied_result(&target, change, env, effects)? {
        return Ok(result);
    }
    let candidate = validate_candidate_stage(&target, change, &admitted, env, effects)?;
    let result = commit_candidate(&target, change, admitted, &candidate)?;
    drop(candidate);
    Ok(result)
}

struct ApplyTarget {
    repo_root: PathBuf,
    paths: RunxPaths,
    relative: String,
    path: PathBuf,
    before: PackageSnapshot,
}

impl ApplyTarget {
    fn resolve(
        repo_root: &Path,
        target_dir: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        let repo_root = canonical_directory(repo_root, "skill workspace")?;
        let paths = RunxPaths::resolve(env, &repo_root);
        let target_relative = normalize_child_path(target_dir)?;
        let relative = display_relative(&target_relative);
        let path = repo_root.join(target_relative);
        let before = package_snapshot(&path)?;
        Ok(Self {
            repo_root,
            paths,
            relative,
            path,
            before,
        })
    }
}

fn reapplied_result(
    target: &ApplyTarget,
    change: &SkillChangeBundle,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<Option<SkillApplyResult>, RuntimeError> {
    if target.before.digest == change.base_digest.as_str() {
        return Ok(None);
    }
    if !recorded_application_matches(
        &target.paths,
        &target.relative,
        change,
        &target.before.digest,
    )? {
        return Err(invalid_skill_change(
            "target package changed since inspection; inspect and author against the current base digest",
        ));
    }
    let validation = validate_stable_target(
        &target.repo_root,
        &target.path,
        &target.relative,
        change,
        &target.before,
        env,
        effects,
    )?;
    Ok(Some(unchanged_write_result(target, change, validation)))
}

fn unchanged_write_result(
    target: &ApplyTarget,
    change: &SkillChangeBundle,
    validation: SkillValidationResult,
) -> SkillApplyResult {
    SkillApplyResult {
        schema: SkillApplyResultSchema::V1,
        target_dir: target.relative.clone().into(),
        decision: SkillChangeDecision::Write,
        verdict: SkillApplyVerdict::Unchanged,
        base_digest: change.base_digest.clone(),
        plan_digest: change.plan_digest.clone(),
        package_digest: target.before.digest.clone().into(),
        changed_paths: Vec::new(),
        deleted_paths: Vec::new(),
        validation: Some(validation),
        residual_risks: Vec::new(),
    }
}

struct ValidatedCandidate {
    stage: CandidateStage,
    snapshot: PackageSnapshot,
    validation: SkillValidationResult,
}

fn validate_candidate_stage(
    target: &ApplyTarget,
    change: &SkillChangeBundle,
    admitted: &AdmittedSkillChange,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<ValidatedCandidate, RuntimeError> {
    let stage = CandidateStage::prepare(&target.repo_root, &target.path, &admitted.bundle)?;
    let snapshot = package_snapshot(&stage.skill_dir)?;
    let before_metrics = package_metrics(&target.before);
    let after_metrics = package_metrics(&snapshot);
    validate_resource_budget(&change.architecture, &after_metrics)?;
    let checks = validate_candidate(
        &target.repo_root,
        &stage.skill_dir,
        env,
        effects,
        true,
        Some(&change.architecture),
    )?;
    let validation = validation_result(
        &target.relative,
        change,
        &target.before,
        &snapshot,
        before_metrics,
        after_metrics,
        checks,
    );
    Ok(ValidatedCandidate {
        stage,
        snapshot,
        validation,
    })
}

fn ensure_target_unchanged(target: &Path, base_digest: &str) -> Result<(), RuntimeError> {
    if package_snapshot(target)?.digest == base_digest {
        return Ok(());
    }
    Err(invalid_skill_change(
        "target package changed during validation; inspect and retry against the current files",
    ))
}

fn commit_candidate(
    target: &ApplyTarget,
    change: &SkillChangeBundle,
    admitted: AdmittedSkillChange,
    candidate: &ValidatedCandidate,
) -> Result<SkillApplyResult, RuntimeError> {
    let _lock = SkillApplicationLock::acquire(&target.paths, &target.relative)?;
    ensure_target_unchanged(&target.path, &candidate.stage.base_digest)?;
    record_application(
        &target.paths,
        &target.relative,
        change,
        &candidate.snapshot.digest,
    )?;
    let (_, applied) = apply_text_bundle_verified(
        "runx.skill.apply",
        &target.repo_root,
        &admitted.bundle,
        || {
            let applied = package_snapshot(&target.path)?;
            if applied.digest != candidate.snapshot.digest {
                return Err(invalid_skill_change(
                    "applied package digest does not match the validated candidate",
                ));
            }
            Ok(applied)
        },
    )?;
    Ok(SkillApplyResult {
        schema: SkillApplyResultSchema::V1,
        target_dir: target.relative.clone().into(),
        decision: SkillChangeDecision::Write,
        verdict: SkillApplyVerdict::ValidatedAndApplied,
        base_digest: change.base_digest.clone(),
        plan_digest: change.plan_digest.clone(),
        package_digest: applied.digest.into(),
        changed_paths: change
            .writes
            .iter()
            .map(|write| write.path.clone())
            .collect(),
        deleted_paths: change.deletes.clone(),
        validation: Some(candidate.validation.clone()),
        residual_risks: Vec::new(),
    })
}
