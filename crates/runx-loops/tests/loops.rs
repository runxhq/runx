//! Loops over real runs: a skill's method, woken by the person, its clock
//! and its own proposals, its records kept apart per loop, its failures
//! retried and then paused (the loop contract §5 to §8, §17).

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use runx_loops::{Ending, LoopName, Loops, Status};
use runx_runtime::orchestrator::LocalOrchestrator;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A skill whose `tick` counts what woke it in its own `ticks` stream and
/// proposes its next wake; `fail` fails when told to.
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
    steps:
      - id: tick
        script: {module: tick.mjs, export: tick}
        inputs: {label: $input.label, fail: $input.fail}
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
        r#"export async function tick(ctx, { label, fail }) {
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
    loop: { changed: true, wake: { at, why: "check again" }, notice: { text: `${label} counted ${version}` } },
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
