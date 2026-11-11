//! The actual outreach graph, revised while a send has only an acknowledgement.

use super::*;
use runx_runtime::scripted_model::{
    managed_agent_env, read_chat_request, tool_call, write_chat_response,
};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

fn prompt(request: &Value) -> TestResult<Value> {
    let text = request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .ok_or("no prompt")?;
    let marker = "Run context (JSON). Each context entry is a line of an index; turn_read reads its data by its locator:\n";
    let (_, context) = text.split_once(marker).ok_or_else(|| {
        format!(
            "no run context: {}",
            text.chars().take(300).collect::<String>()
        )
    })?;
    Ok(serde_json::Deserializer::from_str(context)
        .into_iter::<Value>()
        .next()
        .ok_or("no context JSON")??)
}

fn response(request: &Value) -> TestResult<Value> {
    let context = prompt(request)?;
    let inputs = &context["inputs"];
    let output = if let Some(qualification) = inputs.get("qualification_context") {
        if context["task"] != "lead-qualify-assess" {
            return Err("the selected qualification task did not reach the model".into());
        }
        json!({"assessment_draft": {"assessments": qualification["criteria"].as_array().ok_or("no criteria")?.iter().map(|criterion| json!({
            "criterion_id": criterion["id"], "status": "met",
            "rationale": "The admitted company industry is B2B software.",
            "fact_fields": ["company.industry"]
        })).collect::<Vec<_>>()}})
    } else {
        if context["task"] != "cold-outreach-draft" {
            return Err("the selected writing task did not reach the model".into());
        }
        json!({"content_draft": {
            "decision": "ready",
            "content_brief": {"objective": inputs["purpose"], "audience": inputs["evidence_context"]["audience"], "channel": "email"},
            "draft": {"title": inputs["offer"], "body": format!("Your company sells B2B software. {}\n{}", inputs["offer"].as_str().ok_or("no offer")?, inputs["required_footer"].as_str().ok_or("no footer")?),
                "claims": [{"claim": "The fictional company sells B2B software.", "source_digests": inputs["evidence_context"]["source_digests"]}]},
            "review_checklist": ["Confirm current contact eligibility before any send."],
            "distribution_notes": {"primary_channel": "email"}, "context_binding": {"applied": []}
        }})
    };
    Ok(tool_call("done", "turn_complete", &output))
}

/// Forward the complete request unchanged to the actual local model. No
/// response or verdict is supplied by the test in this optional proof.
fn local_response(request: &Value) -> TestResult<Value> {
    let mut child = Command::new("curl")
        .args([
            "-q",
            "-fsS",
            "--max-time",
            "180",
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
        ])
        .arg(
            std::env::var("RUNX_TEST_OUTREACH_MODEL_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:11434/v1/chat/completions".to_owned()),
        )
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("no model input pipe")?
        .write_all(request.to_string().as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(format!(
            "local model failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn assert_reuse(root: &Path, expected: bool) -> TestResult {
    let read = runx_runtime::run_record::read_run(
        &std::collections::BTreeMap::new(),
        root,
        "revised",
        None,
    )?;
    let paths = runx_runtime::RunxPaths::resolve(&std::collections::BTreeMap::new(), root);
    let receipts = runx_runtime::LocalReceiptStore::of_project(root.join("receipts"), &paths);
    let mut reused = std::collections::BTreeSet::new();
    for record in read.events {
        if let runx_contracts::RunEventFact::Call {
            target: runx_contracts::CallTarget::Skill { skill, .. },
            reuse: Some(reuse),
            outcome,
            ..
        } = record.event
            && skill.as_str() == "../lead-qualify#qualify"
            && let runx_contracts::ReuseDecision::Reused { from } = reuse.decision
            && let runx_contracts::RunCallOutcome::Completed { receipt_id, .. } = *outcome
        {
            let receipt = receipts.read_exact_without_proof(receipt_id.as_str())?;
            assert_eq!(receipt.lineage.reused_from, Some(from.clone()));
            assert!(
                receipt.lineage.children.is_empty(),
                "reusing starts no new child"
            );
            let source = receipts.read_exact_without_proof(from.as_str())?;
            assert!(
                source.subject.reference.uri.contains(RUN_ID),
                "original source is retained"
            );
            reused.insert(receipt_id);
        }
    }
    assert_eq!(reused.len(), if expected { 3 } else { 0 });
    Ok(())
}

fn assert_unconfirmed_ack(root: &Path) -> TestResult {
    let connection = rusqlite::Connection::open_with_flags(
        root.join(".runx/kernel-v2.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut query = connection.prepare("SELECT key, data FROM kernel_events WHERE stream = 'effect' AND event_type = 'acknowledged'")?;
    let mut bob = None;
    for row in query.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })? {
        let (key, data) = row?;
        let event: Value = serde_json::from_slice(&data)?;
        if event["acknowledgement"]["write_result"]["to"] == "bob@example.test" {
            bob = Some(key);
        }
    }
    let key = bob.ok_or("Bob has no durable acknowledgement")?;
    let latest: String = connection.query_row(
        "SELECT event_type FROM kernel_keys WHERE stream = 'effect' AND key = ?1",
        [key],
        |row| row.get(0),
    )?;
    assert_eq!(
        latest, "acknowledged",
        "the send has no confirmed or finalized result"
    );
    Ok(())
}

fn assert_bob_waits(root: &Path, held: &Value) -> TestResult {
    let requests = held["requests"].as_array().ok_or("no pending requests")?;
    assert_eq!(requests.len(), 1, "{held}");
    assert_eq!(requests[0]["kind"], "retry", "{held}");
    let index = requests[0]["id"]
        .as_str()
        .and_then(|id| id.strip_prefix("send/"))
        .and_then(|id| id.strip_suffix("/retry"))
        .ok_or("the send's retry has no call index")?
        .parse::<u32>()?;
    let connection = rusqlite::Connection::open_with_flags(
        root.join(".runx/kernel-v2.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let input: Vec<u8> = connection.query_row(
        "SELECT b.data FROM kernel_events e JOIN kernel_blobs b ON b.digest = json_extract(CAST(e.data AS TEXT), '$.input') WHERE e.stream = 'run' AND e.event_type = 'call' AND e.key LIKE 'revised/send@%' AND json_extract(CAST(e.data AS TEXT), '$.index') = ?1 ORDER BY e.sequence DESC LIMIT 1",
        [index], |row| row.get(0))?;
    let input: Value = serde_json::from_slice(&input)?;
    assert_eq!(
        input["input"]["to"], "bob@example.test",
        "the unknown recipient alone waits"
    );
    Ok(())
}

/// Answer qualifications in reverse order, observing each completed call in
/// the native record before allowing the next answer.
struct Model {
    endpoint: String,
    local: bool,
    record_path: PathBuf,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Value>>>,
    server: Option<JoinHandle<io::Result<()>>>,
}

impl Model {
    fn serve(root: &Path, local: bool) -> TestResult<Self> {
        let root = root.to_path_buf();
        let record_path = root.join("model-requests.json");
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopping = Arc::clone(&stop);
        let recorded = Arc::clone(&requests);
        let server = std::thread::spawn(move || {
            let accept = || -> io::Result<Option<(TcpStream, Value)>> {
                let (mut stream, _) = listener.accept()?;
                if stopping.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
                let request = read_chat_request(&mut stream)?;
                recorded
                    .lock()
                    .map_err(|_| io::Error::other("requests poisoned"))?
                    .push(request.clone());
                Ok(Some((stream, request)))
            };
            let answer = |mut stream: TcpStream, request: Value| -> io::Result<()> {
                let body = if local {
                    local_response(&request)
                } else {
                    response(&request)
                }
                .map_err(|error| io::Error::other(error.to_string()))?;
                write_chat_response(&mut stream, &body)?;
                Ok(())
            };
            if !local {
                let mut initial = Vec::new();
                for _ in 0..3 {
                    let Some(request) = accept()? else {
                        return Ok(());
                    };
                    initial.push(request);
                }
                for (answered, name) in ["carol", "bob", "ann"].into_iter().enumerate() {
                    let index = initial
                        .iter()
                        .position(|(_, request)| {
                            prompt(request).is_ok_and(|context| {
                                context["inputs"]["qualification_context"]["subject"]["id"]
                                    == format!("person:{name}")
                            })
                        })
                        .ok_or_else(|| {
                            io::Error::other("expected three different qualifications")
                        })?;
                    let (stream, request) = initial.remove(index);
                    answer(stream, request)?;
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                    loop {
                        let read = runx_runtime::run_record::read_run(
                            &std::collections::BTreeMap::new(),
                            &root,
                            RUN_ID,
                            None,
                        )
                        .map_err(|error| io::Error::other(error.to_string()))?;
                        let completed = read.events.iter().filter(|record| matches!(&record.event,
                        runx_contracts::RunEventFact::Call { target: runx_contracts::CallTarget::Skill { skill, .. }, outcome, .. }
                        if skill.as_str() == "../lead-qualify#qualify" && matches!(outcome.as_ref(), runx_contracts::RunCallOutcome::Completed { .. }))).count();
                        if completed == answered + 1 {
                            break;
                        }
                        if std::time::Instant::now() >= deadline {
                            return Err(io::Error::other(
                                "qualification did not complete before the next answer",
                            ));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }
            }
            while let Some((stream, request)) = accept()? {
                answer(stream, request)?;
            }
            Ok(())
        });
        Ok(Self {
            endpoint,
            local,
            record_path,
            stop,
            requests,
            server: Some(server),
        })
    }

    fn counts(&self) -> TestResult<(usize, usize)> {
        let requests = self.requests.lock().map_err(|_| "requests poisoned")?;
        let qualifications = requests
            .iter()
            .filter(|request| {
                prompt(request)
                    .is_ok_and(|context| context["inputs"].get("qualification_context").is_some())
            })
            .count();
        Ok((qualifications, requests.len() - qualifications))
    }

    fn finish(&mut self) -> TestResult {
        if self.server.is_none() {
            return Ok(());
        }
        fs::write(
            &self.record_path,
            serde_json::to_vec(&*self.requests.lock().map_err(|_| "requests poisoned")?)?,
        )?;
        self.stop.store(true, Ordering::SeqCst);
        let address = self
            .endpoint
            .trim_start_matches("http://")
            .split('/')
            .next()
            .ok_or("no address")?;
        drop(TcpStream::connect(address)?);
        if let Some(server) = self.server.take() {
            server.join().map_err(|_| "model server panicked")??;
        }
        Ok(())
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn batch() -> Value {
    json!({
        "campaign": "revise-proof", "sender": {"name": "Example Ops", "from_address": "ops@example.test", "reply_to": "ops@example.test"},
        "audience": "engineering maintainers", "purpose": "Offer a release review using fictional facts.",
        "offer": "A release review.", "required_footer": "Example Ops - reply to opt out.",
        "criteria": [{"id": "industry", "description": "Works for a B2B software company.", "required": true}],
        "requested_fields": ["person.name", "company.industry"], "as_of": "2026-11-01T00:00:00Z",
        "leads": (["ann", "bob", "carol"].into_iter().enumerate().map(|(index, name)| json!({
            "id": name, "subject": {"id": format!("person:{name}"), "kind": "person"},
            "sources": [{"source_id": format!("crm:{name}"), "source_digest": format!("sha256:{}", ["a", "b", "c"][index].repeat(64)),
                "subject_id": format!("person:{name}"), "observed_at": "2026-10-30T00:00:00Z", "match_confidence": "high",
                "facts": [{"field": "person.name", "value": name}, {"field": "company.industry", "value": "B2B software"}]}],
            "prospect": {"email": format!("{name}@example.test"), "name": name, "company": "Example SaaS"},
            "contact_policy": {"recipient": format!("{name}@example.test"), "status": "eligible", "suppressed": false,
                "policy_id": "fixture:consent", "evidence_ids": ["fixture:explicit-demo-consent"]},
            "facts": ["The fictional company sells B2B software.", "Example Ops offers a release review and a build notification walkthrough.", format!("The fictional prospect {name} maintains monthly release notes and is evaluating release automation.")]
        })).collect::<Vec<_>>())
    })
}

fn project(root: &Path, reuse: bool) -> TestResult<PathBuf> {
    mail_project(root)?;
    fs::write(
        root.join("connectors/fixture/mail/connector.yaml"),
        LOCAL_MAIL.replace(
            "    verb: write\n",
            "    verb: write\n    approval: required\n",
        ),
    )?;
    let repo = crate::support::repo_root()?;
    crate::support::copy_dir(&repo.join("connectors"), &root.join("connectors"))?;
    for name in [
        "cold-outreach",
        "lead-enrichment",
        "lead-qualify",
        "ghostwrite",
        "brand-voice",
        "taste-profile",
        "research",
    ] {
        crate::support::copy_dir(
            &repo.join("skills").join(name),
            &root.join("skills").join(name),
        )?;
    }
    let skill = root.join("skills/revisable-outreach");
    crate::support::copy_dir(&repo.join("fixtures/skills/revisable-outreach"), &skill)?;
    if !reuse {
        let path = root.join("skills/cold-outreach/batch.mjs");
        let original = fs::read_to_string(&path)?;
        let option = "{ reuse: { max_age_seconds: QUALIFICATION_MAX_AGE_SECONDS } }";
        assert_eq!(original.matches(option).count(), 1);
        fs::write(path, original.replace(option, "{}"))?;
    }
    Ok(skill)
}

fn command(root: &Path, model: &Model) -> Command {
    let mut command = unsigned_runx_command_at(root);
    command
        .envs(managed_agent_env(
            &model.endpoint,
            if model.local {
                "qwen3-8b-32k"
            } else {
                "outreach-proof"
            },
        ))
        .env("MAIL_TOKEN", "local-mailbox-token");
    if model.local {
        command.env(
            "RUNX_AGENT_PROVIDER",
            std::env::var("RUNX_TEST_OUTREACH_PROVIDER").unwrap_or_else(|_| "ollama".to_owned()),
        );
    }
    command
}

fn start(
    root: &Path,
    skill: &Path,
    model: &Model,
    run_id: &str,
    batch: &Value,
) -> TestResult<Value> {
    let input = root.join(format!("{run_id}.json"));
    fs::write(&input, json!({"batch": batch}).to_string())?;
    status(
        &command(root, model)
            .args(["skill", "--drive"])
            .args(["--drive-rounds", "8"])
            .arg(skill)
            .arg("--inputs")
            .arg(input.file_name().ok_or("input has no file name")?)
            .args(["--run-id", run_id, "--receipt-dir"])
            .arg(root.join("receipts"))
            .arg("--json")
            .output()?,
    )
}

fn approve(root: &Path, model: &Model, run_id: &str, pending: &Value) -> TestResult<Command> {
    let approvals = pending["requests"]
        .as_array()
        .ok_or("no approval requests")?
        .iter()
        .filter(|request| request["kind"] == "approval")
        .map(|request| {
            (
                request["id"].as_str().unwrap_or_default().to_owned(),
                Value::Bool(true),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    assert!(
        !approvals.is_empty(),
        "the remaining exact drafts are approved: {pending}"
    );
    let history = unsigned_runx_command_at(root)
        .args(["history", run_id, "--json"])
        .output()?;
    let bound = crate::support::bind_resume(
        &serde_json::from_slice(&history.stdout)?,
        json!({"approvals": approvals}),
    )?;
    let answers = root.join(format!("{run_id}-approvals.json"));
    fs::write(&answers, bound.to_string())?;
    let mut command = command(root, model);
    command
        .args(["resume", run_id])
        .arg(answers)
        .arg("--receipt-dir")
        .arg(root.join("receipts"))
        .arg("--json");
    Ok(command)
}

fn prove(reuse: bool, local: bool) -> TestResult {
    let root = crate::support::temp_root("revise-outreach");
    let skill = project(&root, reuse)?;
    let mut model = Model::serve(&root, local)?;
    let original = batch();
    fs::create_dir_all(root.join("mail"))?;
    fs::write(root.join("mail/refuse"), "carol@example.test\n")?;
    let ready = crate::support::rendezvous::Pipe::new(root.join("mail/ready"))?;
    let release = crate::support::rendezvous::Pipe::new(root.join("mail/release"))?;
    // The process has returned its ACK before runx invokes readback. This
    // gate leaves actual persisted custody at ACK, with no confirmation.
    let script = MAIL_SEND
        .replace("  while read -r sent to; do", "  printf '%s\\n' \"$RUNX_INPUT_TO\" >> \"$mailbox/readbacks\"\n  if [ \"$RUNX_INPUT_TO\" = bob@example.test ] && [ -f \"$mailbox/hold\" ]; then\n    echo ready > \"$mailbox/ready\"\n    read -r go < \"$mailbox/release\"\n  fi\n  while read -r sent to; do")
        .replace("printf '%s %s\\n'", "printf '%s' \"$RUNX_INPUT_BODY\" > \"$mailbox/$RUNX_INPUT_TO.body\"\nprintf '%s %s\\n'");
    fs::write(
        root.join("connectors/fixture/mail/tools/mail/send/send.sh"),
        script,
    )?;
    fs::write(root.join("mail/hold"), "")?;
    let pending = start(&root, &skill, &model, RUN_ID, &original)?;
    let initial_counts = model.counts()?;
    assert!(
        initial_counts.0 >= 3 && initial_counts.1 >= 3,
        "first prepare: {pending}"
    );
    if !local {
        assert_eq!(initial_counts, (3, 3));
    }
    assert_eq!(
        pending["requests"].as_array().map(Vec::len),
        Some(3),
        "{pending}"
    );
    let mut child = approve(&root, &model, RUN_ID, &pending)?
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    assert_eq!(ready.reached(child.id())?.as_deref(), Some("ready\n"));
    assert_eq!(sent_to(&root, "ann@example.test")?, 1);
    assert_eq!(sent_to(&root, "bob@example.test")?, 1);
    child.kill()?;
    release.release(child.id())?;
    crashed(&child.wait_with_output()?);
    fs::remove_file(root.join("mail/hold"))?;
    assert_unconfirmed_ack(&root)?;
    let persisted = history(&root)?;
    assert!(
        calls(&persisted, RUN_ID).contains(&pair(1, "claimed")),
        "{persisted}"
    );
    fs::write(root.join("mail/refuse"), "")?;

    let mut revised = original.clone();
    revised["offer"] = json!("A build notification walkthrough.");
    let pending = start(&root, &skill, &model, "revised", &revised)?;
    let revised_counts = model.counts()?;
    if reuse {
        assert_eq!(
            revised_counts.0, initial_counts.0,
            "offer changes reuse qualification: {pending}"
        );
    } else {
        assert!(revised_counts.0 >= initial_counts.0 + 3);
    }
    assert!(
        revised_counts.1 >= initial_counts.1 + 3,
        "drafts regenerate"
    );
    assert_reuse(&root, reuse)?;
    let held = status(&approve(&root, &model, "revised", &pending)?.output()?)?;
    assert_bob_waits(&root, &held)?;
    assert_eq!(
        sent_to(&root, "bob@example.test")?,
        1,
        "unknown outcome cannot dispatch"
    );
    assert_eq!(
        sent_to(&root, "carol@example.test")?,
        1,
        "sibling continues"
    );
    let before = lines(&root, "mail/readbacks")?
        .iter()
        .filter(|to| *to == "bob@example.test")
        .count();
    assert_eq!(
        before, 1,
        "the revised run does not read back another intent"
    );
    let first = drive_mail(&root, RUN_ID)?;
    assert_eq!(first["status"], "sealed", "{first}");
    assert!(
        lines(&root, "mail/readbacks")?
            .iter()
            .filter(|to| *to == "bob@example.test")
            .count()
            == before + 1,
        "original recovery actually reads the mailbox"
    );
    let sealed = drive_mail(&root, "revised")?;
    assert_eq!(sealed["status"], "sealed", "{sealed}");
    let first = result_of(&first)?;
    let final_result = result_of(&sealed)?;
    for index in [0, 1] {
        let cached = &final_result["sent"][index];
        // A new draft may be withheld for Ann, whose original send already
        // completed. That stays her hold; Bob must return the original act
        // after reconciliation regardless of that sibling's draft outcome.
        if index == 0 && cached["status"] == "held" {
            continue;
        }
        assert_eq!(cached["status"], "already_done", "{final_result}");
        assert_eq!(
            cached["packet"]["already_done"]["plan_digest"],
            first["sent"][index]["packet"]["plan_digest"]
        );
        assert_eq!(
            cached["packet"]["result"],
            first["sent"][index]["packet"]["result"]
        );
        assert!(
            cached["packet"]["already_done"]["receipt_id"].is_string(),
            "original receipt is retained: {cached}"
        );
        let paths = runx_runtime::RunxPaths::resolve(&std::collections::BTreeMap::new(), &root);
        let store = runx_runtime::LocalReceiptStore::of_project(root.join("receipts"), &paths);
        let receipt = store.read_exact_without_proof(
            cached["packet"]["already_done"]["receipt_id"]
                .as_str()
                .ok_or("no original receipt")?,
        )?;
        let intent = receipt
            .acts
            .iter()
            .find_map(|act| act.operation.as_ref())
            .ok_or("no original intent")?;
        let to = cached["to"].as_str().ok_or("no recipient")?;
        let body = fs::read_to_string(root.join("mail").join(format!("{to}.body")))?;
        assert_eq!(
            intent.payload_digest.as_str(),
            runx_contracts::canonical_digest(&json!({"to": to, "body": body}))?,
            "the original receipt binds the payload actually sent"
        );
        assert_eq!(
            cached["packet"]["already_done"]["plan_digest"],
            runx_contracts::canonical_digest(intent)?
        );
    }
    for to in ["ann@example.test", "bob@example.test", "carol@example.test"] {
        assert_eq!(sent_to(&root, to)?, 1, "{to}");
    }
    revised["criteria"][0]["description"] =
        json!("Specifically sells B2B software, rather than consulting services.");
    let changed = start(&root, &skill, &model, "criteria_changed", &revised)?;
    let final_counts = model.counts()?;
    assert!(
        final_counts.0 >= revised_counts.0 + 3,
        "criteria invalidates qualification: {changed}"
    );
    assert!(final_counts.1 >= revised_counts.1 + 3);
    fs::write(root.join("model-counts.json"), json!({"local": local, "reuse": reuse, "initial": initial_counts, "revised": revised_counts, "criteria_changed": final_counts}).to_string())?;
    model.finish()?;
    Ok(())
}

#[test]
fn revise_keeps_qualification_and_reconciles_an_acknowledged_send_with_and_without_reuse()
-> TestResult {
    prove(true, false)?;
    prove(false, false)
}

#[test]
#[ignore = "requires a keyless local chat-completions server with qwen3-8b-32k"]
fn a_local_model_revises_and_recovers_the_same_outreach_batch() -> TestResult {
    prove(true, true)
}

#[test]
#[ignore = "requires a keyless local chat-completions server with qwen3-8b-32k"]
fn a_local_model_recovers_the_same_outreach_batch_without_reuse() -> TestResult {
    prove(false, true)
}
