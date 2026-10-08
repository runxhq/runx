use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::Path;

use fs2::FileExt;
use runx_contracts::{
    JsonValue, SkillApplyResult, SkillApplyResultSchema, SkillApplyVerdict, SkillChangeBundle,
    SkillChangeDecision, SkillPackageMetrics, SkillValidationCheck, SkillValidationCheckStatus,
    SkillValidationResult, SkillValidationResultSchema, sha256_prefixed,
};

use super::super::path::{
    assert_allowed_package_delete_path, assert_allowed_package_write_path, invalid_skill_change,
    normalize_package_file, reject_secret_material,
};
use super::super::snapshot::{PackageSnapshot, package_delta, package_metrics};
use super::super::validation::validate_candidate;
use super::admission::{validate_digest, validate_resource_budget};
use super::{RuntimeEffectRegistry, RuntimeError};
use crate::filesystem::{TextBundle, TextFileWrite};
use crate::paths::RunxPaths;

const APPLICATION_RECORD_SCHEMA: &str = "runx.skill.application_record.v1";

pub(super) struct SkillApplicationLock {
    file: File,
}

impl SkillApplicationLock {
    pub(super) fn acquire(paths: &RunxPaths, target_dir: &str) -> Result<Self, RuntimeError> {
        let digest = sha256_prefixed(target_dir.as_bytes());
        let digest = digest
            .strip_prefix("sha256:")
            .ok_or_else(|| invalid_skill_change("authoring lock digest is invalid"))?;
        let locks = paths.authoring_locks();
        fs::create_dir_all(&locks).map_err(|source| {
            RuntimeError::io(
                format!(
                    "creating skill authoring lock directory {}",
                    locks.display()
                ),
                source,
            )
        })?;
        let path = crate::filesystem::resolve_contained_file_target(
            "runx.skill.apply",
            &locks,
            &format!("{digest}.lock"),
        )?;
        let file = open_lock_file(&path)?;
        file.lock_exclusive().map_err(|source| {
            RuntimeError::io(
                format!("acquiring skill authoring lock {}", path.display()),
                source,
            )
        })?;
        Ok(Self { file })
    }
}

impl Drop for SkillApplicationLock {
    fn drop(&mut self) {
        let _ignored = FileExt::unlock(&self.file);
    }
}

fn open_lock_file(path: &Path) -> Result<File, RuntimeError> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|source| {
        RuntimeError::io(
            format!("opening skill authoring lock {}", path.display()),
            source,
        )
    })
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SkillApplicationRecord {
    schema: String,
    target_dir: String,
    base_digest: String,
    plan_digest: String,
    bundle_digest: String,
    package_digest: String,
}

pub(super) struct AdmittedSkillChange {
    pub(super) bundle: TextBundle,
}

pub(super) fn admit_skill_change(
    target_dir: &str,
    target: &Path,
    mode: &str,
    change: &SkillChangeBundle,
) -> Result<AdmittedSkillChange, RuntimeError> {
    let writes = change
        .writes
        .iter()
        .map(|write| admit_write(target_dir, target, mode, write))
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    let deletes = change
        .deletes
        .iter()
        .map(|delete| admit_delete(target_dir, mode, delete.as_str()))
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    Ok(AdmittedSkillChange {
        bundle: TextBundle { writes, deletes },
    })
}

fn admit_write(
    target_dir: &str,
    target: &Path,
    mode: &str,
    write: &runx_contracts::SkillFileWrite,
) -> Result<TextFileWrite, RuntimeError> {
    let relative = normalize_package_file(write.path.as_str().to_owned())?;
    assert_allowed_package_write_path(&relative, target, mode)?;
    reject_secret_material(&relative, &write.contents)?;
    Ok(TextFileWrite {
        path: format!("{target_dir}/{relative}"),
        contents: write.contents.clone(),
    })
}

fn admit_delete(target_dir: &str, mode: &str, delete: &str) -> Result<String, RuntimeError> {
    let relative = normalize_package_file(delete.to_owned())?;
    assert_allowed_package_delete_path(&relative, mode)?;
    if relative == "SKILL.md" {
        return Err(invalid_skill_change("SKILL.md cannot be deleted"));
    }
    Ok(format!("{target_dir}/{relative}"))
}

pub(super) fn validation_result(
    target_dir: &str,
    change: &SkillChangeBundle,
    before: &PackageSnapshot,
    after: &PackageSnapshot,
    before_metrics: SkillPackageMetrics,
    after_metrics: SkillPackageMetrics,
    checks: runx_contracts::JsonObject,
) -> SkillValidationResult {
    let harness_status = checks
        .get("harness")
        .and_then(JsonValue::as_object)
        .and_then(|harness| harness.get("status"))
        .and_then(JsonValue::as_str)
        .unwrap_or("passed");
    let check_results = validation_checks(harness_status);
    let delta = package_delta(&before_metrics, &after_metrics);
    SkillValidationResult {
        schema: SkillValidationResultSchema::V1,
        target_dir: target_dir.into(),
        base_digest: before.digest.clone().into(),
        plan_digest: change.plan_digest.clone(),
        candidate_digest: after.digest.clone().into(),
        checks: check_results,
        before: before_metrics,
        after: after_metrics,
        delta,
        residual_risks: Vec::new(),
    }
}

fn validation_checks(harness_status: &str) -> Vec<SkillValidationCheck> {
    vec![
        passed_check("package_contract", "aggregate package validation passed"),
        passed_check("package_inspection", "package inspection passed"),
        passed_check(
            "focused_harness",
            if harness_status == "passed" {
                "focused package harness passed"
            } else {
                "focused package harness completed without failure"
            },
        ),
        passed_check(
            "resource_budget",
            "declared file, executable-line, fan-out, process, network, and domain-module budgets passed",
        ),
    ]
}

pub(super) fn validate_stable_target(
    repo_root: &Path,
    target: &Path,
    target_dir: &str,
    change: &SkillChangeBundle,
    snapshot: &PackageSnapshot,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<SkillValidationResult, RuntimeError> {
    let metrics = package_metrics(snapshot);
    validate_resource_budget(&change.architecture, &metrics)?;
    let checks = validate_candidate(
        repo_root,
        target,
        env,
        effects,
        true,
        Some(&change.architecture),
    )?;
    Ok(validation_result(
        target_dir,
        change,
        snapshot,
        snapshot,
        metrics.clone(),
        metrics,
        checks,
    ))
}

pub(super) fn recorded_application_matches(
    paths: &RunxPaths,
    target_dir: &str,
    change: &SkillChangeBundle,
    package_digest: &str,
) -> Result<bool, RuntimeError> {
    let path = paths
        .authoring_applications()
        .join(application_record_file(change.plan_digest.as_str())?);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(RuntimeError::io(
                format!("reading skill application record {}", path.display()),
                source,
            ));
        }
    };
    let record: SkillApplicationRecord = serde_json::from_str(&contents).map_err(|source| {
        RuntimeError::json(
            format!("parsing skill application record {}", path.display()),
            source,
        )
    })?;
    Ok(record.schema == APPLICATION_RECORD_SCHEMA
        && record.target_dir == target_dir
        && record.base_digest == change.base_digest.as_str()
        && record.plan_digest == change.plan_digest.as_str()
        && record.bundle_digest == change_bundle_digest(change)?
        && record.package_digest == package_digest)
}

/// Record that `change` applied to `target_dir` yields `package_digest`. It is
/// written before the package: a crash in between leaves the target at its
/// base digest, so the change applies again; once both land, a repeat of the
/// change finds the target already applied.
pub(super) fn record_application(
    paths: &RunxPaths,
    target_dir: &str,
    change: &SkillChangeBundle,
    package_digest: &str,
) -> Result<(), RuntimeError> {
    let record = SkillApplicationRecord {
        schema: APPLICATION_RECORD_SCHEMA.to_owned(),
        target_dir: target_dir.to_owned(),
        base_digest: change.base_digest.as_str().to_owned(),
        plan_digest: change.plan_digest.as_str().to_owned(),
        bundle_digest: change_bundle_digest(change)?,
        package_digest: package_digest.to_owned(),
    };
    let contents = serde_json::to_string_pretty(&record)
        .map_err(|source| RuntimeError::json("serializing skill application record", source))?;
    let applications = paths.authoring_applications();
    fs::create_dir_all(&applications).map_err(|source| {
        RuntimeError::io(
            format!(
                "creating skill application records {}",
                applications.display()
            ),
            source,
        )
    })?;
    let write = TextFileWrite {
        path: application_record_file(change.plan_digest.as_str())?,
        contents: format!("{contents}\n"),
    };
    crate::filesystem::apply_text_bundle(
        "runx.skill.apply",
        &applications,
        &TextBundle {
            writes: vec![write],
            deletes: Vec::new(),
        },
    )?;
    Ok(())
}

fn application_record_file(plan_digest: &str) -> Result<String, RuntimeError> {
    validate_digest("plan_digest", plan_digest)?;
    let digest = plan_digest
        .strip_prefix("sha256:")
        .ok_or_else(|| invalid_skill_change("plan_digest prefix was not validated"))?;
    Ok(format!("{digest}.json"))
}

fn change_bundle_digest(change: &SkillChangeBundle) -> Result<String, RuntimeError> {
    let value: JsonValue = serde_json::to_value(change)
        .and_then(serde_json::from_value)
        .map_err(|source| RuntimeError::json("serializing skill change bundle", source))?;
    let canonical = runx_contracts::canonical_stable_json(&value)
        .map_err(|error| invalid_skill_change(format!("canonicalizing change bundle: {error}")))?;
    Ok(sha256_prefixed(canonical.as_bytes()))
}

pub(super) fn no_change_result(
    target_dir: &str,
    change: &SkillChangeBundle,
    package_digest: String,
) -> SkillApplyResult {
    SkillApplyResult {
        schema: SkillApplyResultSchema::V1,
        target_dir: target_dir.into(),
        decision: change.decision,
        verdict: if change.decision == SkillChangeDecision::NeedsCore {
            SkillApplyVerdict::NeedsCore
        } else {
            SkillApplyVerdict::Unchanged
        },
        base_digest: change.base_digest.clone(),
        plan_digest: change.plan_digest.clone(),
        package_digest: package_digest.into(),
        changed_paths: Vec::new(),
        deleted_paths: Vec::new(),
        validation: None,
        residual_risks: Vec::new(),
    }
}

fn passed_check(name: &'static str, detail: &'static str) -> SkillValidationCheck {
    SkillValidationCheck {
        name: name.into(),
        status: SkillValidationCheckStatus::Passed,
        detail: detail.into(),
    }
}
