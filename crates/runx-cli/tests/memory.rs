//! `runx/memory` across calls, as a host drives it: what's kept, what an
//! index and a search give a turn, and what a correction or a disable changes
//! (memory.md, §9's memory row). Each test keeps entries in its own home and
//! reads them back from the CLI.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A workspace holding a copy of the package, and a home of its own.
struct Memory {
    root: PathBuf,
}

/// The run's authority over memory's streams: `targets` on `private`, read
/// and written, from the package as a workspace or the runx home binds it.
fn authority(targets: &[&str]) -> String {
    let targets = targets
        .iter()
        .flat_map(|target| {
            [
                format!("project:memory/{target}"),
                format!("runx/memory/{target}"),
            ]
        })
        .collect::<Vec<_>>();
    json!({"delegations": [{"connector": "private", "terms": [{
        "verbs": ["read", "write"],
        "bounds": {
            "operations": ["private/*"],
            "scopes": ["store.read", "store.append"],
            "targets": targets,
            "packages": ["project:memory", "runx/memory"],
        },
    }]}]})
    .to_string()
}

const EVERY_STREAM: &[&str] = &["said:*", "observed:*"];

impl Memory {
    fn new(name: &str) -> TestResult<Self> {
        let root = crate::support::temp_root(name);
        fs::create_dir_all(root.join("home"))?;
        let package = crate::support::repo_root()?.join("skills/memory");
        let copy = root.join("skills/memory");
        for entry in walk(&package)? {
            let relative = entry.strip_prefix(&package)?;
            fs::create_dir_all(
                copy.join(relative)
                    .parent()
                    .ok_or("a file has a directory")?,
            )?;
            fs::copy(&entry, copy.join(relative))?;
        }
        Ok(Self { root })
    }

    fn runx(&self, args: &[&str]) -> TestResult<Output> {
        Ok(crate::support::unsigned_runx_command_at(&self.root)
            .env("RUNX_CWD", &self.root)
            .args(args)
            .output()?)
    }

    /// `method` run with `inputs` under `targets`, its JSON output.
    fn run(&self, method: &str, inputs: Value, targets: &[&str]) -> TestResult<Value> {
        let file = format!("inputs-{method}.json");
        fs::write(self.root.join(&file), inputs.to_string())?;
        let output = self.runx(&[
            "skill",
            &format!("skills/memory#{method}"),
            "--inputs",
            &file,
            "--run-authority",
            &authority(targets),
            "--json",
        ])?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn completed(&self, method: &str, inputs: Value) -> TestResult<Value> {
        let ran = self.run(method, inputs, EVERY_STREAM)?;
        assert_eq!(ran["outcome"], "completed", "{ran}");
        Ok(ran["result"].clone())
    }

    /// The person's words, kept once a resume approves them.
    fn remember(&self, inputs: Value) -> TestResult<String> {
        Ok(self.remember_shown(inputs)?.0)
    }

    /// The person's words, kept once a resume approves them, and what the
    /// gate showed the person beside its reason.
    fn remember_shown(&self, inputs: Value) -> TestResult<(String, Value)> {
        let paused = self.run("remember", inputs, EVERY_STREAM)?;
        assert_eq!(paused["status"], "needs_approval", "{paused}");
        let run_id = paused["run_id"].as_str().ok_or("no run id")?.to_owned();
        let read: Value =
            serde_json::from_slice(&self.runx(&["history", &run_id, "--json"])?.stdout)?;
        let shown = gate_summary(&read, "memory.remember")?;
        fs::write(
            self.root.join("approve.json"),
            crate::support::bind_resume(&read, json!({"approvals": {"memory.remember": true}}))?
                .to_string(),
        )?;
        let resumed: Value = serde_json::from_slice(
            &self
                .runx(&["resume", &run_id, "approve.json", "--json"])?
                .stdout,
        )?;
        assert_eq!(resumed["outcome"], "completed", "{resumed}");
        Ok((
            resumed["result"]["handle"]
                .as_str()
                .ok_or("no handle")?
                .to_owned(),
            shown,
        ))
    }

    /// A change a gate decides, approved on resume.
    fn approved(&self, method: &str, gate: &str, inputs: Value) -> TestResult<Value> {
        let paused = self.run(method, inputs, EVERY_STREAM)?;
        assert_eq!(paused["status"], "needs_approval", "{paused}");
        let run_id = paused["run_id"].as_str().ok_or("no run id")?.to_owned();
        let read: Value =
            serde_json::from_slice(&self.runx(&["history", &run_id, "--json"])?.stdout)?;
        fs::write(
            self.root.join("approve.json"),
            crate::support::bind_resume(&read, json!({"approvals": {gate: true}}))?.to_string(),
        )?;
        let resumed: Value = serde_json::from_slice(
            &self
                .runx(&["resume", &run_id, "approve.json", "--json"])?
                .stdout,
        )?;
        assert_eq!(resumed["outcome"], "completed", "{resumed}");
        Ok(resumed["result"].clone())
    }

    fn observe(&self, text: &str, date: &str) -> TestResult<String> {
        self.observe_with(observation(text, date))
    }

    fn observe_with(&self, inputs: Value) -> TestResult<String> {
        let kept = self.completed("observe", inputs)?;
        Ok(kept["handle"].as_str().ok_or("no handle")?.to_owned())
    }

    fn index(&self, max_characters: Option<u64>) -> TestResult<(Vec<Value>, bool)> {
        let (entries, omitted, _) = self.index_with(json!({}), max_characters)?;
        Ok((entries, omitted))
    }

    /// The index with `extra` inputs, its entries, whether it cut any, and
    /// whether the standing rules alone passed its budget.
    fn index_with(
        &self,
        extra: Value,
        max_characters: Option<u64>,
    ) -> TestResult<(Vec<Value>, bool, bool)> {
        let mut inputs = json!({"levels": [{"level": "personal"}]});
        if let Some(max) = max_characters {
            inputs["max_characters"] = json!(max);
        }
        for (name, value) in extra.as_object().ok_or("inputs are an object")? {
            inputs[name] = value.clone();
        }
        let result = self.completed("index", inputs)?;
        Ok((
            entries(&result["entries"])?,
            result["entries_omitted"] == true,
            result["over_budget"] == true,
        ))
    }

    /// What the store counted while the index of the personal streams ran,
    /// from a process that ran it alone.
    fn counted_index(&self) -> TestResult<Value> {
        fs::write(
            self.root.join("inputs-index.json"),
            json!({"levels": [{"level": "personal"}]}).to_string(),
        )?;
        let counts = self.root.join("counts.json");
        let output = crate::support::unsigned_runx_command_at(&self.root)
            .env("RUNX_CWD", &self.root)
            .env("RUNX_PERF_COUNTS_PATH", &counts)
            .args([
                "skill",
                "skills/memory#index",
                "--inputs",
                "inputs-index.json",
                "--run-authority",
                &authority(EVERY_STREAM),
                "--json",
            ])
            .output()?;
        let ran: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(ran["outcome"], "completed", "{ran}");
        Ok(serde_json::from_slice(&fs::read(&counts)?)?)
    }

    fn search(&self, query: &str) -> TestResult<Vec<Value>> {
        let result = self.completed(
            "search",
            json!({"levels": [{"level": "personal"}], "query": query}),
        )?;
        entries(&result["entries"])
    }
}

fn walk(directory: &Path) -> TestResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(walk(&path)?);
        } else {
            files.push(path);
        }
    }
    Ok(files)
}

/// A packet output's entries, as the run's result holds them.
fn entries(value: &Value) -> TestResult<Vec<Value>> {
    let list = value.get("data").unwrap_or(value);
    Ok(list
        .as_array()
        .ok_or_else(|| format!("no entries in {value}"))?
        .clone())
}

fn handles(entries: &[Value]) -> Vec<&str> {
    entries
        .iter()
        .filter_map(|entry| entry["data"]["handle"].as_str())
        .collect()
}

/// The person's words as they gave them, plain memory, shown by the line
/// `summary`.
fn words(text: &str) -> Value {
    said(text, text, "confirmed_memory")
}

/// The person's words with the line the index shows for them and their
/// purpose.
fn said(text: &str, summary: &str, purpose: &str) -> Value {
    json!({"level": "personal", "text": text, "summary": summary, "purpose": purpose, "date": "2026-11-08", "sources": ["conversation"]})
}

/// An observation dated `date`, shown by its own text.
fn observation(text: &str, date: &str) -> Value {
    json!({"level": "personal", "text": text, "summary": text, "date": date, "sources": ["fixture"]})
}

/// Each line of an index, as a turn reads it.
fn lines(entries: &[Value]) -> Vec<&str> {
    entries
        .iter()
        .filter_map(|entry| entry["summary"].as_str())
        .collect()
}

/// What the gate `gate` a run waits on shows beside its reason, as the run's
/// read holds it.
fn gate_summary(read: &Value, gate: &str) -> TestResult<Value> {
    let mut found = None;
    let mut stack = vec![read];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(object) => {
                if object.get("id").and_then(Value::as_str) == Some(gate)
                    && let Some(summary) = object.get("summary")
                {
                    found = Some(summary.clone());
                }
                stack.extend(object.values());
            }
            Value::Array(items) => stack.extend(items),
            _ => {}
        }
    }
    found.ok_or_else(|| format!("no gate {gate} in {read}").into())
}

/// An index line is its entry's handle, its date and the summary its writer
/// wrote for recall, and a person's words rank above an observation, even one
/// kept after them.
#[test]
fn the_index_puts_the_persons_words_first_each_line_dated() -> TestResult {
    let memory = Memory::new("runx-memory-index")?;
    let said = memory.remember(said(
        "I take my tea black, no sugar, and never before nine in the morning.",
        "How I take my tea",
        "confirmed_memory",
    ))?;
    let observed = memory.observe("The build runs on Node 26.", "2026-11-09")?;
    let (index, omitted) = memory.index(None)?;
    assert!(!omitted);
    assert_eq!(handles(&index), [said.as_str(), observed.as_str()]);
    assert_eq!(
        index[0]["summary"],
        format!("{said} 2026-11-08 How I take my tea")
    );
    assert_eq!(
        index[0]["data"]["text"],
        "I take my tea black, no sugar, and never before nine in the morning."
    );
    assert_eq!(index[0]["labels"]["purpose"], "confirmed_memory");
    assert_eq!(index[0]["labels"]["trust"], "person_confirmed");
    assert_eq!(index[1]["labels"]["purpose"], "observation");
    assert_eq!(index[1]["labels"]["trust"], "untrusted_evidence");
    for entry in &index {
        assert_eq!(entry["labels"]["sharing"], "configured_model");
        assert!(entry["origin"].get("run_id").is_none(), "{entry}");
    }
    Ok(())
}

/// A date in an entry's data can't reorder the index: it's the fact's date,
/// and the index orders by when each was kept.
#[test]
fn an_entrys_date_never_reorders_the_index() -> TestResult {
    let memory = Memory::new("runx-memory-dates")?;
    let first = memory.observe("Kept first, dated far ahead.", "2099-01-01")?;
    let second = memory.observe("Kept second, dated long ago.", "1999-01-01")?;
    let (index, _) = memory.index(None)?;
    assert_eq!(handles(&index), [second.as_str(), first.as_str()]);
    Ok(())
}

/// A correction is the entry's next version: search finds only the corrected
/// words, even for a query the old words match best, and reading history is
/// explicit.
#[test]
fn search_finds_the_corrected_words_and_read_reaches_the_old() -> TestResult {
    let memory = Memory::new("runx-memory-correct")?;
    let kept = memory.remember(words("I take my tea black."))?;
    let corrected = memory.approved(
        "correct",
        "memory.correct",
        json!({"handle": kept, "text": "I take my coffee with milk.", "summary": "I take my coffee with milk.", "purpose": "confirmed_memory", "date": "2026-11-09", "sources": ["conversation"]}),
    )?;
    let corrected = corrected["handle"].as_str().ok_or("no handle")?;
    assert!(corrected.ends_with("@2"), "{corrected}");
    assert!(memory.search("tea black")?.is_empty());
    assert_eq!(handles(&memory.search("coffee")?), [corrected]);
    let read = memory.completed("read", json!({"handle": kept}))?;
    // The entry, in the envelope its packet output arrives in.
    assert_eq!(
        read["entry"]["data"]["data"]["text"],
        "I take my tea black."
    );
    Ok(())
}

/// A disabled entry stops applying: the index and search leave it out, and
/// the record keeps it.
#[test]
fn a_disabled_entry_leaves_the_index_and_search() -> TestResult {
    let memory = Memory::new("runx-memory-disable")?;
    let kept = memory.remember(words("I take my tea black."))?;
    let disabled = memory.approved(
        "disable",
        "memory.disable",
        json!({"handle": kept, "reason": "No longer true."}),
    )?;
    assert!(
        disabled["handle"]
            .as_str()
            .is_some_and(|handle| handle.ends_with("@2"))
    );
    assert!(memory.index(None)?.0.is_empty());
    assert!(memory.search("tea")?.is_empty());
    let read = memory.completed("read", json!({"handle": kept}))?;
    // The entry, in the envelope its packet output arrives in.
    assert_eq!(
        read["entry"]["data"]["data"]["text"],
        "I take my tea black."
    );
    Ok(())
}

/// Past its budget the index says it cut entries, lines lose their preview
/// before any goes, and search still finds a cut entry.
#[test]
fn a_cut_index_says_so_and_search_still_finds_what_it_cut() -> TestResult {
    let memory = Memory::new("runx-memory-budget")?;
    let oldest = memory.observe(
        "The oldest observation mentions a lighthouse.",
        "2026-11-01",
    )?;
    for day in 2..=6 {
        memory.observe(
            &format!("Observation from day {day}."),
            &format!("2026-11-0{day}"),
        )?;
    }
    let (index, omitted) = memory.index(Some(256))?;
    assert!(omitted);
    assert!(!index.is_empty());
    assert!(!handles(&index).contains(&oldest.as_str()));
    let total = index
        .iter()
        .map(|entry| {
            entry["summary"]
                .as_str()
                .map_or(0, |line| line.chars().count())
        })
        .sum::<usize>()
        + index.len().saturating_sub(1);
    assert!(total <= 256, "{total}");
    assert_eq!(handles(&memory.search("lighthouse")?), [oldest.as_str()]);
    Ok(())
}

/// Two brands in one home don't mix, and a term on one can't read the other.
#[test]
fn a_term_on_one_brand_reads_nothing_of_another() -> TestResult {
    let memory = Memory::new("runx-memory-brands")?;
    let brand = |name: &str, text: &str| {
        memory.completed(
            "observe",
            json!({"level": "brand", "brand": name, "text": text, "summary": text, "date": "2026-11-09", "sources": ["fixture"]}),
        )
    };
    let nitrosend = brand("nitrosend", "Nitrosend sends from its own domain.")?;
    brand("sourcey", "Sourcey publishes reports weekly.")?;
    let handle = nitrosend["handle"].as_str().ok_or("no handle")?;
    // The stream a handle names, which carries the workspace's id.
    let stream = handle
        .strip_prefix("memory:")
        .and_then(|rest| rest.split_once('#'))
        .map(|(stream, _)| stream)
        .ok_or("a handle names its stream")?;
    assert!(
        stream.starts_with("observed:brand:ws_") && stream.ends_with(":nitrosend"),
        "{stream}"
    );
    let levels =
        json!([{"level": "brand", "brand": "nitrosend"}, {"level": "brand", "brand": "sourcey"}]);
    let read = memory.run("index", json!({"levels": levels}), &[stream])?;
    assert_eq!(read["outcome"], "completed", "{read}");
    let index = entries(&read["result"]["entries"])?;
    assert_eq!(handles(&index), [handle]);
    Ok(())
}

/// The index's listing steps every row it visits and counts it, the
/// disabled entries it passes over included, so its rows visited are the
/// rows its query plan walks (journey 5).
#[test]
fn the_index_counts_every_row_it_visits_the_ones_it_passes_over_included() -> TestResult {
    let memory = Memory::new("runx-memory-rows")?;
    memory.observe("The first observation.", "2026-11-01")?;
    let second = memory.observe("The second observation.", "2026-11-02")?;
    memory.observe("The third observation.", "2026-11-03")?;
    memory.approved(
        "disable",
        "memory.disable",
        json!({"handle": second, "reason": "No longer true."}),
    )?;
    assert_eq!(memory.index(None)?.0.len(), 2);
    let counted = memory.counted_index()?;
    assert_eq!(counted["store_key_rows_visited"], 3, "{counted}");
    assert_eq!(counted["store_key_rows_passed_over"], 1, "{counted}");
    Ok(())
}

/// A contact memory already holds is drafted as known: cold-outreach's
/// prepare searches memory for its recipient at the levels its caller
/// names, under a term that reads memory, and its draft is given what memory
/// observed, labelled by memory's own run. Named no levels, it searches
/// nothing and its draft is given none, as before (plans 390a69f).
#[test]
fn a_known_contact_is_drafted_as_known() -> TestResult {
    let memory = Memory::new("runx-memory-outreach")?;
    let observed = "Morgan at Example SaaS asked to hear back after their launch in December.";
    let term = |packages: &[&str]| {
        json!({"delegations": [{"connector": "private", "terms": [{
            "verbs": ["read", "write"],
            "bounds": {
                "operations": ["private/*"],
                "scopes": ["store.read", "store.append"],
                "targets": ["runx/memory/said:*", "runx/memory/observed:*"],
                "packages": packages,
            },
        }]}]})
        .to_string()
    };
    fs::write(
        memory.root.join("observe.json"),
        json!({"level": "personal", "text": observed, "summary": "Morgan asked to hear back after their December launch", "date": "2026-10-20", "sources": ["crm:morgan"]})
            .to_string(),
    )?;
    let kept: Value = serde_json::from_slice(
        &memory
            .runx(&[
                "skill",
                "runx/memory#observe",
                "--inputs",
                "observe.json",
                "--run-authority",
                &term(&["runx/memory"]),
                "--json",
            ])?
            .stdout,
    )?;
    assert_eq!(kept["outcome"], "completed", "{kept}");

    let prepare = format!(
        "{}#prepare",
        crate::support::repo_root()?
            .join("skills/cold-outreach")
            .display()
    );
    // The request cold-outreach's draft waits on, with `levels` named and
    // the run under `authority`.
    let draft = |levels: Option<Value>, authority: Option<String>| -> TestResult<String> {
        let mut inputs = json!({
            "sender": {"name": "Example Ops", "from_address": "ops@example.com", "reply_to": "ops@example.com"},
            "prospect": {"email": "morgan@example.org", "name": "Morgan", "company": "Example SaaS"},
            "audience": "engineering maintainers",
            "purpose": "Offer a relevant release-automation review.",
            "offer": "A source-bound build notification workflow.",
            "required_footer": "Example Ops - reply to opt out.",
            "contact_policy": {"recipient": "morgan@example.org", "status": "eligible", "suppressed": false, "policy_id": "fixture:demo", "evidence_ids": ["fixture:explicit-demo-consent"]},
            "facts": ["The fictional prospect is evaluating release automation."],
        });
        if let Some(levels) = levels {
            inputs["memory"] = levels;
        }
        fs::write(memory.root.join("prepare.json"), inputs.to_string())?;
        let mut args = vec![
            "skill",
            prepare.as_str(),
            "--inputs",
            "prepare.json",
            "--json",
        ];
        if let Some(authority) = authority.as_deref() {
            args.extend(["--run-authority", authority]);
        }
        let paused: Value = serde_json::from_slice(&memory.runx(&args)?.stdout)?;
        assert_eq!(paused["status"], "needs_agent", "{paused}");
        let request = paused["artifacts"][0]["path"]
            .as_str()
            .ok_or("the draft's request")?;
        Ok(fs::read_to_string(request)?)
    };
    let known = draft(
        Some(json!([{"level": "personal"}])),
        Some(term(&["runx/memory", "project:cold-outreach"])),
    )?;
    assert!(known.contains(observed), "{known}");
    assert!(
        known.contains(r#""trust":"untrusted_evidence""#),
        "memory's own label: {known}"
    );
    let unasked = draft(None, None)?;
    assert!(!unasked.contains(observed), "{unasked}");
    Ok(())
}

/// The person's standing rules, their words kept as a mandate or a
/// procedure, come first, every one, oldest first, whatever their age, and
/// read to a turn with that purpose. A rule disabled, or corrected into plain
/// memory, leaves them.
#[test]
fn standing_rules_come_first_oldest_first_whatever_their_age() -> TestResult {
    let memory = Memory::new("runx-memory-rules")?;
    let mandate = memory.remember(said(
        "Always copy Sam on anything sent to a customer.",
        "Copy Sam on customer mail",
        "mandate",
    ))?;
    let fact = memory.remember(words("I take my tea black."))?;
    let procedure = memory.remember(said(
        "To ship, tag the release, then post the notes.",
        "How to ship",
        "procedure",
    ))?;
    let observed = memory.observe("The build runs on Node 26.", "2026-11-09")?;
    let (index, omitted) = memory.index(None)?;
    assert!(!omitted);
    assert_eq!(
        handles(&index),
        [
            mandate.as_str(),
            procedure.as_str(),
            fact.as_str(),
            observed.as_str()
        ]
    );
    assert_eq!(index[0]["labels"]["purpose"], "mandate");
    assert_eq!(index[1]["labels"]["purpose"], "procedure");
    assert_eq!(index[2]["labels"]["purpose"], "confirmed_memory");
    for entry in &index[..3] {
        assert_eq!(entry["labels"]["trust"], "person_confirmed");
    }
    // A correction into plain memory moves the rule among the facts, newest
    // first, and a disabled rule is gone.
    let corrected = memory.approved(
        "correct",
        "memory.correct",
        json!({"handle": mandate, "text": "Copy Sam on customer mail when you can.", "summary": "Copy Sam when you can", "purpose": "confirmed_memory", "date": "2026-11-09", "sources": ["conversation"]}),
    )?;
    let corrected = corrected["handle"].as_str().ok_or("no handle")?.to_owned();
    memory.approved(
        "disable",
        "memory.disable",
        json!({"handle": procedure, "reason": "We ship differently now."}),
    )?;
    let (index, _) = memory.index(None)?;
    assert_eq!(
        handles(&index),
        [corrected.as_str(), fact.as_str(), observed.as_str()]
    );
    assert_eq!(index[0]["labels"]["purpose"], "confirmed_memory");
    Ok(())
}

/// Past its budget, lines lose their summary from the bottom up, the
/// lowest-ranked fact's first and the standing rules' last, and a fact's
/// line goes only once every summary has. A standing rule's line never goes:
/// when the rules alone pass the budget, every one shows, bare, and the index
/// says so.
#[test]
fn a_budget_cuts_summaries_from_the_bottom_and_never_a_rule() -> TestResult {
    let memory = Memory::new("runx-memory-rule-budget")?;
    let mut rules = Vec::new();
    for (index, text) in ["Copy Sam on customer mail.", "Sign off as Kam."]
        .into_iter()
        .enumerate()
    {
        rules.push(memory.remember(said(text, &format!("Rule {index}"), "mandate"))?);
    }
    let mut facts = Vec::new();
    for day in 1..=4 {
        facts.push(memory.observe(
            &format!("Observation from day {day}."),
            &format!("2026-11-0{day}"),
        )?);
    }
    // Every line whole, then less room: the oldest fact's summary goes
    // first, and the rules keep theirs.
    let (whole, omitted, over) = memory.index_with(json!({}), Some(4096))?;
    assert!(!omitted && !over);
    let full = lines(&whole)
        .iter()
        .map(|line| line.chars().count())
        .sum::<usize>()
        + whole.len()
        - 1;
    let (cut, omitted, over) = memory.index_with(json!({}), Some(full as u64 - 1))?;
    assert!(!omitted && !over);
    assert_eq!(handles(&cut), handles(&whole));
    let oldest = facts[0].as_str();
    let bare = |entry: &Value| {
        entry["summary"]
            .as_str()
            .is_some_and(|line| line.ends_with(entry["data"]["date"].as_str().unwrap_or("none")))
    };
    for entry in &cut {
        assert_eq!(
            bare(entry),
            entry["data"]["handle"] == oldest,
            "only the lowest-ranked fact's summary goes first: {entry}"
        );
    }
    // With room only for the rules' bare lines, every fact goes and the
    // rules lose their summaries, but no rule's line goes.
    let rule_lines = rules
        .iter()
        .map(|handle| handle.chars().count() + " 2026-11-08".len())
        .sum::<usize>()
        + rules.len()
        - 1;
    let (only_rules, omitted, over) =
        memory.index_with(json!({}), Some(rule_lines.max(256) as u64))?;
    assert!(omitted);
    assert!(!over);
    assert_eq!(handles(&only_rules), [rules[0].as_str(), rules[1].as_str()]);
    // Rules past the budget: every one shows, and the index says so.
    for index in 2..5 {
        rules.push(memory.remember(said(
            &format!("Standing rule number {index}."),
            &format!("Rule {index}"),
            "procedure",
        ))?);
    }
    let (over_budget, omitted, over) = memory.index_with(json!({}), Some(256))?;
    assert!(over, "the rules alone pass 256 characters");
    assert!(omitted);
    assert_eq!(
        handles(&over_budget),
        rules.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert!(over_budget.iter().all(bare), "{over_budget:?}");
    Ok(())
}

/// An entry whose last valid date has passed by the run's clock leaves the
/// index, passed over by the store's listing and counted as it; search and
/// read still have it. One that ends later stays.
#[test]
fn an_entry_past_its_end_date_leaves_the_index_and_search_still_finds_it() -> TestResult {
    let memory = Memory::new("runx-memory-ended")?;
    let ended = memory.observe_with(json!({
        "level": "personal", "text": "The lighthouse tour runs on Tuesdays.",
        "summary": "Lighthouse tour days", "date": "1999-06-01",
        "valid_until": "2000-01-01", "sources": ["fixture"],
    }))?;
    let rule = memory.remember(json!({
        "level": "personal", "text": "Bring a coat to the lighthouse.",
        "summary": "Coat for the lighthouse", "purpose": "mandate",
        "date": "1999-06-01", "valid_until": "2000-01-01", "sources": ["conversation"],
    }))?;
    let later = memory.observe_with(json!({
        "level": "personal", "text": "The ferry runs hourly.",
        "summary": "Ferry times", "date": "2026-11-09",
        "valid_until": "2999-12-31", "sources": ["fixture"],
    }))?;
    let (index, _) = memory.index(None)?;
    assert_eq!(handles(&index), [later.as_str()]);
    assert_eq!(index[0]["data"]["valid_until"], "2999-12-31");
    let mut found = handles(&memory.search("lighthouse")?)
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    found.sort();
    let mut expected = vec![ended.clone(), rule];
    expected.sort();
    assert_eq!(found, expected);
    let read = memory.completed("read", json!({"handle": ended}))?;
    assert_eq!(read["entry"]["data"]["data"]["valid_until"], "2000-01-01");
    // The listing visited the ended observation and passed over it.
    let counted = memory.counted_index()?;
    assert_eq!(counted["store_key_rows_passed_over"], 2, "{counted}");
    Ok(())
}

/// With a `focus`, the facts that match it come first, ranked by the search,
/// equal scores newest first, and the rest follow newest first. The standing
/// rules stay first, as they are.
#[test]
fn a_focus_puts_the_facts_that_match_it_first() -> TestResult {
    let memory = Memory::new("runx-memory-focus")?;
    let rule = memory.remember(said("Sign off as Kam.", "Sign-off", "mandate"))?;
    let old_match = memory.observe("The invoice goes to accounts.", "2026-11-01")?;
    let other = memory.observe("The build runs on Node 26.", "2026-11-02")?;
    let new_match = memory.observe("The invoice is due Friday.", "2026-11-03")?;
    let newest = memory.observe("The office closes at six.", "2026-11-04")?;
    let (focused, _, _) = memory.index_with(json!({"focus": "invoice"}), None)?;
    assert_eq!(
        handles(&focused),
        [
            rule.as_str(),
            new_match.as_str(),
            old_match.as_str(),
            newest.as_str(),
            other.as_str()
        ]
    );
    let (unfocused, _, _) = memory.index_with(json!({}), None)?;
    assert_eq!(
        handles(&unfocused),
        [
            rule.as_str(),
            newest.as_str(),
            new_match.as_str(),
            other.as_str(),
            old_match.as_str()
        ]
    );
    Ok(())
}

/// The confirmation binds what's kept and how it shows: the gate shows the
/// person the quote, the line, the purpose, the date, the end date and the
/// sources, and the stream it goes to.
#[test]
fn the_persons_words_gate_shows_the_line_and_the_purpose() -> TestResult {
    let memory = Memory::new("runx-memory-gate")?;
    let (_, shown) = memory.remember_shown(json!({
        "level": "personal", "text": "Always copy Sam on customer mail until the launch.",
        "summary": "Copy Sam on customer mail", "purpose": "mandate",
        "date": "2026-11-08", "valid_until": "2026-12-01", "sources": ["conversation"],
    }))?;
    let mut shown = shown;
    // The read names the digest of what the gate showed beside it.
    shown
        .as_object_mut()
        .ok_or("the gate's summary is an object")?
        .remove("payload_digest");
    assert_eq!(
        shown,
        json!({
            "quote": "Always copy Sam on customer mail until the launch.",
            "summary": "Copy Sam on customer mail",
            "purpose": "mandate",
            "date": "2026-11-08",
            "valid_until": "2026-12-01",
            "sources": ["conversation"],
            "stream": "said:personal",
        })
    );
    Ok(())
}
