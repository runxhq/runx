#![cfg(feature = "process-tool")]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use runx_contracts::{JsonObject, JsonValue};
use runx_parser::ProcessTool;
use runx_runtime::RUNX_CWD_ENV;
use runx_runtime::adapter::{Executable, InvocationStatus, SkillAdapter, SkillInvocation};
use runx_runtime::adapters::process_tool::ProcessToolAdapter;
use runx_runtime::credentials::CredentialDelivery;
use serde::Deserialize;

#[derive(Deserialize)]
struct FixtureSuite {
    probe: String,
    skill_directory: String,
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    id: String,
    mode: String,
    cwd: Option<String>,
    input_mode: Option<runx_parser::InputMode>,
    large_input_bytes: Option<usize>,
    timeout_seconds: u64,
    inputs: JsonObject,
    expected: FixtureExpected,
}

#[derive(Deserialize)]
struct FixtureExpected {
    status: String,
    stdout_bytes: Option<usize>,
    stdout_json: Option<JsonValue>,
    stderr_contains: Option<String>,
    max_duration_ms: Option<u128>,
}

#[test]
fn rust_matches_skill_author_runtime_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    let fixture_root = repo_root().join("fixtures/skill-author-runtime");
    let suite = fixture_suite(&fixture_root)?;
    let probe_path = fixture_root.join(&suite.probe);
    let skill_directory = fixture_root.join(&suite.skill_directory);

    for fixture in suite.cases {
        let temp_dir = tempfile::tempdir()?;
        let started = Instant::now();
        let output = ProcessToolAdapter.invoke(SkillInvocation {
            historical_context: Vec::new(),
            skill_name: format!("skill-author-runtime.{}", fixture.id),
            step_id: None,
            artifacts: None,
            executable: Executable::Process(fixture_tool(&fixture, &probe_path)?),
            outputs: None,
            inputs: fixture_inputs(&fixture),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: skill_directory.clone(),
            env: fixture_env(&fixture_root, temp_dir.path())?,
            requirements: Default::default(),
            credential_delivery: CredentialDelivery::none(),
        })?;
        let duration_ms = started.elapsed().as_millis();

        assert_eq!(
            normalized_status(&output.status),
            fixture.expected.status,
            "{} status",
            fixture.id
        );
        if let Some(expected) = fixture.expected.stderr_contains.as_ref() {
            assert!(
                output
                    .process_stderr()
                    .is_some_and(|stderr| stderr.contains(expected)),
                "{} stderr should contain {expected:?}",
                fixture.id
            );
        } else {
            assert_eq!(output.process_stderr(), Some(""), "{} stderr", fixture.id);
        }
        if let Some(expected) = fixture.expected.stdout_json {
            assert_eq!(output.value, expected, "{} stdout_json", fixture.id);
        }
        if let Some(expected) = fixture.expected.stdout_bytes {
            assert_eq!(
                output.rendered_value().len(),
                expected,
                "{} stdout_bytes",
                fixture.id
            );
        }
        if let Some(max_duration_ms) = fixture.expected.max_duration_ms {
            assert!(
                duration_ms < max_duration_ms,
                "{} duration {duration_ms}ms exceeded {max_duration_ms}ms",
                fixture.id
            );
        }
    }

    Ok(())
}

fn fixture_suite(fixture_root: &Path) -> Result<FixtureSuite, Box<dyn std::error::Error>> {
    let suite = fs::read_to_string(fixture_root.join("cases.json"))?;
    Ok(serde_json::from_str(&suite)?)
}

fn fixture_tool(
    fixture: &FixtureCase,
    probe_path: &Path,
) -> Result<ProcessTool, Box<dyn std::error::Error>> {
    Ok(ProcessTool {
        command: "node".to_owned(),
        args: vec![path_string(probe_path)?, fixture.mode.clone()],
        cwd: fixture.cwd.clone(),
        timeout_seconds: Some(fixture.timeout_seconds),
        input_mode: fixture.input_mode,
        environment: Default::default(),
    })
}

fn fixture_inputs(fixture: &FixtureCase) -> JsonObject {
    let mut inputs = fixture.inputs.clone();
    if let Some(bytes) = fixture.large_input_bytes {
        inputs.insert("large".to_owned(), JsonValue::String("x".repeat(bytes)));
    }
    inputs
}

fn fixture_env(
    fixture_root: &Path,
    temp_dir: &Path,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut env = BTreeMap::new();
    if let Some(path) = std::env::var_os("PATH").and_then(|value| value.into_string().ok()) {
        env.insert("PATH".to_owned(), path);
    }
    env.insert(RUNX_CWD_ENV.to_owned(), path_string(fixture_root)?);
    env.insert("TMPDIR".to_owned(), path_string(temp_dir)?);
    Ok(env)
}

fn normalized_status(status: &InvocationStatus) -> &'static str {
    match status {
        InvocationStatus::Success => "sealed",
        InvocationStatus::Failure => "failure",
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn path_string(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(path
        .to_str()
        .ok_or_else(|| format!("path is not utf-8: {}", path.display()))?
        .to_owned())
}
