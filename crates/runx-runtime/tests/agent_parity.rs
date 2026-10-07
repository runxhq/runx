#![cfg(feature = "agent")]

use std::cell::RefCell;
use std::collections::BTreeMap;

use runx_contracts::{AgentActSourceType, JsonNumber, JsonObject, JsonValue, ResolutionRequest};
use runx_parser::SkillSource;
use runx_runtime::adapters::agent::{
    AgentAdapter, AgentExecutionTelemetry, AgentResolution, AgentResolver, AgentResolverError,
    AgentToolExecutionTrace,
};
use runx_runtime::{
    InvocationStatus, ManagedAgentConfig, ModelDialect, RuntimeError, RuntimeOptions, SecretString,
    SkillAdapter, SkillInvocation, run_harness_fixture_with_adapter,
};

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";

#[test]
fn agent_task_invocation_id_and_envelope_shape() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = RecordingResolver::success(
        JsonValue::Object(BTreeMap::from([(
            "result".to_owned(),
            JsonValue::String("done".to_owned()),
        )])),
        None,
    );
    let mut env = BTreeMap::new();
    env.insert(
        "RUNX_TOOL_ROOTS".to_owned(),
        "/tmp/runx-tools:/opt/runx-tools".to_owned(),
    );
    env.insert(
        runx_runtime::RUNX_RUN_ID_ENV.to_owned(),
        "run_agent-parity".to_owned(),
    );

    let output = AgentAdapter::agent_task(config(), &resolver).invoke(SkillInvocation {
        historical_context: Vec::new(),
        env,
        ..invocation(
            runx_parser::SourceKind::AgentStep,
            "fixture.step",
            source(
                runx_parser::SourceKind::AgentStep,
                Some("assistant"),
                Some("draft release notes"),
                Some(BTreeMap::from([(
                    "result".to_owned(),
                    JsonValue::String("string".to_owned()),
                )])),
            ),
            JsonObject::new(),
        )
    })?;

    assert_eq!(output.status, InvocationStatus::Success);
    let requests = resolver.requests.borrow();
    assert_eq!(requests.len(), 1);
    let ResolutionRequest::AgentAct { id, invocation } = &requests[0] else {
        return Err(std::io::Error::other("missing agent_act request").into());
    };
    assert_eq!(id, "agent_task.draft_release_notes.output");
    assert_eq!(invocation.id, "agent_task.draft_release_notes.output");
    assert_eq!(invocation.source_type, AgentActSourceType::AgentStep);
    assert_eq!(invocation.agent.as_deref(), Some("assistant"));
    assert_eq!(invocation.task.as_deref(), Some("draft release notes"));

    assert_eq!(invocation.envelope.run_id, "run_agent-parity");
    assert_eq!(invocation.envelope.skill, "fixture.step");
    assert!(!invocation.envelope.instructions.is_empty());
    assert!(invocation.envelope.allowed_tools.is_empty());
    assert!(invocation.envelope.current_context.is_empty());
    assert!(invocation.envelope.historical_context.is_empty());
    assert!(invocation.envelope.provenance.is_empty());
    assert!(
        invocation
            .envelope
            .trust_boundary
            .contains("caller-mediated resolution is the default")
    );
    let execution_location = invocation
        .envelope
        .execution_location
        .as_ref()
        .ok_or_else(|| std::io::Error::other("missing execution location"))?;
    assert!(
        execution_location
            .skill_directory
            .as_ref()
            .ends_with("fixtures/skills/agent-task")
    );
    let tool_roots = execution_location
        .tool_roots
        .as_ref()
        .ok_or_else(|| std::io::Error::other("missing tool roots"))?;
    assert_eq!(
        tool_roots.iter().map(AsRef::as_ref).collect::<Vec<&str>>(),
        vec!["/tmp/runx-tools", "/opt/runx-tools"]
    );

    let agent_hook = object_field(&output.metadata, "agent_hook")?;
    assert_eq!(agent_hook.get("source_type"), Some(&string("agent-task")));
    assert_eq!(agent_hook.get("agent"), Some(&string("assistant")));
    assert_eq!(agent_hook.get("task"), Some(&string("draft release notes")));
    assert_eq!(agent_hook.get("route"), Some(&string("native")));
    assert_eq!(agent_hook.get("provider"), Some(&string("openai")));
    assert_eq!(agent_hook.get("model"), Some(&string("gpt-test")));
    assert_eq!(agent_hook.get("status"), Some(&string("success")));
    Ok(())
}

#[test]
fn agent_task_injects_skill_instructions_and_resolved_profiles()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let skills_dir = temp.path().join("skills");
    let skill_dir = skills_dir.join("fixture");
    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: fixture-profiled-agent
description: Profile injection fixture.
---
# Fixture

Write one direct operator response.
"#,
    )?;
    std::fs::write(
        skills_dir.join("VOICE.md"),
        "# Voice\n\nUse direct, concrete language.\n",
    )?;
    let resolver = RecordingResolver::success(
        JsonValue::Object(BTreeMap::from([(
            "result".to_owned(),
            JsonValue::String("done".to_owned()),
        )])),
        None,
    );
    let mut request = invocation(
        runx_parser::SourceKind::AgentStep,
        "fixture.profiled",
        source(
            runx_parser::SourceKind::AgentStep,
            Some("assistant"),
            Some("profiled"),
            Some(BTreeMap::from([(
                "result".to_owned(),
                JsonValue::String("string".to_owned()),
            )])),
        ),
        JsonObject::new(),
    );
    request.skill_directory = skill_dir;
    request.env.insert(
        "RUNX_CWD".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );

    let output = AgentAdapter::agent_task(config(), &resolver).invoke(request)?;
    let requests = resolver.requests.borrow();
    let ResolutionRequest::AgentAct { invocation, .. } = &requests[0] else {
        return Err(std::io::Error::other("missing agent_act request").into());
    };
    assert!(
        invocation
            .envelope
            .instructions
            .contains("Write one direct operator response.")
    );
    let voice = invocation
        .envelope
        .voice_profile
        .as_ref()
        .ok_or_else(|| std::io::Error::other("missing voice profile"))?;
    assert_eq!(voice.path.as_ref(), "VOICE.md");
    assert!(voice.content.contains("direct, concrete language"));
    assert_eq!(
        output.metadata.get("voice_profile"),
        Some(&JsonValue::String(voice.sha256.as_ref().to_owned()))
    );
    Ok(())
}

#[test]
fn agent_declared_text_field_success() -> Result<(), Box<dyn std::error::Error>> {
    let telemetry = AgentExecutionTelemetry {
        rounds: Some(2),
        model_calls: Some(2),
        tool_calls: Some(1),
        tools: Some(vec!["fs.read".to_owned()]),
        tool_executions: Some(vec![AgentToolExecutionTrace {
            tool: "fs.read".to_owned(),
            status: "success".to_owned(),
            receipt_id: Some("rct_1".to_owned()),
            resolution_kind: None,
        }]),
    };
    let resolver = RecordingResolver::success(
        JsonValue::Object(BTreeMap::from([(
            "summary".to_owned(),
            JsonValue::String("plain final answer".to_owned()),
        )])),
        Some(telemetry),
    );

    let output = AgentAdapter::agent(config(), &resolver).invoke(invocation(
        runx_parser::SourceKind::Agent,
        "fixture.agent",
        source(
            runx_parser::SourceKind::Agent,
            Some("assistant"),
            Some("summarize"),
            Some(BTreeMap::from([(
                "summary".to_owned(),
                JsonValue::String("string".to_owned()),
            )])),
        ),
        JsonObject::new(),
    ))?;

    assert_eq!(output.status, InvocationStatus::Success);
    assert_eq!(
        output.value,
        JsonValue::Object(BTreeMap::from([(
            "summary".to_owned(),
            JsonValue::String("plain final answer".to_owned()),
        )]))
    );
    assert_eq!(output.exit_code(), None);
    let agent_runner = object_field(&output.metadata, "agent_runner")?;
    assert_eq!(agent_runner.get("skill"), Some(&string("fixture.agent")));
    assert_eq!(agent_runner.get("route"), Some(&string("native")));
    assert_eq!(agent_runner.get("provider"), Some(&string("openai")));
    assert_eq!(agent_runner.get("model"), Some(&string("gpt-test")));
    assert_eq!(agent_runner.get("status"), Some(&string("success")));
    assert_eq!(
        agent_runner.get("rounds"),
        Some(&JsonValue::Number(JsonNumber::U64(2)))
    );
    assert_eq!(
        agent_runner.get("tool_calls"),
        Some(&JsonValue::Number(JsonNumber::U64(1)))
    );
    assert_eq!(
        agent_runner.get("tools"),
        Some(&JsonValue::Array(vec![string("fs.read")]))
    );
    let tool_executions = array_field(agent_runner, "tool_executions")?;
    assert_eq!(tool_executions.len(), 1);
    Ok(())
}

#[test]
fn agent_task_structured_json_payload_success() -> Result<(), Box<dyn std::error::Error>> {
    let payload = JsonValue::Object(
        [
            ("title".to_owned(), JsonValue::String("Release".to_owned())),
            ("ready".to_owned(), JsonValue::Bool(true)),
        ]
        .into(),
    );
    let resolver = RecordingResolver::success(payload.clone(), None);
    let outputs = [
        ("title".to_owned(), JsonValue::String("string".to_owned())),
        ("ready".to_owned(), JsonValue::String("boolean".to_owned())),
    ]
    .into();

    let output = AgentAdapter::agent_task(config(), &resolver).invoke(invocation(
        runx_parser::SourceKind::AgentStep,
        "fixture.structured",
        source(
            runx_parser::SourceKind::AgentStep,
            Some("assistant"),
            Some("structured"),
            Some(outputs),
        ),
        JsonObject::new(),
    ))?;

    assert_eq!(output.status, InvocationStatus::Success);
    assert_eq!(output.value, payload);
    let requests = resolver.requests.borrow();
    let ResolutionRequest::AgentAct { invocation, .. } = &requests[0] else {
        return Err(std::io::Error::other("missing agent_act request").into());
    };
    assert_eq!(
        invocation
            .envelope
            .output
            .as_ref()
            .map(|output| output.len()),
        Some(2)
    );
    Ok(())
}

#[test]
fn provider_error_failure_sanitizes_stderr_and_metadata() -> Result<(), Box<dyn std::error::Error>>
{
    let resolver = RecordingResolver::failure("provider leaked sk-secret-value");

    let output = AgentAdapter::agent_task(config(), &resolver).invoke(invocation(
        runx_parser::SourceKind::AgentStep,
        "fixture.fail",
        source(
            runx_parser::SourceKind::AgentStep,
            Some("assistant"),
            Some("fail"),
            Some(BTreeMap::from([(
                "result".to_owned(),
                JsonValue::String("string".to_owned()),
            )])),
        ),
        JsonObject::new(),
    ))?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert_eq!(output.value, JsonValue::Null);
    assert_eq!(
        output.failure_message().as_deref(),
        Some("Managed agent provider request failed.")
    );
    assert_eq!(output.exit_code(), None);
    assert!(!format!("{output:?}").contains("sk-secret-value"));
    let agent_hook = object_field(&output.metadata, "agent_hook")?;
    assert_eq!(agent_hook.get("status"), Some(&string("failure")));
    assert_eq!(agent_hook.get("route"), Some(&string("native")));
    assert_eq!(agent_hook.get("provider"), Some(&string("openai")));
    assert_eq!(agent_hook.get("model"), Some(&string("gpt-test")));
    Ok(())
}

#[test]
fn bounded_failure_projects_reason_and_telemetry_without_raw_content()
-> Result<(), Box<dyn std::error::Error>> {
    let resolver = RecordingResolver::bounded_failure(
        "round_budget_exhausted",
        "Managed agent exceeded 3 tool-call rounds without finalizing.",
        AgentExecutionTelemetry {
            rounds: Some(3),
            model_calls: Some(3),
            tool_calls: Some(3),
            tools: Some(vec!["fs.read".to_owned()]),
            tool_executions: Some(vec![AgentToolExecutionTrace {
                tool: "fs.read".to_owned(),
                status: "success".to_owned(),
                receipt_id: None,
                resolution_kind: None,
            }]),
        },
    );

    let output = AgentAdapter::agent(config(), &resolver).invoke(invocation(
        runx_parser::SourceKind::Agent,
        "fixture.budget-failure",
        source(
            runx_parser::SourceKind::Agent,
            Some("assistant"),
            Some("private task"),
            Some(BTreeMap::from([(
                "result".to_owned(),
                JsonValue::String("string".to_owned()),
            )])),
        ),
        JsonObject::new(),
    ))?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert_eq!(
        output.failure_message().as_deref(),
        Some("Managed agent exceeded 3 tool-call rounds without finalizing.")
    );
    let agent_runner = object_field(&output.metadata, "agent_runner")?;
    assert_eq!(
        agent_runner.get("reason_code"),
        Some(&string("round_budget_exhausted"))
    );
    assert_eq!(
        agent_runner.get("rounds"),
        Some(&JsonValue::Number(JsonNumber::U64(3)))
    );
    assert_eq!(
        agent_runner.get("model_calls"),
        Some(&JsonValue::Number(JsonNumber::U64(3)))
    );
    assert_eq!(
        agent_runner.get("tool_calls"),
        Some(&JsonValue::Number(JsonNumber::U64(3)))
    );
    assert!(!format!("{output:?}").contains("private task"));
    Ok(())
}

#[test]
fn unsupported_source_type_returns_runtime_error() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = RecordingResolver::success(JsonValue::String("unused".to_owned()), None);
    let error = AgentAdapter::agent(config(), &resolver).invoke(invocation(
        runx_parser::SourceKind::AgentStep,
        "fixture.unsupported",
        source(
            runx_parser::SourceKind::AgentStep,
            Some("assistant"),
            Some("task"),
            None,
        ),
        JsonObject::new(),
    ));

    match error {
        Err(RuntimeError::UnsupportedAdapter { adapter_type }) => {
            assert_eq!(adapter_type, "agent-task");
            Ok(())
        }
        Ok(_) => Err(std::io::Error::other("adapter unexpectedly succeeded").into()),
        Err(other) => Err(std::io::Error::other(format!("unexpected error: {other}")).into()),
    }
}

#[test]
fn harness_replay_runs_agent_skill_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = RecordingResolver::success(JsonValue::String("agent replayed".to_owned()), None);
    let temp = tempfile::tempdir()?;
    let skill_dir = temp.path().join("skill");
    crate::support::write_test_skill_package(
        &skill_dir,
        r#"---
name: fixture-agent
description: Fixture agent skill.
---
Summarize the topic.
"#,
        r#"skill: fixture-agent
runners:
  fixture-agent:
    default: true
    type: agent
    agent: assistant
    task: summarize
    outputs:
      summary: string
    inputs:
      topic:
        type: string
        required: true
"#,
    )?;
    let fixture_path = temp.path().join("harness.yaml");
    // Agent runners replay from the caller's recorded answer, keyed by the
    // stable package runner id rather than from a live adapter resolver.
    std::fs::write(
        &fixture_path,
        r#"
name: fixture-agent
kind: agent
target: skill
inputs:
  topic: harness replay
caller:
  answers:
    agent.fixture-agent.output:
      summary: agent replayed
expect:
  status: sealed
"#,
    )?;

    let replay = run_harness_fixture_with_adapter(
        &fixture_path,
        AgentAdapter::agent(config(), &resolver),
        fixture_runtime_options(),
    )?;

    assert_eq!(replay.status, runx_runtime::HarnessExpectedStatus::Sealed);
    let output = replay
        .skill_output
        .ok_or_else(|| std::io::Error::other("missing replay skill output"))?;
    assert_eq!(output.status, InvocationStatus::Success);
    assert!(output.rendered_value().contains("agent replayed"));
    Ok(())
}

fn fixture_runtime_options() -> RuntimeOptions {
    RuntimeOptions {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        created_at: FIXTURE_CREATED_AT.to_owned(),
        ..RuntimeOptions::local_development(std::env::vars().collect())
    }
}

struct RecordingResolver {
    requests: RefCell<Vec<ResolutionRequest>>,
    result: Result<AgentResolution, AgentResolverError>,
}

impl RecordingResolver {
    fn success(payload: JsonValue, telemetry: Option<AgentExecutionTelemetry>) -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            result: Ok(AgentResolution::agent(payload, telemetry)),
        }
    }

    fn failure(message: &str) -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            result: Err(AgentResolverError::provider_error(message)),
        }
    }

    fn bounded_failure(
        reason_code: &str,
        message: &str,
        telemetry: AgentExecutionTelemetry,
    ) -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            result: Err(AgentResolverError::bounded_failure(
                reason_code,
                message,
                telemetry,
            )),
        }
    }
}

impl AgentResolver for &RecordingResolver {
    fn resolve(&self, request: ResolutionRequest) -> Result<AgentResolution, AgentResolverError> {
        self.requests.borrow_mut().push(request);
        self.result.clone()
    }
}

fn invocation(
    source_type: runx_parser::SourceKind,
    skill_name: &str,
    source: SkillSource,
    inputs: JsonObject,
) -> SkillInvocation {
    let skill_directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures/skills/agent-task");
    let mut request = SkillInvocation {
        historical_context: Vec::new(),
        skill_name: skill_name.to_owned(),
        step_id: None,
        requirements: Default::default(),
        artifacts: None,
        allowed_tools: None,
        source,
        inputs,
        resolved_inputs: JsonObject::new(),
        current_context: Vec::new(),
        provenance: Vec::new(),
        skill_directory,
        env: BTreeMap::new(),
        credential_delivery: runx_runtime::CredentialDelivery::none(),
    };
    request.source.source_type = source_type;
    request
}

fn source(
    source_type: runx_parser::SourceKind,
    agent: Option<&str>,
    task: Option<&str>,
    outputs: Option<JsonObject>,
) -> SkillSource {
    SkillSource {
        act: None,
        source_type,
        command: None,
        module: None,
        javascript_export: None,
        pages: None,
        args: Vec::new(),
        cwd: None,
        timeout_seconds: None,
        input_mode: None,
        environment: Default::default(),
        agent: agent.map(str::to_owned),
        task: task.map(str::to_owned),
        outputs,
        graph: None,
        raw: JsonObject::new(),
    }
}

fn config() -> ManagedAgentConfig {
    ManagedAgentConfig {
        provider: "openai".into(),
        dialect: ModelDialect::ChatCompletions,
        model: "gpt-test".to_owned(),
        api_key: Some(SecretString::new("sk-test")),
        endpoint_url: "https://api.openai.com/v1/chat/completions".to_owned(),
        auth_mode: runx_runtime::ManagedAgentAuthMode::ApiKey,
    }
}

fn object<'a>(value: &'a JsonValue, label: &str) -> Result<&'a JsonObject, std::io::Error> {
    let JsonValue::Object(object) = value else {
        return Err(std::io::Error::other(format!("{label} must be an object")));
    };
    Ok(object)
}

fn object_field<'a>(object: &'a JsonObject, key: &str) -> Result<&'a JsonObject, std::io::Error> {
    let Some(value) = object.get(key) else {
        return Err(std::io::Error::other(format!("{key} is missing")));
    };
    self::object(value, key)
}

fn array_field<'a>(
    object: &'a JsonObject,
    key: &str,
) -> Result<&'a Vec<JsonValue>, std::io::Error> {
    let Some(JsonValue::Array(value)) = object.get(key) else {
        return Err(std::io::Error::other(format!("{key} is missing")));
    };
    Ok(value)
}

fn string(value: &str) -> JsonValue {
    JsonValue::String(value.to_owned())
}
