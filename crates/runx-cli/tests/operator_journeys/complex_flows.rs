use super::*;
use std::io::Read;
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

const THREAD: &str = "slack://T123ABC/C123ABC/1750000000.000001";
const OTHER_THREAD: &str = "slack://T123ABC/C123ABC/1750000000.000009";
const REPLY: &str = "I\u{2019}ll verify this now.";
const REPLY_DIGEST: &str =
    "sha256:8604f5c08db8d76b6fa8066c769fbdc6a4992101373d0d17bcfe426cf8e77647";

#[derive(Debug)]
struct ProviderRequestTrace {
    operation: String,
    provider_target: String,
    idempotency_key: Option<String>,
}

type ConnectFixture = (
    String,
    thread::JoinHandle<Result<Vec<ProviderRequestTrace>, String>>,
);

/// A runx Connect stand-in for the Slack grant. It answers the operator's
/// session and the first `answers` of `operations` provider operations; an
/// operation past `answers` is read and its connection closed unanswered, as
/// a transport failure after the provider may have acted.
fn spawn_slack_connect(operations: usize, answers: usize) -> TestResult<ConnectFixture> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || -> Result<Vec<ProviderRequestTrace>, String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut traces = Vec::new();
        while traces.len() < operations {
            if Instant::now() >= deadline {
                return Err(format!(
                    "Connect fixture timed out after {} operations",
                    traces.len()
                ));
            }
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => return Err(error.to_string()),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            let request = read_http_request(&mut stream)?;
            let request_line = request.lines().next().unwrap_or_default();
            if request_line.starts_with("GET /v1/me HTTP/1.1") {
                write_http_json(
                    &mut stream,
                    &serde_json::json!({
                        "status": "success",
                        "principal": {"principal_id": "operator:test", "role": "user"}
                    }),
                )?;
                continue;
            }
            if !request_line.starts_with("POST /v1/provider-operations HTTP/1.1") {
                return Err(format!("unexpected Connect request: {request_line}"));
            }
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .ok_or_else(|| "Connect request omitted its body".to_owned())?;
            let value: Value = serde_json::from_str(body)
                .map_err(|error| format!("invalid Connect request JSON: {error}"))?;
            let trace = ProviderRequestTrace {
                operation: value["operation"].as_str().unwrap_or_default().to_owned(),
                provider_target: value["target"].as_str().unwrap_or_default().to_owned(),
                idempotency_key: value["input"]["idempotency_key"]
                    .as_str()
                    .map(str::to_owned),
            };
            if traces.len() >= answers {
                traces.push(trace);
                continue;
            }
            let mutation = trace.operation == "thread.reply";
            let mut result = serde_json::json!({
                "provider": "slack",
                "external_tenant_ref": "slack:T123ABC",
                "connected_subject_ref": "slack:U123ABC",
                "conversation_locator": "slack://T123ABC/C123ABC",
                "thread_locator": trace.provider_target.clone(),
                "message_locator": "slack://T123ABC/C123ABC/1750000000.000002",
                "occurred_at": "2026-08-11T00:00:00Z",
                "content_digest": REPLY_DIGEST
            });
            let mut response = serde_json::json!({
                "status": "success",
                "provider": "slack",
                "operation": trace.operation.clone(),
                "target": trace.provider_target.clone(),
                "access": if mutation { "mutate" } else { "read" },
                "readback_ref": format!("runx:provider_readback:slack-reply:{}", traces.len() + 1)
            });
            if mutation {
                let key = trace
                    .idempotency_key
                    .clone()
                    .ok_or_else(|| "the reply omitted its idempotency key".to_owned())?;
                result["idempotency_key"] = Value::String(key.clone());
                response["operation_id"] = Value::String("slack-message-001".to_owned());
                response["idempotency_key"] = Value::String(key);
            }
            response["result"] = result;
            write_http_json(&mut stream, &response)?;
            traces.push(trace);
        }
        Ok(traces)
    });
    Ok((format!("http://{address}"), server))
}

fn read_http_request(stream: &mut std::net::TcpStream) -> Result<String, String> {
    // Accepted sockets inherit the listener's nonblocking flag on macOS.
    // Restore blocking reads so the configured timeout waits for request bytes.
    stream
        .set_nonblocking(false)
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected = None;
    loop {
        let length = stream
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if length == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..length]);
        if expected.is_none()
            && let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
        {
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            });
            expected = Some(header_end + 4 + content_length.unwrap_or(0));
        }
        if expected.is_some_and(|expected| bytes.len() >= expected) {
            break;
        }
    }
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

fn write_http_json(stream: &mut std::net::TcpStream, body: &Value) -> Result<(), String> {
    let body = body.to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .map_err(|error| error.to_string())
}

fn provider_command(root: &Path, base_url: &str) -> Command {
    let mut command = command(root);
    command
        .env("RUNX_PROVIDER_PERMISSION_GRANT_ID", "grant-slack-demo")
        .env(
            "RUNX_PROVIDER_PERMISSION_GRANTED_SCOPES",
            r#"["thread.reply","thread.reply.read"]"#,
        )
        .env(
            "RUNX_PROVIDER_PERMISSION_PRINCIPAL_REF",
            "runx:principal:operator:test",
        )
        .env("RUNX_PUBLIC_API_BASE_URL", base_url)
        .env("RUNX_PUBLIC_API_TOKEN", "rxk_operator_journey")
        .env("RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK", "1");
    command
}

fn reply_inputs(thread_locator: &str, idempotency_key: &str) -> Value {
    serde_json::json!({
        "send_plan": {
            "decision": "ready",
            "action_family": "send-as",
            "principal": {"type": "user", "ref": "user:kam"},
            "provider": {"name": "slack", "account_ref": "workspace:T123ABC", "runtime_path": "slack"},
            "send_class": "internal",
            "channel": "chat",
            "audience": {"type": "thread", "ref": thread_locator, "requires_reconfirmation": false},
            "content": {"draft_ref": "inline", "digest": REPLY_DIGEST, "subject_or_title": "Slack thread reply"},
            "gates": {"preflight_required": true, "human_approval_required": true, "approval_ref": "slack.deliver-reply.approval"},
            "blockers": [],
            "provider_actions": ["thread.reply"],
            "evidence_refs": [],
            "success_checkpoint": {
                "milestone": "plan_ready",
                "description": "Digest-bound Slack reply awaits approved provider execution."
            }
        },
        "thread_locator": thread_locator,
        "text": REPLY,
        "idempotency_key": idempotency_key
    })
}

fn deliver_reply(
    root: &Path,
    base_url: &str,
    receipt_dir: &Path,
    inputs: &str,
) -> TestResult<Output> {
    let repo_root = crate::support::repo_root()?;
    Ok(provider_command(root, base_url)
        .arg("skill")
        .arg(repo_root.join("skills/slack"))
        .arg("deliver_reply")
        .arg("--inputs")
        .arg(inputs)
        .arg("--receipt-dir")
        .arg(receipt_dir)
        .arg("--json")
        .output()?)
}

/// Start `deliver_reply` from `inputs` and expect its one exact approval.
fn reply_pause(
    root: &Path,
    base_url: &str,
    receipt_dir: &Path,
    inputs: &str,
) -> TestResult<(String, String)> {
    let pause = assert_json(&deliver_reply(root, base_url, receipt_dir, inputs)?, 2)?;
    assert_eq!(pause["status"], "needs_approval", "{pause}");
    let requests = pause["requests"]
        .as_array()
        .ok_or("the reply pause listed no requests")?;
    assert_eq!(requests.len(), 1, "one exact approval gates the reply");
    let approval_id = requests[0]["id"]
        .as_str()
        .ok_or("the reply pause omitted its approval")?
        .to_owned();
    Ok((json_string(&pause, "run_id")?.to_owned(), approval_id))
}

fn approve(
    root: &Path,
    base_url: &str,
    receipt_dir: &Path,
    run_id: &str,
    approval_id: &str,
) -> TestResult<Output> {
    let approval = serde_json::json!({
        "approvals": {
            (approval_id): {
                "approved": true,
                "reason": "Post this exact reply to this exact thread once."
            }
        }
    })
    .to_string();
    let mut resume = provider_command(root, base_url);
    resume
        .arg("resume")
        .arg(run_id)
        .arg("-")
        .arg("--receipt-dir")
        .arg(receipt_dir)
        .arg("--json");
    output_with_stdin(&mut resume, &approval)
}

#[test]
fn slack_reply_journey_gates_once_and_closes_on_provider_readback() -> TestResult {
    let root = crate::support::temp_root("runx-slack-reply-journey");
    let receipt_dir = root.join(".runx/receipts");
    fs::create_dir_all(&root)?;
    fs::write(
        root.join("reply.json"),
        reply_inputs(THREAD, "00000000-0000-4000-8000-000000000001").to_string(),
    )?;
    let (base_url, server) = spawn_slack_connect(2, 2)?;

    let (run_id, approval_id) = reply_pause(&root, &base_url, &receipt_dir, "reply.json")?;
    let sent = assert_json(
        &approve(&root, &base_url, &receipt_dir, &run_id, &approval_id)?,
        0,
    )?;
    let server = server
        .join()
        .map_err(|_| "Connect fixture panicked")?
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;

    assert_eq!(sent["status"], "sealed", "{sent}");
    assert_eq!(sent["outcome"], "completed");
    assert_eq!(sent["run_id"], run_id.as_str());
    let readback = &sent["result"]["provider_operation"]["data"];
    assert_eq!(readback["operation"], "thread.reply.read", "{sent}");
    assert_eq!(readback["result"]["thread_locator"], THREAD);
    assert_eq!(readback["result"]["content_digest"], REPLY_DIGEST);
    assert_eq!(server.len(), 2);
    assert_eq!(server[0].operation, "thread.reply");
    assert_eq!(server[1].operation, "thread.reply.read");
    assert_eq!(server[0].provider_target, THREAD);
    assert_eq!(server[1].provider_target, THREAD);
    assert!(server[0].idempotency_key.is_some());
    assert!(server[1].idempotency_key.is_none(), "a read carries no key");
    assert!(sent.get("execution").is_none());
    assert!(
        sent.to_string().len() < 16_384,
        "default output is not compact"
    );
    let receipt_id = json_string(&sent, "receipt_id")?;
    assert_receipt_verifies(&root, &receipt_dir, receipt_id)?;
    let history =
        super::assert_history_contains_local_receipt(&root, &receipt_dir, &run_id, receipt_id)?;
    assert!(!super::run_is_paused(&history, &run_id));
    Ok(())
}

#[test]
fn ambiguous_reply_holds_for_reconciliation_and_another_thread_needs_its_own_approval() -> TestResult
{
    let root = crate::support::temp_root("runx-slack-reply-recovery-journey");
    let receipt_dir = root.join(".runx/receipts");
    fs::create_dir_all(&root)?;
    let key = "00000000-0000-4000-8000-000000000003";
    fs::write(
        root.join("reply.json"),
        reply_inputs(THREAD, key).to_string(),
    )?;
    fs::write(
        root.join("other-reply.json"),
        reply_inputs(OTHER_THREAD, key).to_string(),
    )?;

    // The reply reaches Connect and the connection drops before any answer.
    let (ambiguous_url, ambiguous_server) = spawn_slack_connect(1, 0)?;
    let (run_id, approval_id) = reply_pause(&root, &ambiguous_url, &receipt_dir, "reply.json")?;
    let ambiguous = assert_json(
        &approve(&root, &ambiguous_url, &receipt_dir, &run_id, &approval_id)?,
        1,
    )?;
    assert_eq!(ambiguous["status"], "failure", "{ambiguous}");
    assert!(
        ambiguous["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("outcome is unknown")),
        "{ambiguous}"
    );
    let ambiguous_trace = ambiguous_server
        .join()
        .map_err(|_| "ambiguous Connect fixture panicked")?
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
    assert_eq!(ambiguous_trace.len(), 1);
    assert_eq!(ambiguous_trace[0].operation, "thread.reply");

    // A reply to another thread is a different consequential act: it needs
    // its own exact approval, never the first one's.
    // Each run asks at its own reply step, and a resume's approval answers
    // only its own run's waiting request.
    let (other_run, other_approval) =
        reply_pause(&root, &ambiguous_url, &receipt_dir, "other-reply.json")?;
    assert_ne!(other_run, run_id);
    assert_eq!(other_approval, approval_id);

    // Retrying the first reply finds its unsettled attempt. The reply may have
    // posted and Slack does not deduplicate it, so the run holds for
    // reconciliation: it neither posts again nor asks for a new approval.
    let retried = assert_json(
        &deliver_reply(&root, &ambiguous_url, &receipt_dir, "reply.json")?,
        1,
    )?;
    assert_eq!(retried["status"], "failure", "{retried}");
    assert!(
        retried["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("reconcile")),
        "{retried}"
    );
    Ok(())
}

#[test]
fn business_ops_durable_route_replays_once_and_returns_reusable_state() -> TestResult {
    let root = crate::support::temp_root("runx-business-ops-durable-journey");
    let repo_root = crate::support::repo_root()?;
    let receipt_dir = root.join(".runx/receipts");
    fs::create_dir_all(&root)?;

    let run = || -> TestResult<Value> {
        let output = command(&root)
            .arg("skill")
            .arg(repo_root.join("skills/business-ops"))
            .arg("route_and_append")
            .args([
                "--input",
                "data_source_ref=tenant://journey/business-ops",
                "--input",
                "resource=ops_routes",
                "--input",
                "aggregate_id=launch-readiness",
                "--input",
                "expected_version=0",
                "--input",
                "idempotency_key=launch-readiness:classify:v1",
                "--input",
                "signal=Launch readiness needs incident, publication, and payment coordination.",
                "--input",
                "operator_context=Reuse this route; consequential effects remain in their owning skills.",
            ])
            .arg("--receipt-dir")
            .arg(&receipt_dir)
            .arg("--json")
            .output()?;
        assert_json(&output, 0)
    };

    let committed = run()?;
    assert_eq!(committed["status"], "sealed");
    assert_eq!(
        committed["result"]["route_persistence"]["data"]["append_status"],
        "committed"
    );
    assert_eq!(
        committed["result"]["route_persistence"]["data"]["projection"]["event_count"],
        1
    );
    assert_eq!(
        committed["result"]["lane_packet"]["data"]["lane"],
        "classify"
    );

    let replayed = run()?;
    assert_eq!(replayed["status"], "sealed");
    assert_eq!(
        replayed["result"]["route_persistence"]["data"]["append_status"],
        "idempotent_replay"
    );
    assert_eq!(
        replayed["result"]["route_persistence"]["data"]["projection"]["event_count"], 1,
        "replaying the same route must not append another event"
    );
    assert_eq!(
        replayed["result"]["route_persistence"]["data"]["projection"]["version"],
        1
    );

    for result in [&committed, &replayed] {
        let receipt_id = json_string(result, "receipt_id")?;
        assert_receipt_verifies(&root, &receipt_dir, receipt_id)?;
    }
    Ok(())
}

#[test]
fn incident_turn_reuses_case_approval_across_agent_resume_without_claiming_delivery() -> TestResult
{
    let root = crate::support::temp_root("runx-incident-resume-journey");
    let repo_root = crate::support::repo_root()?;
    let receipt_dir = root.join(".runx/receipts");
    let inputs = root.join("incident-inputs.json");
    fs::create_dir_all(&root)?;
    fs::write(
        &inputs,
        serde_json::json!({
            "case_id": "inc-sev2-checkout",
            "driver_id": "driver-primary",
            "incident_objective": "send",
            "case_state": {
                "declared": true,
                "severity": "SEV-2",
                "scope": "checkout-api",
                "turn": 4,
                "pending_escalation": {
                    "status": "awaiting_approval",
                    "lane": "human:incident-reviewer",
                    "proposed_handoff": {
                        "skill": "send-as",
                        "runner": "plan",
                        "principal": "incident:comms:morgan",
                        "channel": "email",
                        "audience": {
                            "list_ref": "stakeholders:checkout-api",
                            "classification": "incident-stakeholders"
                        },
                        "content_digest": "sha256:4e44f31f628799267f0957c554b8c47d90d2eabf41e5a84248941d28c45955ae"
                    }
                }
            },
            "roster": [
                {"role": "commander", "principal": "incident:commander:alex", "skill": "ops-desk", "scope": ["incident.command"]},
                {"role": "responder_lead", "principal": "incident:responder:rio", "skill": "incident-response", "scope": ["incident.respond"]},
                {"role": "comms_lead", "principal": "incident:comms:morgan", "skill": "send-as", "scope": ["stakeholder.send"]}
            ],
            "approval": {
                "principal": "incident:comms:morgan",
                "reason": "Approve this exact audience and content digest for send planning."
            }
        })
        .to_string(),
    )?;

    let pause = command(&root)
        .arg("skill")
        .arg(repo_root.join("skills/incident-commander"))
        .arg("advance")
        .arg("--inputs")
        .arg("incident-inputs.json")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .args(["--json"])
        .output()?;
    let pause = assert_json(&pause, 2)?;
    assert_eq!(pause["status"], "needs_agent");
    let run_id = json_string(&pause, "run_id")?;
    let request_id = pause["requests"]
        .as_array()
        .and_then(|requests| requests.first())
        .and_then(|request| request["id"].as_str())
        .ok_or("incident pause omitted its agent request")?;

    let answer = serde_json::json!({
        "answers": {
            (request_id): {
                "decision": "dispatch",
                "reason": "The existing comms-lead approval permits a separate send-as planning run.",
                "dispatch": {
                    "member": "comms_lead",
                    "skill": "send-as",
                    "task": "Plan the approved stakeholder update from the bound audience and content digest.",
                    "needed_scope": ["stakeholder.send"],
                    "consequence": "stakeholder_send_handoff",
                    "verification": {
                        "expected_receipt": "runx.receipt.v1",
                        "readback": "Return provider-backed send evidence before claiming delivery."
                    }
                }
            }
        }
    })
    .to_string();
    let mut resume = command(&root);
    resume
        .arg("resume")
        .arg(run_id)
        .arg("-")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json");
    let resumed = output_with_stdin(&mut resume, &answer)?;
    let resumed = assert_json(&resumed, 0)?;
    assert_eq!(resumed["status"], "sealed");
    assert_eq!(resumed["run_id"], run_id);
    let turn = &resumed["result"]["incident_turn"]["data"];
    assert_eq!(turn["decision"], "advanced");
    assert_eq!(turn["downstream_handoff"]["skill"], "send-as");
    assert_eq!(turn["downstream_handoff"]["state"], "ready_for_planning");
    assert_eq!(turn["delivery_status"], "not_sent");
    assert_eq!(turn["effect_state"]["provider_delivery"], "not_executed");
    assert_eq!(turn["validation"]["status"], "pass");
    let receipt_id = json_string(&resumed, "receipt_id")?;
    assert_receipt_verifies(&root, &receipt_dir, receipt_id)?;
    let history =
        super::assert_history_contains_local_receipt(&root, &receipt_dir, run_id, receipt_id)?;
    assert!(!super::run_is_paused(&history, run_id));
    Ok(())
}
