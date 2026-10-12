// Module rationale: this first dev-mode slice keeps fixture
// discovery, workspace materialization, and result projection together until
// native skill/graph dev execution creates the next durable module boundary.
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use runx_contracts::{DoctorStatus, JsonObject, JsonValue, json_string_field as string_field};
use runx_parser::{
    DevFixtureExpectation, DevFixtureGit, DevFixtureGitConfig, DevFixtureLane,
    DevFixtureTargetKind, DevFixtureWorkspace, DevOutputExpectation,
};

use super::materialize::{materialize_fixture_string, materialize_fixture_value};
use super::skill::run_skill_or_graph_fixture;
use super::support::elapsed_ms;
use super::tool::run_tool_fixture;
use super::types::{
    DevError, DevFixtureAssertion, DevFixtureAssertionKind, DevFixtureExecutionRoots,
    DevFixtureExecutor, DevFixtureResult, DevFixtureStatus, DevLoopOptions, DevReport,
    DevReportSchema, DevReportStatus, LoadedDevFixture, LocalDevFixtureExecutor,
    PreparedDevFixtureWorkspace,
};
use crate::doctor::{default_doctor_options, run_doctor};
use crate::path_util::lexical_normalize;

pub fn run_dev_once(options: &DevLoopOptions) -> Result<DevReport, DevError> {
    run_dev_once_with_executor(
        options,
        &LocalDevFixtureExecutor {
            env: options.env.clone(),
        },
    )
}

pub fn run_dev_once_with_executor(
    options: &DevLoopOptions,
    executor: &impl DevFixtureExecutor,
) -> Result<DevReport, DevError> {
    let root = lexical_normalize(&options.root);
    let doctor = run_doctor(&root, &default_doctor_options())?;
    if doctor.status == DoctorStatus::Failure {
        return Ok(DevReport {
            schema: DevReportSchema::V1,
            status: DevReportStatus::Failure,
            doctor,
            fixtures: Vec::new(),
            receipt_id: None,
        });
    }

    let fixture_paths = discover_fixture_paths(
        options.unit_path.as_deref().unwrap_or(root.as_path()),
        &root,
    )?;
    let mut fixtures = Vec::new();
    for fixture_path in fixture_paths {
        let parsed = load_dev_fixture_file(&fixture_path)?;
        fixtures.push(run_or_skip_fixture(&root, &parsed, options.lane, executor)?);
    }

    Ok(DevReport {
        schema: DevReportSchema::V1,
        status: report_status(&fixtures),
        doctor,
        fixtures,
        receipt_id: None,
    })
}

pub fn discover_fixture_paths(unit_path: &Path, root: &Path) -> Result<Vec<PathBuf>, DevError> {
    let stat_path = if unit_path.exists() { unit_path } else { root };
    let mut paths = yaml_files_in(&stat_path.join("fixtures"))?;
    if !paths.is_empty() && stat_path != root {
        paths.sort();
        return Ok(paths);
    }
    for tool_dir in discover_tool_directories(root)? {
        paths.extend(yaml_files_in(&tool_dir.join("fixtures"))?);
    }
    paths.sort();
    Ok(paths)
}

impl DevFixtureExecutor for LocalDevFixtureExecutor {
    fn run_fixture(
        &self,
        root: &Path,
        fixture: &LoadedDevFixture,
    ) -> Result<DevFixtureResult, DevError> {
        match fixture.target().kind {
            DevFixtureTargetKind::Tool => run_tool_fixture(root, fixture, &self.env),
            DevFixtureTargetKind::Skill | DevFixtureTargetKind::Graph => {
                run_skill_or_graph_fixture(root, fixture, &self.env)
            }
        }
    }
}

fn run_or_skip_fixture(
    root: &Path,
    fixture: &LoadedDevFixture,
    selected_lane: Option<DevFixtureLane>,
    executor: &impl DevFixtureExecutor,
) -> Result<DevFixtureResult, DevError> {
    let started = Instant::now();
    let fixture_lane = fixture.lane().as_str();
    if let Some(selected_lane) = selected_lane
        && fixture.lane() != selected_lane
    {
        return Ok(DevFixtureResult {
            name: fixture.name().to_owned(),
            lane: fixture_lane.to_owned(),
            target: fixture.target_json(),
            status: DevFixtureStatus::Skipped,
            duration_ms: elapsed_ms(started),
            assertions: Vec::new(),
            skip_reason: Some(format!(
                "lane {} excluded by --lane {}",
                fixture_lane,
                selected_lane.as_str()
            )),
            output: None,
            replay_path: None,
        });
    }
    if !matches!(
        fixture.lane(),
        DevFixtureLane::Deterministic | DevFixtureLane::RepoIntegration
    ) {
        return Ok(DevFixtureResult {
            name: fixture.name().to_owned(),
            lane: fixture_lane.to_owned(),
            target: fixture.target_json(),
            status: DevFixtureStatus::Skipped,
            duration_ms: elapsed_ms(started),
            assertions: Vec::new(),
            skip_reason: Some(format!(
                "{} fixtures are parsed but not executed in dev v1",
                fixture_lane
            )),
            output: None,
            replay_path: None,
        });
    }
    executor.run_fixture(root, fixture)
}

fn load_dev_fixture_file(path: &Path) -> Result<LoadedDevFixture, DevError> {
    let contents = fs::read_to_string(path).map_err(|source| DevError::ReadFixture {
        path: path.to_path_buf(),
        source,
    })?;
    let definition =
        runx_parser::parse_dev_fixture(&contents).map_err(|source| DevError::ParseFixture {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(LoadedDevFixture {
        path: path.to_path_buf(),
        definition,
    })
}

pub(super) fn prepare_fixture_workspace(
    root: &Path,
    fixture_path: &Path,
    workspace: Option<&DevFixtureWorkspace>,
) -> Result<PreparedDevFixtureWorkspace, DevError> {
    let fixture_dir = fixture_path.parent().unwrap_or(root);
    let mut tokens = BTreeMap::from([
        (
            "RUNX_REPO_ROOT".to_owned(),
            root.to_string_lossy().into_owned(),
        ),
        (
            "RUNX_FIXTURE_FILE".to_owned(),
            fixture_path.to_string_lossy().into_owned(),
        ),
        (
            "RUNX_FIXTURE_DIR".to_owned(),
            fixture_dir.to_string_lossy().into_owned(),
        ),
    ]);
    let Some(workspace) = workspace else {
        return Ok(PreparedDevFixtureWorkspace { root: None, tokens });
    };
    let fixture_root = unique_temp_dir()?;
    tokens.insert(
        "RUNX_FIXTURE_ROOT".to_owned(),
        fixture_root.to_string_lossy().into_owned(),
    );
    write_fixture_file_map(
        &fixture_root,
        Some(&workspace.files),
        &tokens,
        false,
        FixtureFileMode::Regular,
    )?;
    write_fixture_file_map(
        &fixture_root,
        Some(&workspace.json_files),
        &tokens,
        true,
        FixtureFileMode::Regular,
    )?;
    write_fixture_file_map(
        &fixture_root,
        Some(&workspace.executable_files),
        &tokens,
        false,
        FixtureFileMode::Executable,
    )?;
    initialize_fixture_git(&fixture_root, workspace.git.as_ref(), &tokens)?;
    Ok(PreparedDevFixtureWorkspace {
        root: Some(fixture_root),
        tokens,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FixtureFileMode {
    Regular,
    Executable,
}

fn write_fixture_file_map(
    root: &Path,
    files: Option<&JsonObject>,
    tokens: &BTreeMap<String, String>,
    force_json: bool,
    mode: FixtureFileMode,
) -> Result<(), DevError> {
    let Some(files) = files else {
        return Ok(());
    };
    for (relative_path, raw_contents) in files {
        let target_path = resolve_inside_fixture_root(root, relative_path)?;
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|source| DevError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let contents = if force_json {
            format!(
                "{}\n",
                serde_json::to_string_pretty(&materialize_fixture_value(
                    raw_contents.clone(),
                    tokens
                ))
                .map_err(|source| DevError::Json {
                    path: target_path.clone(),
                    source,
                })?
            )
        } else if let JsonValue::String(value) = raw_contents {
            materialize_fixture_string(value, tokens)
        } else {
            format!(
                "{}\n",
                serde_json::to_string_pretty(&materialize_fixture_value(
                    raw_contents.clone(),
                    tokens
                ))
                .map_err(|source| DevError::Json {
                    path: target_path.clone(),
                    source,
                })?
            )
        };
        fs::write(&target_path, contents).map_err(|source| DevError::Io {
            path: target_path.clone(),
            source,
        })?;
        apply_fixture_file_mode(&target_path, mode)?;
    }
    Ok(())
}

fn apply_fixture_file_mode(path: &Path, mode: FixtureFileMode) -> Result<(), DevError> {
    if mode != FixtureFileMode::Executable {
        return Ok(());
    }
    apply_executable_fixture_file_mode(path)
}

#[cfg(unix)]
fn apply_executable_fixture_file_mode(path: &Path) -> Result<(), DevError> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(|source| DevError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|source| DevError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn apply_executable_fixture_file_mode(_path: &Path) -> Result<(), DevError> {
    Ok(())
}

fn initialize_fixture_git(
    root: &Path,
    git: Option<&DevFixtureGit>,
    tokens: &BTreeMap<String, String>,
) -> Result<(), DevError> {
    let config = match git {
        Some(DevFixtureGit::Enabled(true)) => None,
        Some(DevFixtureGit::Config(config)) => Some(config),
        None | Some(DevFixtureGit::Enabled(false)) => return Ok(()),
    };
    let branch = config
        .and_then(|config| config.initial_branch.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("main");
    run_required_process("git", &["init", "-b", branch], root)?;
    run_required_process(
        "git",
        &["config", "user.email", "fixture@example.com"],
        root,
    )?;
    run_required_process("git", &["config", "user.name", "Runx Fixture"], root)?;

    if config.and_then(|config| config.commit) != Some(false) {
        run_required_process("git", &["add", "."], root)?;
        run_required_process("git", &["commit", "-m", "fixture baseline"], root)?;
    }

    if let Some(DevFixtureGitConfig { dirty_files, .. }) = config {
        write_fixture_file_map(
            root,
            Some(dirty_files),
            tokens,
            false,
            FixtureFileMode::Regular,
        )?;
    }
    Ok(())
}

fn run_required_process(command: &str, args: &[&str], cwd: &Path) -> Result<(), DevError> {
    let child_cwd = crate::process::child_process_cwd(cwd);
    let output = crate::process::with_spawn_lock(|| {
        Command::new(command)
            .args(args)
            .current_dir(child_cwd.as_ref())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    })
    .and_then(|child| child.wait_with_output())
    .map_err(|source| DevError::Spawn {
        command: command.to_owned(),
        source,
    })?;
    if output.status.success() {
        return Ok(());
    }

    let status = output.status.code().unwrap_or(1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    Err(DevError::FixtureCommand {
        command: format!("{} {}", command, args.join(" ")),
        status,
        output: detail.to_owned(),
    })
}

fn resolve_inside_fixture_root(root: &Path, relative_path: &str) -> Result<PathBuf, DevError> {
    let relative = Path::new(relative_path);
    if relative.is_absolute() {
        return Err(DevError::AbsoluteWorkspacePath {
            path: relative_path.to_owned(),
        });
    }
    let resolved = lexical_normalize(&root.join(relative));
    if !resolved.starts_with(root) {
        return Err(DevError::EscapingWorkspacePath {
            path: relative_path.to_owned(),
        });
    }
    Ok(resolved)
}

pub(super) fn resolve_fixture_execution_roots(
    root: &Path,
    lane: DevFixtureLane,
    workspace_root: Option<&Path>,
) -> Option<DevFixtureExecutionRoots> {
    if lane == DevFixtureLane::RepoIntegration {
        let workspace_root = workspace_root?;
        return Some(DevFixtureExecutionRoots {
            cwd: workspace_root.to_path_buf(),
            repo_root: workspace_root.to_path_buf(),
        });
    }
    Some(DevFixtureExecutionRoots {
        cwd: workspace_root.unwrap_or(root).to_path_buf(),
        repo_root: root.to_path_buf(),
    })
}

pub(super) fn assert_fixture_expectation(
    expectation: &DevFixtureExpectation,
    exit_code: i32,
    output: Option<&JsonValue>,
) -> Vec<DevFixtureAssertion> {
    let mut assertions = Vec::new();
    let expected_status = expectation.status.as_str();
    let actual_status = if exit_code == 0 { "success" } else { "failure" };
    if expected_status != actual_status {
        assertions.push(DevFixtureAssertion {
            path: "expect.status".to_owned(),
            expected: Some(JsonValue::String(expected_status.to_owned())),
            actual: Some(JsonValue::String(actual_status.to_owned())),
            kind: DevFixtureAssertionKind::StatusMismatch,
            message: format!("Expected status {expected_status}, got {actual_status}."),
        });
    }
    if let Some(output_expectation) = &expectation.output {
        assertions.extend(assert_output_expectation(
            output_expectation,
            output.unwrap_or(&JsonValue::String(String::new())),
            "expect.output",
        ));
    }
    assertions
}

fn assert_output_expectation(
    expectation: &DevOutputExpectation,
    output: &JsonValue,
    base_path: &str,
) -> Vec<DevFixtureAssertion> {
    let mut assertions = Vec::new();
    if let Some(exact) = &expectation.exact
        && output != exact
    {
        assertions.push(DevFixtureAssertion {
            path: format!("{base_path}.exact"),
            expected: Some(exact.clone()),
            actual: Some(output.clone()),
            kind: DevFixtureAssertionKind::ExactMismatch,
            message: "Output did not exactly match.".to_owned(),
        });
    }
    if let Some(subset) = &expectation.subset {
        let subset_output =
            subset_assertion_output(expectation, subset, output, base_path, &mut assertions);
        assertions.extend(assert_subset(subset, subset_output, ""));
    }
    assertions
}

fn subset_assertion_output<'a>(
    expectation: &DevOutputExpectation,
    subset: &JsonValue,
    output: &'a JsonValue,
    base_path: &str,
    assertions: &mut Vec<DevFixtureAssertion>,
) -> &'a JsonValue {
    let Some(output_object) = object_value(output) else {
        return output;
    };

    if let Some(expected_packet) = expectation.matches_packet.as_deref() {
        let actual_schema = string_field(output_object, "schema").unwrap_or_default();
        if actual_schema != expected_packet {
            assertions.push(DevFixtureAssertion {
                path: format!("{base_path}.matches_packet"),
                expected: Some(JsonValue::String(expected_packet.to_owned())),
                actual: Some(JsonValue::String(actual_schema.to_owned())),
                kind: DevFixtureAssertionKind::ExactMismatch,
                message: "Output packet schema did not match.".to_owned(),
            });
        }
        if subset_addresses_packet_wrapper(subset) {
            return output;
        }
        return output_object.get("data").unwrap_or(output);
    }

    if output_object.contains_key("schema") && !subset_addresses_packet_wrapper(subset) {
        return output_object.get("data").unwrap_or(output);
    }
    output
}

fn subset_addresses_packet_wrapper(subset: &JsonValue) -> bool {
    match subset {
        JsonValue::Object(object) => object.contains_key("schema") || object.contains_key("data"),
        _ => false,
    }
}

fn assert_subset(
    expected: &JsonValue,
    actual: &JsonValue,
    base_path: &str,
) -> Vec<DevFixtureAssertion> {
    let JsonValue::Object(expected_object) = expected else {
        return if expected == actual {
            Vec::new()
        } else {
            vec![DevFixtureAssertion {
                path: base_path.to_owned(),
                expected: Some(expected.clone()),
                actual: Some(actual.clone()),
                kind: DevFixtureAssertionKind::SubsetMiss,
                message: "Subset value did not match.".to_owned(),
            }]
        };
    };
    let mut assertions = Vec::new();
    for (key, value) in expected_object {
        let path = if base_path.is_empty() {
            key.clone()
        } else {
            format!("{base_path}.{key}")
        };
        let actual_value = match actual {
            JsonValue::Object(object) => object.get(key).unwrap_or(&JsonValue::Null),
            _ => &JsonValue::Null,
        };
        assertions.extend(assert_subset(value, actual_value, &path));
    }
    assertions
}

fn object_value(value: &JsonValue) -> Option<&JsonObject> {
    match value {
        JsonValue::Object(object) => Some(object),
        _ => None,
    }
}

pub(super) fn failed_fixture(
    fixture: &LoadedDevFixture,
    started: Instant,
    assertions: Vec<DevFixtureAssertion>,
) -> DevFixtureResult {
    DevFixtureResult {
        name: fixture.name().to_owned(),
        lane: fixture.lane().as_str().to_owned(),
        target: fixture.target_json(),
        status: DevFixtureStatus::Failure,
        duration_ms: elapsed_ms(started),
        assertions,
        skip_reason: None,
        output: None,
        replay_path: None,
    }
}

fn report_status(fixtures: &[DevFixtureResult]) -> DevReportStatus {
    if fixtures
        .iter()
        .any(|fixture| fixture.status == DevFixtureStatus::Failure)
    {
        DevReportStatus::Failure
    } else if fixtures
        .iter()
        .any(|fixture| fixture.status == DevFixtureStatus::Success)
    {
        DevReportStatus::Success
    } else {
        DevReportStatus::Skipped
    }
}

fn discover_tool_directories(root: &Path) -> Result<Vec<PathBuf>, DevError> {
    let tools_root = root.join("tools");
    let mut directories = Vec::new();
    for namespace in safe_read_dir(&tools_root)? {
        let namespace_path = namespace.path();
        if !namespace_path.is_dir() {
            continue;
        }
        for tool in safe_read_dir(&namespace_path)? {
            let tool_path = tool.path();
            if tool_path.is_dir() {
                directories.push(tool_path);
            }
        }
    }
    directories.sort();
    Ok(directories)
}

fn yaml_files_in(directory: &Path) -> Result<Vec<PathBuf>, DevError> {
    Ok(safe_read_dir(directory)?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_yaml_file(path))
        .collect())
}

fn safe_read_dir(directory: &Path) -> Result<Vec<fs::DirEntry>, DevError> {
    match fs::read_dir(directory) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| DevError::Io {
                path: directory.to_path_buf(),
                source,
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(DevError::Io {
            path: directory.to_path_buf(),
            source,
        }),
    }
}

fn unique_temp_dir() -> Result<PathBuf, DevError> {
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temp_id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!(
        "runx-fixture-{}-{nanos}-{temp_id}",
        std::process::id()
    ));
    fs::create_dir_all(&path).map_err(|source| DevError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subset_expectations_unwrap_packet_data() {
        let assertions = assert_fixture_expectation(
            &DevFixtureExpectation {
                status: runx_parser::DevExpectedStatus::Success,
                output: Some(DevOutputExpectation {
                    subset: Some(object_value_from([(
                        "message",
                        JsonValue::String("hello".to_owned()),
                    )])),
                    ..DevOutputExpectation::default()
                }),
            },
            0,
            Some(&object_value_from([
                ("schema", JsonValue::String("runx.echo.v1".to_owned())),
                (
                    "data",
                    object_value_from([("message", JsonValue::String("hello".to_owned()))]),
                ),
            ])),
        );

        assert!(assertions.is_empty(), "{assertions:#?}");
    }

    #[test]
    fn matches_packet_checks_schema_and_unwraps_data() {
        let assertions = assert_fixture_expectation(
            &DevFixtureExpectation {
                status: runx_parser::DevExpectedStatus::Success,
                output: Some(DevOutputExpectation {
                    matches_packet: Some("runx.echo.v1".to_owned()),
                    subset: Some(object_value_from([(
                        "message",
                        JsonValue::String("hello".to_owned()),
                    )])),
                    ..DevOutputExpectation::default()
                }),
            },
            0,
            Some(&object_value_from([
                ("schema", JsonValue::String("runx.echo.v1".to_owned())),
                (
                    "data",
                    object_value_from([("message", JsonValue::String("hello".to_owned()))]),
                ),
            ])),
        );

        assert!(assertions.is_empty(), "{assertions:#?}");
    }

    #[test]
    fn subset_expectations_can_address_packet_wrapper() {
        let assertions = assert_fixture_expectation(
            &DevFixtureExpectation {
                status: runx_parser::DevExpectedStatus::Success,
                output: Some(DevOutputExpectation {
                    subset: Some(object_value_from([(
                        "data",
                        object_value_from([("message", JsonValue::String("hello".to_owned()))]),
                    )])),
                    ..DevOutputExpectation::default()
                }),
            },
            0,
            Some(&object_value_from([
                ("schema", JsonValue::String("runx.echo.v1".to_owned())),
                (
                    "data",
                    object_value_from([("message", JsonValue::String("hello".to_owned()))]),
                ),
            ])),
        );

        assert!(assertions.is_empty(), "{assertions:#?}");
    }

    fn object_value_from(
        entries: impl IntoIterator<Item = (&'static str, JsonValue)>,
    ) -> JsonValue {
        JsonValue::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }
}
