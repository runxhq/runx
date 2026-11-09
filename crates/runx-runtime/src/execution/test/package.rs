//! A package's tests: each `tests/<name>.yaml`, run as a case through the
//! one test engine.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::fixtures::TestFixture;
use super::runner::{load_test_case, run_test_case_with_adapter};
use super::{TestExpectedStatus, TestOptions, TestReplayError};
use crate::RuntimeError;
use crate::effects::RuntimeEffectRegistry;
use crate::execution::skill_front::{ExecutableAdapter, SkillRunError};
use crate::paths::{RUNX_CWD_ENV, RUNX_RECEIPT_DIR_ENV, RunxPaths};
use crate::services::ReceiptServices;

/// Aggregate result of running a package's tests. Mirrors the publish-test
/// summary the registry records: a status, counts, per-case assertion
/// failures, case names, sealed receipt ids, and how many tests proved the
/// skill composes, by running a fixture package that calls it.
#[derive(Clone, Debug, Serialize)]
pub struct PackageTestReport {
    pub status: &'static str,
    pub case_count: usize,
    pub assertion_error_count: usize,
    pub assertion_errors: Vec<String>,
    pub case_names: Vec<String>,
    pub receipt_ids: Vec<String>,
    pub workflow_case_count: usize,
}

impl PackageTestReport {
    fn not_declared() -> Self {
        Self {
            status: "not_declared",
            case_count: 0,
            assertion_error_count: 0,
            assertion_errors: Vec::new(),
            case_names: Vec::new(),
            receipt_ids: Vec::new(),
            workflow_case_count: 0,
        }
    }
}

/// A case as loaded: its fixture and target, or why it could not be read.
type LoadedCase = Result<(TestFixture, PathBuf), String>;

/// Run every test a package owns, its `tests/*.yaml`, each as a case through
/// the one test engine. Discovery is deterministic, and this is the single
/// package entry point the CLI and publishing use.
pub(crate) fn run_package_test_with_effects(
    skill_path: &Path,
    receipt_dir: Option<&Path>,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<PackageTestReport, SkillRunError> {
    let (loaded, test) = prepare_package_tests(skill_path, receipt_dir, env)?;
    let mut report = PackageTestReport::not_declared();
    let cases = test_paths(&loaded.directory)?
        .into_iter()
        .map(|path| load_test_case(&path).map_err(|error| format!("{}: {error}", path.display())))
        .collect();
    replay_cases(cases, &test, effects, &mut report)?;
    finalize_report(&mut report);
    Ok(report)
}

/// Run one of a package's tests, `<package>/tests/<name>.yaml`, as a run of
/// all of them runs each: in a workspace of its own, with the package's
/// `fixtures/` staged whole and its installed connectors and skills. The
/// outer error is the workspace's; the inner one is the case's.
pub(crate) fn run_package_case_with_effects(
    test_path: &Path,
    receipt_dir: Option<&Path>,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<Result<super::TestReplayOutput, TestReplayError>, SkillRunError> {
    let test_path = match fs::canonicalize(test_path) {
        Ok(path) => path,
        Err(source) => {
            return Ok(Err(RuntimeError::io(
                format!("resolving the test {}", test_path.display()),
                source,
            )
            .into()));
        }
    };
    let Some(package) = test_path.parent().and_then(Path::parent) else {
        return Ok(Err(TestReplayError::TargetWithoutParent {
            target: test_path,
        }));
    };
    let (_loaded, test) = prepare_package_tests(package, receipt_dir, env)?;
    let mut options = TestOptions::new(test.env.clone())?;
    options.effects = effects.clone();
    options.env.insert(
        RUNX_RECEIPT_DIR_ENV.to_owned(),
        test.fixture_receipt_dir(0).to_string_lossy().into_owned(),
    );
    let receipt_services =
        ReceiptServices::from_signature_config(options.receipt_signature.clone());
    let output = match load_test_case(&test_path).and_then(|(fixture, target)| {
        run_test_case_with_adapter(fixture, target, ExecutableAdapter, options)
    }) {
        Ok(output) => output,
        Err(error) => return Ok(Err(error)),
    };
    persist_fixture_receipts(&receipt_services, &test.receipt_dir, &output)?;
    Ok(Ok(output))
}

/// The package at `skill_path`, and the workspace its tests run in: the
/// environment without ambient authority or the package's credentials, its
/// `fixtures/`, and its installed connectors and skills.
fn prepare_package_tests(
    skill_path: &Path,
    receipt_dir: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> Result<(crate::LoadedSkillPackage, PackageTestEnvironment), SkillRunError> {
    let loaded = crate::load_validated_skill_package(skill_path)?;
    let workspace = crate::WorkspaceEnv::from_admitted(env.clone()).map_err(RuntimeError::from)?;
    let mut base_env = workspace.env().clone();
    let credentials = super::credential_variables(&loaded.directory, &base_env)?;
    super::isolate_test_environment(&mut base_env, &credentials);
    let test = PackageTestEnvironment::prepare(
        base_env,
        workspace.paths(),
        &loaded.directory,
        receipt_dir,
    )?;
    test.stage_fixtures(&loaded)?;
    test.stage_installed_connectors(&loaded, workspace.paths())?;
    test.stage_installed_skills(&loaded, workspace.env())?;
    Ok((loaded, test))
}

fn replay_cases(
    cases: Vec<LoadedCase>,
    test: &PackageTestEnvironment,
    effects: &RuntimeEffectRegistry,
    report: &mut PackageTestReport,
) -> Result<(), SkillRunError> {
    if cases.is_empty() {
        return Ok(());
    }
    let mut base_options = TestOptions::new(test.env.clone())?;
    base_options.effects = effects.clone();
    let receipt_services =
        ReceiptServices::from_signature_config(base_options.receipt_signature.clone());
    for (index, case) in cases.into_iter().enumerate() {
        report.case_count += 1;
        let (fixture, target_path) = match case {
            Ok(case) => case,
            Err(error) => {
                report.assertion_errors.push(error);
                continue;
            }
        };
        let mut options = base_options.clone();
        options.env.insert(
            RUNX_RECEIPT_DIR_ENV.to_owned(),
            test.fixture_receipt_dir(index)
                .to_string_lossy()
                .into_owned(),
        );
        let name = fixture.name.clone();
        let is_workflow = fixture.package.is_some();
        let denial_expected = fixture.expect.status == Some(TestExpectedStatus::PolicyDenied);
        match run_test_case_with_adapter(fixture, target_path, ExecutableAdapter, options) {
            Ok(output) => {
                persist_fixture_receipts(&receipt_services, &test.receipt_dir, &output)?;
                if is_workflow {
                    report.workflow_case_count += 1;
                }
                report.case_names.push(output.fixture.name);
                report.receipt_ids.push(output.receipt.id.to_string());
            }
            Err(error) if denial_expected && is_authority_denial(&error) => {
                if is_workflow {
                    report.workflow_case_count += 1;
                }
                report.case_names.push(name);
            }
            Err(error) => report.assertion_errors.push(format!("{name}: {error}")),
        }
    }
    Ok(())
}

/// A case runs below the skill front so it can expose step receipts and
/// output assertions, so a native authority denial returns directly instead
/// of as the front's sealed terminal envelope. Only that exact refusal stands
/// for a case that expects `policy_denied`; a broad `failure` expectation
/// never hides a parser, test or runtime-invariant defect.
fn is_authority_denial(error: &TestReplayError) -> bool {
    matches!(
        error,
        TestReplayError::Runtime(RuntimeError::AuthorityDenied { .. })
    )
}

fn persist_fixture_receipts(
    receipt_services: &ReceiptServices,
    receipt_dir: &Path,
    output: &super::TestReplayOutput,
) -> Result<(), SkillRunError> {
    receipt_services.write_local_receipts(
        output
            .steps
            .iter()
            .flat_map(|step| {
                step.nested_receipts
                    .iter()
                    .chain(std::iter::once(&step.receipt))
            })
            .chain(std::iter::once(&output.receipt)),
        receipt_dir,
    )?;
    Ok(())
}

fn finalize_report(report: &mut PackageTestReport) {
    report.assertion_error_count = report.assertion_errors.len();
    report.status = if report.case_count == 0 {
        "not_declared"
    } else if report.assertion_errors.is_empty() {
        "passed"
    } else {
        "failed"
    };
}

struct PackageTestEnvironment {
    env: BTreeMap<String, String>,
    receipt_dir: PathBuf,
    scratch_root: PathBuf,
    workspace: PathBuf,
}

impl PackageTestEnvironment {
    fn prepare(
        mut env: BTreeMap<String, String>,
        operator: &RunxPaths,
        skill_dir: &Path,
        receipt_dir: Option<&Path>,
    ) -> Result<Self, SkillRunError> {
        crate::services::merge_inferred_tool_roots(&mut env, skill_dir);
        let scratch_root = unique_scratch_root(operator);
        let workspace = scratch_root.join("workspace");
        fs::create_dir_all(&workspace).map_err(|source| {
            RuntimeError::io(
                format!("creating isolated test workspace {}", workspace.display()),
                source,
            )
        })?;
        let receipt_dir = operator.test_receipts(receipt_dir);
        env.insert(
            RUNX_RECEIPT_DIR_ENV.to_owned(),
            receipt_dir.to_string_lossy().into_owned(),
        );
        env.insert(
            RUNX_CWD_ENV.to_owned(),
            workspace.to_string_lossy().into_owned(),
        );
        crate::paths::isolate(&mut env, &scratch_root);
        Ok(Self {
            env,
            receipt_dir,
            scratch_root,
            workspace,
        })
    }

    /// Stage every file under the package's `fixtures/`, whole, with
    /// `fixtures/bin/` executable.
    fn stage_fixtures(&self, loaded: &crate::LoadedSkillPackage) -> Result<(), SkillRunError> {
        for path in &loaded.package.test_files {
            if !path.starts_with("fixtures/") {
                continue;
            }
            let contents =
                loaded
                    .package
                    .file_bytes(path)
                    .ok_or_else(|| RuntimeError::SkillFailed {
                        skill_name: "package-test".to_owned(),
                        message: format!("validated test fixture {path:?} is unavailable"),
                    })?;
            self.stage_file(path, contents, is_declared_test_executable(path))?;
        }
        Ok(())
    }

    /// Stage in the disposable workspace the installed connector versions
    /// the skill binds, read from the operator's project: its cases reach
    /// exactly what its operations resolve, and nothing else installed there.
    fn stage_installed_connectors(
        &self,
        loaded: &crate::LoadedSkillPackage,
        operator: &RunxPaths,
    ) -> Result<(), SkillRunError> {
        let failed = |message: String| RuntimeError::SkillFailed {
            skill_name: "package-test".to_owned(),
            message: format!("staging an installed connector failed: {message}"),
        };
        let staged = RunxPaths::resolve(&self.env, &self.workspace).added_connectors();
        for connector in bound_installed_connectors(loaded, operator)? {
            if let crate::kernel::connectors::ConnectorOrigin::Installed { directory } =
                &connector.origin
            {
                crate::registry::write_connector_version(&staged, &connector, Some(directory))
                    .map_err(failed)?;
            }
        }
        Ok(())
    }

    /// Stage in the disposable project the installed registry skills the
    /// package's closure names, read from the operator's project, each as its
    /// install left it: its cases read the registry skills a run there reads,
    /// and nothing else installed there.
    fn stage_installed_skills(
        &self,
        loaded: &crate::LoadedSkillPackage,
        operator_env: &BTreeMap<String, String>,
    ) -> Result<(), SkillRunError> {
        stage_installed_registry_skills(loaded, operator_env, &self.env).map_err(|message| {
            RuntimeError::SkillFailed {
                skill_name: "package-test".to_owned(),
                message: format!("staging an installed registry skill failed: {message}"),
            }
            .into()
        })
    }

    fn stage_file(
        &self,
        relative: &str,
        contents: &[u8],
        executable: bool,
    ) -> Result<(), SkillRunError> {
        let destination = self.workspace.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                RuntimeError::io(
                    format!("creating test fixture directory {}", parent.display()),
                    source,
                )
            })?;
        }
        fs::write(&destination, contents).map_err(|source| {
            RuntimeError::io(
                format!("staging test support file {}", destination.display()),
                source,
            )
        })?;
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).map_err(
                |source| {
                    RuntimeError::io(
                        format!(
                            "marking test support file executable {}",
                            destination.display()
                        ),
                        source,
                    )
                },
            )?;
        }
        #[cfg(not(unix))]
        let _ = executable;
        Ok(())
    }

    fn fixture_receipt_dir(&self, index: usize) -> PathBuf {
        self.scratch_root
            .join("fixture-receipts")
            .join(index.to_string())
    }
}

/// Copy into the project of `target_env` each registry skill the closure of
/// `loaded` names that the project of `source_env` holds, with its install
/// record and the connectors it carries: execution reads registry skills
/// only from the project a run is in.
pub(super) fn stage_installed_registry_skills(
    loaded: &crate::LoadedSkillPackage,
    source_env: &BTreeMap<String, String>,
    target_env: &BTreeMap<String, String>,
) -> Result<(), String> {
    let source = RunxPaths::resolve(source_env, &loaded.directory);
    let target = RunxPaths::resolve(target_env, &loaded.directory);
    if source.project() == target.project() {
        return Ok(());
    }
    for reference in crate::skill_package::closure_registry_refs(loaded, source_env)
        .map_err(|error| error.to_string())?
    {
        if let Some(directory) =
            crate::execution::workflow::installed_registry_skill(&reference, &source)
                .map_err(|error| error.to_string())?
        {
            crate::registry::copy_installed_skill(&source, &target, &directory)?;
        }
    }
    Ok(())
}

/// Resolve only the pinned versions the package declares. An unrelated or
/// broken install in the operator's project cannot affect this test.
pub(super) fn bound_installed_connectors(
    loaded: &crate::LoadedSkillPackage,
    paths: &RunxPaths,
) -> Result<Vec<Arc<crate::kernel::connectors::LoadedConnector>>, SkillRunError> {
    let Some(manifest) = loaded.manifest() else {
        return Ok(Vec::new());
    };
    let scope = crate::ConnectorScope::in_project(&loaded.directory, paths)
        .map_err(|error| RuntimeError::effect_state("selecting test connectors", error))?;
    let mut bound: Vec<Arc<crate::kernel::connectors::LoadedConnector>> = Vec::new();
    for (key, requirement) in &manifest.connectors {
        let references = match requirement.choose {
            Some(_) => requirement.from.clone(),
            None => vec![key.clone()],
        };
        for reference in references {
            // An unavailable connector is the run's to refuse, not a staging
            // error. No other installed version belongs in the case.
            let Ok(connector) = scope.requirement(&reference, requirement) else {
                continue;
            };
            if matches!(
                connector.origin,
                crate::kernel::connectors::ConnectorOrigin::Installed { .. }
            ) && !bound
                .iter()
                .any(|known: &Arc<_>| known.connector_digest == connector.connector_digest)
            {
                bound.push(connector);
            }
        }
    }
    Ok(bound)
}

fn is_declared_test_executable(relative: &str) -> bool {
    let mut segments = relative.split('/');
    matches!(segments.next(), Some("fixtures"))
        && matches!(segments.next(), Some("bin"))
        && segments.next().is_some()
}

fn unique_scratch_root(operator: &RunxPaths) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    operator
        .test_runs()
        .join(format!("run-{}-{nanos}", std::process::id()))
}

impl Drop for PackageTestEnvironment {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.scratch_root);
    }
}

/// A package's tests, `tests/*.yaml`, in name order.
fn test_paths(skill_dir: &Path) -> Result<Vec<PathBuf>, SkillRunError> {
    let tests_dir = skill_dir.join("tests");
    let entries = match fs::read_dir(&tests_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(
                RuntimeError::io(format!("reading {}", tests_dir.display()), source).into(),
            );
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| {
            RuntimeError::io(format!("reading {}", tests_dir.display()), source)
        })?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| matches!(extension, "yaml" | "yml"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::{
        PackageTestEnvironment, PackageTestReport, RUNX_CWD_ENV, RUNX_RECEIPT_DIR_ENV, RunxPaths,
        finalize_report, is_authority_denial, run_package_test_with_effects,
    };

    #[test]
    fn a_case_refused_at_its_inputs_still_proves_only_the_requests_it_made()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("unsent-exchange")?;
        let skill_dir = operator_workspace.join("skills/unsent-exchange");
        fs::create_dir_all(&skill_dir)?;
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: unsent-exchange\ndescription: Proves a refused case declares no exchange.\n---\n\n# Unsent Exchange\n",
        )?;
        // The inputs are refused before any step, so the write the case
        // declares was never sent, and the case cannot pass as though it had
        // been.
        fs::write(
            skill_dir.join("run.yaml"),
            r#"version: "0.1.0"
minimum_runx: 0.12.0
methods:
  inspect:
    inputs:
      path: {type: string}
    result_from: [read]
    steps:
      - id: read
        tool: fs.read
        inputs:
          path: $input.path
"#,
        )?;
        fs::create_dir_all(skill_dir.join("tests"))?;
        fs::write(
            skill_dir.join("tests/refused-before-its-write.yaml"),
            r#"name: refused-before-its-write
method: inspect
inputs: {}
providers:
  http:
    - request: {method: PATCH, url: "https://fixture.runx.invalid/item", body: none}
      response: {status: 200, headers: {content-type: application/json}, body: "{}"}
expect:
  status: failure
"#,
        )?;
        let env = BTreeMap::from([(
            RUNX_CWD_ENV.to_owned(),
            operator_workspace.to_string_lossy().into_owned(),
        )]);

        let report = run_package_test_with_effects(
            &skill_dir,
            None,
            &env,
            &crate::RuntimeEffectRegistry::default(),
        )?;

        assert_eq!(report.status, "failed", "{report:?}");
        assert!(
            report
                .assertion_errors
                .iter()
                .any(|error| error
                    .contains("never requested: PATCH https://fixture.runx.invalid/item")),
            "{report:?}"
        );
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[test]
    fn only_a_native_authority_denial_stands_for_an_expected_policy_denial() {
        let denial = crate::execution::test::TestReplayError::Runtime(
            crate::RuntimeError::AuthorityDenied {
                verb: runx_contracts::AuthorityVerb::Read,
                step_id: "provider-read".to_owned(),
                reason: "no admitted grant".to_owned(),
            },
        );
        assert!(is_authority_denial(&denial));
        let unrelated =
            crate::execution::test::TestReplayError::Runtime(crate::RuntimeError::SkillFailed {
                skill_name: "provider-read".to_owned(),
                message: "unexpected adapter defect".to_owned(),
            });
        assert!(!is_authority_denial(&unrelated));
    }

    #[test]
    fn empty_package_test_remains_not_declared() {
        let mut report = PackageTestReport::not_declared();

        finalize_report(&mut report);

        assert_eq!(report.status, "not_declared");
        assert_eq!(report.case_count, 0);
    }

    #[test]
    fn package_test_uses_disposable_workspace_owned_runx_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("isolated")?;
        fs::create_dir_all(&operator_workspace)?;
        let skill_dir = operator_workspace.join("skills/demo");
        let test = PackageTestEnvironment::prepare(
            BTreeMap::new(),
            &operator(&operator_workspace),
            &skill_dir,
            None,
        )?;
        let workspace = PathBuf::from(
            test.env
                .get(RUNX_CWD_ENV)
                .ok_or("missing isolated RUNX_CWD")?,
        );
        let scratch_root = test.scratch_root.clone();

        assert!(workspace.starts_with(operator_workspace.join(".runx").join("test")));
        assert_eq!(workspace, scratch_root.join("workspace"));
        assert_eq!(test.workspace, workspace);
        assert_eq!(
            test.receipt_dir,
            operator_workspace.join(".runx").join("test-receipts")
        );
        assert_eq!(
            test.env.get(RUNX_RECEIPT_DIR_ENV),
            Some(
                &operator_workspace
                    .join(".runx")
                    .join("test-receipts")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert_eq!(
            test.env.get(crate::paths::RUNX_PROJECT_DIR_ENV),
            Some(&scratch_root.join("project").to_string_lossy().into_owned())
        );
        assert_ne!(test.fixture_receipt_dir(0), test.fixture_receipt_dir(1));
        drop(test);
        assert!(!scratch_root.exists());
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[test]
    fn explicit_test_workspace_owns_disposable_run_state() -> Result<(), Box<dyn std::error::Error>>
    {
        let operator_workspace = unique_test_root("explicit-operator")?;
        fs::create_dir_all(&operator_workspace)?;
        let mut env = BTreeMap::new();
        env.insert(
            RUNX_CWD_ENV.to_owned(),
            operator_workspace.to_string_lossy().into_owned(),
        );
        let skill_dir = operator_workspace.join("skills/demo");
        let test =
            PackageTestEnvironment::prepare(env, &operator(&operator_workspace), &skill_dir, None)?;
        let scratch_root = test.scratch_root.clone();

        assert!(scratch_root.starts_with(operator_workspace.join(".runx").join("test")));
        assert_eq!(
            test.env.get(RUNX_CWD_ENV),
            Some(
                &scratch_root
                    .join("workspace")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        drop(test);
        assert!(!scratch_root.exists());
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[test]
    fn relative_explicit_receipt_dir_is_anchored_before_workspace_isolation()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("explicit-receipts")?;
        fs::create_dir_all(&operator_workspace)?;
        let test = PackageTestEnvironment::prepare(
            BTreeMap::new(),
            &operator(&operator_workspace),
            &operator_workspace.join("skills/demo"),
            Some(PathBuf::from(".runx/custom-receipts").as_path()),
        )?;

        let expected = operator_workspace.join(".runx").join("custom-receipts");
        assert_eq!(test.receipt_dir, expected);
        assert_eq!(
            test.env.get(RUNX_RECEIPT_DIR_ENV),
            Some(&expected.to_string_lossy().into_owned())
        );
        drop(test);
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[test]
    fn package_test_keeps_workspace_tool_catalogs_after_cwd_isolation()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("tool-roots")?;
        let skill_dir = operator_workspace.join("skills/demo");
        let tools_dir = operator_workspace.join("tools");
        fs::create_dir_all(&skill_dir)?;
        fs::create_dir_all(&tools_dir)?;

        let test = PackageTestEnvironment::prepare(
            BTreeMap::new(),
            &operator(&operator_workspace),
            &skill_dir,
            None,
        )?;
        let configured = test
            .env
            .get("RUNX_TOOL_ROOTS")
            .ok_or("missing inferred tool roots")?;
        let roots = std::env::split_paths(configured).collect::<Vec<_>>();

        assert!(roots.contains(&tools_dir));
        drop(test);
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[test]
    fn package_test_stages_its_fixtures_whole() -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("staged-files")?;
        let skill_dir = operator_workspace.join("skills/demo");
        fs::create_dir_all(skill_dir.join("fixtures"))?;
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Test staging fixture.\n---\n\n# Demo\n",
        )?;
        fs::write(
            skill_dir.join("run.yaml"),
            r#"version: "0.1.0"
minimum_runx: 0.12.0
methods:
  inspect:
    result_from:
      - digest
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: demo
"#,
        )?;
        fs::write(skill_dir.join("fixtures/declared.txt"), "declared")?;
        fs::write(skill_dir.join("fixtures/undeclared.txt"), "undeclared")?;
        let loaded = crate::load_validated_skill_package(&skill_dir)?;
        let test = PackageTestEnvironment::prepare(
            BTreeMap::new(),
            &operator(&operator_workspace),
            &skill_dir,
            None,
        )?;

        test.stage_fixtures(&loaded)?;

        assert_eq!(
            fs::read_to_string(test.workspace.join("fixtures/declared.txt"))?,
            "declared"
        );
        assert_eq!(
            fs::read_to_string(test.workspace.join("fixtures/undeclared.txt"))?,
            "undeclared",
            "fixtures/ is staged whole"
        );
        drop(test);
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn package_test_marks_declared_fixture_bin_files_executable()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("staged-executable")?;
        let skill_dir = operator_workspace.join("skills/demo");
        fs::create_dir_all(skill_dir.join("fixtures/bin"))?;
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Test executable fixture.\n---\n\n# Demo\n",
        )?;
        fs::write(
            skill_dir.join("run.yaml"),
            r#"version: "0.1.0"
minimum_runx: 0.12.0
methods:
  inspect:
    result_from:
      - digest
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: demo
"#,
        )?;
        fs::write(
            skill_dir.join("fixtures/bin/demo-command"),
            "#!/bin/sh\nexit 0\n",
        )?;
        let loaded = crate::load_validated_skill_package(&skill_dir)?;
        let test = PackageTestEnvironment::prepare(
            BTreeMap::new(),
            &operator(&operator_workspace),
            &skill_dir,
            None,
        )?;

        test.stage_fixtures(&loaded)?;

        let mode = fs::metadata(test.workspace.join("fixtures/bin/demo-command"))?
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
        drop(test);
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    fn operator(workspace: &std::path::Path) -> RunxPaths {
        let env = BTreeMap::from([(
            RUNX_CWD_ENV.to_owned(),
            workspace.to_string_lossy().into_owned(),
        )]);
        RunxPaths::resolve(&env, workspace)
    }

    fn unique_test_root(label: &str) -> Result<PathBuf, std::io::Error> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        Ok(std::env::temp_dir().join(format!(
            "runx-package-test-{label}-{}-{nanos}",
            std::process::id()
        )))
    }
}
