use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    Digest, SkillApplyResult, SkillApplyResultSchema, SkillApplyVerdict, SkillAuthoringContext,
    SkillAuthoringMode, SkillChangeBundle, SkillChangeBundleSchema, SkillChangeDecision,
    SkillChangeDraft, SkillValidationResult,
};

use super::path::{
    canonical_directory, display_relative, invalid_skill_change, normalize_child_path, package_file,
};
use super::snapshot::{PackageSnapshot, package_metrics, package_snapshot};
use super::staging::CandidateStage;
use super::validation::{Authored, validate_candidate};
use crate::RuntimeError;
use crate::TestProcesses;
use crate::effects::RuntimeEffectRegistry;
use crate::filesystem::apply_text_bundle_verified;
use crate::paths::RunxPaths;

mod admission;
mod application;
#[cfg(test)]
mod commit_tests;

use admission::{
    validate_change_contract, validate_change_shape, validate_digest, validate_mode_decision,
    validate_mode_target,
};
use application::{
    AdmittedSkillChange, SkillApplicationLock, admit_skill_change, no_change_result,
    record_application, recorded_application_matches, validate_stable_target, validation_result,
};

/// Bind an author's `draft` to the package `context` inspected, once its
/// decision fits `mode` and the target and its files are the target's: the
/// bundle names the base it was authored against, so a change to the
/// package since refuses it.
pub(crate) fn bind_skill_change(
    context: &SkillAuthoringContext,
    mode: SkillAuthoringMode,
    draft: SkillChangeDraft,
) -> Result<SkillChangeBundle, RuntimeError> {
    validate_digest("base_digest", context.base_digest.as_str())?;
    validate_mode_decision(mode, draft.decision)?;
    if mode != SkillAuthoringMode::Design {
        validate_mode_target(mode, draft.decision, context.target_exists)?;
    }
    let bundle = SkillChangeBundle {
        schema: SkillChangeBundleSchema::V1,
        decision: draft.decision,
        base_digest: context.base_digest.as_str().into(),
        summary: draft.summary,
        writes: draft.writes,
        deletes: draft.deletes,
    };
    validate_change_shape(&bundle)?;
    let files = bundle
        .writes
        .iter()
        .map(|write| write.path.as_str())
        .chain(bundle.deletes.iter().map(NonEmptyString::as_str));
    for path in files {
        let target = context.target_dir.as_deref().ok_or_else(|| {
            invalid_skill_change(
                "a change writes a package, and none was inspected: inspect its target_dir",
            )
        })?;
        package_file(target, path)?;
    }
    Ok(bundle)
}

/// Apply `change` to the skill at `target_dir` as `mode` does: validate the
/// package it makes, staged, and write it, or, for a design, write nothing.
pub(crate) fn apply_skill_change(
    repo_root: &Path,
    target_dir: &str,
    mode: SkillAuthoringMode,
    change: &SkillChangeBundle,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<SkillApplyResult, RuntimeError> {
    let target = ApplyTarget::resolve(repo_root, target_dir, env)?;
    validate_change_contract(change, mode, target.before.digest.as_str())?;
    let secret_prefixes = super::path::declared_secret_prefixes(repo_root, env)?;
    let admitted = admit_skill_change(&target.relative, mode, change, &secret_prefixes)?;
    if change.decision != SkillChangeDecision::Write {
        let standing = PackageDigests::standing(&target, env)?;
        return Ok(no_change_result(&target.relative, change, standing));
    }
    let authored = target.authored();
    if mode != SkillAuthoringMode::Design
        && let Some(result) = reapplied_result(&target, change, &authored, env, effects)?
    {
        return Ok(result);
    }
    if mode != SkillAuthoringMode::Design {
        validate_mode_target(mode, change.decision, target.path.is_dir())?;
    }
    let candidate = validate_candidate_stage(&target, &admitted, &authored, env, effects)?;
    if mode == SkillAuthoringMode::Design {
        // A design shows the package and its checks, and writes nothing.
        return Ok(validated_result(&target, change, candidate.validation));
    }
    let journal = crate::filesystem::Journal::of_run(env, repo_root)?;
    let result = commit_candidate(&target, change, admitted, &candidate, journal.as_ref(), env)?;
    drop(candidate);
    Ok(result)
}

/// What applying `change` to the skill at `target_dir` would change: its
/// writes and deletes as one bundle beneath the repository root. A change
/// that decides not to write changes nothing.
pub(crate) fn skill_change_effect(
    repo_root: &Path,
    target_dir: &str,
    mode: SkillAuthoringMode,
    change: &SkillChangeBundle,
    reach: runx_contracts::Reach,
) -> Result<Option<crate::capability::LocalEffect>, RuntimeError> {
    let target = display_relative(&normalize_child_path(target_dir)?);
    let under = |path: &str| package_file(&target, path).map(|file| format!("{target}/{file}"));
    let bundle = match change.decision {
        // A design writes nothing.
        SkillChangeDecision::Write if mode != SkillAuthoringMode::Design => {
            crate::filesystem::TextBundle {
                writes: change
                    .writes
                    .iter()
                    .map(|write| {
                        Ok(crate::filesystem::TextFileWrite {
                            path: under(write.path.as_str())?,
                            contents: write.contents.clone(),
                        })
                    })
                    .collect::<Result<_, RuntimeError>>()?,
                deletes: change
                    .deletes
                    .iter()
                    .map(|path| under(path.as_str()))
                    .collect::<Result<_, RuntimeError>>()?,
                expected: Default::default(),
            }
        }
        _ => crate::filesystem::TextBundle::default(),
    };
    crate::filesystem::text_bundle_effect("runx.skill.apply", repo_root, &bundle, reach)
}

/// A package's digests as an apply result states them.
pub(super) struct PackageDigests {
    pub(super) package: Option<NonEmptyString>,
    pub(super) distribution: Option<NonEmptyString>,
}

impl PackageDigests {
    /// The package an apply wrote, or found already written: it validated,
    /// so it loads.
    fn applied(target: &ApplyTarget, env: &BTreeMap<String, String>) -> Result<Self, RuntimeError> {
        let loaded = crate::load_validated_skill_package(&target.path)
            .map_err(|error| invalid_skill_change(error.to_string()))?;
        Self::of(&loaded, target, env)
    }

    /// The package standing at the target of a change that writes nothing:
    /// none when no valid package stands there, as before a package is
    /// created or while one is broken.
    fn standing(
        target: &ApplyTarget,
        env: &BTreeMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        match crate::load_validated_skill_package(&target.path) {
            Ok(loaded) => Self::of(&loaded, target, env),
            Err(_) => Ok(Self {
                package: None,
                distribution: None,
            }),
        }
    }

    /// `loaded`'s package digest, and its distribution digest unless it
    /// doesn't package, as a publish would refuse it.
    fn of(
        loaded: &crate::LoadedSkillPackage,
        target: &ApplyTarget,
        env: &BTreeMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        Ok(Self {
            package: Some(loaded.package.package_digest.as_str().into()),
            distribution: crate::registry::distribution_digest(loaded, env, &target.repo_root)?
                .map(Into::into),
        })
    }
}

struct ApplyTarget {
    repo_root: PathBuf,
    paths: RunxPaths,
    relative: String,
    path: PathBuf,
    before: PackageSnapshot,
}

impl ApplyTarget {
    /// What a package written here must say of itself: the name of the
    /// package that stands here, which a change to it keeps. A target with
    /// no package, or one that doesn't load, names none.
    fn authored(&self) -> Authored {
        let standing = crate::load_validated_skill_package(&self.path).ok();
        Authored {
            keeps_name: standing
                .as_ref()
                .map(|loaded| loaded.package.skill.name.clone()),
            standing_turns: standing
                .as_ref()
                .and_then(|loaded| loaded.manifest())
                .map(|manifest| {
                    super::validation::turns(manifest)
                        .into_iter()
                        .map(|(method, step)| (method, step.id.clone()))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

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
    authored: &Authored,
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
        target.before.digest.as_str(),
    )? {
        return Err(invalid_skill_change(
            "target package changed since inspection; inspect and author against the current base digest",
        ));
    }
    let validation = validate_stable_target(
        &target.repo_root,
        &target.path,
        &target.relative,
        &target.before,
        authored,
        env,
        effects,
    )?;
    let applied = PackageDigests::applied(target, env)?;
    Ok(Some(unchanged_write_result(
        target, change, validation, applied,
    )))
}

fn unchanged_write_result(
    target: &ApplyTarget,
    change: &SkillChangeBundle,
    validation: SkillValidationResult,
    applied: PackageDigests,
) -> SkillApplyResult {
    SkillApplyResult {
        schema: SkillApplyResultSchema::V1,
        target_dir: target.relative.clone().into(),
        decision: SkillChangeDecision::Write,
        verdict: SkillApplyVerdict::Unchanged,
        base_digest: change.base_digest.clone(),
        package_digest: applied.package,
        distribution_digest: applied.distribution,
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
    admitted: &AdmittedSkillChange,
    authored: &Authored,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<ValidatedCandidate, RuntimeError> {
    let stage = CandidateStage::prepare(&target.repo_root, &target.path, &admitted.bundle)?;
    let snapshot = package_snapshot(&stage.skill_dir)?;
    let before_metrics = package_metrics(&target.before);
    let after_metrics = package_metrics(&snapshot);
    let checks = validate_candidate(
        &target.repo_root,
        &stage.skill_dir,
        env,
        effects,
        TestProcesses::Run,
        Some(authored),
    )?
    .applicable()?;
    let validation = validation_result(
        &target.relative,
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

/// A design's result: its candidate validated, and nothing written.
fn validated_result(
    target: &ApplyTarget,
    change: &SkillChangeBundle,
    validation: SkillValidationResult,
) -> SkillApplyResult {
    SkillApplyResult {
        schema: SkillApplyResultSchema::V1,
        target_dir: target.relative.clone().into(),
        decision: SkillChangeDecision::Write,
        verdict: SkillApplyVerdict::Validated,
        base_digest: change.base_digest.clone(),
        package_digest: None,
        distribution_digest: None,
        changed_paths: Vec::new(),
        deleted_paths: Vec::new(),
        validation: Some(validation),
        residual_risks: Vec::new(),
    }
}

fn ensure_target_unchanged(target: &Path, base_digest: &Digest) -> Result<(), RuntimeError> {
    if package_snapshot(target)?.digest == *base_digest {
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
    journal: Option<&crate::filesystem::Journal>,
    env: &BTreeMap<String, String>,
) -> Result<SkillApplyResult, RuntimeError> {
    let _lock = SkillApplicationLock::acquire(&target.paths, &target.relative)?;
    ensure_target_unchanged(&target.path, &candidate.stage.base_digest)?;
    record_application(
        &target.paths,
        &target.relative,
        change,
        candidate.snapshot.digest.as_str(),
    )?;
    // The repository root was bounded when the tool resolved it; the paths
    // here are the skill package's own, admitted by `admit_skill_change`.
    let (report, ()) = apply_text_bundle_verified(
        "runx.skill.apply",
        &target.repo_root,
        &admitted.bundle,
        runx_contracts::Reach::Inputs,
        journal,
        || {
            if package_snapshot(&target.path)?.digest != candidate.snapshot.digest {
                return Err(invalid_skill_change(
                    "applied package snapshot does not match the validated candidate",
                ));
            }
            Ok(())
        },
    )?;
    let applied = PackageDigests::applied(target, env)?;
    let changed = changed_writes(&report);
    Ok(SkillApplyResult {
        schema: SkillApplyResultSchema::V1,
        target_dir: target.relative.clone().into(),
        decision: SkillChangeDecision::Write,
        verdict: SkillApplyVerdict::ValidatedAndApplied,
        base_digest: change.base_digest.clone(),
        package_digest: applied.package,
        distribution_digest: applied.distribution,
        // A write of what the file already held changed nothing.
        changed_paths: change
            .writes
            .iter()
            .filter(|write| changed.contains(write.path.as_str()))
            .map(|write| write.path.clone())
            .collect(),
        deleted_paths: change.deletes.clone(),
        validation: Some(candidate.validation.clone()),
        residual_risks: Vec::new(),
    })
}

/// The files an applied bundle's report says it changed: each write whose
/// content differs from what the file held, or that made the file.
fn changed_writes(report: &runx_contracts::JsonObject) -> std::collections::BTreeSet<String> {
    report
        .get("writes")
        .and_then(runx_contracts::JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(runx_contracts::JsonValue::as_object)
        .filter(|write| {
            let digest = |field: &str| write.get(field).and_then(runx_contracts::JsonValue::as_str);
            digest("before_digest") != digest("content_digest")
        })
        .filter_map(|write| {
            write
                .get("path")
                .and_then(runx_contracts::JsonValue::as_str)
                .map(str::to_owned)
        })
        .collect()
}
