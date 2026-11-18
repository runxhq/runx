//! Loops over real runs: a skill's method, woken by the person, its clock
//! and its own proposals, its records kept apart per loop, its failures
//! retried and then paused (the loop contract §5 to §8, §17).

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use runx_loops::{Ending, LoopName, Loops, Status, Waiting};
use runx_runtime::orchestrator::LocalOrchestrator;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A skill whose `tick` counts what woke it in its own `ticks` stream and
/// proposes its next wake, and with `propose` a brief; `fail` fails when
/// told to.
fn write_skill(root: &Path) -> TestResult {
    let dir = root.join("skills/ticker");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: ticker\ndescription: Counts what wakes it.\n---\n\nCounts.\n",
    )?;
    std::fs::write(
        dir.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  tick:
    inputs:
      label: {type: string}
      fail: {type: boolean, optional: true}
      propose: {type: boolean, optional: true}
    steps:
      - id: tick
        script: {module: tick.mjs, export: tick}
        inputs: {label: $input.label, fail: $input.fail, propose: $input.propose}
        calls:
          operations: [store/key.inspect, store/events.append]
        max_calls: 20
        outputs:
          loop: {type: object}
          count: {type: integer}
          kinds: {type: array, items: {type: string}}
          brief: {type: string}
"#,
    )?;
    std::fs::write(
        dir.join("tick.mjs"),
        r#"export async function tick(ctx, { label, fail, propose }) {
  if (fail) throw new Error("told to fail");
  const entries = ctx.context.current;
  const wake = entries.find((entry) => entry.type === "runx.loop.wake").data;
  const batch = entries.find((entry) => entry.type === "runx.loop.batch").data;
  const seen = (await ctx.operation("store/key.inspect", { stream: "ticks", key: label }))?.provider_operation?.result;
  let version = seen?.version ?? 0;
  for (const event of batch.events) {
    const appended = await ctx.operation(
      "store/events.append",
      { stream: "ticks", key: label, expected_version: version, type: "tick.counted", data: { kind: event.kind, identity: event.identity } },
      { idempotency_key: `tick:${label}:${event.identity}` },
    );
    version = appended?.provider_operation?.result?.version ?? version + 1;
  }
  const at = new Date(Date.parse(wake.now) + 10 * 60 * 1000).toISOString();
  return {
    loop: {
      changed: true,
      wake: { at, why: "check again" },
      notice: { text: `${label} counted ${version}` },
      ...(propose ? { change: { brief: `Count ${label}'s ticks, ${version} so far.`, why: `it has counted ${version}` } } : {}),
    },
    count: version,
    kinds: batch.events.map((event) => event.kind),
    brief: wake.brief,
  };
}
"#,
    )?;
    Ok(())
}

fn write_loop(root: &Path, name: &str, body: &str) -> TestResult {
    std::fs::create_dir_all(root.join("loops"))?;
    std::fs::write(root.join("loops").join(format!("{name}.yaml")), body)?;
    Ok(())
}

fn env(root: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned()),
        (
            "RUNX_HOME".to_owned(),
            root.join("home").to_string_lossy().into_owned(),
        ),
        // A local model, so unattended wakes may run; no step here asks it.
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        (
            "RUNX_AGENT_ENDPOINT_URL".to_owned(),
            "http://127.0.0.1:9/v1/chat/completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), "local-model".to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
    ])
}

/// The orchestrator the CLI runs loops with: the runtime's local effects.
fn orchestrator() -> TestResult<LocalOrchestrator> {
    Ok(LocalOrchestrator::with_effects(
        runx_runtime::RuntimeEffectRegistry::local()?,
    ))
}

fn at(text: &str) -> TestResult<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(text)?.with_timezone(&Utc))
}

/// What run `run` returned, read back by its id.
fn result(root: &Path, run: &str) -> TestResult<serde_json::Value> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run, None)?;
    assert_eq!(
        read.run.state,
        runx_contracts::RunState::Sealed,
        "{run} sealed"
    );
    let request = runx_runtime::orchestrator::SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: root.join("skills/ticker"),
        receipt_dir: None,
        run_id: Some(run.to_owned()),
        answers: None,
        inputs: BTreeMap::new(),
        env: env(root),
        cwd: root.to_path_buf(),
        drive: runx_runtime::orchestrator::DrivePolicy::HostDriven,
    };
    let sealed = orchestrator()?.run_skill_with_binding(&request, Some("tick"), None, None)?;
    Ok(serde_json::to_value(&sealed.output)?
        .get("result")
        .cloned()
        .ok_or("no result")?)
}

#[test]
fn a_loop_wakes_on_the_person_its_clock_and_its_own_proposal_and_keeps_its_records_apart()
-> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    write_skill(root)?;
    write_loop(
        root,
        "acme",
        "calls: ./skills/ticker#tick\nbrief: Count Acme's ticks.\ninputs: {label: acme}\non: [{every: 1h}]\n",
    )?;
    write_loop(
        root,
        "beta",
        "calls: ./skills/ticker#tick\nbrief: Count Beta's ticks.\ninputs: {label: acme}\npolicy: {min_interval: 30m}\n",
    )?;
    let loops = Loops::open(env(root), root, orchestrator()?)?;
    let acme = LoopName::new("acme")?;
    let beta = LoopName::new("beta")?;
    let start = at("2026-11-17T00:00:00Z")?;

    // Started, nothing has woken it: a pass anchors its clock and runs nothing.
    loops.start(&acme, "person:kam", start)?;
    let first = loops.pass(&acme, start)?;
    assert_eq!(first.woke, None);
    assert_eq!(first.next, Some(at("2026-11-17T01:00:00Z")?));

    // The person's "run now" wakes it once.
    loops.run_now(&acme, "person:kam")?;
    let manual = loops.pass(&acme, start + Duration::seconds(5))?;
    let run = manual
        .woke
        .clone()
        .ok_or_else(|| format!("no wake: {manual:?}"))?;
    assert!(
        matches!(manual.settled.as_slice(), [(settled, Ending::Answered { .. })] if *settled == run)
    );
    let told = result(root, &run)?;
    assert_eq!(told["count"], 1);
    assert_eq!(told["kinds"], serde_json::json!(["manual"]));
    assert_eq!(told["brief"], "Count Acme's ticks.");
    let state = loops.state(&acme)?;
    assert_eq!(state.position, 1);
    assert_eq!(
        state.notices.last().map(|notice| notice.text.as_str()),
        Some("acme counted 1")
    );
    // Its proposed wake waits for its time; the next pass before it runs nothing.
    assert_eq!(state.wakes.len(), 1);
    assert_eq!(loops.pass(&acme, start + Duration::minutes(5))?.woke, None);

    // Its own wake comes due: it's put in the inbox, claimed and run.
    let proposed = loops.pass(&acme, start + Duration::minutes(11))?;
    let run = proposed.woke.ok_or("its proposed wake didn't run")?;
    assert_eq!(result(root, &run)?["kinds"], serde_json::json!(["clock"]));
    assert_eq!(result(root, &run)?["count"], 2);

    // An hour after it started its clock comes due, as has the wake its last
    // run proposed: one wake takes both.
    let clock = loops.pass(&acme, start + Duration::minutes(61))?;
    let run = clock.woke.ok_or("its clock didn't run")?;
    let told = result(root, &run)?;
    assert_eq!(told["kinds"], serde_json::json!(["clock", "clock"]));
    assert_eq!(told["count"], 4);
    assert_eq!(loops.state(&acme)?.position, 4);

    // Another loop on the same skill, writing the same key, keeps its own
    // records: it counts from one.
    loops.start(&beta, "person:kam", start)?;
    loops.run_now(&beta, "person:kam")?;
    let other = loops.pass(&beta, start + Duration::minutes(62))?;
    let run = other.woke.ok_or("beta didn't run")?;
    assert_eq!(result(root, &run)?["count"], 1);
    // It asked to wake in ten minutes; its min_interval paces it to thirty.
    let wakes: Vec<_> = loops
        .state(&beta)?
        .wakes
        .into_values()
        .map(|wake| wake.at)
        .collect();
    assert_eq!(wakes, [start + Duration::minutes(92)]);
    Ok(())
}

/// A wake that asks the person stays the loop's live wake, and says it waits
/// on them, so a listing tells them to answer it rather than that it works.
#[test]
fn a_wake_that_asks_the_person_waits_on_them() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let dir = root.join("skills/asker");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: asker\ndescription: Writes once it's approved.\n---\n\nWrites.\n",
    )?;
    std::fs::write(
        dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  write:\n    effects: ask\n    steps:\n      - id: write\n        tool: fs.write\n        inputs: {path: told.txt, contents: told}\n",
    )?;
    write_loop(
        root,
        "asking",
        "calls: ./skills/asker#write\nbrief: Write it.\nauthority: bounded\npolicy: {spend: {per_day_usd: 1}}\n",
    )?;
    runx_runtime::saved_authorities::save(
        &env(root),
        root,
        "bounded",
        runx_runtime::RunAuthority {
            max_spend_micro_usd: Some(100_000),
            ..Default::default()
        },
        &[],
        false,
        "2026-11-17T00:00:00Z",
    )?;
    let loops = Loops::open(env(root), root, orchestrator()?)?;
    let asking = LoopName::new("asking")?;
    let start = at("2026-11-17T00:00:00Z")?;
    loops.start(&asking, "person:kam", start)?;
    loops.run_now(&asking, "person:kam")?;

    let passed = loops.pass(&asking, start + Duration::seconds(5))?;
    let run = passed
        .woke
        .clone()
        .ok_or_else(|| format!("no wake: {passed:?}"))?;
    assert!(passed.settled.is_empty(), "{passed:?}");
    assert_eq!(passed.waiting, Some(Waiting::Person));
    assert_eq!(
        loops.state(&asking)?.live.map(|live| live.run),
        Some(run.clone())
    );
    assert_eq!(loops.waiting(&run)?, Some(Waiting::Person));
    assert!(!root.join("told.txt").exists());
    let state = loops.state(&asking)?;
    assert_eq!(
        state.reservations[&run].micro_usd, 100_000,
        "a waiting wake reserves only what its authority lets it spend"
    );
    loops.person_run(
        &asking,
        "write",
        serde_json::Map::new(),
        "person:kam",
        start + Duration::seconds(10),
    )?;
    let state = loops.state(&asking)?;
    let person_run = &state.people["write"].run;
    assert_eq!(loops.waiting(person_run)?, Some(Waiting::Person));
    assert_eq!(state.reservations[person_run].micro_usd, 100_000);
    assert_eq!(state.left_today(1_000_000, "2026-11-17"), 800_000);
    Ok(())
}

/// A change is accepted only on the revision its run ran under: one proposed
/// before a change the person accepted can't undo it, and is only declined.
#[test]
fn an_older_proposal_can_t_undo_a_change_accepted_since() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    write_skill(root)?;
    write_loop(
        root,
        "acme",
        "calls: ./skills/ticker#tick\nbrief: Count Acme's ticks.\ninputs: {label: acme, propose: true}\n",
    )?;
    let loops = Loops::open(env(root), root, orchestrator()?)?;
    let acme = LoopName::new("acme")?;
    let start = at("2026-11-17T00:00:00Z")?;
    loops.start(&acme, "person:kam", start)?;
    let mut proposed = Vec::new();
    for hours in [1, 2] {
        loops.run_now(&acme, "person:kam")?;
        let passed = loops.pass(&acme, start + Duration::hours(hours))?;
        proposed.push(format!("{}/change", passed.woke.ok_or("no wake")?));
    }
    let brief = |id: &str| -> TestResult<String> {
        match &loops
            .state(&acme)?
            .proposals
            .get(id)
            .ok_or("no proposal")?
            .proposal
        {
            runx_loops::Proposal::Change { brief, .. } => Ok(brief.clone().ok_or("no brief")?),
            other => Err(format!("{other:?}").into()),
        }
    };
    let (older, newer) = (proposed[0].as_str(), proposed[1].as_str());
    let accepted = brief(newer)?;
    assert_ne!(brief(older)?, accepted);

    let now = start + Duration::hours(3);
    loops.accept(&acme, newer, "person:kam", None, now)?;
    let file = || std::fs::read_to_string(root.join("loops/acme.yaml"));
    assert!(file()?.contains(&accepted), "{}", file()?);
    let state = loops.state(&acme)?;
    assert!(state.outdated(state.proposals.get(older).ok_or("no proposal")?));

    let refused = loops
        .accept(&acme, older, "person:kam", None, now)
        .err()
        .ok_or("the older proposal was accepted")?;
    assert!(refused.to_string().contains("no longer runs"), "{refused}");
    assert!(file()?.contains(&accepted), "{}", file()?);
    loops.decline(&acme, older, "person:kam", Some("it's out of date"), now)?;
    assert!(loops.state(&acme)?.proposals.is_empty());
    Ok(())
}

#[test]
fn a_failing_wake_is_tried_again_on_the_ladder_and_pauses_after_three() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    write_skill(root)?;
    write_loop(
        root,
        "acme",
        "calls: ./skills/ticker#tick\nbrief: Fail.\ninputs: {label: acme, fail: true}\npolicy: {retry: [1m, 5m]}\n",
    )?;
    let loops = Loops::open(env(root), root, orchestrator()?)?;
    let acme = LoopName::new("acme")?;
    let start = at("2026-11-17T00:00:00Z")?;
    loops.start(&acme, "person:kam", start)?;
    loops.run_now(&acme, "person:kam")?;
    let failed = loops.pass(&acme, start)?;
    assert!(failed.woke.is_some(), "{failed:?}");
    assert!(
        matches!(failed.settled.as_slice(), [(_, Ending::Failed { .. })]),
        "{failed:?}"
    );
    let state = loops.state(&acme)?;
    assert_eq!(
        (state.failures, state.position),
        (1, 0),
        "a failure keeps its events"
    );
    // Not before its retry time.
    assert_eq!(loops.pass(&acme, start + Duration::seconds(30))?.woke, None);
    // The person's "run now" since is their act, and wakes it at once.
    loops.run_now(&acme, "person:kam")?;
    assert!(
        loops
            .pass(&acme, start + Duration::seconds(40))?
            .woke
            .is_some()
    );
    assert!(
        loops
            .pass(&acme, start + Duration::minutes(8))?
            .woke
            .is_some()
    );
    let state = loops.state(&acme)?;
    assert!(
        matches!(&state.status, Status::Paused { why } if why.starts_with("3 failures in a row")),
        "{:?}",
        state.status
    );
    // Paused, it runs nothing; the person's retry gives the batch a new attempt.
    assert_eq!(loops.pass(&acme, start + Duration::hours(1))?.woke, None);
    write_loop(
        root,
        "acme",
        "calls: ./skills/ticker#tick\nbrief: Fail.\ninputs: {label: acme}\npolicy: {retry: [1m, 5m]}\n",
    )?;
    loops.retry(&acme, "person:kam", start + Duration::hours(1))?;
    let again = loops.pass(&acme, start + Duration::hours(2))?;
    assert!(
        matches!(again.settled.as_slice(), [(_, Ending::Answered { .. })]),
        "{again:?}"
    );
    // Both of the person's wakes are answered.
    assert_eq!(loops.state(&acme)?.position, 2);
    Ok(())
}

/// The loop runs the file as written now: going back to an earlier revision
/// runs it again, and a file that doesn't validate starts nothing, the
/// revision before it included, until it's fixed, which clears its
/// couldn't-run (Codex, review 3, finding 11).
#[test]
fn the_loop_runs_its_file_as_written_now_and_an_invalid_one_starts_nothing() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    write_skill(root)?;
    let a = "calls: ./skills/ticker#tick\nbrief: A.\ninputs: {label: a}\n";
    let b = "calls: ./skills/ticker#tick\nbrief: B.\ninputs: {label: b}\n";
    write_loop(root, "acme", a)?;
    let loops = Loops::open(env(root), root, orchestrator()?)?;
    let acme = LoopName::new("acme")?;
    let start = at("2026-11-17T00:00:00Z")?;
    loops.start(&acme, "person:kam", start)?;
    let first = loops.state(&acme)?.active;
    write_loop(root, "acme", b)?;
    loops.pass(&acme, start)?;
    assert_ne!(loops.state(&acme)?.active, first, "B is the loop's now");
    write_loop(root, "acme", a)?;
    loops.pass(&acme, start)?;
    assert_eq!(loops.state(&acme)?.active, first, "back to A, A runs again");

    write_loop(root, "acme", "calls: [not a skill\n")?;
    loops.run_now(&acme, "person:kam")?;
    let invalid = loops.pass(&acme, start + Duration::minutes(1))?;
    assert_eq!(
        invalid.woke, None,
        "nothing starts on a file that doesn't validate"
    );
    assert!(loops.state(&acme)?.couldnt.is_some());
    write_loop(root, "acme", a)?;
    let fixed = loops.pass(&acme, start + Duration::minutes(2))?;
    assert!(
        fixed.woke.is_some(),
        "the person's run now goes once it's fixed"
    );
    assert_eq!(loops.state(&acme)?.couldnt, None);
    Ok(())
}

/// A mail connector whose inbox is a feed, served over HTTP.
const MAIL: &str = r#"
schema: runx.connector.v1
name: example/mail
title: Example Mail
description: Mail with an inbox feed.
auth: {none: {}}
placements: {local: {auth: [none]}}
feeds:
  inbox:
    description: Replies, bounces and unsubscribes, oldest first.
    operation: inbox.read
    cursor: {input: after, result: cursor}
    events: {result: events, id: id}
    more: more
    wait: wait
    gap: gap
operations:
  inbox.read:
    description: Read what arrived after a cursor.
    verb: read
    scopes: [mail.read]
    target: {template: "https://mail.example.com/inbox"}
    input_schema:
      type: object
      additionalProperties: false
      properties:
        after: {type: string}
        wait: {type: integer, minimum: 0, maximum: 60}
    result_fields: [events, cursor, more, gap]
    cost: {usd_micro: 0, per: request, source: docs, source_url: "https://mail.example.com/pricing"}
    implementation:
      http:
        allowed_hosts: [mail.example.com]
        requests:
          - http_method: GET
            url: "https://mail.example.com/inbox"
            query: {after: {"$argument": "/after"}, wait: {"$argument": "/wait"}}
        result_paths: {events: acknowledgement.events, cursor: acknowledgement.cursor, more: acknowledgement.more, gap: acknowledgement.gap}
"#;

/// The mail provider's wire: it answers each read with the next page it was
/// given, the last one again once they run out, and keeps each URL it got.
#[derive(Clone, Default)]
struct Wire {
    pages: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    sent: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl runx_runtime::RuntimeHttpTransport for Wire {
    fn send(
        &self,
        request: runx_runtime::RuntimeHttpRequest,
    ) -> Result<runx_runtime::RuntimeHttpResponse, runx_runtime::RuntimeHttpError> {
        if let Ok(mut sent) = self.sent.lock() {
            sent.push(request.url);
        }
        let mut pages =
            self.pages
                .lock()
                .map_err(|_| runx_runtime::RuntimeHttpError::Transport {
                    message: "the wire's pages are poisoned".to_owned(),
                })?;
        let page = if pages.len() > 1 {
            pages.remove(0)
        } else {
            pages[0].clone()
        };
        Ok(runx_runtime::RuntimeHttpResponse::new(
            200,
            page.to_string(),
        ))
    }
}

/// What a feed answers is taken into the loop's inbox once, under each
/// event's provider id, its cursor moving after it; a poll that finds
/// nothing appends nothing; and the wake it causes has the events in its
/// batch. A provider that can't replay from the cursor wakes it with a gap.
#[test]
fn a_feed_s_events_go_into_the_inbox_once_and_wake_the_loop_with_them() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    write_skill(root)?;
    std::fs::create_dir_all(root.join("connectors/example/mail"))?;
    std::fs::write(root.join("connectors/example/mail/connector.yaml"), MAIL)?;
    let wire = Wire::default();
    let reply = serde_json::json!({"id": "m1", "type": "reply", "from": "ana@example.org"});
    let bounce = serde_json::json!({"id": "m2", "type": "bounce"});
    wire.pages.lock().map_err(|_| "poisoned")?.extend([
        serde_json::json!({"events": [reply], "cursor": "1", "more": true, "gap": false}),
        serde_json::json!({"events": [bounce], "cursor": "2", "more": false, "gap": false}),
        // Read again after a crash, before its cursor moved: nothing twice.
        serde_json::json!({"events": [bounce], "cursor": "2", "more": false, "gap": false}),
        serde_json::json!({"events": [], "cursor": "2", "more": false, "gap": false}),
        serde_json::json!({"events": [], "cursor": "9", "more": false, "gap": true}),
    ]);
    let effects = runx_runtime::RuntimeEffectRegistry::with_effect(
        runx_runtime::OperationEffect::with_http_transport(wire.clone()),
    )?;
    let loops = Loops::open(env(root), root, LocalOrchestrator::with_effects(effects))?;
    let acme = LoopName::new("acme")?;
    write_loop(
        root,
        "acme",
        "calls: ./skills/ticker#tick\nbrief: Count Acme's mail.\ninputs: {label: acme}\non: [{feed: example/mail/outbox}]\n",
    )?;
    let refused = loops
        .start(&acme, "person:kam", Utc::now())
        .err()
        .map(|error| error.to_string());
    assert!(
        refused
            .as_deref()
            .is_some_and(|why| why.contains("connector example/mail offers no feed outbox")),
        "{refused:?}"
    );
    write_loop(
        root,
        "acme",
        "calls: ./skills/ticker#tick\nbrief: Count Acme's mail.\ninputs: {label: acme}\non: [{feed: example/mail/inbox}]\n",
    )?;
    loops.start(&acme, "person:kam", Utc::now())?;
    let feeds = loops.feeds(&acme)?;
    assert_eq!(feeds.len(), 1);
    let wait = std::time::Duration::from_secs(50);

    let first = loops.poll(&acme, &feeds[0], wait)?;
    assert_eq!((first.taken, first.more), (1, true));
    let second = loops.poll(&acme, &feeds[0], wait)?;
    assert_eq!((second.taken, second.more), (1, false));
    assert_eq!(
        loops.poll(&acme, &feeds[0], wait)?.taken,
        0,
        "nothing twice"
    );
    let inbox = loops.state(&acme)?;
    assert_eq!(
        inbox.cursors.get("example/mail/inbox").map(String::as_str),
        Some("\"2\"")
    );
    let before = loops.state(&acme)?.version;
    assert_eq!(loops.poll(&acme, &feeds[0], wait)?.taken, 0);
    assert_eq!(
        loops.state(&acme)?.version,
        before,
        "a poll that finds nothing records nothing"
    );
    let sent = wire.sent.lock().map_err(|_| "poisoned")?.clone();
    assert!(
        !sent[0].contains("after="),
        "from the feed's start: {}",
        sent[0]
    );
    assert!(
        sent[1].contains("after=1") && sent[1].contains("wait=50"),
        "{}",
        sent[1]
    );
    assert!(sent[3].contains("after=2"), "{}", sent[3]);

    let woke = loops.pass(&acme, Utc::now())?;
    let run = woke
        .woke
        .clone()
        .ok_or_else(|| format!("the feed's events didn't wake it: {woke:?}"))?;
    assert_eq!(
        result(root, &run)?["kinds"],
        serde_json::json!(["feed", "feed"])
    );

    let gap = loops.poll(&acme, &feeds[0], wait)?;
    assert_eq!(gap.taken, 1);
    assert_eq!(
        loops
            .state(&acme)?
            .cursors
            .get("example/mail/inbox")
            .map(String::as_str),
        Some("\"9\""),
        "read on from where the provider can replay"
    );
    let woke = loops.pass(&acme, Utc::now())?;
    let run = woke.woke.ok_or("the gap didn't wake it")?;
    assert_eq!(result(root, &run)?["kinds"], serde_json::json!(["gap"]));
    Ok(())
}

/// A feed that can't be read says so on the loop's record, once for each
/// reason however often it's tried, so a loop that can't hear its feed never
/// looks well; a read that works clears it.
#[test]
fn a_feed_that_can_t_be_read_says_so_on_the_loop_until_it_reads_again() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    write_skill(root)?;
    std::fs::create_dir_all(root.join("connectors/example/mail"))?;
    std::fs::write(root.join("connectors/example/mail/connector.yaml"), MAIL)?;
    let wire = Wire::default();
    wire.pages.lock().map_err(|_| "poisoned")?.extend([
        serde_json::json!({"nothing": "a feed reads"}),
        serde_json::json!({"nothing": "a feed reads"}),
        serde_json::json!({"events": [], "cursor": "1", "more": false, "gap": false}),
    ]);
    let effects = runx_runtime::RuntimeEffectRegistry::with_effect(
        runx_runtime::OperationEffect::with_http_transport(wire.clone()),
    )?;
    let loops = Loops::open(env(root), root, LocalOrchestrator::with_effects(effects))?;
    let acme = LoopName::new("acme")?;
    write_loop(
        root,
        "acme",
        "calls: ./skills/ticker#tick\nbrief: Count Acme's mail.\ninputs: {label: acme}\non: [{feed: example/mail/inbox}]\n",
    )?;
    loops.start(&acme, "person:kam", Utc::now())?;
    let feeds = loops.feeds(&acme)?;
    let wait = std::time::Duration::from_secs(50);
    let refused = loops
        .poll(&acme, &feeds[0], wait)
        .err()
        .ok_or("a page with no events read")?;
    let failing = loops
        .state(&acme)?
        .feeds_failing
        .get("example/mail/inbox")
        .cloned()
        .ok_or("the failure isn't on the loop")?;
    assert_eq!(failing.why, refused.to_string());
    let version = loops.state(&acme)?.version;
    assert!(loops.poll(&acme, &feeds[0], wait).is_err());
    assert_eq!(
        loops.state(&acme)?.version,
        version,
        "the same reason is said once"
    );
    loops.poll(&acme, &feeds[0], wait)?;
    assert!(
        loops.state(&acme)?.feeds_failing.is_empty(),
        "a read that works clears it"
    );
    Ok(())
}

/// A webhook whose trigger names its sender's delivery header knows each
/// delivery by it: a retry with another body is the delivery it repeats,
/// another id is another event, and a request without the header is refused.
#[test]
fn a_webhook_knows_each_delivery_by_its_sender_s_header() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    write_skill(root)?;
    write_loop(
        root,
        "acme",
        "calls: ./skills/ticker#tick\nbrief: Count Acme's mail.\ninputs: {label: acme}\non: [{webhook: inbound, delivery: X-Delivery-Id}]\n",
    )?;
    let loops = Loops::open(env(root), root, orchestrator()?)?;
    let acme = LoopName::new("acme")?;
    loops.rotate_token(&acme, None, &runx_loops::digest(b"token"))?;
    loops.start(&acme, "person:kam", Utc::now())?;
    let delivered = |id: &str| vec![("X-Delivery-Id".to_owned(), id.to_owned())];
    assert!(loops.take_webhook(&acme, "inbound", &delivered("d1"), br#"{"n": 1}"#)?);
    assert!(
        !loops.take_webhook(
            &acme,
            "inbound",
            &delivered("d1"),
            br#"{"n": 1, "retried": true}"#
        )?,
        "a retry is the delivery it repeats"
    );
    assert!(
        loops.take_webhook(&acme, "inbound", &delivered("d2"), br#"{"n": 1}"#)?,
        "another delivery is another event"
    );
    let missing = loops
        .take_webhook(&acme, "inbound", &[], br#"{"n": 3}"#)
        .err()
        .map(|error| error.to_string());
    assert_eq!(
        missing.as_deref(),
        Some(
            "webhook inbound knows each delivery by its x-delivery-id header, which this request doesn't carry"
        )
    );
    let malformed = runx_loops::LoopFile::parse(
        b"calls: ./skills/ticker#tick\nbrief: Count.\non: [{webhook: inbound, delivery: \"not a header\"}]\n",
    )
    .err()
    .map(|error| error.to_string());
    assert!(
        malformed.is_some_and(|why| why.contains("isn't a header name")),
        "a header name is a token"
    );
    Ok(())
}

/// A webhook's token routes a request to its loop while the loop takes it;
/// each payload goes in the inbox once, by its body, and wakes the loop with
/// it in the batch; a new token retires the one before.
#[test]
fn a_webhook_s_payload_goes_into_the_inbox_once_and_a_new_token_retires_the_old() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    write_skill(root)?;
    write_loop(
        root,
        "acme",
        "calls: ./skills/ticker#tick\nbrief: Count Acme's mail.\ninputs: {label: acme}\non: [{webhook: inbound}]\n",
    )?;
    let loops = Loops::open(env(root), root, orchestrator()?)?;
    let acme = LoopName::new("acme")?;
    let first = runx_loops::digest(b"first-token");
    assert_eq!(loops.rotate_token(&acme, None, &first)?, "inbound");
    let refused = loops
        .rotate_token(&acme, Some("outbound"), &first)
        .err()
        .map(|error| error.to_string());
    assert_eq!(
        refused.as_deref(),
        Some("loop acme takes no webhook outbound; its file names webhook inbound")
    );
    // Not started, it takes nothing.
    assert_eq!(loops.webhook_of(&first)?, None);
    loops.start(&acme, "person:kam", Utc::now())?;
    let (name, webhook) = loops
        .webhook_of(&first)?
        .ok_or("its token routes nowhere")?;
    assert_eq!((name.as_str(), webhook.as_str()), ("acme", "inbound"));
    let body = br#"{"id": "m1", "type": "reply", "from": "ana@example.org"}"#;
    assert!(loops.take_webhook(&acme, "inbound", &[], body)?);
    assert!(
        !loops.take_webhook(&acme, "inbound", &[], body)?,
        "the same payload once"
    );
    let invalid = loops
        .take_webhook(&acme, "inbound", &[], b"not json")
        .err()
        .map(|error| error.to_string());
    assert!(invalid.is_some_and(|why| why.starts_with("a webhook's payload is JSON")));

    let woke = loops.pass(&acme, Utc::now())?;
    let run = woke
        .woke
        .clone()
        .ok_or_else(|| format!("the payload didn't wake it: {woke:?}"))?;
    assert_eq!(result(root, &run)?["kinds"], serde_json::json!(["webhook"]));

    let second = runx_loops::digest(b"second-token");
    loops.rotate_token(&acme, Some("inbound"), &second)?;
    assert_eq!(
        loops.webhook_of(&first)?,
        None,
        "the old token routes nowhere"
    );
    assert!(loops.webhook_of(&second)?.is_some());
    loops.stop(&acme, "person:kam", "done", Utc::now())?;
    assert_eq!(
        loops.webhook_of(&second)?,
        None,
        "a stopped loop takes nothing"
    );
    Ok(())
}
