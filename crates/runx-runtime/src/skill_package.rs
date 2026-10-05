//! Filesystem admission for parser-owned skill package truth.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_parser::harness_fixture::HarnessExpectedStatus;
use runx_parser::{
    OperatorJourneyMode, SkillPackageSource, SkillRunnerDefinition, SkillRunnerManifest,
    ValidatedSkillPackage, validate_skill_package,
};

use crate::RuntimeError;
use crate::filesystem::{DirectoryEntry, find_files_named, read_dir_sorted};

mod inspection;
mod retention;

pub(crate) use inspection::inspect_loaded_execution_closure_binding;
#[cfg(feature = "cli-tool")]
pub(crate) use inspection::{LocalExecutionClosure, inspect_loaded_local_execution_closure};
pub use inspection::{SkillInspectionError, inspect_skill_package};
pub use retention::{
    SkillRetentionError, inspect_local_skill_binding, resolve_retained_skill_binding,
    retain_skill_binding,
};

pub(crate) const MAX_PACKAGE_FILES: usize = 500;
pub(crate) const MAX_PACKAGE_BYTES: usize = 20 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct LoadedSkillPackage {
    /// Canonical absolute directory selected by the caller. For an internal
    /// profile this is the profile directory; for a public package it is the
    /// package root.
    pub directory: PathBuf,
    pub package_root: PathBuf,
    pub profile_path: Option<String>,
    pub package: ValidatedSkillPackage,
    /// Exact catalog documents admitted for packet-valued inputs. Publication
    /// and harness staging reuse these bytes instead of rereading mutable files.
    pub(crate) resolved_input_packet_schemas:
        BTreeMap<String, crate::packet_schemas::PacketSchemaEntry>,
}

impl LoadedSkillPackage {
    #[must_use]
    pub fn manifest(&self) -> Option<&runx_parser::SkillRunnerManifest> {
        self.profile_path
            .as_deref()
            .and_then(|path| self.package.manifest_at(path))
    }
}

/// Return the authored runner examples, or derive the smallest copy-valid
/// direct-use example from the standalone harness journey that already proves
/// the public runner. This keeps generated agent instructions executable
/// without duplicating fixture payloads in every manifest.
pub(crate) fn effective_runner_examples(
    package: &ValidatedSkillPackage,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
) -> Vec<runx_contracts::JsonObject> {
    if !runner.examples.is_empty() {
        return runner.examples.clone();
    }
    sealed_standalone_inputs(package, manifest, runner)
        .cloned()
        .into_iter()
        .collect()
}

pub(crate) fn runner_has_sealed_standalone_journey(
    package: &ValidatedSkillPackage,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
) -> bool {
    sealed_standalone_inputs(package, manifest, runner).is_some()
}

fn sealed_standalone_inputs<'a>(
    package: &'a ValidatedSkillPackage,
    manifest: &'a SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
) -> Option<&'a runx_contracts::JsonObject> {
    manifest
        .harness
        .iter()
        .flat_map(|harness| harness.cases.iter())
        .find(|case| {
            fixture_selects_runner(case.runner.as_deref(), manifest, runner)
                && usable_example_status(case.expect.status.as_ref())
                && has_standalone_journey(&case.operator_journeys, &runner.name)
        })
        .map(|case| &case.inputs)
        .or_else(|| {
            package.harness_fixtures.values().find_map(|fixture| {
                (fixture_selects_runner(fixture.runner.as_deref(), manifest, runner)
                    && usable_example_status(fixture.expect.status.as_ref())
                    && has_standalone_journey(&fixture.operator_journeys, &runner.name))
                .then_some(&fixture.inputs)
            })
        })
}

fn fixture_selects_runner(
    selected: Option<&str>,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
) -> bool {
    selected.map_or_else(
        || runner.default || manifest.runners.len() == 1,
        |selected| selected == runner.name,
    )
}

fn has_standalone_journey(journeys: &[runx_parser::OperatorJourneyClaim], runner: &str) -> bool {
    journeys.iter().any(|journey| {
        journey.mode == OperatorJourneyMode::Standalone
            && journey
                .exercises_runner
                .as_deref()
                .is_none_or(|selected| selected == runner)
    })
}

fn usable_example_status(status: Option<&HarnessExpectedStatus>) -> bool {
    matches!(status, Some(HarnessExpectedStatus::Sealed))
}

pub(crate) fn verify_loaded_execution_binding(
    loaded: impl Into<std::sync::Arc<LoadedSkillPackage>>,
    runner: &str,
    env: &std::collections::BTreeMap<String, String>,
    expected_package_digest: Option<&str>,
    expected_execution_closure_digest: Option<&str>,
) -> Result<Option<String>, SkillInspectionError> {
    let loaded = loaded.into();
    if let Some(expected) = expected_package_digest
        && expected != loaded.package.package_digest
    {
        return Err(SkillInspectionError::PackageDigestMismatch {
            expected: expected.to_owned(),
            received: loaded.package.package_digest.clone(),
        });
    }
    let closure = inspect_loaded_execution_closure_binding(loaded, runner, env)?;
    if let Some(expected) = expected_execution_closure_digest {
        if !closure.fully_bound {
            return Err(SkillInspectionError::ClosureNotFullyBound {
                runner: runner.to_owned(),
            });
        }
        if closure.digest != expected {
            return Err(SkillInspectionError::ClosureDigestMismatch {
                expected: expected.to_owned(),
                received: closure.digest,
            });
        }
    }
    Ok(closure.fully_bound.then_some(closure.digest))
}

pub fn load_validated_skill_package(path: &Path) -> Result<LoadedSkillPackage, RuntimeError> {
    let unresolved_directory = resolve_skill_package_directory(path)?;
    // Runtime-owned package identity is absolute. Keeping caller-relative paths
    // here makes nested adapters accidentally reinterpret a valid package
    // against a harness, resume, or daemon workspace later in execution.
    let directory = fs::canonicalize(&unresolved_directory).map_err(|source| {
        RuntimeError::io(
            format!(
                "resolving skill package directory {}",
                unresolved_directory.display()
            ),
            source,
        )
    })?;
    let package_root = resolve_owning_package_root(&directory)?;
    let mut source = SkillPackageSource::default();
    let mut totals = PackageTotals::default();
    collect_package_source(&package_root, &package_root, &mut source, &mut totals)?;
    let mut package = validate_skill_package(source)?;
    let profile_path = selected_profile_path(&package_root, &directory)?
        .filter(|path| package.manifest_at(path).is_some());
    let resolved_input_packet_schemas = crate::packet_schemas::hydrate_packet_input_contracts(
        &mut package,
        &directory,
        &package_root,
    )?;
    Ok(LoadedSkillPackage {
        directory,
        package_root,
        profile_path,
        package,
        resolved_input_packet_schemas,
    })
}

/// Discover executable skill-package roots in a workspace.
///
/// A repository may carry a root `SKILL.md` as its native operator manual while
/// also owning a `skills/` catalog. That manual is exportable agent context, not
/// a package boundary around the entire repository. A standalone root remains a
/// package, as does an explicit root `X.yaml` package in a catalog workspace.
pub(crate) fn discover_workspace_skill_package_dirs(
    root: &Path,
) -> Result<Vec<PathBuf>, RuntimeError> {
    let skills_root = root.join("skills");
    let mut directories = find_files_named(&skills_root, "SKILL.md")?
        .into_iter()
        .filter(|path| !is_fixture_manual(&skills_root, path))
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect::<Vec<_>>();
    if root.join("SKILL.md").is_file() && (!skills_root.is_dir() || root.join("X.yaml").is_file()) {
        directories.push(root.to_path_buf());
    }
    directories.sort();
    directories.dedup();
    Ok(directories)
}

fn is_fixture_manual(skills_root: &Path, manual_path: &Path) -> bool {
    manual_path.strip_prefix(skills_root).is_ok_and(|relative| {
        relative
            .components()
            .any(|component| component.as_os_str() == std::ffi::OsStr::new("fixtures"))
    })
}

pub(crate) fn resolve_skill_package_directory(path: &Path) -> Result<PathBuf, RuntimeError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| RuntimeError::io(format!("reading {}", path.display()), source))?;
    if metadata.file_type().is_symlink() {
        return Err(runx_parser::SkillPackageError::invalid(
            path.to_string_lossy(),
            "skill package root must not be a symbolic link",
        )
        .into());
    }
    if metadata.is_dir() {
        return Ok(path.to_path_buf());
    }
    if metadata.is_file()
        && matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("SKILL.md" | "X.yaml")
        )
    {
        return path.parent().map(Path::to_path_buf).ok_or_else(|| {
            runx_parser::SkillPackageError::invalid(
                path.to_string_lossy(),
                "skill package document has no package directory",
            )
            .into()
        });
    }
    Err(runx_parser::SkillPackageError::invalid(
        path.to_string_lossy(),
        "skill package reference must be a directory, SKILL.md, or X.yaml",
    )
    .into())
}

fn resolve_owning_package_root(directory: &Path) -> Result<PathBuf, RuntimeError> {
    if let Some(package_root) = find_owning_package_root(directory)
        && (package_root == directory || directory.join("X.yaml").is_file())
    {
        return Ok(package_root);
    }
    if !directory.join("X.yaml").is_file() {
        return Err(runx_parser::SkillPackageError::invalid(
            directory.to_string_lossy(),
            "skill package directory must contain SKILL.md or an owned X.yaml profile",
        )
        .into());
    }
    Err(runx_parser::SkillPackageError::invalid(
        directory.to_string_lossy(),
        "execution profile has no owning SKILL.md ancestor",
    )
    .into())
}

pub(crate) fn find_owning_package_root(directory: &Path) -> Option<PathBuf> {
    directory
        .ancestors()
        .find(|ancestor| ancestor.join("SKILL.md").is_file())
        .map(Path::to_path_buf)
}

fn selected_profile_path(
    package_root: &Path,
    directory: &Path,
) -> Result<Option<String>, RuntimeError> {
    if !directory.join("X.yaml").is_file() {
        return Ok(None);
    }
    let relative = directory.strip_prefix(package_root).map_err(|_| {
        runx_parser::SkillPackageError::invalid(
            directory.to_string_lossy(),
            "execution profile is outside its owning package",
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Ok(Some("X.yaml".to_owned()));
    }
    Ok(Some(format!("{}/X.yaml", relative_path(relative)?)))
}

#[derive(Default)]
struct PackageTotals {
    files: usize,
    bytes: usize,
}

fn collect_package_source(
    root: &Path,
    current: &Path,
    source: &mut SkillPackageSource,
    totals: &mut PackageTotals,
) -> Result<(), RuntimeError> {
    for entry in read_dir_sorted(current)? {
        if ignored_package_entry(&entry.name) {
            continue;
        }
        collect_package_entry(root, entry, source, totals)?;
    }
    Ok(())
}

fn collect_package_entry(
    root: &Path,
    entry: DirectoryEntry,
    source: &mut SkillPackageSource,
    totals: &mut PackageTotals,
) -> Result<(), RuntimeError> {
    let relative = entry.path.strip_prefix(root).map_err(|_| {
        runx_parser::SkillPackageError::invalid(
            entry.path.to_string_lossy(),
            "package entry escaped its root",
        )
    })?;
    let relative = relative_path(relative)?;
    let metadata = fs::symlink_metadata(&entry.path)
        .map_err(|source| RuntimeError::io(format!("reading {}", entry.path.display()), source))?;
    if metadata.file_type().is_symlink() {
        source.symlinks.insert(relative);
        return Ok(());
    }
    if metadata.is_dir() {
        return collect_package_source(root, &entry.path, source, totals);
    }
    if !metadata.is_file() {
        return Err(runx_parser::SkillPackageError::invalid(
            relative,
            "skill packages may contain only regular files and directories",
        )
        .into());
    }
    let contents = fs::read(&entry.path).map_err(|source| {
        RuntimeError::io(
            format!("reading package file {}", entry.path.display()),
            source,
        )
    })?;
    totals.files = totals.files.saturating_add(1);
    totals.bytes = totals.bytes.checked_add(contents.len()).ok_or_else(|| {
        runx_parser::SkillPackageError::invalid(
            relative.clone(),
            "skill package byte count overflow",
        )
    })?;
    if totals.files > MAX_PACKAGE_FILES || totals.bytes > MAX_PACKAGE_BYTES {
        return Err(runx_parser::SkillPackageError::invalid(
            relative,
            format!(
                "skill package exceeds the {MAX_PACKAGE_FILES} file / {MAX_PACKAGE_BYTES} byte admission limit"
            ),
        )
        .into());
    }
    source.files.insert(relative, contents);
    Ok(())
}

fn relative_path(path: &Path) -> Result<String, RuntimeError> {
    let mut segments = Vec::new();
    for component in path.components() {
        let std::path::Component::Normal(segment) = component else {
            return Err(runx_parser::SkillPackageError::invalid(
                path.to_string_lossy(),
                "package entry has a non-normal relative path",
            )
            .into());
        };
        let segment = segment.to_str().ok_or_else(|| {
            runx_parser::SkillPackageError::invalid(
                path.to_string_lossy(),
                "package paths must be valid UTF-8",
            )
        })?;
        segments.push(segment);
    }
    Ok(segments.join("/"))
}

pub(crate) fn ignored_package_entry(name: &str) -> bool {
    matches!(name, ".git" | ".runx" | "node_modules" | "target")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::fs;

    use super::discover_workspace_skill_package_dirs;

    const MANUAL: &str = "---\nname: demo\ndescription: Demo skill.\nsource:\n  type: cli-tool\n  command: demo\n---\n\n# Demo\n";

    #[test]
    fn catalog_workspace_does_not_wrap_root_manual_around_repository() {
        let temp = tempfile::tempdir().expect("temporary workspace");
        let root = temp.path();
        let child = root.join("skills/demo");
        fs::create_dir_all(&child).expect("skill directory");
        fs::write(root.join("SKILL.md"), MANUAL).expect("root manual");
        fs::write(child.join("SKILL.md"), MANUAL).expect("child manual");

        let discovered = discover_workspace_skill_package_dirs(root).expect("skill discovery");

        assert_eq!(discovered, vec![child]);
    }

    #[test]
    fn standalone_root_manual_remains_a_skill_package() {
        let temp = tempfile::tempdir().expect("temporary workspace");
        let root = temp.path();
        fs::write(root.join("SKILL.md"), MANUAL).expect("root manual");

        let discovered = discover_workspace_skill_package_dirs(root).expect("skill discovery");

        assert_eq!(discovered, vec![root.to_path_buf()]);
    }

    #[test]
    fn catalog_discovery_excludes_nested_harness_fixture_packages() {
        let temp = tempfile::tempdir().expect("temporary workspace");
        let root = temp.path();
        let public = root.join("skills/prior-art");
        let fixture = public.join("fixtures/workspace/skills/moltbook");
        fs::create_dir_all(&fixture).expect("fixture skill directory");
        fs::write(public.join("SKILL.md"), MANUAL).expect("public manual");
        fs::write(fixture.join("SKILL.md"), MANUAL).expect("fixture manual");

        let discovered = discover_workspace_skill_package_dirs(root).expect("skill discovery");

        assert_eq!(discovered, vec![public]);
    }
}
