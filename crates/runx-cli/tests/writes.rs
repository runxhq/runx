//! Who may write, and whether a write waits: a turn's own call never writes
//! to `store` (D19), under `effects: ask` every write with no approval of its
//! own waits on a person, at any depth (D25), and a read turn keeps nothing
//! but the person's words, in `said:` on `private`, under its host's term
//! (D22).
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

/// Start the bundled `agent` at `method` under `authority`, which `save`
/// saves as a host does, and stop at its turn, which is named for the method.
fn start_session(
    mut runx: Command,
    save: Command,
    method: &str,
    authority: &str,
) -> TestResult<String> {
    let name = crate::support::save_authority(save, authority)?;
    let started = assert_json(
        &runx
            .args(["skill", &format!("agent#{method}")])
            .args(["--authority", &name, "--json"])
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

/// A consent key, named as contact-consent-review's runs name it
/// (skills/contact-consent-review/run.yaml). A turn's call reaches only its
/// own package's stream of that name: another's can't be spelled.
fn consent_stream() -> Value {
    json!({"stream": "consent", "key": "contact:probe"})
}

#[test]
fn a_turns_own_call_never_appends_to_a_stream_at_any_effects() -> TestResult {
    for (method, authority) in [("read", ASKING), ("write", r#"{"effects":"allow"}"#)] {
        let root = crate::support::temp_root("runx-writes-turn-append");
        // A workspace, which a saved authority is for.
        std::fs::create_dir_all(root.join("skills"))?;
        let run = start_session(command(&root), command(&root), method, authority)?;
        let call = |tool: &str, input: Value| turn_call(command(&root), &run, method, tool, input);

        let loaded = call("load", json!({"connector": "store"}))?;
        assert_eq!(loaded["status"], "completed", "{loaded}");

        let mut forged = consent_stream();
        forged["expected_version"] = json!(0);
        forged["type"] = json!("list_hygiene.consent_transitioned");
        forged["data"] = json!({"new_state": "re_permission", "contact": "probe@example.invalid"});
        let appended = call(
            "store__events_append",
            json!({"idempotency_key": "forged-consent", "input": forged}),
        )?;
        // Naming the skill's package is no way round it: a write has no
        // package field.
        let mut named = forged.clone();
        named["package"] = json!("runx/contact-consent-review");
        let spelled = call(
            "store__events_append",
            json!({"idempotency_key": "forged-named", "input": named}),
        )?;
        assert_ne!(spelled["status"], "completed", "{spelled}");
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
        // A workspace, which a saved authority is for.
        std::fs::create_dir_all(project.join("skills"))?;
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

    /// A session of the agent's `method` in `project` under `authority`
    /// that has loaded the Slack connector, and the outcome of the one call
    /// it then makes.
    fn call_in(
        &self,
        project: &Path,
        method: &str,
        authority: &str,
        tool: &str,
        input: Value,
    ) -> TestResult<Value> {
        let run = start_session(self.runx(project)?, self.runx(project)?, method, authority)?;
        let loaded = turn_call(
            self.runx(project)?,
            &run,
            method,
            "load",
            json!({"connector": "slack"}),
        )?;
        assert_eq!(loaded["status"], "completed", "{loaded}");
        turn_call(self.runx(project)?, &run, method, tool, input)
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

    let posted = slack.call_in(
        &project(&root),
        "write",
        ASKING,
        "slack__channel_post",
        post(),
    )?;
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

    let replied = slack.call_in(
        &project(&root),
        "write",
        ASKING,
        "slack__thread_reply",
        reply(),
    )?;
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

    let replied = slack.call_in(
        &project(&root),
        "write",
        &authority,
        "slack__thread_reply",
        reply(),
    )?;
    assert_eq!(replied["status"], "completed", "{replied}");
    assert_eq!(slack.sent()?, vec!["thread.reply".to_owned()], "{replied}");

    // It names one operation: a post still keeps its own approval.
    let posted = slack.call_in(
        &project(&root),
        "write",
        &authority,
        "slack__channel_post",
        post(),
    )?;
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
        let called = slack.call_in(&project(&root), "write", none, tool, input)?;
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

// D16: `agent#read` takes `effects: none`, so a read turn changes nothing at
// any depth, whatever its host allows.

#[test]
fn a_read_turn_changes_nothing_at_any_depth_whatever_its_host_allows() -> TestResult {
    let root = crate::support::temp_root("runx-writes-read-wall");
    std::fs::create_dir_all(&root)?;
    let slack = Slack::serve(&root)?;
    let allowing = r#"{"effects":"allow","reach":"workspace"}"#;

    // A provider write the read turn reaches, even one with its own approval.
    for (tool, input) in [
        ("slack__thread_reply", reply()),
        ("slack__channel_post", post()),
    ] {
        let called = slack.call_in(&project(&root), "read", allowing, tool, input)?;
        assert_eq!(called["status"], "refused", "{tool}: {called}");
        assert!(
            called.to_string().contains("effects are none"),
            "{tool}: {called}"
        );
    }
    assert_eq!(slack.sent()?, Vec::<String>::new());

    // A project skill the read turn loads, whose process would write a file.
    let marker = project(&root).join("marker");
    let skill = project(&root).join("skills/marker");
    std::fs::create_dir_all(&skill)?;
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: marker\ndescription: Writes a marker.\n---\n# Marker\n\nWrite the marker.\n",
    )?;
    std::fs::write(
        skill.join("run.yaml"),
        format!(
            "minimum_runx: 0.12.0\nmethods:\n  mark:\n    steps:\n      - id: mark\n        tool: {{command: sh, args: [-c, 'echo written > {}']}}\n",
            marker.display()
        ),
    )?;
    let run = start_session(
        slack.runx(&project(&root))?,
        slack.runx(&project(&root))?,
        "read",
        allowing,
    )?;
    let loaded = turn_call(
        slack.runx(&project(&root))?,
        &run,
        "read",
        "load",
        json!({"skill": "skills/marker", "method": "mark"}),
    )?;
    let tool = loaded["output"]["tools"]
        .as_array()
        .and_then(|tools| tools.first())
        .and_then(|tool| tool["name"].as_str())
        .ok_or_else(|| format!("the load offers the method: {loaded}"))?
        .to_owned();
    let called = turn_call(slack.runx(&project(&root))?, &run, "read", &tool, json!({}))?;
    assert_ne!(called["status"], "completed", "{called}");
    assert!(
        called
            .to_string()
            .contains("effects are none, so it changes nothing: running sh"),
        "{called}"
    );
    assert!(!marker.exists(), "a read turn's load wrote at depth");
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

// D22: a read turn may keep the person's exact words, in `said:` on
// `private`, under a term its host grants. It isn't an exception to D16's
// wall, which covers the workspace and providers, and no skill's name is in
// the rule: a `said:` stream on `private` is the person's own record,
// whichever package keeps it.

/// A project package a read turn loads: `keep` appends words to the
/// `private` stream it's given, `count` appends to its own `store` stream,
/// and `read` reads a `private` stream back.
const KEEPER: &str = r#"minimum_runx: 0.12.0
methods:
  keep:
    inputs:
      stream: {type: string}
      text: {type: string}
    steps:
      - id: before
        operation: private/key.inspect
        inputs: {stream: $input.stream, key: words}
      - id: append
        operation: private/events.append
        idempotency_key: $input.text
        inputs:
          stream: $input.stream
          key: words
          expected_version: $steps.before.provider_operation.result.version
          type: words.kept
          data: {text: $input.text}
  count:
    steps:
      - id: append
        operation: store/events.append
        idempotency_key: counted
        inputs: {stream: counts, key: words, expected_version: 0, type: words.counted, data: {}}
  read:
    inputs:
      stream: {type: string}
    steps:
      - id: events
        operation: private/events.read
        inputs: {stream: $input.stream, key: words}
"#;

/// Where the keeper's streams live when a read turn loads it from the
/// workspace: under its own `project:keeper`, since a loaded package is bound
/// by itself and the turn's package doesn't carry it (§4.8, identity.md).
const KEEPER_STREAMS: &str = "project:keeper/";

/// A project with the keeper package, which reaches a `private` instance.
fn keeper_project(root: &Path) -> TestResult {
    let skill = root.join("skills/keeper");
    std::fs::create_dir_all(&skill)?;
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: keeper\ndescription: Keeps words.\n---\n# Keeper\n\nKeep the words.\n",
    )?;
    std::fs::write(skill.join("run.yaml"), KEEPER)?;
    let inspected = assert_json(
        &command(root)
            .args(["skill", "inspect", "skills/keeper#keep", "--json"])
            .output()?,
        0,
    )?;
    let reaches = inspected["connector_readiness"]
        .as_array()
        .is_some_and(|readiness| {
            readiness
                .iter()
                .any(|entry| entry["connector"] == "private")
        });
    if !reaches {
        return Err(format!("the keeper reaches no private instance: {inspected}").into());
    }
    Ok(())
}

/// A host's authority that allows every change, so only the read turn's own
/// `effects: none` holds, with a delegation on `private` when the host grants
/// one: one term of `verbs` over the keeper's streams that begin with
/// `streams`, for the read turn and the keeper.
fn keeper_authority(term: Option<(&[&str], &str)>) -> String {
    let mut authority = json!({"effects": "allow", "reach": "workspace"});
    if let Some((verbs, streams)) = term {
        authority["delegations"] = json!([{
            "connector": "private",
            "terms": [{
                "verbs": verbs,
                "bounds": {
                    "operations": ["private/*"],
                    "scopes": ["store.read", "store.append"],
                    "targets": [format!("{KEEPER_STREAMS}{streams}*")],
                    "packages": ["runx/agent", "project:keeper"],
                },
            }],
        }]);
    }
    authority.to_string()
}

/// The outcome of a fresh read turn under `authority` that loads the
/// keeper's `method` and calls it with `input`. The keeper reaches
/// connectors the turn doesn't, so its first load in the home waits on the
/// person's trust, which they give.
fn keeper_call(root: &Path, authority: &str, method: &str, input: Value) -> TestResult<Value> {
    let run = start_session(command(root), command(root), "read", authority)?;
    let load = json!({"skill": "skills/keeper", "method": method});
    let loaded = turn_call(command(root), &run, "read", "load", load)?;
    if let Some(trust) = waiting_approval(&loaded).filter(|gate| gate["gate"]["type"] == "trust") {
        let address = trust["id"].as_str().ok_or("a trust request has an id")?;
        let read = assert_json(
            &command(root).args(["history", &run, "--json"]).output()?,
            0,
        )?;
        let answers = root.join("trust.json");
        std::fs::write(
            &answers,
            crate::support::bind_resume(&read, json!({"approvals": {address: true}}))?.to_string(),
        )?;
        let resumed = command(root)
            .args(["resume", &run])
            .arg(&answers)
            .arg("--json")
            .output()?;
        assert_eq!(resumed.status.code(), Some(2), "{resumed:?}");
    } else {
        assert_eq!(loaded["status"], "completed", "{loaded}");
    }
    turn_call(
        command(root),
        &run,
        "read",
        &format!("skill__keeper__{method}"),
        input,
    )
}

/// The texts the keeper's `private` `stream` holds, read by a read turn
/// under `authority`.
fn kept(root: &Path, authority: &str, stream: &str) -> TestResult<Vec<Value>> {
    let read = keeper_call(root, authority, "read", json!({"stream": stream}))?;
    assert_eq!(read["status"], "completed", "{read}");
    let result = &read["output"]["provider_operation"]["data"]["result"];
    assert_eq!(
        result["stream"],
        format!("{KEEPER_STREAMS}{stream}"),
        "{read}"
    );
    Ok(result["events"]
        .as_array()
        .ok_or_else(|| format!("no events: {read}"))?
        .iter()
        .map(|event| event["data"]["text"].clone())
        .collect())
}

/// Whether a read turn's call of the keeper was refused at its `step`,
/// for `reason`, and so never ran past it.
fn refused_at(called: &Value, step: &str, reason: &str) -> bool {
    called["status"] != "completed"
        && called["reason"].as_str().is_some_and(|stopped| {
            stopped.contains(&format!("workflow step '{step}'")) && stopped.contains(reason)
        })
}

#[test]
fn a_read_turn_keeps_the_persons_words_in_said_on_private_under_its_hosts_term() -> TestResult {
    let root = crate::support::temp_root("runx-writes-read-remembers");
    std::fs::create_dir_all(&root)?;
    keeper_project(&root)?;
    let said = keeper_authority(Some((&["read", "write"], "said:")));

    let kept_words = keeper_call(
        &root,
        &said,
        "keep",
        json!({"stream": "said:personal", "text": "I write in Australian English."}),
    )?;
    assert_eq!(kept_words["status"], "completed", "{kept_words}");
    let appended = &kept_words["output"]["provider_operation"]["data"]["result"];
    assert_eq!(appended["status"], "committed", "{kept_words}");
    // A loaded workspace package writes under its own name, not its loader's.
    assert_eq!(
        appended["stream"], "project:keeper/said:personal",
        "{kept_words}"
    );
    assert_eq!(
        kept(&root, &said, "said:personal")?,
        vec![json!("I write in Australian English.")]
    );
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn a_read_turn_without_its_hosts_term_keeps_nothing() -> TestResult {
    let root = crate::support::temp_root("runx-writes-read-no-term");
    std::fs::create_dir_all(&root)?;
    keeper_project(&root)?;
    let words = json!({"stream": "said:personal", "text": "Call me Sam."});
    let reading = keeper_authority(Some((&["read"], "said:")));

    // No term at all: no call on private is made.
    let called = keeper_call(&root, &keeper_authority(None), "keep", words.clone())?;
    assert!(
        refused_at(&called, "before", "every private call needs a term"),
        "{called}"
    );
    // A term that only reads the person's words: the read runs, the write
    // is refused by the term, and the wall never stood in its way.
    let called = keeper_call(&root, &reading, "keep", words)?;
    assert!(
        refused_at(&called, "append", "authority Write denied"),
        "{called}"
    );
    assert!(!called.to_string().contains("effects are none"), "{called}");
    assert_eq!(kept(&root, &reading, "said:personal")?, Vec::<Value>::new());
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn a_read_turn_keeps_nothing_but_said_on_private_whatever_its_term_reaches() -> TestResult {
    let root = crate::support::temp_root("runx-writes-read-only-said");
    std::fs::create_dir_all(&root)?;
    keeper_project(&root)?;
    let everything = keeper_authority(Some((&["read", "write"], "")));

    // An observation, and streams that only start like the person's words.
    for stream in ["observed:personal", "saidx:personal", "said"] {
        let called = keeper_call(
            &root,
            &everything,
            "keep",
            json!({"stream": stream, "text": "The person prefers tea."}),
        )?;
        assert!(
            refused_at(
                &called,
                "append",
                "the run's effects are none, so it changes nothing: the private/events.append write is refused"
            ),
            "{stream}: {called}"
        );
        assert_eq!(kept(&root, &everything, stream)?, Vec::<Value>::new());
    }

    // The package's own store state: a read turn's wall holds it too.
    let counted = keeper_call(&root, &everything, "count", json!({}))?;
    assert!(
        refused_at(
            &counted,
            "append",
            "the run's effects are none, so it changes nothing: the store/events.append write is refused"
        ),
        "{counted}"
    );
    std::fs::remove_dir_all(&root)?;
    Ok(())
}
