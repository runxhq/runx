#![cfg(feature = "agent")]

use std::cell::Cell;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
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

struct ReadExecutor(Cell<u32>);

impl ToolExecutor for ReadExecutor {
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
        })
    }
}

fn read_request(stream: &mut TcpStream) -> io::Result<Value> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let (header_end, content_length) = loop {
        let n = stream.read(&mut chunk)?;
        assert!(n > 0, "request ended before headers");
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
            assert!(headers.starts_with("post /v1/chat/completions http/1.1"));
            assert!(
                !headers.contains("authorization:"),
                "keyless local call must carry no API key"
            );
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "content length"))?;
            break (index + 4, length);
        }
    };
    while bytes.len() - header_end < content_length {
        let n = stream.read(&mut chunk)?;
        assert!(n > 0, "request ended before body");
        bytes.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&bytes[header_end..header_end + content_length])
        .map_err(io::Error::other)
}

fn serve_responses<const N: usize>(
    listener: TcpListener,
    responses: [Value; N],
) -> io::Result<Vec<Value>> {
    let mut requests = Vec::new();
    for response in responses {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        requests.push(read_request(&mut stream)?);
        let body = response.to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
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
                json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call-1","type":"function","function":{"name":"fixture_read","arguments":"{\"value\":\"proof\"}"}}]}}]}),
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
    let model =
        ChatCompletionsCaller::new(transport, endpoint, None, "fixture-model".to_owned(), tools);
    let mut executor = ReadExecutor(Cell::new(0));
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
        let _request = read_request(&mut stream)?;
        let body = json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[
            {"id":"first","type":"function","function":{"name":"fixture_read","arguments":"{\"value\":\"proof\"}"}},
            {"id":"second","type":"function","function":{"name":"unoffered_tool","arguments":"{}"}}
        ]}}]}).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
        Ok(())
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
        tools,
    );
    let mut executor = ReadExecutor(Cell::new(0));
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
        "---\nname: fixture-agent\ndescription: fixture-agent fixture.\n---\n# Fixture\n\nCall data.digest with value proof and encoding utf8_text, then finish with status done.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "skill: fixture-agent\ntools: [data.digest]\nrunners:\n  proof:\n    default: true\n    steps:\n      - id: verify\n        agent: { role: fixture, task: proof }\n        outputs:\n          status: string\n",
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
        ("RUNX_AGENT_PROVIDER".to_owned(), "openai".to_owned()),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill(&SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: ManagedAgentPolicy::inline(3)?,
    })?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    assert_eq!(output["result"]["status"], "done", "{output}");
    let receipt_id = output["receipt_id"].as_str().ok_or("missing receipt_id")?;
    let receipts = temp.path().join("receipts");
    let graph = crate::support::read_test_signed_receipt(&receipts, receipt_id)?;
    let step = crate::support::read_test_first_step_receipt(&receipts, &graph)?;
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

#[test]
fn a_managed_turn_that_fails_still_names_the_calls_it_made()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: fixture-agent\ndescription: fixture-agent fixture.\n---\n# Fixture\n\nCall data.digest, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "skill: fixture-agent\ntools: [data.digest]\nrunners:\n  proof:\n    default: true\n    steps:\n      - id: verify\n        agent: { role: fixture, task: proof }\n        outputs:\n          status: string\n",
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
        ("RUNX_AGENT_PROVIDER".to_owned(), "openai".to_owned()),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill(&SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: ManagedAgentPolicy::inline(3)?,
    })?;
    finish_server(server)?;
    let output = serde_json::to_value(&result.output)?;
    assert_eq!(output["closure"]["disposition"], "failed", "{output}");
    let receipt_id = output["receipt_id"].as_str().ok_or("missing receipt_id")?;
    let receipts = temp.path().join("receipts");
    let graph = crate::support::read_test_signed_receipt(&receipts, receipt_id)?;
    let step = crate::support::read_test_first_step_receipt(&receipts, &graph)?;
    assert_eq!(
        step.lineage.children.len(),
        1,
        "the failed turn's receipt names the call it made"
    );
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
        "---\nname: child\ndescription: Reports a status.\n---\n# Child\n\nFinish with a status.\n",
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        "skill: child\nrunners:\n  report:\n    default: true\n    outputs:\n      status: string\n",
    )?;
    let skill_dir = temp.path().join("parent");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: parent\ndescription: Asks its child.\n---\n# Parent\n\nRun the child, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "skill: parent\nskills: [../child]\nrunners:\n  proof:\n    default: true\n    outputs:\n      status: string\n",
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
        ("RUNX_AGENT_PROVIDER".to_owned(), "openai".to_owned()),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill(&SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: ManagedAgentPolicy::inline(3)?,
    })?;
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
        "---\nname: child\ndescription: Reports a status.\n---\n# Child\n\nAsk, then finish with a status.\n",
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        "skill: child\nasks: true\nrunners:\n  report:\n    default: true\n    outputs:\n      status: string\n",
    )?;
    let skill_dir = temp.path().join("parent");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: parent\ndescription: Asks its child.\n---\n# Parent\n\nRun the child, then finish.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "skill: parent\nskills: [../child]\nrunners:\n  proof:\n    default: true\n    outputs:\n      status: string\n",
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
        ("RUNX_AGENT_PROVIDER".to_owned(), "openai".to_owned()),
        ("RUNX_AGENT_MODEL".to_owned(), "fixture-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    let receipts = temp.path().join("receipts");
    let paused = LocalOrchestrator::default().run_skill(&SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipts.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: env.clone(),
        cwd: temp.path().to_path_buf(),
        managed_agent: ManagedAgentPolicy::inline(3)?,
    })?;
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
    let answers = temp.path().join("answers.json");
    fs::write(&answers, json!({"approvals": {ask: true}}).to_string())?;
    let sealed = LocalOrchestrator::default().resume(&runx_runtime::ResumeRequest {
        run_id,
        answers_path: answers,
        receipt_dir: Some(receipts),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: ManagedAgentPolicy::inline(3)?,
        package_digest: None,
        execution_closure_digest: None,
    })?;
    let output = serde_json::to_value(&sealed.output)?;
    assert_eq!(output["status"], "sealed", "{output}");
    assert_eq!(output["result"]["status"], "done", "{output}");
    assert_eq!(finish_server(server)?.len(), 4);
    Ok(())
}
