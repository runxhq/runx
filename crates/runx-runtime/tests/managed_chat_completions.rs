#![cfg(all(feature = "agent", feature = "scripted-model"))]

use std::cell::Cell;
use std::io;
use std::net::TcpListener;
use std::{collections::BTreeMap, fs};

use runx_contracts::{JsonObject, JsonValue};
use runx_runtime::adapters::agent_chat_completions::ChatCompletionsCaller;
use runx_runtime::adapters::agent_loop::{
    AgentLoopConfig, ToolExecutor, ToolOutcome, run_agent_loop,
};
use runx_runtime::adapters::agent_tool_definitions::{AgentToolDefinition, AgentToolNameMap};
use runx_runtime::{
    LocalOrchestrator, ManagedAgentPolicy, ReqwestHttpTransport, RuntimeError, SkillRunRequest,
};
use serde_json::{Value, json};
use tempfile::tempdir;

/// Reads the fixture: the calls it made, and the tools it offers.
struct ReadExecutor(Cell<u32>, AgentToolNameMap);

impl ToolExecutor for ReadExecutor {
    fn tools(&self) -> &AgentToolNameMap {
        &self.1
    }

    fn admitted_tool_name(&self, tool: &str) -> Option<String> {
        (tool == "fixture.read").then(|| tool.to_owned())
    }

    fn execute(&mut self, tool: &str, input: &JsonValue) -> Result<ToolOutcome, RuntimeError> {
        assert_eq!(tool, "fixture.read");
        assert_eq!(
            input
                .as_object()
                .and_then(|object| object.get("value"))
                .and_then(JsonValue::as_str),
            Some("proof")
        );
        self.0.set(self.0.get() + 1);
        Ok(ToolOutcome::Result {
            content: serde_json::to_string(&JsonValue::Object(JsonObject::from([(
                "proof".to_owned(),
                JsonValue::String("verified".to_owned()),
            )])))
            .unwrap_or_default(),
            is_error: false,
            guidance: None,
        })
    }
}

fn serve_responses<const N: usize>(
    listener: TcpListener,
    responses: [Value; N],
) -> io::Result<Vec<Value>> {
    let mut requests = Vec::new();
    for response in responses {
        // The client sends its whole request or closes, so the read ends.
        let (mut stream, _) = listener.accept()?;
        requests.push(runx_runtime::scripted_model::read_chat_request(
            &mut stream,
        )?);
        runx_runtime::scripted_model::write_chat_response(&mut stream, &response)?;
    }
    Ok(requests)
}

fn finish_server(
    server: std::thread::JoinHandle<io::Result<Vec<Value>>>,
) -> io::Result<Vec<Value>> {
    server
        .join()
        .map_err(|_| io::Error::other("model server thread failed"))?
}

#[test]
fn exact_local_transport_replays_tool_result_before_schema_valid_final_result()
-> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-1","type":"function","function":{"name":"fixture_read","arguments":"{\"value\":\"proof\"}"},"extra_content":{"google":{"thought_signature":"sig-1"}}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"final-1","type":"function","function":{"name":"runx_final_result","arguments":"{\"status\":\"done\"}"}}]}}]}),
            ],
        )
    });

    let endpoint = format!("http://{address}/v1/chat/completions");
    let transport = ReqwestHttpTransport::for_exact_loopback_agent(&endpoint)?;
    let tools = AgentToolNameMap::new(vec![
        AgentToolDefinition {
            name: "fixture.read".to_owned(),
            description: "Read a harmless fixture.".to_owned(),
            input_schema: serde_json::from_value(
                json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}),
            )?,
        },
        AgentToolDefinition {
            name: "runx_final_result".to_owned(),
            description: "Submit the final result.".to_owned(),
            input_schema: serde_json::from_value(
                json!({"type":"object","properties":{"status":{"type":"string"}},"required":["status"]}),
            )?,
        },
    ])?;
    let model = ChatCompletionsCaller::new(transport, endpoint, None, "fixture-model".to_owned());
    let mut executor = ReadExecutor(Cell::new(0), tools);
    let config = AgentLoopConfig {
        max_rounds: 3,
        max_empty_turn_resamples: 0,
        final_result_tool: "runx_final_result".to_owned(),
        final_result_output: None,
        final_result_schema: Some(serde_json::from_value(
            json!({"type":"object","properties":{"status":{"type":"string","enum":["done"]}},"required":["status"],"additionalProperties":false}),
        )?),
    };
    let result = run_agent_loop(
        &config,
        &model,
        &mut executor,
        "Read the fixture then finish.".to_owned(),
    )?;
    assert_eq!(executor.0.get(), 1);
    assert_eq!(
        result
            .answer
            .as_ref()
            .and_then(JsonValue::as_object)
            .and_then(|object| object.get("status"))
            .and_then(JsonValue::as_str),
        Some("done")
    );
    let requests = finish_server(server)?;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["tools"][0]["function"]["name"], "fixture_read");
    assert_eq!(requests[1]["messages"][1]["tool_calls"][0]["id"], "call-1");
    // Gemini refuses a replayed call without the signature it returned.
    assert_eq!(
        requests[1]["messages"][1]["tool_calls"][0]["extra_content"],
        json!({"google":{"thought_signature":"sig-1"}})
    );
    assert_eq!(requests[1]["messages"][2]["tool_call_id"], "call-1");
    assert!(
        requests[1]["messages"][2]["content"]
            .as_str()
            .is_some_and(|content| content.contains("verified"))
    );
    Ok(())
}

#[test]
fn openai_batch_with_late_unknown_tool_executes_nothing() -> Result<(), Box<dyn std::error::Error>>
{
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || -> io::Result<()> {
        let (mut stream, _) = listener.accept()?;
        let _request = runx_runtime::scripted_model::read_chat_request(&mut stream)?;
        let body = json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[
            {"id":"first","type":"function","function":{"name":"fixture_read","arguments":"{\"value\":\"proof\"}"}},
            {"id":"second","type":"function","function":{"name":"unoffered_tool","arguments":"{}"}}
        ]}}]});
        runx_runtime::scripted_model::write_chat_response(&mut stream, &body)
    });
    let tools = AgentToolNameMap::new(vec![AgentToolDefinition {
        name: "fixture.read".to_owned(),
        description: "Read a harmless fixture.".to_owned(),
        input_schema: serde_json::from_value(json!({"type":"object"}))?,
    }])?;
    let model = ChatCompletionsCaller::new(
        ReqwestHttpTransport::for_exact_loopback_agent(&endpoint)?,
        endpoint,
        None,
        "fixture-model".to_owned(),
    );
    let mut executor = ReadExecutor(Cell::new(0), tools);
    let Err(error) = run_agent_loop(
        &AgentLoopConfig {
            max_rounds: 2,
            max_empty_turn_resamples: 0,
            final_result_tool: "runx_final_result".to_owned(),
            final_result_output: None,
            final_result_schema: None,
        },
        &model,
        &mut executor,
        "go".to_owned(),
    ) else {
        return Err("unknown tool should reject the entire batch".into());
    };
    assert_eq!(executor.0.get(), 0);
    assert_eq!(error.telemetry().tool_calls, Some(1));
    server.join().map_err(|_| "server thread failed")??;
    Ok(())
}

#[test]
fn managed_agent_step_executes_governed_tool_round_trip() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let skill_dir = temp.path().join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: fixture-agent\ndescription: fixture-agent fixture.\n---\n# Fixture\n\n## Agent task: proof\n\nCall data.digest with value proof and encoding utf8_text, then finish with status done.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  proof:\n    steps:\n      - id: verify\n        agent: proof\n        calls:\n          tools: [data.digest]\n        outputs:\n          status: {type: string}\n",
    )?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-digest","type":"function","function":{"name":"data_digest","arguments":"{\"value\":\"proof\",\"encoding\":\"utf8_text\"}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"done\"}"}}]}}]}),
            ],
        )
    });

    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(3)?,
        },
        None,
        None,
        None,
    )?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    assert_eq!(output["result"]["status"], "done", "{output}");
    let receipt_id = output["receipt_id"].as_str().ok_or("missing receipt_id")?;
    let receipts = temp.path().join("receipts");
    let workflow = crate::support::read_test_signed_receipt(&receipts, receipt_id)?;
    let step = crate::support::read_test_first_step_receipt(&receipts, &workflow)?;
    assert_eq!(
        step.lineage.children.len(),
        1,
        "the managed agent's call is recorded and sealed under its turn"
    );
    let requests = finish_server(server)?;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["tools"][0]["function"]["name"], "data_digest");
    assert!(
        requests[1]["messages"]
            .as_array()
            .is_some_and(
                |messages| messages.iter().any(|message| message["role"] == "tool"
                    && message["tool_call_id"] == "call-digest"
                    && message["content"]
                        .as_str()
                        .is_some_and(|content| content.contains(
                            "c1cda26362828b69266512052b97cb3729e3b052e4ade47c0a1e3383defe73c7"
                        )))
            ),
        "the step must replay the actual native digest result"
    );
    Ok(())
}

/// A one-turn skill whose turn requires `requirement` of its agent harness,
/// worked by the managed agent at `endpoint`.
fn run_requiring(
    root: &std::path::Path,
    requirement: &str,
    endpoint: String,
) -> Result<Value, Box<dyn std::error::Error>> {
    let skill_dir = root.join(requirement);
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: judge\ndescription: judge fixture.\n---\n# Judge\n\n## Agent task: judge\n\nFinish with status done.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        format!(
            "minimum_runx: 0.12.0\nmethods:\n  judge:\n    steps:\n      - id: judge\n        agent: judge\n        requires: [{requirement}]\n        outputs:\n          status: {{type: string}}\n"
        ),
    )?;
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            root.join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(3)?,
        },
        None,
        None,
        None,
    )?;
    Ok(serde_json::to_value(&result.output)?)
}

/// The managed agent starts each turn from its act alone, so it meets a
/// fresh context and says so, and it has no tools of its own and no second
/// reviewer, so a turn requiring either ends failed, naming the requirement,
/// before any model is called.
#[test]
fn the_managed_agent_meets_a_fresh_context_and_refuses_what_it_cannot()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    // Nothing listens here, so a model call would fail at once, not hang.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        format!("http://{}/v1/chat/completions", listener.local_addr()?)
    };
    for requirement in ["native_tools", "separate_reviewer"] {
        let output = run_requiring(temp.path(), requirement, closed.clone())?;
        assert_eq!(output["outcome"], "failed", "{output}");
        assert!(
            output
                .to_string()
                .contains(&format!("unsupported_requirement {requirement}")),
            "{output}"
        );
    }

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"done\"}"}}]}}]}),
            ],
        )
    });
    let output = run_requiring(temp.path(), "fresh_context", endpoint)?;
    assert_eq!(output["status"], "sealed", "{output}");
    assert_eq!(output["outcome"], "completed", "{output}");
    assert_eq!(output["result"]["status"], "done", "{output}");
    assert_eq!(finish_server(server)?.len(), 1);
    Ok(())
}

#[test]
fn the_managed_agent_calls_the_method_it_loads_and_reads_its_manual_as_guidance()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let child_dir = temp.path().join("skills").join("child");
    fs::create_dir_all(&child_dir)?;
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: child\ndescription: Digests values.\n---\n# Child\n\nDigest the exact value the person gave.\n",
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\ndefault_method: digest\nmethods:\n  digest:\n    description: Digest one value.\n    inputs:\n      value: {type: string}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n  other:\n    description: Another method, never bound here.\n    inputs:\n      value: {type: string}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n",
    )?;
    let skill_dir = temp.path().join("session");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: session\ndescription: Works with what it loads.\n---\n# Session\n\n## Agent task: work\n\nLoad the child, digest proof, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  work:\n    steps:\n      - id: work\n        agent: work\n        calls:\n          load: [{project: true}]\n        inputs: $input\n        outputs:\n          status: {type: string}\n",
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-load","type":"function","function":{"name":"load","arguments":"{\"skill\":\"skills/child\",\"method\":\"digest\"}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-digest","type":"function","function":{"name":"skill__child__digest","arguments":"{\"value\":\"proof\"}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"done\"}"}}]}}]}),
            ],
        )
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(4)?,
        },
        None,
        None,
        None,
    )?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    let requests = finish_server(server)?;
    assert_eq!(requests.len(), 3);
    let tools = |request: &Value| -> Vec<String> {
        request["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(!tools(&requests[0]).contains(&"skill__child__digest".to_owned()));
    assert!(
        tools(&requests[1]).contains(&"skill__child__digest".to_owned()),
        "the method the load bound is offered on the next round"
    );
    assert!(!tools(&requests[1]).contains(&"skill__child__other".to_owned()));
    let messages = requests[1]["messages"]
        .as_array()
        .ok_or("the second request carries the transcript")?;
    let (index, load_result) = messages
        .iter()
        .enumerate()
        .find(|(_, message)| message["tool_call_id"] == "call-load")
        .ok_or("the load's result is replayed")?;
    let load_result = load_result["content"].as_str().unwrap_or_default();
    assert!(
        !load_result.contains("Digest the exact value"),
        "the manual is never a result's data: {load_result}"
    );
    assert!(load_result.contains("Digest one value."), "{load_result}");
    let guidance = &messages[index + 1];
    assert_eq!(guidance["role"], "user");
    assert!(
        guidance["content"]
            .as_str()
            .is_some_and(|text| text.contains("operating guidance")
                && text.contains("Digest the exact value the person gave.")),
        "{guidance}"
    );
    Ok(())
}

/// A run's record reads back what its turn did: the package a load bound, by
/// its digest, and the model the turn's answer stated. A reader learns both
/// from the record, never from what the turn claims.
#[test]
fn a_run_reads_back_the_package_a_load_bound_and_the_model_its_turn_stated()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let child_dir = temp.path().join("skills").join("child");
    fs::create_dir_all(&child_dir)?;
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: child\ndescription: Digests values.\n---\n# Child\n\nDigest the exact value the person gave.\n",
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  digest:\n    description: Digest one value.\n    inputs:\n      value: {type: string}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n",
    )?;
    let child_digest = runx_runtime::load_validated_skill_package(&child_dir)?
        .package
        .package_digest
        .clone();
    let skill_dir = temp.path().join("session");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: session\ndescription: Works with what it loads.\n---\n# Session\n\n## Agent task: work\n\nLoad the child, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  work:\n    result_from: [record]\n    steps:\n      - id: work\n        agent: work\n        calls:\n          load: [{project: true}]\n        inputs: $input\n        outputs:\n          status: {type: string}\n      - id: record\n        tool: run.read\n",
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-load","type":"function","function":{"name":"load","arguments":"{\"skill\":\"skills/child\"}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"done\"}"}}]}}]}),
            ],
        )
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env: env.clone(),
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(3)?,
        },
        None,
        None,
        None,
    )?;
    finish_server(server)?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    let run_id = output["run_id"].as_str().ok_or("the run has an id")?;
    let read = serde_json::to_value(runx_runtime::run_record::read_run(
        &env,
        temp.path(),
        run_id,
        None,
    )?)?;
    let events = read["events"].as_array().ok_or("the read has events")?;
    let load = events
        .iter()
        .map(|record| &record["event"])
        .find(|event| event["kind"] == "call" && event["target"]["kind"] == "load")
        .ok_or_else(|| format!("the load is recorded: {read}"))?;
    assert_eq!(load["loaded"]["skill"], "skills/child", "{load}");
    assert_eq!(
        load["loaded"]["package_digest"],
        child_digest.as_str(),
        "{load}"
    );
    let step = events
        .iter()
        .map(|record| &record["event"])
        .find(|event| event["kind"] == "step" && event["step_id"] == "work")
        .ok_or_else(|| format!("the turn's step is recorded: {read}"))?;
    assert_eq!(step["model"], "fixture-model", "{step}");
    // The run reads its own record through `run.read`, as the CLI's history
    // does, and finds the same load.
    let own = &output["result"]["run_read"]["data"];
    let own_load = own["events"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|record| &record["event"])
        .find(|event| event["kind"] == "call" && event["target"]["kind"] == "load")
        .ok_or_else(|| format!("the run reads its load back: {output}"))?;
    assert_eq!(own_load["loaded"], load["loaded"], "{own_load}");
    assert_eq!(own["run"]["run_id"], run_id, "{own}");
    Ok(())
}

#[test]
fn a_managed_agent_reads_the_memory_its_run_is_given_by_locator()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: fixture-agent\ndescription: fixture-agent fixture.\n---\n# Fixture\n\n## Agent task: proof\n\nRead what you remember, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  proof:\n    steps:\n      - id: proof\n        agent: proof\n        inputs: $input\n        outputs:\n          status: {type: string}\n",
    )?;
    // The workspace's own MEMORY.md is never sent; the memory the host
    // supplies is.
    fs::write(
        temp.path().join("MEMORY.md"),
        "Ambient memory runx never reads.\n",
    )?;
    // Kept on this machine: the managed agent's model is served from a
    // loopback endpoint, so it may read it.
    let memory: runx_contracts::ContextEntry = serde_json::from_value(json!({
        "schema": "runx.context_entry.v1",
        "type": "fieldwork.memory",
        "summary": "Kam's launch date",
        "data": {"quote": "We launch on the third of November."},
        "labels": {
            "purpose": "confirmed_memory",
            "trust": "person_confirmed",
            "sharing": "local_only",
        },
        "sources": ["conversation:2026-10-20"],
        "origin": {
            "artifact_id": "memory:1",
            "run_id": "rx_host",
            "step_id": null,
            "producer": {"skill": "fieldwork", "method": "memory"},
            "created_at": "2026-10-20T00:00:00Z",
            "digest": "sha256:memory",
            "size_bytes": 48,
        },
    }))?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-read","type":"function","function":{"name":"turn_read","arguments":"{\"locator\":\"current:0\"}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"done\"}"}}]}}]}),
            ],
        )
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: vec![memory],
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(3)?,
        },
        None,
        None,
        None,
    )?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    let requests = finish_server(server)?;
    let sent = serde_json::to_string(&requests)?;
    assert!(!sent.contains("Ambient memory runx never reads."), "{sent}");
    let prompt = requests[0]["messages"][0]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(
        prompt.contains("current:0") && prompt.contains("Kam's launch date"),
        "the memory is a line of the act's index: {prompt}"
    );
    assert!(
        !prompt.contains("third of November"),
        "and its data is read, never pasted: {prompt}"
    );
    let read = requests[1]["messages"]
        .as_array()
        .and_then(|messages| {
            messages
                .iter()
                .find(|message| message["tool_call_id"] == "call-read")
        })
        .ok_or("the read's result is replayed")?;
    assert!(
        read["content"]
            .as_str()
            .is_some_and(|content| content.contains("third of November")),
        "{read}"
    );
    Ok(())
}

#[test]
fn a_managed_agent_at_a_loopback_endpoint_is_given_a_local_only_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: fixture-agent\ndescription: fixture-agent fixture.\n---\n# Fixture\n\n## Agent task: proof\n\nRead the artifact, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  proof:\n    inputs:\n      source: {type: object}\n    steps:\n      - id: proof\n        agent: proof\n        inputs: $input\n        outputs:\n          status: {type: string}\n",
    )?;
    // Kept on this machine: the model is served from a loopback endpoint, so
    // the turn may be given it, as it may a local_only context entry.
    let digest = format!("sha256:{}", "d".repeat(64));
    let artifact: JsonValue = serde_json::from_value(json!({
        "schema": "runx.artifact.v1",
        "ref": format!("runx:project-artifact:{digest}"),
        "digest": digest,
        "size_bytes": 24,
        "media_type": "application/json",
        "summary": "Notes kept on this machine",
        "labels": {"sharing": "local_only"},
    }))?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"done\"}"}}]}}]}),
            ],
        )
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::from([("source".to_owned(), artifact)]),
            env,
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(3)?,
        },
        None,
        None,
        None,
    )?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    let sent = serde_json::to_string(&finish_server(server)?)?;
    assert!(sent.contains("Notes kept on this machine"), "{sent}");
    Ok(())
}

#[test]
fn a_managed_agent_at_a_loopback_endpoint_reads_a_local_only_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    fs::write(temp.path().join("kept.txt"), "kept on this machine")?;
    let skill_dir = temp.path().join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: fixture-agent\ndescription: fixture-agent fixture.\n---\n# Fixture\n\n## Agent task: proof\n\nRead the admitted notes, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  proof:\n    steps:\n      - id: kept\n        tool: artifact.admit\n        inputs: {path: kept.txt, media_type: text/plain, labels: {sharing: local_only}}\n      - id: proof\n        agent: proof\n        calls:\n          tools: [artifact.read]\n        inputs:\n          kept: $steps.kept.artifact.ref\n        outputs:\n          status: {type: string}\n",
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    // The model reads the artifact's reference from its act, reads the
    // artifact by it, then finishes.
    let server = std::thread::spawn(move || -> io::Result<Vec<Value>> {
        let tool_call = |id: &str, name: &str, arguments: Value| json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}}]});
        let mut requests = Vec::new();
        for turn in 0..3 {
            let (mut stream, _) = listener.accept()?;
            let request = runx_runtime::scripted_model::read_chat_request(&mut stream)?;
            let response = match turn {
                0 => tool_call("call-input", "turn_read", json!({"locator": "input:kept"})),
                1 => {
                    let sent = request.to_string();
                    let start = sent
                        .find("runx:local-artifact:sha256:")
                        .ok_or_else(|| io::Error::other("the act names no artifact"))?;
                    let reference = &sent[start..start + "runx:local-artifact:sha256:".len() + 64];
                    tool_call(
                        "call-read",
                        "artifact_read",
                        json!({"ref": reference, "encoding": "utf8"}),
                    )
                }
                _ => tool_call("call-final", "turn_complete", json!({"status": "done"})),
            };
            requests.push(request);
            runx_runtime::scripted_model::write_chat_response(&mut stream, &response)?;
        }
        Ok(requests)
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(4)?,
        },
        None,
        None,
        None,
    )?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    let sent = serde_json::to_string(&finish_server(server)?)?;
    assert!(sent.contains("kept on this machine"), "{sent}");
    Ok(())
}

#[test]
fn a_managed_agent_at_a_loopback_endpoint_is_given_a_local_only_page_a_tool_step_read()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    fs::write(temp.path().join("kept.txt"), "kept on this machine")?;
    let skill_dir = temp.path().join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: fixture-agent\ndescription: fixture-agent fixture.\n---\n# Fixture\n\n## Agent task: proof\n\nRead the page, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  proof:\n    steps:\n      - id: kept\n        tool: artifact.admit\n        inputs: {path: kept.txt, media_type: text/plain, labels: {sharing: local_only}}\n      - id: read\n        tool: artifact.read\n        inputs: {ref: $steps.kept.artifact.ref, encoding: utf8}\n      - id: proof\n        agent: proof\n        inputs:\n          page: $steps.read.artifact_page\n        outputs:\n          status: {type: string}\n",
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-page","type":"function","function":{"name":"turn_read","arguments":"{\"locator\":\"input:page\"}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"done\"}"}}]}}]}),
            ],
        )
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(3)?,
        },
        None,
        None,
        None,
    )?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    let sent = serde_json::to_string(&finish_server(server)?)?;
    assert!(sent.contains("kept on this machine"), "{sent}");
    Ok(())
}

#[test]
fn what_a_loopback_turn_stores_of_a_local_only_page_is_local_only_and_refused_to_a_remote_turn()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    fs::write(temp.path().join("kept.txt"), "kept on this machine")?;
    let skill_dir = temp.path().join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: fixture-agent\ndescription: fixture-agent fixture.\n---\n# Fixture\n\n## Agent task: proof\n\nKeep the notes, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  proof:\n    steps:\n      - id: kept\n        tool: artifact.admit\n        inputs: {path: kept.txt, media_type: text/plain, labels: {sharing: local_only}}\n      - id: read\n        tool: artifact.read\n        inputs: {ref: $steps.kept.artifact.ref, encoding: utf8}\n      - id: proof\n        agent: proof\n        calls:\n          tools: [artifact.store]\n        inputs:\n          page: $steps.read.artifact_page\n        outputs:\n          status: {type: string}\n",
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    // The model reads the page, stores its text, stores it again saying it
    // may go to any configured model, then finishes.
    let tool_call = |id: &str, name: &str, arguments: Value| json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}}]});
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                tool_call("call-page", "turn_read", json!({"locator": "input:page"})),
                tool_call(
                    "call-store",
                    "artifact_store",
                    json!({"value": {"notes": "kept on this machine"}}),
                ),
                tool_call(
                    "call-widen",
                    "artifact_store",
                    json!({"value": {"notes": "kept on this machine"}, "labels": {"sharing": "configured_model"}}),
                ),
                tool_call("call-final", "turn_complete", json!({"status": "done"})),
            ],
        )
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let request = |skill_path: std::path::PathBuf,
                   inputs: BTreeMap<String, JsonValue>,
                   env,
                   managed_agent| {
        SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs,
            env,
            cwd: temp.path().to_path_buf(),
            managed_agent,
        }
    };
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &request(
            skill_dir,
            BTreeMap::new(),
            env.clone(),
            ManagedAgentPolicy::inline(5)?,
        ),
        None,
        None,
        None,
    )?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    let sent = finish_server(server)?;
    // The call's result as the model read it, in the request after it.
    let result = |request: &Value| -> Result<Value, Box<dyn std::error::Error>> {
        let content = request["messages"]
            .as_array()
            .and_then(|messages| messages.last())
            .and_then(|message| message["content"].as_str())
            .ok_or("the request carries no call result")?;
        Ok(serde_json::from_str(content)?)
    };
    let stored = result(&sent[2])?;
    let artifact = stored["outcome"]["output"]["artifact"]["data"].clone();
    assert_eq!(artifact["labels"]["sharing"], "local_only", "{stored}");
    let widened = result(&sent[3])?;
    assert_ne!(widened["outcome"]["status"], "completed", "{widened}");
    assert!(
        widened
            .to_string()
            .contains("labels.sharing configured_model would widen local_only"),
        "{widened}"
    );

    // A later run's turn, on a host that can't say its model runs on this
    // machine, bound to the artifact the first stored.
    let later = temp.path().join("later");
    fs::create_dir_all(&later)?;
    fs::write(
        later.join("SKILL.md"),
        "---\nname: later\ndescription: Answers from kept notes.\n---\n# Later\n\n## Agent task: answer\n\nAnswer from the notes.\n",
    )?;
    fs::write(
        later.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  answer:\n    inputs:\n      notes: {type: object}\n    steps:\n      - id: answer\n        agent: answer\n        inputs:\n          notes: $input.notes\n        outputs:\n          answer: {type: string}\n",
    )?;
    let mut remote = BTreeMap::from([(
        "RUNX_HOME".to_owned(),
        temp.path().join("home").to_string_lossy().into_owned(),
    )]);
    crate::support::insert_test_signing_env(&mut remote);
    let refused = LocalOrchestrator::default().run_skill_with_binding(
        &request(
            later,
            BTreeMap::from([("notes".to_owned(), serde_json::from_value(artifact)?)]),
            remote,
            ManagedAgentPolicy::default(),
        ),
        None,
        None,
        None,
    )?;
    let rendered = serde_json::to_string(&refused.output)?;
    assert!(!rendered.contains("needs_agent"), "{rendered}");
    assert!(
        rendered.contains(
            "what it is given is labelled local_only, and its host can't say its model runs on this machine"
        ),
        "{rendered}"
    );
    Ok(())
}

#[test]
fn a_managed_turn_that_fails_still_names_the_calls_it_made()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: fixture-agent\ndescription: fixture-agent fixture.\n---\n# Fixture\n\n## Agent task: proof\n\nCall data.digest, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  proof:\n    steps:\n      - id: verify\n        agent: proof\n        calls:\n          tools: [data.digest]\n        outputs:\n          status: {type: string}\n",
    )?;
    // The model digests once, then stops calling tools; the provider is gone
    // before the loop's resamples run out.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-digest","type":"function","function":{"name":"data_digest","arguments":"{\"value\":\"proof\",\"encoding\":\"utf8_text\"}"}}]}}]}),
                json!({"choices":[{"finish_reason":"stop","message":{"content":"done"}}]}),
            ],
        )
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(3)?,
        },
        None,
        None,
        None,
    )?;
    finish_server(server)?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["outcome"], "failed", "{output}");
    let receipt_id = output["receipt_id"].as_str().ok_or("missing receipt_id")?;
    let receipts = temp.path().join("receipts");
    let workflow = crate::support::read_test_signed_receipt(&receipts, receipt_id)?;
    let step = crate::support::read_test_first_step_receipt(&receipts, &workflow)?;
    assert_eq!(
        step.lineage.children.len(),
        1,
        "the failed turn's receipt names the call it made"
    );
    Ok(())
}

#[test]
fn a_child_whose_managed_agent_fails_stops_its_parent_for_the_same_reason()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let child_dir = temp.path().join("agent-child");
    fs::create_dir_all(&child_dir)?;
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: agent-child\ndescription: agent-child fixture.\n---\n# Fixture\n\n## Agent task: proof\n\nCall data.digest, then finish.\n",
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  proof:\n    steps:\n      - id: verify\n        agent: proof\n        calls:\n          tools: [data.digest]\n        outputs:\n          status: {type: string}\n",
    )?;
    let parent_dir = temp.path().join("parent");
    fs::create_dir_all(&parent_dir)?;
    fs::write(
        parent_dir.join("SKILL.md"),
        "---\nname: parent\ndescription: parent fixture.\n---\n# Parent\n\nRun the child.\n",
    )?;
    fs::write(
        parent_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  default:\n    steps:\n      - id: invoke\n        skill: ../agent-child\n",
    )?;
    // The model stops without finalizing, so the child's managed agent fails.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [json!({"choices":[{"finish_reason":"stop","message":{"content":"done"}}]})],
        )
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: parent_dir,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(3)?,
        },
        None,
        None,
        None,
    )?;
    finish_server(server)?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["outcome"], "failed", "{output}");
    // The parent fails for the reason its child did, which its child's
    // receipt names too.
    let receipts = temp.path().join("receipts");
    let workflow = crate::support::read_test_signed_receipt(
        &receipts,
        output["receipt_id"].as_str().ok_or("missing receipt_id")?,
    )?;
    let invoke = crate::support::read_test_first_step_receipt(&receipts, &workflow)?;
    let child = crate::support::read_test_first_step_receipt(&receipts, &invoke)?;
    let reason = child.seal.reason.ok_or("the child names why it failed")?;
    assert_ne!(reason, runx_contracts::SealReason::WorkflowStepFailed);
    assert_eq!(invoke.seal.reason, Some(reason));
    assert_eq!(workflow.seal.reason, Some(reason));
    assert_eq!(output["seal_reason"], reason.label(), "{output}");
    Ok(())
}

#[test]
fn the_managed_agent_works_the_turn_its_own_call_opens_in_a_child_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let child_dir = temp.path().join("child");
    fs::create_dir_all(&child_dir)?;
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: child\ndescription: Reports a status.\n---\n# Child\n\n## Agent task: report\n\nFinish with a status.\n",
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  report:\n    steps:\n      - id: report\n        agent: report\n        inputs: $input\n        outputs:\n          status: {type: string}\n",
    )?;
    let skill_dir = temp.path().join("parent");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: parent\ndescription: Asks its child.\n---\n# Parent\n\n## Agent task: proof\n\nRun the child, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  proof:\n    steps:\n      - id: proof\n        agent: proof\n        calls:\n          skills: [../child]\n        inputs: $input\n        outputs:\n          status: {type: string}\n",
    )?;
    // The parent's turn calls the child; the child's turn, opened by that
    // call, completes; then the parent's turn completes.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-child","type":"function","function":{"name":"skill__child","arguments":"{}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"child-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"child done\"}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"done\"}"}}]}}]}),
            ],
        )
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(3)?,
        },
        None,
        None,
        None,
    )?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    assert_eq!(output["result"]["status"], "done", "{output}");
    let requests = finish_server(server)?;
    let tools = |request: &Value| -> Vec<String> {
        request["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(tools(&requests[0]).contains(&"skill__child".to_owned()));
    assert!(
        !tools(&requests[1]).contains(&"skill__child".to_owned()),
        "the second request works the child's own turn"
    );
    Ok(())
}

#[test]
fn a_managed_run_paused_inside_a_child_turn_goes_on_once_the_human_answers()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let child_dir = temp.path().join("child");
    fs::create_dir_all(&child_dir)?;
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: child\ndescription: Reports a status.\n---\n# Child\n\n## Agent task: report\n\nAsk, then finish with a status.\n",
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  report:\n    steps:\n      - id: report\n        agent: report\n        calls:\n          ask: true\n        inputs: $input\n        outputs:\n          status: {type: string}\n",
    )?;
    let skill_dir = temp.path().join("parent");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: parent\ndescription: Asks its child.\n---\n# Parent\n\n## Agent task: proof\n\nRun the child, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  proof:\n    steps:\n      - id: proof\n        agent: proof\n        calls:\n          skills: [../child]\n        inputs: $input\n        outputs:\n          status: {type: string}\n",
    )?;
    // First pass: the parent calls the child, whose turn asks the human. The
    // second pass, once the human answers, ends the child's turn, then the
    // parent's.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let server = std::thread::spawn(move || {
        serve_responses(
            listener,
            [
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-child","type":"function","function":{"name":"skill__child","arguments":"{}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"child-ask","type":"function","function":{"name":"ask_human","arguments":"{\"reason\":\"Go on?\"}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"child-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"child done\"}"}}]}}]}),
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"done\"}"}}]}}]}),
            ],
        )
    });
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let receipts = temp.path().join("receipts");
    let paused = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(receipts.clone()),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env: env.clone(),
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(3)?,
        },
        None,
        None,
        None,
    )?;
    let ids = |result: &runx_runtime::RunResult| -> Vec<String> {
        result
            .pending_requests
            .iter()
            .filter_map(|request| {
                request
                    .as_object()
                    .and_then(|request| request.get("id"))
                    .and_then(JsonValue::as_str)
                    .map(str::to_owned)
            })
            .collect()
    };
    let ask = "proof/0#report/0";
    assert_eq!(
        ids(&paused),
        ["proof", "proof/0#report", ask],
        "the paused run lists the child's ask beside both turns"
    );
    let run_id = serde_json::to_value(&paused.output)?["run_id"]
        .as_str()
        .ok_or("missing run_id")?
        .to_owned();
    let answers = crate::support::bind_resume(
        &env,
        temp.path(),
        &run_id,
        json!({"approvals": {ask: true}}),
    )?;
    let sealed = LocalOrchestrator::default().resume(&runx_runtime::ResumeRequest {
        run_id,
        answers,
        receipt_dir: Some(receipts),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: ManagedAgentPolicy::inline(3)?,
        package_digest: None,
        closure_digest: None,
    })?;
    let output = serde_json::to_value(&sealed.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    assert_eq!(output["result"]["status"], "done", "{output}");
    assert_eq!(finish_server(server)?.len(), 4);
    Ok(())
}

/// A turn runx's managed agent worked from a fresh context, stating the
/// model that answered, leaves its run an eligible source: a later call that
/// asks the same finds it and, until its reads are authorized for this run,
/// recomputes as not allowed rather than ineligible (reuse.md, D2).
#[test]
fn a_turn_runxs_managed_agent_worked_leaves_an_eligible_source()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let child = temp.path().join("answer-child");
    fs::create_dir_all(&child)?;
    fs::write(
        child.join("SKILL.md"),
        "---\nname: answer-child\ndescription: Answers once.\n---\n# Answer child\n\n## Agent task: answer\n\nFinish with status done.\n",
    )?;
    fs::write(
        child.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  answer:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: answer\n        agent: answer\n        inputs: {value: $input.value}\n        outputs:\n          status: {type: string}\n",
    )?;
    let parent = temp.path().join("asker");
    fs::create_dir_all(&parent)?;
    fs::write(
        parent.join("SKILL.md"),
        "---\nname: asker\ndescription: Asks its child once.\n---\n# Asker\n\nAsk.\n",
    )?;
    fs::write(
        parent.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  ask:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: ask\n        script: {module: ask.mjs}\n        inputs: {value: $input.value}\n        calls:\n          skills: [../answer-child]\n        max_calls: 1\n        outputs:\n          done: {type: boolean}\n",
    )?;
    fs::write(
        parent.join("ask.mjs"),
        "export default async function (ctx, { value }) {\n  await ctx.skill(\"../answer-child\", { value }, { reuse: { max_age_seconds: 604800 } });\n  return { done: true };\n}\n",
    )?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let done = json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-final","type":"function","function":{"name":"turn_complete","arguments":"{\"status\":\"done\"}"}}]}}]});
    let server = std::thread::spawn(move || serve_responses(listener, [done.clone(), done]));
    let mut env = BTreeMap::from([
        (
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        ),
        (
            "RUNX_CWD".to_owned(),
            temp.path().to_string_lossy().into_owned(),
        ),
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let mut decisions = Vec::new();
    for _ in 0..2 {
        let result = LocalOrchestrator::default().run_skill_with_binding(
            &SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: None,
                act: None,
                skill_path: parent.clone(),
                receipt_dir: Some(temp.path().join("receipts")),
                run_id: None,
                answers: None,
                inputs: BTreeMap::from([("value".to_owned(), JsonValue::String("x".to_owned()))]),
                env: env.clone(),
                cwd: temp.path().to_path_buf(),
                managed_agent: ManagedAgentPolicy::inline(3)?,
            },
            None,
            None,
            None,
        )?;
        let output = serde_json::to_value(&result.output)?;
        assert_eq!(output["status"], "sealed", "{output}");
        let run_id = output["run_id"].as_str().ok_or("missing run_id")?;
        let read = runx_runtime::run_record::read_run(&env, temp.path(), run_id, None)?;
        decisions.extend(
            read.events
                .into_iter()
                .filter_map(|record| match record.event {
                    runx_contracts::RunEventFact::Call {
                        reuse: Some(reuse), ..
                    } if record.stream.as_str() == run_id
                        || runx_runtime::run_record::is_calls_stream_of(
                            record.stream.as_str(),
                            run_id,
                        ) =>
                    {
                        Some(reuse.decision)
                    }
                    _ => None,
                }),
        );
    }
    finish_server(server)?;
    let recomputed = |reason| runx_contracts::ReuseDecision::Recomputed { reason };
    assert_eq!(
        decisions,
        [
            recomputed(runx_contracts::RecomputeReason::NoCandidate),
            recomputed(runx_contracts::RecomputeReason::NotAllowed),
        ]
    );
    Ok(())
}
