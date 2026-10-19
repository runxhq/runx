//! A skill package's harness: its runners' inline cases and its fixture
//! files, each run as a case through the one harness engine.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::JsonObject;
use runx_parser::RunnerHarnessCase;
use serde::Serialize;

use super::fixtures::{HarnessFixture, HarnessSetup};
use super::runner::{load_harness_case, run_harness_case_with_adapter};
use super::{HarnessExpectedStatus, HarnessFixtureKind, HarnessOptions, HarnessReplayError};
use crate::RuntimeError;
use crate::effects::RuntimeEffectRegistry;
use crate::execution::skill_front::{ExecutableAdapter, SkillRunError};
use crate::paths::{RUNX_CWD_ENV, RUNX_RECEIPT_DIR_ENV, RunxPaths};
use crate::services::ReceiptServices;

/// Aggregate result of running a package's inline and conventional fixture
/// cases. Mirrors the publish-harness summary the registry records: a status,
/// counts, per-case assertion failures, case names, sealed receipt ids, and how
/// many `kind: graph` fixtures proved the skill composes into a graph.
#[derive(Clone, Debug, Serialize)]
pub struct PackageHarnessReport {
    pub status: &'static str,
    pub case_count: usize,
    pub assertion_error_count: usize,
    pub assertion_errors: Vec<String>,
    pub case_names: Vec<String>,
    pub receipt_ids: Vec<String>,
    pub graph_case_count: usize,
}

impl PackageHarnessReport {
    fn not_declared() -> Self {
        Self {
            status: "not_declared",
            case_count: 0,
            assertion_error_count: 0,
            assertion_errors: Vec::new(),
            case_names: Vec::new(),
            receipt_ids: Vec::new(),
            graph_case_count: 0,
        }
    }
}

/// A case as loaded: its fixture and target, or why it could not be read.
type LoadedCase = Result<(HarnessFixture, PathBuf), String>;

/// Run every harness case a skill package owns, its runners' inline
/// `harness.cases` and its conventional `fixtures/*.yaml` files, each as a
/// case through the one harness engine. Discovery is deterministic, and this
/// is the single package entry point the CLI and publishing use.
pub(crate) fn run_package_harness_with_effects(
    skill_path: &Path,
    receipt_dir: Option<&Path>,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<PackageHarnessReport, SkillRunError> {
    let loaded = crate::load_validated_skill_package(skill_path)?;
    let workspace = crate::WorkspaceEnv::from_admitted(env.clone()).map_err(RuntimeError::from)?;
    let mut base_env = workspace.env().clone();
    let credentials = super::credential_variables(&loaded.directory, &base_env)?;
    super::isolate_harness_environment(&mut base_env, &credentials);
    let harness = PackageHarnessEnvironment::prepare(
        base_env,
        workspace.paths(),
        &loaded.directory,
        receipt_dir,
    )?;
    harness.stage_declared_files(&loaded)?;
    harness.stage_installed_connectors(&loaded, workspace.paths())?;
    let mut report = PackageHarnessReport::not_declared();
    let mut cases = inline_cases(&loaded)?;
    for path in conventional_fixture_paths(&loaded.directory)? {
        cases
            .push(load_harness_case(&path).map_err(|error| format!("{}: {error}", path.display())));
    }
    replay_cases(cases, &harness, effects, &mut report)?;
    finalize_report(&mut report);
    Ok(report)
}

/// A runner's inline cases, each the fixture it is: a skill case against the
/// package itself.
fn inline_cases(loaded: &crate::LoadedSkillPackage) -> Result<Vec<LoadedCase>, SkillRunError> {
    let Some(harness) = loaded
        .manifest()
        .and_then(|manifest| manifest.harness.as_ref())
    else {
        return Ok(Vec::new());
    };
    let target = fs::canonicalize(&loaded.directory).map_err(|source| {
        RuntimeError::io(
            format!("resolving harness target {}", loaded.directory.display()),
            source,
        )
    })?;
    Ok(harness
        .cases
        .iter()
        .map(|case| Ok((inline_fixture(case), target.clone())))
        .collect())
}

fn inline_fixture(case: &RunnerHarnessCase) -> HarnessFixture {
    HarnessFixture {
        name: case.name.clone(),
        kind: HarnessFixtureKind::Skill,
        target: ".".to_owned(),
        runner: case.runner.clone(),
        setup: HarnessSetup::default(),
        inputs: case.inputs.clone(),
        env: case.env.clone(),
        caller: case.caller.clone(),
        expect: case.expect.clone(),
        operator_journeys: case.operator_journeys.clone(),
        metadata: JsonObject::new(),
    }
}

fn replay_cases(
    cases: Vec<LoadedCase>,
    harness: &PackageHarnessEnvironment,
    effects: &RuntimeEffectRegistry,
    report: &mut PackageHarnessReport,
) -> Result<(), SkillRunError> {
    if cases.is_empty() {
        return Ok(());
    }
    let mut base_options = HarnessOptions::new(harness.env.clone())?;
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
            harness
                .fixture_receipt_dir(index)
                .to_string_lossy()
                .into_owned(),
        );
        let name = fixture.name.clone();
        let is_graph = matches!(fixture.kind, HarnessFixtureKind::Graph);
        let denial_expected = fixture.expect.status == Some(HarnessExpectedStatus::PolicyDenied);
        match run_harness_case_with_adapter(fixture, target_path, ExecutableAdapter, options) {
            Ok(output) => {
                persist_fixture_receipts(&receipt_services, &harness.receipt_dir, &output)?;
                if is_graph {
                    report.graph_case_count += 1;
                }
                report.case_names.push(output.fixture.name);
                report.receipt_ids.push(output.receipt.id.to_string());
            }
            Err(error) if denial_expected && is_authority_denial(&error) => {
                if is_graph {
                    report.graph_case_count += 1;
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
/// never hides a parser, harness or runtime-invariant defect.
fn is_authority_denial(error: &HarnessReplayError) -> bool {
    matches!(
        error,
        HarnessReplayError::Runtime(RuntimeError::AuthorityDenied { .. })
    )
}

fn persist_fixture_receipts(
    receipt_services: &ReceiptServices,
    receipt_dir: &Path,
    output: &super::HarnessReplayOutput,
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

fn finalize_report(report: &mut PackageHarnessReport) {
    report.assertion_error_count = report.assertion_errors.len();
    report.status = if report.case_count == 0 {
        "not_declared"
    } else if report.assertion_errors.is_empty() {
        "passed"
    } else {
        "failed"
    };
}

struct PackageHarnessEnvironment {
    env: BTreeMap<String, String>,
    receipt_dir: PathBuf,
    scratch_root: PathBuf,
    workspace: PathBuf,
}

impl PackageHarnessEnvironment {
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
                format!(
                    "creating isolated harness workspace {}",
                    workspace.display()
                ),
                source,
            )
        })?;
        let receipt_dir = operator.harness_receipts(receipt_dir);
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

    fn stage_declared_files(
        &self,
        loaded: &crate::LoadedSkillPackage,
    ) -> Result<(), SkillRunError> {
        let mut packet_schemas = crate::packet_schemas::PacketSchemaCatalog::default();
        for schema in loaded.resolved_input_packet_schemas.values() {
            packet_schemas
                .insert(schema.clone())
                .map_err(|error| RuntimeError::SkillFailed {
                    skill_name: "package-harness".to_owned(),
                    message: format!("packet schema catalog failed: {error}"),
                })?;
        }
        packet_schemas
            .discover_directories([
                loaded.directory.join("packets"),
                loaded.package_root.join("packets"),
            ])
            .map_err(|error| RuntimeError::SkillFailed {
                skill_name: "package-harness".to_owned(),
                message: format!("packet schema catalog failed: {error}"),
            })?;
        for schema in packet_schemas.entries() {
            self.stage_file(
                &format!("packets/{}", schema.file_name),
                schema.source.as_bytes(),
                false,
            )?;
        }
        let Some(harness) = loaded
            .manifest()
            .and_then(|manifest| manifest.harness.as_ref())
        else {
            return Ok(());
        };
        let profile_directory = loaded
            .profile_path
            .as_deref()
            .and_then(|path| path.rsplit_once('/').map(|(directory, _)| directory));
        for declared in &harness.files {
            let source_path = profile_directory.map_or_else(
                || declared.clone(),
                |directory| format!("{directory}/{declared}"),
            );
            let contents = loaded.package.file_bytes(&source_path).ok_or_else(|| {
                RuntimeError::SkillFailed {
                    skill_name: "package-harness".to_owned(),
                    message: format!(
                        "validated harness support file {source_path:?} is unavailable"
                    ),
                }
            })?;
            self.stage_file(declared, contents, is_declared_harness_executable(declared))?;
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
            skill_name: "package-harness".to_owned(),
            message: format!("staging an installed connector failed: {message}"),
        };
        let Some(manifest) = loaded.manifest() else {
            return Ok(());
        };
        let scope = crate::ConnectorScope::in_project(&loaded.directory, operator)
            .map_err(|error| failed(error.to_string()))?;
        let staged = RunxPaths::resolve(&self.env, &self.workspace).added_connectors();
        for (key, requirement) in &manifest.connectors {
            let references = match requirement.connector_from {
                Some(_) => requirement.allowed_connectors.clone(),
                None => vec![key.clone()],
            };
            for reference in references {
                // A connector out of reach is the run's to refuse, as it would
                // anywhere; only an installed version is staged.
                let Ok(connector) = scope.requirement(&reference, requirement) else {
                    continue;
                };
                if let crate::kernel::connectors::ConnectorOrigin::Installed { directory } =
                    &connector.origin
                {
                    crate::registry::write_connector_version(&staged, &connector, Some(directory))
                        .map_err(failed)?;
                }
            }
        }
        Ok(())
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
                    format!("creating harness fixture directory {}", parent.display()),
                    source,
                )
            })?;
        }
        fs::write(&destination, contents).map_err(|source| {
            RuntimeError::io(
                format!("staging harness support file {}", destination.display()),
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
                            "marking harness support file executable {}",
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

fn is_declared_harness_executable(relative: &str) -> bool {
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
        .harness_runs()
        .join(format!("run-{}-{nanos}", std::process::id()))
}

impl Drop for PackageHarnessEnvironment {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.scratch_root);
    }
}

fn conventional_fixture_paths(skill_dir: &Path) -> Result<Vec<PathBuf>, SkillRunError> {
    let fixtures_dir = skill_dir.join("fixtures");
    let entries = match fs::read_dir(&fixtures_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(
                RuntimeError::io(format!("reading {}", fixtures_dir.display()), source).into(),
            );
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| {
            RuntimeError::io(format!("reading {}", fixtures_dir.display()), source)
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
        PackageHarnessEnvironment, PackageHarnessReport, RUNX_CWD_ENV, RUNX_RECEIPT_DIR_ENV,
        RunxPaths, finalize_report, is_authority_denial, run_package_harness_with_effects,
    };

    #[test]
    fn package_harness_proves_missing_native_scope_is_refused()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("permission-claim")?;
        let skill_dir = operator_workspace.join("skills/permission-claim");
        fs::create_dir_all(&skill_dir)?;
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: permission-claim\ndescription: Proves harness scope admission.\n---\n\n# Permission Claim\n",
        )?;
        fs::create_dir_all(skill_dir.join("fixtures"))?;
        fs::write(skill_dir.join("fixtures/present.txt"), "present\n")?;
        fs::write(
            skill_dir.join("run.yaml"),
            r#"skill: permission-claim
version: "0.1.0"
harness:
  files:
    - fixtures/present.txt
  cases:
    - name: refuses-missing-filesystem-scope
      runner: inspect
      inputs: {}
      expect:
        status: failure
runners:
  inspect:
    default: true
    result_from: [read]
    steps:
      - id: read
        tool: fs.read
        inputs:
          path: fixtures/present.txt
"#,
        )?;
        let env = BTreeMap::from([(
            RUNX_CWD_ENV.to_owned(),
            operator_workspace.to_string_lossy().into_owned(),
        )]);

        let report = run_package_harness_with_effects(
            &skill_dir,
            None,
            &env,
            &crate::RuntimeEffectRegistry::default(),
        )?;

        assert_eq!(report.status, "passed", "{report:?}");
        assert_eq!(report.case_names, ["refuses-missing-filesystem-scope"]);
        assert!(report.assertion_errors.is_empty());
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

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
            r#"skill: unsent-exchange
version: "0.1.0"
harness:
  cases:
    - name: refused-before-its-write
      runner: inspect
      inputs: {}
      caller:
        http_exchanges:
          - request: { method: PATCH, url: "https://fixture.runx.invalid/item", body: none }
            response: { status: 200, headers: { content-type: application/json }, body: "{}" }
      expect:
        status: failure
runners:
  inspect:
    default: true
    inputs:
      path: { type: string, required: true }
    result_from: [read]
    steps:
      - id: read
        tool: fs.read
        inputs:
          path: $input.path
"#,
        )?;
        let env = BTreeMap::from([(
            RUNX_CWD_ENV.to_owned(),
            operator_workspace.to_string_lossy().into_owned(),
        )]);

        let report = run_package_harness_with_effects(
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
        let denial = crate::execution::harness::HarnessReplayError::Runtime(
            crate::RuntimeError::AuthorityDenied {
                verb: runx_contracts::AuthorityVerb::Read,
                step_id: "provider-read".to_owned(),
                reason: "no admitted grant".to_owned(),
            },
        );
        assert!(is_authority_denial(&denial));
        let unrelated = crate::execution::harness::HarnessReplayError::Runtime(
            crate::RuntimeError::SkillFailed {
                skill_name: "provider-read".to_owned(),
                message: "unexpected adapter defect".to_owned(),
            },
        );
        assert!(!is_authority_denial(&unrelated));
    }

    #[test]
    fn empty_package_harness_remains_not_declared() {
        let mut report = PackageHarnessReport::not_declared();

        finalize_report(&mut report);

        assert_eq!(report.status, "not_declared");
        assert_eq!(report.case_count, 0);
    }

    #[test]
    fn package_harness_uses_disposable_workspace_owned_runx_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("isolated")?;
        fs::create_dir_all(&operator_workspace)?;
        let skill_dir = operator_workspace.join("skills/demo");
        let harness = PackageHarnessEnvironment::prepare(
            BTreeMap::new(),
            &operator(&operator_workspace),
            &skill_dir,
            None,
        )?;
        let workspace = PathBuf::from(
            harness
                .env
                .get(RUNX_CWD_ENV)
                .ok_or("missing isolated RUNX_CWD")?,
        );
        let scratch_root = harness.scratch_root.clone();

        assert!(workspace.starts_with(operator_workspace.join(".runx").join("harness")));
        assert_eq!(workspace, scratch_root.join("workspace"));
        assert_eq!(harness.workspace, workspace);
        assert_eq!(
            harness.receipt_dir,
            operator_workspace.join(".runx").join("harness-receipts")
        );
        assert_eq!(
            harness.env.get(RUNX_RECEIPT_DIR_ENV),
            Some(
                &operator_workspace
                    .join(".runx")
                    .join("harness-receipts")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert_eq!(
            harness.env.get(crate::paths::RUNX_PROJECT_DIR_ENV),
            Some(&scratch_root.join("project").to_string_lossy().into_owned())
        );
        assert_ne!(
            harness.fixture_receipt_dir(0),
            harness.fixture_receipt_dir(1)
        );
        drop(harness);
        assert!(!scratch_root.exists());
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[test]
    fn explicit_harness_workspace_owns_disposable_run_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("explicit-operator")?;
        fs::create_dir_all(&operator_workspace)?;
        let mut env = BTreeMap::new();
        env.insert(
            RUNX_CWD_ENV.to_owned(),
            operator_workspace.to_string_lossy().into_owned(),
        );
        let skill_dir = operator_workspace.join("skills/demo");
        let harness = PackageHarnessEnvironment::prepare(
            env,
            &operator(&operator_workspace),
            &skill_dir,
            None,
        )?;
        let scratch_root = harness.scratch_root.clone();

        assert!(scratch_root.starts_with(operator_workspace.join(".runx").join("harness")));
        assert_eq!(
            harness.env.get(RUNX_CWD_ENV),
            Some(
                &scratch_root
                    .join("workspace")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        drop(harness);
        assert!(!scratch_root.exists());
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[test]
    fn relative_explicit_receipt_dir_is_anchored_before_workspace_isolation()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("explicit-receipts")?;
        fs::create_dir_all(&operator_workspace)?;
        let harness = PackageHarnessEnvironment::prepare(
            BTreeMap::new(),
            &operator(&operator_workspace),
            &operator_workspace.join("skills/demo"),
            Some(PathBuf::from(".runx/custom-receipts").as_path()),
        )?;

        let expected = operator_workspace.join(".runx").join("custom-receipts");
        assert_eq!(harness.receipt_dir, expected);
        assert_eq!(
            harness.env.get(RUNX_RECEIPT_DIR_ENV),
            Some(&expected.to_string_lossy().into_owned())
        );
        drop(harness);
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[test]
    fn package_harness_keeps_workspace_tool_catalogs_after_cwd_isolation()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("tool-roots")?;
        let skill_dir = operator_workspace.join("skills/demo");
        let tools_dir = operator_workspace.join("tools");
        fs::create_dir_all(&skill_dir)?;
        fs::create_dir_all(&tools_dir)?;

        let harness = PackageHarnessEnvironment::prepare(
            BTreeMap::new(),
            &operator(&operator_workspace),
            &skill_dir,
            None,
        )?;
        let configured = harness
            .env
            .get("RUNX_TOOL_ROOTS")
            .ok_or("missing inferred tool roots")?;
        let roots = std::env::split_paths(configured).collect::<Vec<_>>();

        assert!(roots.contains(&tools_dir));
        drop(harness);
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[test]
    fn package_harness_stages_only_declared_profile_relative_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("staged-files")?;
        let skill_dir = operator_workspace.join("skills/demo");
        fs::create_dir_all(skill_dir.join("fixtures"))?;
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Harness staging fixture.\n---\n\n# Demo\n",
        )?;
        fs::write(
            skill_dir.join("run.yaml"),
            r#"skill: demo
version: "0.1.0"
harness:
  files:
    - fixtures/declared.txt
  cases: []
runners:
  inspect:
    default: true
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
        let harness = PackageHarnessEnvironment::prepare(
            BTreeMap::new(),
            &operator(&operator_workspace),
            &skill_dir,
            None,
        )?;

        harness.stage_declared_files(&loaded)?;

        assert_eq!(
            fs::read_to_string(harness.workspace.join("fixtures/declared.txt"))?,
            "declared"
        );
        assert!(!harness.workspace.join("fixtures/undeclared.txt").exists());
        drop(harness);
        fs::remove_dir_all(operator_workspace)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn package_harness_marks_declared_fixture_bin_files_executable()
    -> Result<(), Box<dyn std::error::Error>> {
        let operator_workspace = unique_test_root("staged-executable")?;
        let skill_dir = operator_workspace.join("skills/demo");
        fs::create_dir_all(skill_dir.join("fixtures/bin"))?;
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Harness executable fixture.\n---\n\n# Demo\n",
        )?;
        fs::write(
            skill_dir.join("run.yaml"),
            r#"skill: demo
version: "0.1.0"
harness:
  files:
    - fixtures/bin/demo-command
  cases: []
runners:
  inspect:
    default: true
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
        let harness = PackageHarnessEnvironment::prepare(
            BTreeMap::new(),
            &operator(&operator_workspace),
            &skill_dir,
            None,
        )?;

        harness.stage_declared_files(&loaded)?;

        let mode = fs::metadata(harness.workspace.join("fixtures/bin/demo-command"))?
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
        drop(harness);
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
            "runx-package-harness-{label}-{}-{nanos}",
            std::process::id()
        )))
    }
}
