//! Filesystem admission for parser-owned skill package truth.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_parser::test_fixture::TestExpectedStatus;
use runx_parser::{
    SkillMethodDefinition, SkillMethodManifest, SkillPackageSource, ValidatedSkillPackage,
    validate_skill_package,
};

use crate::RuntimeError;
use crate::filesystem::{DirectoryEntry, read_dir_sorted};

mod inspection;
mod resolve;

pub use inspection::inspect_connector_readiness;
pub(crate) use inspection::{
    CaseClosureTarget, LocalClosure, case_closure_connectors, closure_registry_refs,
    inspect_loaded_local_closure, unheld_loaded_registry_refs,
};
pub use inspection::{
    SkillInspectionError, inspect_skill_package, inspect_skill_readiness, unheld_registry_refs_at,
};
pub(crate) use inspection::{inspect_loaded_closure_binding, method_makes_calls, reached_methods};
pub use resolve::resolve_local_skill_reference;

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
    /// Exact catalog documents admitted for packet-valued inputs. Inspection
    /// and publication reuse these bytes instead of rereading mutable files.
    pub(crate) resolved_input_packet_schemas:
        BTreeMap<runx_contracts::PacketId, crate::packet_schemas::PacketSchemaEntry>,
}

impl LoadedSkillPackage {
    #[must_use]
    pub fn manifest(&self) -> Option<&runx_parser::SkillMethodManifest> {
        self.profile_path
            .as_deref()
            .and_then(|path| self.package.manifest_at(path))
    }
}

/// Each method `manifest` declares, as a model chooses it from an index.
pub(crate) fn catalog_methods(
    manifest: &SkillMethodManifest,
) -> Vec<runx_contracts::CatalogMethod> {
    manifest
        .methods
        .values()
        .map(|method| runx_contracts::CatalogMethod {
            name: method.name.clone(),
            purpose: method.description.clone(),
        })
        .collect()
}

/// Whether one of the package's tests runs `method` itself and seals: the
/// proof a method is offered on its own by.
pub(crate) fn method_has_sealed_test(
    package: &ValidatedSkillPackage,
    manifest: &SkillMethodManifest,
    method: &SkillMethodDefinition,
) -> bool {
    package.test_fixtures.values().any(|test| {
        test.package.is_none()
            && test.method.as_deref().map_or_else(
                || manifest.default_method().map(|default| &default.name) == Some(&method.name),
                |selected| selected == method.name,
            )
            && usable_example_status(test.expect.status.as_ref())
    })
}

fn usable_example_status(status: Option<&TestExpectedStatus>) -> bool {
    matches!(status, Some(TestExpectedStatus::Sealed))
}

/// The packages a run admitted, by canonical directory: its skill and every
/// child skill its closure binds, as they were when the run was
/// bound, each with who it is (D30). A run reads every package it executes
/// from here and never from disk again: its modules, its child skills'
/// methods, its agents' instructions. So what a run executes is what it
/// admitted, however the files change while it runs; a run continued in
/// another process binds again, under the digests it started with, and so
/// verifies each signature again.
#[derive(Clone, Debug)]
pub(crate) struct AdmittedPackages {
    packages: Arc<BTreeMap<PathBuf, Arc<LoadedSkillPackage>>>,
    /// Who each package is, by its package root, as the binding set it.
    identities: Arc<BTreeMap<PathBuf, crate::identity::PackageIdentity>>,
    /// The root of each `project:<name>` the binding binds as itself, which
    /// is that package for the whole run.
    projects: Arc<BTreeMap<String, PathBuf>>,
}

impl AdmittedPackages {
    /// Who the admitted package rooted at `package_root` is.
    pub(crate) fn identity(
        &self,
        package_root: &Path,
    ) -> Option<&crate::identity::PackageIdentity> {
        self.identities.get(package_root)
    }

    /// The root of each `project:<name>` the binding binds as itself.
    pub(crate) fn projects(&self) -> &BTreeMap<String, PathBuf> {
        &self.projects
    }

    /// The admitted package at `directory`, as the run names it or as the
    /// loader resolves it. A package the run did not admit is refused.
    pub(crate) fn get(&self, directory: &Path) -> Result<Arc<LoadedSkillPackage>, RuntimeError> {
        if let Some(package) = self.packages.get(directory) {
            return Ok(Arc::clone(package));
        }
        let resolved = resolve_skill_package_directory(directory)?;
        let canonical = fs::canonicalize(&resolved).map_err(|source| {
            RuntimeError::io(
                format!("resolving skill package directory {}", resolved.display()),
                source,
            )
        })?;
        self.packages.get(&canonical).cloned().ok_or_else(|| {
            runx_parser::SkillPackageError::invalid(
                canonical.to_string_lossy(),
                "is not a skill package this run admitted",
            )
            .into()
        })
    }
}

/// What a run binds to: its closure's digest, when the closure is
/// fully bound, and every package the closure binds.
pub(crate) struct ExecutionBinding {
    pub(crate) closure_digest: Option<String>,
    pub(crate) packages: AdmittedPackages,
}

pub(crate) fn verify_loaded_execution_binding(
    loaded: impl Into<Arc<LoadedSkillPackage>>,
    method: &str,
    env: &std::collections::BTreeMap<String, String>,
    expected_package_digest: Option<&str>,
    expected_closure_digest: Option<&str>,
) -> Result<ExecutionBinding, SkillInspectionError> {
    let loaded = loaded.into();
    if let Some(expected) = expected_package_digest
        && expected != loaded.package.package_digest
    {
        return Err(SkillInspectionError::PackageDigestMismatch {
            expected: expected.to_owned(),
            received: loaded.package.package_digest.clone(),
        });
    }
    let closure = inspect_loaded_closure_binding(loaded, method, env)?;
    if let Some(expected) = expected_closure_digest {
        if !closure.fully_bound {
            return Err(SkillInspectionError::ClosureNotFullyBound {
                method: method.to_owned(),
            });
        }
        if closure.digest != expected {
            return Err(SkillInspectionError::ClosureDigestMismatch {
                expected: expected.to_owned(),
                received: closure.digest,
            });
        }
    }
    Ok(ExecutionBinding {
        closure_digest: closure.fully_bound.then_some(closure.digest),
        packages: AdmittedPackages {
            packages: Arc::new(closure.packages),
            identities: Arc::new(closure.identities),
            projects: Arc::new(closure.projects),
        },
    })
}

pub fn load_validated_skill_package(path: &Path) -> Result<LoadedSkillPackage, RuntimeError> {
    let unresolved_directory = resolve_skill_package_directory(path)?;
    // Runtime-owned package identity is absolute. Keeping caller-relative paths
    // here makes nested adapters accidentally reinterpret a valid package
    // against a test, resume, or daemon workspace later in execution.
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
    crate::work_counts::count(crate::work_counts::Work::PackageRead);
    let mut package = validated_package(source)?;
    let profile_path = selected_profile_path(&package_root, &directory)?
        .filter(|path| package.manifest_at(path).is_some());
    let resolved_input_packet_schemas =
        crate::packet_schemas::hydrate_packet_input_contracts(&mut package, &package_root)?;
    Ok(LoadedSkillPackage {
        directory,
        package_root,
        profile_path,
        package,
        resolved_input_packet_schemas,
    })
}

/// A package's validation, once per exact source. Validation is a pure
/// function of the package bytes, and one run loads the same package for every
/// step and module call that names it, so each distinct source is parsed once
/// per process. Any changed byte is another source.
fn validated_package(
    source: SkillPackageSource,
) -> Result<runx_parser::ValidatedSkillPackage, runx_parser::SkillPackageError> {
    use std::sync::{Mutex, OnceLock};

    const RETAINED: usize = 64;
    static VALIDATED: OnceLock<Mutex<BTreeMap<[u8; 32], runx_parser::ValidatedSkillPackage>>> =
        OnceLock::new();

    let key = source_identity(&source);
    let cache = VALIDATED.get_or_init(Mutex::default);
    if let Some(validated) = cache.lock().ok().and_then(|cache| cache.get(&key).cloned()) {
        return Ok(validated);
    }
    crate::work_counts::package_verified(key);
    let validated = validate_skill_package(source)?;
    if let Ok(mut cache) = cache.lock() {
        if cache.len() >= RETAINED {
            cache.clear();
        }
        cache.insert(key, validated.clone());
    }
    Ok(validated)
}

/// A package source's identity: the digest of every path and its bytes, and
/// every link, so any changed byte is another source.
pub(crate) fn source_identity(source: &SkillPackageSource) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for (path, bytes) in &source.files {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    for link in &source.symlinks {
        hasher.update((link.len() as u64).to_le_bytes());
        hasher.update(link.as_bytes());
    }
    hasher.finalize().into()
}

/// A connector's tool package as it ships: every regular file below `dir`,
/// keyed by its package-relative POSIX path and read within a skill
/// package's bounds. A link is refused, so the tool that runs is exactly the
/// bytes its digest names.
pub(crate) fn read_tool_package(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>, RuntimeError> {
    let metadata = fs::symlink_metadata(dir)
        .map_err(|source| RuntimeError::io(format!("reading {}", dir.display()), source))?;
    if !metadata.is_dir() {
        return Err(runx_parser::SkillPackageError::invalid(
            dir.to_string_lossy(),
            "a connector tool package is a directory, never a link",
        )
        .into());
    }
    let mut source = SkillPackageSource::default();
    collect_package_source(dir, dir, &mut source, &mut PackageTotals::default())?;
    if let Some(link) = source.symlinks.first() {
        return Err(runx_parser::SkillPackageError::invalid(
            link.clone(),
            "a connector tool package holds no links",
        )
        .into());
    }
    Ok(source.files)
}

/// Discover executable skill-package roots in a workspace.
///
/// A repository may carry a root `SKILL.md` as its native operator manual while
/// also owning a `skills/` catalog. That manual is exportable agent context, not
/// a package boundary around the entire repository. A standalone root remains a
/// package, as does an explicit root `run.yaml` package in a catalog workspace.
pub(crate) fn discover_workspace_skill_package_dirs(
    root: &Path,
) -> Result<Vec<PathBuf>, RuntimeError> {
    let skills_root = root.join("skills");
    let mut directories = Vec::new();
    collect_skill_package_roots(&skills_root, &mut directories)?;
    if root.join("SKILL.md").is_file()
        && (!skills_root.is_dir() || root.join(runx_parser::PROFILE_FILE).is_file())
    {
        directories.push(root.to_path_buf());
    }
    directories.sort();
    directories.dedup();
    Ok(directories)
}

fn collect_skill_package_roots(
    directory: &Path,
    packages: &mut Vec<PathBuf>,
) -> Result<(), RuntimeError> {
    let entries = read_dir_sorted(directory)?;
    if entries
        .iter()
        .any(|entry| entry.is_file && entry.name == "SKILL.md")
    {
        packages.push(directory.to_path_buf());
        if entries.iter().any(|entry| {
            entry.is_file && entry.name == crate::registry::package_bundle::PACKAGE_BUNDLE_FILE
        }) {
            // A published bundle explicitly owns its nested packages. Source
            // trees can instead contain separately authored workflow-stage manuals
            // that list/doctor must still discover with their own metadata.
            crate::registry::package_bundle::package_bundle_root(directory).map_err(|message| {
                RuntimeError::SkillFailed {
                    skill_name: directory.display().to_string(),
                    message,
                }
            })?;
            return Ok(());
        }
    }
    for entry in entries {
        if entry.is_dir
            && !matches!(
                entry.name.as_str(),
                ".git" | ".runx" | "node_modules" | "target" | "fixtures"
            )
        {
            collect_skill_package_roots(&entry.path, packages)?;
        }
    }
    Ok(())
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
            Some("SKILL.md" | runx_parser::PROFILE_FILE)
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
        "skill package reference must be a directory, SKILL.md, or run.yaml",
    )
    .into())
}

fn resolve_owning_package_root(directory: &Path) -> Result<PathBuf, RuntimeError> {
    if let Some(package_root) = find_owning_package_root(directory)
        && (package_root == directory || directory.join(runx_parser::PROFILE_FILE).is_file())
    {
        return Ok(package_root);
    }
    if !directory.join(runx_parser::PROFILE_FILE).is_file() {
        return Err(runx_parser::SkillPackageError::invalid(
            directory.to_string_lossy(),
            "skill package directory must contain SKILL.md or an owned run.yaml profile",
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
    if !directory.join(runx_parser::PROFILE_FILE).is_file() {
        return Ok(None);
    }
    let relative = directory.strip_prefix(package_root).map_err(|_| {
        runx_parser::SkillPackageError::invalid(
            directory.to_string_lossy(),
            "execution profile is outside its owning package",
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Ok(Some(runx_parser::PROFILE_FILE.to_owned()));
    }
    Ok(Some(format!(
        "{}/{}",
        relative_path(relative)?,
        runx_parser::PROFILE_FILE
    )))
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
        // A package's signature is the registry's envelope over its files,
        // never one of them: bind verifies it apart (D30).
        if ignored_package_entry(&entry.name)
            || (current == root && entry.name == crate::registry::SIGNATURE_FILE)
        {
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
    let over_limit = |relative: String| -> RuntimeError {
        runx_parser::SkillPackageError::invalid(
            relative,
            format!(
                "skill package exceeds the {MAX_PACKAGE_FILES} file / {MAX_PACKAGE_BYTES} byte admission limit"
            ),
        )
        .into()
    };
    totals.files = totals.files.saturating_add(1);
    if totals.files > MAX_PACKAGE_FILES {
        return Err(over_limit(relative));
    }
    let remaining = MAX_PACKAGE_BYTES.saturating_sub(totals.bytes);
    let Some(contents) = read_within(&entry.path, remaining).map_err(|source| {
        RuntimeError::io(
            format!("reading package file {}", entry.path.display()),
            source,
        )
    })?
    else {
        return Err(over_limit(relative));
    };
    totals.bytes += contents.len();
    source.files.insert(relative, contents);
    Ok(())
}

/// A package file's bytes when it holds at most `budget` of them. Reading
/// stops one byte past the budget, so an oversized file is refused without
/// ever being held whole.
pub(crate) fn read_within(path: &Path, budget: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut contents = Vec::new();
    let file = fs::File::open(path)?;
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, (budget as u64).saturating_add(1)),
        &mut contents,
    )?;
    Ok((contents.len() <= budget).then_some(contents))
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
    matches!(
        name,
        ".git" | ".runx" | "node_modules" | "target" | "__pycache__"
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::fs;

    use super::discover_workspace_skill_package_dirs;

    const MANUAL: &str = "---\nname: demo\ndescription: Demo skill.\n---\n\n# Demo\n";

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
    fn catalog_discovery_excludes_nested_test_fixture_packages() {
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

    #[test]
    fn installed_package_dependencies_are_not_independent_catalog_entries() {
        let temp = tempfile::tempdir().expect("temporary workspace");
        let root = temp.path();
        let parent = root.join("skills/runx/receipt-audit/sha-parent");
        let nested = parent.join("dependencies/1");
        let independent = root.join("skills/runx/receipt-query/sha-child");
        fs::create_dir_all(&nested).expect("dependency directory");
        fs::create_dir_all(&independent).expect("independent package");
        for path in [&parent, &nested, &independent] {
            fs::write(path.join("SKILL.md"), MANUAL).expect("package manual");
        }
        let descriptor = crate::registry::package_bundle::encode_package_bundle(
            std::collections::BTreeSet::from([
                crate::registry::package_bundle::PackageBundleDependency {
                    from: ".".to_owned(),
                    reference: "../receipt-query".to_owned(),
                    path: "dependencies/1".to_owned(),
                },
            ]),
            std::collections::BTreeSet::new(),
        )
        .expect("valid published bundle descriptor");
        fs::write(
            parent.join(crate::registry::package_bundle::PACKAGE_BUNDLE_FILE),
            descriptor,
        )
        .expect("published bundle descriptor");
        assert_eq!(
            discover_workspace_skill_package_dirs(root).expect("discovery"),
            vec![parent, independent]
        );
    }

    fn write(path: &std::path::Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("a directory")).expect("directory");
        fs::write(path, contents).expect("file");
    }

    const DIGEST_METHOD: &str = "    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: hi\n          encoding: utf8_text\n";

    /// A workspace whose `skills/caller` runs a stage of its own by relative
    /// path, and `registry:acme/echo@1.0.0`, installed and signed.
    fn bound_workspace(
        caller_name: &str,
    ) -> (
        tempfile::TempDir,
        std::collections::BTreeMap<String, String>,
    ) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let caller = root.join("skills/caller");
        write(
            &caller.join("SKILL.md"),
            &format!("---\nname: {caller_name}\ndescription: Calls.\n---\n# Caller\n"),
        );
        write(
            &caller.join("run.yaml"),
            "minimum_runx: 0.12.0\nmethods:\n  main:\n    steps:\n      - id: stage\n        skill: ./stage#run\n      - id: echo\n        skill: registry:acme/echo@1.0.0#say\n",
        );
        write(
            &caller.join("stage/SKILL.md"),
            "---\nname: stage\ndescription: A stage.\n---\n# Stage\n",
        );
        write(
            &caller.join("stage/run.yaml"),
            &format!("minimum_runx: 0.12.0\nmethods:\n  run:\n{DIGEST_METHOD}"),
        );
        let echo = root.join(".runx/added-skills/acme/echo/1.0.0");
        write(
            &echo.join("SKILL.md"),
            "---\nname: echo\ndescription: Echoes.\n---\n# Echo\n",
        );
        write(
            &echo.join("run.yaml"),
            &format!("version: \"1.0.0\"\nminimum_runx: 0.12.0\nmethods:\n  say:\n{DIGEST_METHOD}"),
        );
        crate::registry::signature::testing::sign(
            &echo,
            runx_contracts::RegistrySubjectKind::Skill,
            "acme/echo",
            "1.0.0",
        )
        .expect("signed");
        let mut env = crate::registry::signature::testing::trusting("acme").expect("keys");
        env.insert(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            root.display().to_string(),
        );
        env.insert(
            crate::paths::RUNX_HOME_ENV.to_owned(),
            root.join("home").display().to_string(),
        );
        (temp, env)
    }

    /// Binding sets who each package is by how it was reached: the root by
    /// what it carries, code it carries by relative path as the root, and a
    /// registry child by its own verified signature (D30).
    #[test]
    fn binding_sets_each_packages_identity_by_how_it_was_reached() {
        let (temp, env) = bound_workspace("caller");
        let caller = temp.path().join("skills/caller");
        let loaded = super::load_validated_skill_package(&caller).expect("caller loads");
        let binding = super::verify_loaded_execution_binding(loaded, "main", &env, None, None)
            .expect("the closure binds");
        let identity = |path: &std::path::Path| {
            binding
                .packages
                .identity(&fs::canonicalize(path).expect("canonical"))
                .cloned()
        };
        let caller_identity = crate::identity::PackageIdentity::Project {
            name: "caller".to_owned(),
        };
        assert_eq!(identity(&caller), Some(caller_identity.clone()));
        assert_eq!(identity(&caller.join("stage")), Some(caller_identity));
        let echo = identity(&temp.path().join(".runx/added-skills/acme/echo/1.0.0"));
        assert!(
            matches!(&echo, Some(crate::identity::PackageIdentity::Signed(signed)) if signed.name == "acme/echo" && signed.version == "1.0.0"),
            "{echo:?}"
        );

        // A child copy altered after it was signed never binds.
        write(
            &temp
                .path()
                .join(".runx/added-skills/acme/echo/1.0.0/references/added.md"),
            "added\n",
        );
        let loaded = super::load_validated_skill_package(&caller).expect("caller loads");
        assert!(super::verify_loaded_execution_binding(loaded, "main", &env, None, None).is_err());
    }

    /// Two different directories never bind as one `project:<name>` in a run:
    /// here the root and the person's own copy of a registry ref.
    #[test]
    fn two_directories_never_bind_as_one_project_name() {
        let (temp, env) = bound_workspace("echo");
        let own = temp.path().join("skills/acme/echo");
        write(
            &own.join("SKILL.md"),
            "---\nname: echo\ndescription: The person's own copy.\n---\n# Echo\n",
        );
        write(
            &own.join("run.yaml"),
            &format!("minimum_runx: 0.12.0\nmethods:\n  say:\n{DIGEST_METHOD}"),
        );
        let loaded = super::load_validated_skill_package(&temp.path().join("skills/caller"))
            .expect("caller loads");
        let error = super::verify_loaded_execution_binding(loaded, "main", &env, None, None)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            error.contains("two directories bind as project:echo"),
            "{error}"
        );
    }

    /// Code a package incorporates writes its streams under its carrier, at
    /// `<carrier>/<name>/...`, so two directories it carries under one name
    /// would share them: a binding refuses them, and one directory reached
    /// twice binds.
    #[test]
    fn a_package_never_carries_two_directories_under_one_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("skills/carrier");
        write(
            &root.join("SKILL.md"),
            "---\nname: carrier\ndescription: Carries two githubs.\n---\n# Carrier\n",
        );
        for stage in ["one/github", "two/github"] {
            write(
                &root.join(stage).join("SKILL.md"),
                "---\nname: github\ndescription: A stage named github.\n---\n# Github\n",
            );
            write(
                &root.join(stage).join("run.yaml"),
                &format!("minimum_runx: 0.12.0\nmethods:\n  say:\n{DIGEST_METHOD}"),
            );
        }
        let carrier = |second: &str| {
            format!(
                "minimum_runx: 0.12.0\nmethods:\n  main:\n    steps:\n      - id: first\n        skill: ./one/github#say\n      - id: second\n        skill: {second}#say\n"
            )
        };
        let env = std::collections::BTreeMap::from([(
            crate::paths::RUNX_HOME_ENV.to_owned(),
            temp.path().join("home").display().to_string(),
        )]);
        let bind = || {
            let loaded = super::load_validated_skill_package(&root).expect("carrier loads");
            super::verify_loaded_execution_binding(loaded, "main", &env, None, None)
                .err()
                .map(|error| error.to_string())
        };
        write(&root.join("run.yaml"), &carrier("./one/github"));
        assert_eq!(bind(), None, "one directory reached twice binds");
        write(&root.join("run.yaml"), &carrier("./two/github"));
        let error = bind().unwrap_or_default();
        assert!(
            error.contains("carries two directories named github"),
            "{error}"
        );
    }

    #[test]
    fn a_file_past_its_budget_is_refused_without_being_read_whole() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("big.bin");
        std::fs::write(&path, vec![7_u8; 64]).expect("write");

        assert_eq!(
            super::read_within(&path, 64)
                .expect("read")
                .map(|bytes| bytes.len()),
            Some(64)
        );
        assert_eq!(super::read_within(&path, 63).expect("read"), None);
    }
}
