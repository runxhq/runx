//! Who may write, and whether a write waits: a turn's own call never writes
//! to `store` (D19), and under `effects: ask` every write with no approval of
//! its own waits on a person, at any depth (D25).
//!
//! Each runs as a host runs its agent: the bundled `agent` skill,
//! started with `effects: ask` and `reach: workspace` and no standing
//! approvals unless the test gives one, in a scratch project with its own runx
//! home and no credentials, its turn driven over `runx mcp --run`. A provider
//! write stays on loopback, at a Connect that records each provider call it is
//! asked to perform, and the catalog's Slack connector is installed from a
//! loopback registry, as a catalog connector is.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::operator_journeys::{assert_json, command, json_string, turn_tool_on};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// How a host starts its agent's session: changes wait on the person, and
/// tools stay inside the workspace.
const ASKING: &str = r#"{"effects":"ask","reach":"workspace"}"#;

/// Start the bundled `agent` at `method` under `authority`, and stop
/// at its turn, which is named for the method.
fn start_session(mut runx: Command, method: &str, authority: &str) -> TestResult<String> {
    let started = assert_json(
        &runx
            .args(["skill", &format!("agent#{method}")])
            .args(["--run-authority", authority, "--json"])
            .output()?,
        2,
    )?;
    assert_eq!(started["status"], "needs_agent", "{started}");
    assert_eq!(started["entry_provenance"]["kind"], "bundled", "{started}");
    Ok(json_string(&started, "run_id")?.to_owned())
}

/// The outcome of one call the turn makes, on its own `runx mcp --run`.
fn turn_call(runx: Command, run: &str, turn: &str, tool: &str, input: Value) -> TestResult<Value> {
    let called = turn_tool_on(
        runx,
        run,
        "turn_call",
        json!({"turn": turn, "tool": tool, "call_id": tool, "input": input}),
    )?;
    Ok(called["outcome"].clone())
}

// D19: a turn's own call never writes to store.

/// contact-consent-review's stream of consent decisions, addressed as its
/// own runs address it (skills/contact-consent-review/run.yaml).
fn consent_stream() -> Value {
    json!({
        "data_source_uri": "local://contact-consent-review/current",
        "resource": "contact_consent_events",
        "aggregate_id": "contact:probe",
    })
}

#[test]
fn a_turns_own_call_never_appends_to_a_stream_at_any_effects() -> TestResult {
    for (method, authority) in [("read", ASKING), ("write", r#"{"effects":"allow"}"#)] {
        let root = crate::support::temp_root("runx-writes-turn-append");
        std::fs::create_dir_all(&root)?;
        let run = start_session(command(&root), method, authority)?;
        let call = |tool: &str, input: Value| turn_call(command(&root), &run, method, tool, input);

        let loaded = call("load", json!({"connector": "store"}))?;
        assert_eq!(loaded["status"], "completed", "{loaded}");

        let mut forged = consent_stream();
        forged["expected_version"] = json!(0);
        forged["event"] =
            json!({"type": "contact_consent.granted", "contact": "probe@example.invalid"});
        let appended = call(
            "store__events_append",
            json!({"idempotency_key": "forged-consent", "input": forged}),
        )?;
        let read = call("store__events_read", json!({"input": consent_stream()}))?;
        let events = &read["output"]["provider_operation"]["data"]["result"]["events"];

        assert_eq!(
            appended["status"], "refused",
            "a {method} turn's own call appended to contact-consent-review's stream under {authority}: append={appended}\nthe stream now holds {events}"
        );
        assert!(
            appended
                .to_string()
                .contains("go through a package's deterministic step"),
            "{appended}"
        );
        assert_eq!(read["status"], "completed", "{read}");
        assert_eq!(events, &json!([]), "the refused append left an event");
        std::fs::remove_dir_all(&root)?;
    }
    Ok(())
}

// D25: under `effects: ask`, every write with no approval of its own waits on
// a person, and `effects: none` refuses every write.

/// The catalog's Slack connector, installed into the project from a loopback
/// registry, and a loopback Connect holding one grant for it, which records
/// every provider call it is asked to perform and answers it as Slack would.
struct Slack {
    registry: crate::registry_fixture::LoopbackRegistry,
    connect: crate::support::MockConnect,
    sent: Arc<Mutex<Vec<Value>>>,
}

impl Slack {
    fn serve(root: &Path) -> TestResult<Self> {
        let catalog = crate::support::repo_root()?.join("connectors/slack");
        let registry = crate::registry_fixture::LoopbackRegistry::serve()?;
        registry.publish_connector(&catalog, "1.0.0", runx_contracts::TrustTier::Verified)?;
        // Connect binds its grant to the catalog's bytes, the bytes the
        // project installs.
        let bound = root.join("connect");
        crate::support::copy_dir(&catalog, &bound.join("connectors/slack"))?;
        let sent = Arc::new(Mutex::new(Vec::<Value>::new()));
        let recorded = Arc::clone(&sent);
        let connect = crate::support::MockConnect::with_operations(
            &bound,
            "operator-1",
            &[("grant_slack", "slack", &["thread.reply", "channel.post"])],
            Box::new(move |request| {
                recorded.lock().ok()?.push(request.clone());
                Some(slack_answer(request))
            }),
        )?;
        let slack = Self {
            registry,
            connect,
            sent,
        };
        let project = root.join("project");
        std::fs::create_dir_all(&project)?;
        assert_json(
            &slack
                .runx(&project)?
                .args(["registry", "install", "slack@1.0.0"])
                .args(["--registry", &slack.registry.url, "--json"])
                .output()?,
            0,
        )?;
        assert_json(
            &slack
                .runx(&project)?
                .args(["connector", "use", "slack", "grant_slack", "--json"])
                .output()?,
            0,
        )?;
        Ok(slack)
    }

    /// `runx` in `project`, pointed at this registry and this Connect.
    fn runx(&self, project: &Path) -> TestResult<Command> {
        let mut runx = command(project);
        runx.envs(self.registry.env_trusting("slack")?)
            .env("RUNX_REGISTRY_URL", &self.registry.url);
        self.connect.apply(&mut runx);
        Ok(runx)
    }

    /// The operations Connect was asked to perform.
    fn sent(&self) -> TestResult<Vec<String>> {
        Ok(self
            .sent
            .lock()
            .map_err(|_| "the Connect fixture panicked")?
            .iter()
            .map(|request| request["operation"].as_str().unwrap_or_default().to_owned())
            .collect())
    }

    /// A write session in `project` under `authority` that has loaded the
    /// Slack connector, and the outcome of the one call it then makes.
    fn develop_call(
        &self,
        project: &Path,
        authority: &str,
        tool: &str,
        input: Value,
    ) -> TestResult<Value> {
        let run = start_session(self.runx(project)?, "write", authority)?;
        let loaded = turn_call(
            self.runx(project)?,
            &run,
            "write",
            "load",
            json!({"connector": "slack"}),
        )?;
        assert_eq!(loaded["status"], "completed", "{loaded}");
        turn_call(self.runx(project)?, &run, "write", tool, input)
    }
}

/// The project a [`Slack`] serves.
fn project(root: &Path) -> PathBuf {
    root.join("project")
}

/// Slack's acknowledgement of the message `request` posts.
fn slack_answer(request: &Value) -> Value {
    let input = &request["input"];
    let message = "slack://T123/C456/1700000000.000200";
    json!({
        "status": "success",
        "provider": "slack",
        "operation": request["operation"],
        "target": request["target"],
        "access": request["access"],
        "result": {
            "provider": "slack",
            "external_tenant_ref": "T123",
            "connected_subject_ref": "U123",
            "conversation_locator": "slack://T123/C456",
            "thread_locator": input["thread_locator"],
            "message_locator": message,
            "occurred_at": "2026-11-08T00:00:00Z",
            "content_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "idempotency_key": input["idempotency_key"],
        },
        "readback_uri": "runx:provider-readback:slack-1",
        "operation_id": message,
        "idempotency_key": input["idempotency_key"],
    })
}

/// The approval a call's outcome waits on, if it waits on one.
fn waiting_approval(outcome: &Value) -> Option<&Value> {
    (outcome["status"] == "pending")
        .then(|| outcome["waiting"].as_array())
        .flatten()?
        .iter()
        .find(|request| request["kind"] == "approval")
}

fn post() -> Value {
    json!({
        "idempotency_key": "6f0d3c8e-2b1a-4c5d-9e7f-0a1b2c3d4e5f",
        "input": {"channel_locator": "slack://T123/C456", "text": "A declared approval."},
    })
}

fn reply() -> Value {
    json!({
        "idempotency_key": "probe-reply",
        "input": {
            "thread_locator": "slack://T123/C456/1700000000.000100",
            "text": "No approval of its own.",
        },
    })
}

#[test]
fn a_provider_write_that_declares_an_approval_keeps_it_under_ask() -> TestResult {
    let root = crate::support::temp_root("runx-writes-declared-approval");
    std::fs::create_dir_all(&root)?;
    let slack = Slack::serve(&root)?;

    let posted = slack.develop_call(&project(&root), ASKING, "slack__channel_post", post())?;
    let approval = waiting_approval(&posted).ok_or_else(|| format!("{posted}"))?;
    assert_eq!(approval["gate"]["type"], "operation", "{posted}");
    assert_eq!(slack.sent()?, Vec::<String>::new(), "{posted}");
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn a_provider_write_with_no_approval_of_its_own_waits_on_a_person_under_ask() -> TestResult {
    let root = crate::support::temp_root("runx-writes-undeclared-approval");
    std::fs::create_dir_all(&root)?;
    let slack = Slack::serve(&root)?;

    let replied = slack.develop_call(&project(&root), ASKING, "slack__thread_reply", reply())?;
    let sent = slack.sent()?;
    let approval = waiting_approval(&replied).ok_or_else(|| {
        format!(
            "a write turn under effects: ask replied in Slack with nothing waiting: reply={replied}\nConnect was asked to perform {sent:?}"
        )
    })?;
    assert_eq!(approval["gate"]["type"], "write", "{replied}");
    assert_eq!(
        approval["gate"]["summary"]["operation"], "thread.reply",
        "{replied}"
    );
    assert_eq!(
        approval["gate"]["payload"]["text"], "No approval of its own.",
        "the person sees the exact call: {replied}"
    );
    assert_eq!(sent, Vec::<String>::new(), "{replied}");
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn a_standing_operations_approval_lets_the_write_it_names_through() -> TestResult {
    let root = crate::support::temp_root("runx-writes-standing-operations");
    std::fs::create_dir_all(&root)?;
    let slack = Slack::serve(&root)?;
    let authority = json!({
        "effects": "ask",
        "reach": "workspace",
        "standing_approvals": [{"operations": {"names": ["slack/thread.reply"]}}],
    })
    .to_string();

    let replied =
        slack.develop_call(&project(&root), &authority, "slack__thread_reply", reply())?;
    assert_eq!(replied["status"], "completed", "{replied}");
    assert_eq!(slack.sent()?, vec!["thread.reply".to_owned()], "{replied}");

    // It names one operation: a post still keeps its own approval.
    let posted = slack.develop_call(&project(&root), &authority, "slack__channel_post", post())?;
    assert!(waiting_approval(&posted).is_some(), "{posted}");
    assert_eq!(slack.sent()?, vec!["thread.reply".to_owned()], "{posted}");
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn effects_none_refuses_every_provider_write_even_one_with_its_own_approval() -> TestResult {
    let root = crate::support::temp_root("runx-writes-none");
    std::fs::create_dir_all(&root)?;
    let slack = Slack::serve(&root)?;
    let none = r#"{"effects":"none","reach":"workspace"}"#;

    for (tool, input) in [
        ("slack__thread_reply", reply()),
        ("slack__channel_post", post()),
    ] {
        let called = slack.develop_call(&project(&root), none, tool, input)?;
        assert_eq!(called["status"], "refused", "{tool}: {called}");
        assert!(
            called.to_string().contains("effects are none"),
            "{tool}: {called}"
        );
    }
    assert_eq!(slack.sent()?, Vec::<String>::new());
    std::fs::remove_dir_all(&root)?;
    Ok(())
}
