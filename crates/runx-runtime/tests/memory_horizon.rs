#![cfg(all(
    feature = "agent",
    feature = "scripted-model",
    feature = "probe-fixtures"
))]

//! memory's long-horizon evaluation (D35): at each size of
//! `skills/memory/evals/horizon.json`, a history is laid down in a fresh home,
//! seeded, with facts planted at known depths and every date relative to
//! today, and the horizon fixture asks the routine each case: phase 1, plain
//! and with a focus, then an approved `memory#correct`, then phase 2. The
//! report names its run date, seed, model and repeats, and gives recall by
//! depth, behaviour by category and tokens per step.
//!
//! The model is a scripted reader unless `RUNX_MEMORY_HORIZON_MODEL` names a
//! local one: it searches for the question, reads the best match and answers
//! with its words, so the bar is a deterministic gate on retrieval, at every
//! size but the largest, which runs on a real model, where the evaluation's
//! threshold is set. Named
//! `qwen3-8b-32k`, the turns go to a local server through a loopback relay
//! that counts the replies that only wrote prose. Nothing here reaches a paid
//! model.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use runx_runtime::probe_fixtures::HistoryEvent;
use runx_runtime::scripted_model::{ScriptedModel, managed_agent_env, memory_reader};
use runx_runtime::{
    HttpMethod, LocalOrchestrator, ManagedAgentPolicy, ReqwestHttpTransport, RuntimeEffectRegistry,
    RuntimeHttpHeader, RuntimeHttpRequest, RuntimeHttpTransport, SkillRunRequest,
};
use serde_json::{Value, json};
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// How the store names a workspace's own memory package's streams.
const PACKAGE: &str = "project:memory";
/// Names the local model the evaluation runs on, in place of the scripted
/// reader.
const MODEL_ENV: &str = "RUNX_MEMORY_HORIZON_MODEL";
/// Where the report is written, when a run wants it kept.
const REPORT_ENV: &str = "RUNX_MEMORY_HORIZON_REPORT";
/// The local model server a named model is served by.
const LOCAL_MODEL: &str = "http://127.0.0.1:11434/v1/chat/completions";
/// The scripted reader's model name.
const SCRIPTED: &str = "scripted";

#[test]
fn memory_holds_across_the_horizon() -> TestResult {
    let horizon: Value = serde_json::from_str(&fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/memory/evals/horizon.json"),
    )?)?;
    let model_name = std::env::var(MODEL_ENV).unwrap_or_else(|_| SCRIPTED.to_owned());
    let repeats = horizon["repeats"][model_name.as_str()]
        .as_u64()
        .ok_or_else(|| format!("horizon.json names no repeats for {model_name}"))?;
    let today = today()?;
    let replies = Arc::new(Mutex::new(Replies::default()));
    let model = if model_name == SCRIPTED {
        ScriptedModel::serve(memory_reader)?
    } else {
        ScriptedModel::serve(relay(Arc::clone(&replies))?)?
    };
    let mut sizes = serde_json::Map::new();
    // The largest size runs on a real model, where the evaluation's threshold
    // is set; the scripted gate runs the rest on every bar.
    let every_size = horizon["sizes"]
        .as_array()
        .ok_or("horizon.json has sizes")?;
    let run_sizes = if model_name == SCRIPTED {
        &every_size[..every_size.len().saturating_sub(1)]
    } else {
        &every_size[..]
    };
    for size in run_sizes {
        let size = usize::try_from(size.as_u64().ok_or("a size is a count")?)?;
        let workspace = Workspace::new(model.endpoint(), &model_name)?;
        let planted = workspace.lay_down(&horizon, size, today)?;
        let mut modes = serde_json::Map::new();
        for focus in [false, true] {
            modes.insert(
                (if focus { "focus" } else { "plain" }).to_owned(),
                json!([workspace.sweep(1, repeats, focus)?]),
            );
        }
        workspace.correct(&horizon, &planted, today)?;
        for focus in [false, true] {
            let mode = if focus { "focus" } else { "plain" };
            let phase_two = workspace.sweep(2, repeats, focus)?;
            if let Some(Value::Array(phases)) = modes.get_mut(mode) {
                phases.push(phase_two);
            }
        }
        sizes.insert(size.to_string(), Value::Object(modes));
        if let Ok(path) = std::env::var(REPORT_ENV) {
            let partial = report(&horizon, &sizes, today, &model_name, repeats, &replies)?;
            fs::write(path, serde_json::to_string_pretty(&partial)?)?;
        }
    }
    model.finish()?;
    let report = report(&horizon, &sizes, today, &model_name, repeats, &replies)?;
    if let Ok(path) = std::env::var(REPORT_ENV) {
        fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    if model_name == SCRIPTED {
        retrieval_holds(&report)?;
    }
    Ok(())
}

/// The scripted reader's gate: every attempt answered and was judged, and
/// it recalls what a search for the question's words finds, at every size
/// and depth, plain and focused: early and recent facts, the current state
/// of a corrected entry, and a correction made between phases. As-of,
/// expiry, a standing rule, an injected page, capture and saying nothing is
/// found take judgement a reader of words hasn't got, so they're reported,
/// never gated.
fn retrieval_holds(report: &Value) -> TestResult {
    for (size, modes) in report["sizes"].as_object().ok_or("the report has sizes")? {
        for (mode, phases) in modes.as_object().ok_or("each size has modes")? {
            for attempt in phases
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|phase| phase["attempts"].as_array().cloned().unwrap_or_default())
            {
                if attempt["ending"] != "answered" || attempt["verdict"].is_null() {
                    return Err(format!(
                        "{size} {mode}: an attempt wasn't answered and judged: {attempt}"
                    )
                    .into());
                }
                let category = attempt["category"].as_str().unwrap_or_default();
                if [
                    "early_recall",
                    "recent_recall",
                    "current_state",
                    "correction",
                ]
                .contains(&category)
                    && attempt["verdict"] != "met"
                {
                    return Err(
                        format!("{size} {mode}: the reader missed {category}: {attempt}").into(),
                    );
                }
            }
        }
    }
    Ok(())
}

/// Today, by the system clock, as days since the epoch: every date the
/// history holds is a number of days before it.
fn today() -> TestResult<i64> {
    let now = runx_runtime::now_iso8601();
    let part = |range: std::ops::Range<usize>| -> TestResult<i64> {
        Ok(now.get(range).ok_or("the clock reads a date")?.parse()?)
    };
    Ok(days_from_civil(part(0..4)?, part(5..7)?, part(8..10)?))
}

/// Days since the epoch of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_cycle - 719_468
}

/// `YYYY-MM-DD` of `days` since the epoch.
fn date(days: i64) -> String {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_cycle = shifted - era * 146_097;
    let of_era = (of_cycle - of_cycle / 1460 + of_cycle / 36_524 - of_cycle / 146_096) / 365;
    let of_year = of_cycle - (365 * of_era + of_era / 4 - of_era / 100);
    let month_index = (5 * of_year + 2) / 153;
    let day = of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// An instant `seconds` before the start of day `today`.
fn instant(today: i64, seconds: i64) -> String {
    let at = today * 86_400 - seconds;
    let (days, second) = (at.div_euclid(86_400), at.rem_euclid(86_400));
    format!(
        "{}T{:02}:{:02}:{:02}.000Z",
        date(days),
        second / 3600,
        (second / 60) % 60,
        second % 60
    )
}

/// A seeded sequence, so the same seed lays down the same history.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<'a>(&mut self, items: &'a Value) -> &'a str {
        let items = items.as_array().map(Vec::as_slice).unwrap_or_default();
        let at = usize::try_from(self.next() % items.len().max(1) as u64).unwrap_or(0);
        items.get(at).and_then(Value::as_str).unwrap_or_default()
    }
}

/// One entry of a stream: where it sits, and the events that make it.
struct Slot {
    key: String,
    text: String,
    summary: String,
    purpose: Option<String>,
    days_ago: i64,
    valid_until: Option<String>,
    sources: Vec<String>,
    corrected: Option<Value>,
}

/// A workspace holding a copy of `runx/memory`, with a home of its own, its
/// turns worked by the model at `endpoint`.
struct Workspace {
    directory: TempDir,
    env: BTreeMap<String, String>,
}

impl Workspace {
    fn new(endpoint: &str, model: &str) -> TestResult<Self> {
        let directory = if let Ok(root) = std::env::var("RUNX_MEMORY_HORIZON_WORKSPACES") {
            fs::create_dir_all(&root)?;
            let mut directory = TempDir::new_in(root)?;
            directory.disable_cleanup(true);
            directory
        } else {
            TempDir::new()?
        };
        copy_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/memory"),
            &directory.path().join("skills/memory"),
        )?;
        let mut env = BTreeMap::from([
            (
                "RUNX_CWD".to_owned(),
                directory.path().to_string_lossy().into_owned(),
            ),
            (
                "RUNX_HOME".to_owned(),
                directory.path().join("home").to_string_lossy().into_owned(),
            ),
            // Nothing listens here, so no turn reaches a registry.
            (
                "RUNX_REGISTRY_URL".to_owned(),
                "http://127.0.0.1:9".to_owned(),
            ),
        ]);
        if model == SCRIPTED {
            env.extend(managed_agent_env(endpoint, "scripted-reader"));
        } else {
            env.extend([
                (
                    "RUNX_AGENT_PROVIDER".to_owned(),
                    std::env::var("RUNX_MEMORY_HORIZON_PROVIDER")
                        .unwrap_or_else(|_| "ollama".to_owned()),
                ),
                (
                    "RUNX_AGENT_DIALECT".to_owned(),
                    "chat_completions".to_owned(),
                ),
                ("RUNX_AGENT_MODEL".to_owned(), model.to_owned()),
                ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
                ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint.to_owned()),
            ]);
        }
        crate::support::insert_test_signing_env(&mut env);
        Ok(Self { directory, env })
    }

    /// `size` entries on each of `said:personal` and `observed:personal`,
    /// oldest first, the planted facts at their depths and their near misses
    /// beside them, every date days before `today`. The planted facts' keys,
    /// by id.
    fn lay_down(
        &self,
        horizon: &Value,
        size: usize,
        today: i64,
    ) -> TestResult<BTreeMap<String, String>> {
        let history = &horizon["history"];
        let span = history["span_days"]
            .as_i64()
            .ok_or("the history spans days")?;
        let mut seeded = Seeded(
            horizon["seed"]
                .as_u64()
                .ok_or("horizon.json has a seed")?
                .max(1),
        );
        let mut streams: BTreeMap<&str, Vec<Option<Slot>>> =
            BTreeMap::from([("said", Vec::new()), ("observed", Vec::new())]);
        for slots in streams.values_mut() {
            slots.resize_with(size, || None);
        }
        let position = |depth: &Value| -> usize {
            match depth.as_f64() {
                // Depth is how far from the newest, as a share of the stream.
                Some(share) => {
                    size - 1 - ((share * (size - 1) as f64).round() as usize).min(size - 1)
                }
                None => 0,
            }
        };
        let place = |slots: &mut Vec<Option<Slot>>, mut at: usize, slot: Slot| {
            while slots[at].is_some() {
                at = (at + 1) % slots.len();
            }
            slots[at] = Some(slot);
            at
        };
        let digest = |seed: &str| {
            runx_contracts::sha256_prefixed(seed.as_bytes())
                .trim_start_matches("sha256:")
                .to_owned()
        };
        let mut planted = BTreeMap::new();
        let mut depths = BTreeMap::new();
        for fact in horizon["planted"]
            .as_array()
            .ok_or("horizon.json plants facts")?
        {
            let id = fact["id"].as_str().ok_or("a planted fact has an id")?;
            let kind = fact["kind"].as_str().ok_or("a planted fact has a kind")?;
            let key = digest(&format!("planted {id}"));
            let slot = Slot {
                key: key.clone(),
                text: fact["text"].as_str().unwrap_or_default().to_owned(),
                summary: fact["summary"].as_str().unwrap_or_default().to_owned(),
                purpose: fact["purpose"].as_str().map(str::to_owned),
                days_ago: fact["days_ago"].as_i64().unwrap_or(0),
                valid_until: fact["ended_days_ago"]
                    .as_i64()
                    .map(|days| date(today - days)),
                sources: fact["sources"]
                    .as_array()
                    .map(|sources| {
                        sources
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["conversation".to_owned()]),
                corrected: fact.get("corrected").cloned(),
            };
            let slots = streams
                .get_mut(kind)
                .ok_or("a planted fact is said or observed")?;
            let at = place(slots, position(&fact["depth"]), slot);
            planted.insert(id.to_owned(), key);
            depths.insert(id.to_owned(), at);
        }
        for near in history["near_misses"].as_array().into_iter().flatten() {
            let target = near["planted"].as_str().unwrap_or_default();
            let at = depths.get(target).copied().unwrap_or(0);
            let slot = Slot {
                key: digest(&format!("near {target}")),
                text: near["text"].as_str().unwrap_or_default().to_owned(),
                summary: near["summary"].as_str().unwrap_or_default().to_owned(),
                purpose: None,
                days_ago: 0,
                valid_until: None,
                sources: vec!["fixture".to_owned()],
                corrected: None,
            };
            let slots = streams
                .get_mut(near["kind"].as_str().unwrap_or("observed"))
                .ok_or("a near miss is said or observed")?;
            place(slots, at, slot);
        }
        let filler = &history["filler"];
        let step = span * 86_400 / i64::try_from(size)?;
        let mut events = Vec::new();
        for (kind, slots) in &mut streams {
            for (at, slot) in slots.iter_mut().enumerate() {
                let behind = i64::try_from(size - at)? * step;
                let slot = slot.get_or_insert_with(|| {
                    let subject = seeded.pick(&filler["subjects"]).to_owned();
                    let fact = seeded
                        .pick(&filler["facts"])
                        .replace("{weekday}", seeded.pick(&filler["weekdays"]))
                        .replace("{name}", seeded.pick(&filler["names"]))
                        .replace("{number}", &(seeded.next() % 900 + 10).to_string());
                    let mut text = format!("{subject} {fact}.");
                    if let Some(first) = text.get_mut(0..1) {
                        first.make_ascii_uppercase();
                    }
                    Slot {
                        key: digest(&format!("{kind} {at}")),
                        summary: text.clone(),
                        text,
                        purpose: None,
                        days_ago: behind / 86_400,
                        valid_until: None,
                        sources: vec!["fixture".to_owned()],
                        corrected: None,
                    }
                });
                if slot.days_ago == 0 {
                    slot.days_ago = behind / 86_400;
                }
                events.push(event(kind, slot, instant(today, behind), today, None));
                if let Some(corrected) = &slot.corrected {
                    let days = corrected["days_ago"].as_i64().unwrap_or(0);
                    events.push(event(
                        kind,
                        slot,
                        instant(today, days * 86_400),
                        today,
                        Some(corrected),
                    ));
                }
            }
        }
        runx_runtime::probe_fixtures::lay_down_private_history(&self.env, &events)?;
        Ok(planted)
    }

    /// The horizon fixture's sweep of `phase`, under the scheduled
    /// delegation: a term reading the person's words and the observations,
    /// and one writing observations, each covering every package a call is
    /// made from: memory, the routine and the sweep.
    fn sweep(&self, phase: u64, repeats: u64, focus: bool) -> TestResult<Value> {
        let packages = [PACKAGE, "project:memory-routine", "project:memory-horizon"];
        let authority = json!({"delegations": [{"connector": "private", "terms": [
            {"verbs": ["read"], "bounds": {"operations": ["private/*"], "scopes": ["store.read"],
                "targets": [format!("{PACKAGE}/said:*"), format!("{PACKAGE}/observed:*")], "packages": packages}},
            {"verbs": ["write"], "bounds": {"operations": ["private/*"], "scopes": ["store.append"],
                "targets": [format!("{PACKAGE}/observed:*")], "packages": packages}},
        ]}]});
        let output = self.run(
            "skills/memory/fixtures/horizon",
            "sweep",
            json!({"phase": phase, "repeats": repeats, "focus": focus}),
            authority,
            None,
        )?;
        if output["outcome"] != "completed" {
            return Err(format!("the sweep of phase {phase} didn't complete: {output}").into());
        }
        Ok(output["result"]["report"].clone())
    }

    /// The phase's correction, through `memory#correct`, approved at its
    /// gate as a person approves it.
    fn correct(
        &self,
        horizon: &Value,
        planted: &BTreeMap<String, String>,
        today: i64,
    ) -> TestResult {
        let correct = horizon["phases"]
            .as_array()
            .and_then(|phases| phases.iter().find_map(|phase| phase.get("correct")))
            .ok_or("horizon.json corrects an entry between its phases")?;
        let id = correct["planted"]
            .as_str()
            .ok_or("a correction names what it corrects")?;
        let key = planted
            .get(id)
            .ok_or("a correction corrects a planted fact")?;
        let authority = json!({"delegations": [{"connector": "private", "terms": [
            {"verbs": ["read", "write"], "bounds": {"operations": ["private/*"], "scopes": ["store.read", "store.append"],
                "targets": [format!("{PACKAGE}/said:*")], "packages": [PACKAGE]}},
        ]}]});
        let inputs = json!({
            "handle": format!("memory:said:personal#{key}@1"),
            "text": correct["text"], "summary": correct["summary"], "purpose": "confirmed_memory",
            "date": date(today - correct["days_ago"].as_i64().unwrap_or(0)), "sources": ["conversation"],
        });
        let paused = self.run(
            "skills/memory",
            "correct",
            inputs.clone(),
            authority.clone(),
            None,
        )?;
        let run_id = paused["run_id"]
            .as_str()
            .ok_or_else(|| format!("the correction waits on its gate: {paused}"))?
            .to_owned();
        let answers = crate::support::bind_resume(
            &self.env,
            self.directory.path(),
            &run_id,
            json!({"approvals": {"memory.correct": true}}),
        )?;
        let resumed = self.run(
            "skills/memory",
            "correct",
            inputs,
            authority,
            Some((run_id, answers)),
        )?;
        if resumed["outcome"] != "completed" {
            return Err(format!("the correction didn't complete: {resumed}").into());
        }
        Ok(())
    }

    fn run(
        &self,
        skill: &str,
        method: &str,
        inputs: Value,
        authority: Value,
        resume: Option<(String, runx_contracts::ResumeDocument)>,
    ) -> TestResult<Value> {
        let orchestrator = LocalOrchestrator::with_effects_and_environment(
            RuntimeEffectRegistry::local()?,
            self.env.clone(),
        );
        let (run_id, answers) =
            resume.map_or((None, None), |(run, answers)| (Some(run), Some(answers)));
        let result = orchestrator.run_skill_with_binding(
            &SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: Some(serde_json::from_value(authority)?),
                act: None,
                skill_path: self.directory.path().join(skill),
                receipt_dir: None,
                run_id,
                answers,
                inputs: serde_json::from_value(inputs)?,
                env: self.env.clone(),
                cwd: self.directory.path().to_path_buf(),
                managed_agent: ManagedAgentPolicy::inline(8)?,
            },
            Some(method),
            None,
            None,
        )?;
        Ok(serde_json::to_value(&result.output)?)
    }
}

/// The store event that keeps `slot`'s entry on `kind`'s stream, or its
/// correction when `corrected` is given.
fn event(
    kind: &str,
    slot: &Slot,
    committed_at: String,
    today: i64,
    corrected: Option<&Value>,
) -> HistoryEvent {
    let (text, summary, days_ago) = match corrected {
        Some(correction) => (
            correction["text"].as_str().unwrap_or_default().to_owned(),
            correction["summary"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            correction["days_ago"].as_i64().unwrap_or(0),
        ),
        None => (slot.text.clone(), slot.summary.clone(), slot.days_ago),
    };
    let mut data = json!({"text": text, "summary": summary, "date": date(today - days_ago), "sources": slot.sources});
    let said = kind == "said";
    if said {
        data["purpose"] = json!(slot.purpose.as_deref().unwrap_or("confirmed_memory"));
        data["confirmation"] = json!({"approved": true});
    }
    HistoryEvent {
        stream: format!("{PACKAGE}/{kind}:personal"),
        key: slot.key.clone(),
        event_type: (match (said, corrected.is_some()) {
            (true, true) => "memory.corrected",
            (true, false) => "memory.remembered",
            (false, _) => "memory.observed",
        })
        .to_owned(),
        data: serde_json::from_value(data).unwrap_or_default(),
        standing: matches!(slot.purpose.as_deref(), Some("mandate" | "procedure")),
        valid_until: slot.valid_until.clone(),
        searchable: Some(vec!["text".to_owned(), "summary".to_owned()]),
        committed_at,
        writer_package: PACKAGE.to_owned(),
    }
}

/// What the relay saw of a named model's replies.
#[derive(Default)]
struct Replies {
    replies: u64,
    prose_only: u64,
}

/// A loopback relay to the local model server that counts the replies that
/// only wrote prose, calling no tool. Test code only.
fn relay(replies: Arc<Mutex<Replies>>) -> TestResult<impl FnMut(&Value) -> Value + Send + 'static> {
    let endpoint =
        std::env::var("RUNX_MEMORY_HORIZON_MODEL_URL").unwrap_or_else(|_| LOCAL_MODEL.to_owned());
    let transport = ReqwestHttpTransport::for_exact_loopback_agent(&endpoint)?;
    Ok(move |request: &Value| {
        let sent = transport.send(RuntimeHttpRequest {
            method: HttpMethod::Post,
            url: endpoint.clone(),
            headers: vec![RuntimeHttpHeader::new("content-type", "application/json")],
            body: Some(request.to_string()),
        });
        let reply = sent
            .ok()
            .and_then(|response| serde_json::from_str::<Value>(&response.body).ok())
            .unwrap_or_else(|| json!({"error": {"message": "the local model didn't answer"}}));
        if let Ok(mut seen) = replies.lock() {
            seen.replies += 1;
            let calls = &reply["choices"][0]["message"]["tool_calls"];
            if calls.as_array().is_none_or(Vec::is_empty) {
                seen.prose_only += 1;
            }
        }
        reply
    })
}

/// The report: what ran, then recall by depth, behaviour by category and
/// tokens per step, at each size and mode.
fn report(
    horizon: &Value,
    sizes: &serde_json::Map<String, Value>,
    today: i64,
    model: &str,
    repeats: u64,
    replies: &Mutex<Replies>,
) -> TestResult<Value> {
    let depth_of = |planted: &str| {
        horizon["planted"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|fact| fact["id"] == planted)
            .map(|fact| fact["depth"].clone())
            .unwrap_or(Value::Null)
    };
    let mut recall = serde_json::Map::new();
    let mut behaviour = serde_json::Map::new();
    let mut tokens = serde_json::Map::new();
    for (size, modes) in sizes {
        let (mut by_depth, mut by_category, mut by_mode_tokens) = (
            serde_json::Map::new(),
            serde_json::Map::new(),
            serde_json::Map::new(),
        );
        for (mode, phases) in modes.as_object().ok_or("each size has modes")? {
            let attempts = phases
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|phase| phase["attempts"].as_array().cloned().unwrap_or_default())
                .collect::<Vec<_>>();
            let mut depth_rows = BTreeMap::<String, (u64, u64)>::new();
            let mut category_rows = BTreeMap::<String, (u64, u64)>::new();
            let (mut steps, mut input, mut output, mut calls) = (0_u64, 0_u64, 0_u64, 0_u64);
            for attempt in &attempts {
                let met = u64::from(attempt["verdict"] == "met");
                let category = attempt["category"].as_str().unwrap_or("none").to_owned();
                let row = category_rows.entry(category.clone()).or_default();
                *row = (row.0 + met, row.1 + 1);
                if category.ends_with("recall") {
                    let depth =
                        depth_of(attempt["planted"].as_str().unwrap_or_default()).to_string();
                    let row = depth_rows.entry(depth).or_default();
                    *row = (row.0 + met, row.1 + 1);
                }
                let usage = &attempt["usage"];
                if let (Some(model_calls), Some(input_tokens), Some(output_tokens)) = (
                    usage["model_calls"].as_u64(),
                    usage["input_tokens"].as_u64(),
                    usage["output_tokens"].as_u64(),
                ) {
                    steps += 1;
                    calls += model_calls;
                    input += input_tokens;
                    output += output_tokens;
                }
            }
            let rate = |(met, of): (u64, u64)| json!({"met": met, "attempts": of});
            by_depth.insert(
                mode.clone(),
                json!(
                    depth_rows
                        .into_iter()
                        .map(|(depth, row)| (depth, rate(row)))
                        .collect::<BTreeMap<_, _>>()
                ),
            );
            by_category.insert(
                mode.clone(),
                json!(
                    category_rows
                        .into_iter()
                        .map(|(category, row)| (category, rate(row)))
                        .collect::<BTreeMap<_, _>>()
                ),
            );
            by_mode_tokens.insert(
                mode.clone(),
                if steps == 0 {
                    json!({"steps_reporting_usage": 0})
                } else {
                    json!({"steps_reporting_usage": steps, "model_calls_per_step": calls as f64 / steps as f64,
                        "input_tokens_per_step": input as f64 / steps as f64, "output_tokens_per_step": output as f64 / steps as f64})
                },
            );
        }
        recall.insert(size.clone(), Value::Object(by_depth));
        behaviour.insert(size.clone(), Value::Object(by_category));
        tokens.insert(size.clone(), Value::Object(by_mode_tokens));
    }
    let mut hosts = sizes
        .values()
        .filter_map(Value::as_object)
        .flat_map(|modes| modes.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|phase| phase["attempts"].as_array())
        .flatten()
        .map(|attempt| attempt["host"].clone())
        .collect::<Vec<_>>();
    hosts.sort_by_key(Value::to_string);
    hosts.dedup();
    let seen = replies
        .lock()
        .map_err(|_| "the relay's count is poisoned")?;
    Ok(json!({
        "run_date": date(today),
        "seed": horizon["seed"],
        "model": model,
        "hosts": hosts,
        "repeats": repeats,
        "recall_by_depth": recall,
        "behaviour": behaviour,
        "tokens": tokens,
        "model_replies": seen.replies,
        "prose_only_replies": seen.prose_only,
        "sizes": sizes,
    }))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
