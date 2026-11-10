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
        let paused = self.run("remember", inputs, EVERY_STREAM)?;
        assert_eq!(paused["status"], "needs_approval", "{paused}");
        let run_id = paused["run_id"].as_str().ok_or("no run id")?.to_owned();
        let read: Value =
            serde_json::from_slice(&self.runx(&["history", &run_id, "--json"])?.stdout)?;
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
        Ok(resumed["result"]["handle"]
            .as_str()
            .ok_or("no handle")?
            .to_owned())
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
        let kept = self.completed(
            "observe",
            json!({"level": "personal", "text": text, "date": date, "sources": ["fixture"]}),
        )?;
        Ok(kept["handle"].as_str().ok_or("no handle")?.to_owned())
    }

    fn index(&self, max_characters: Option<u64>) -> TestResult<(Vec<Value>, bool)> {
        let mut inputs = json!({"levels": [{"level": "personal"}]});
        if let Some(max) = max_characters {
            inputs["max_characters"] = json!(max);
        }
        let result = self.completed("index", inputs)?;
        Ok((
            entries(&result["entries"])?,
            result["entries_omitted"] == true,
        ))
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

fn words(text: &str) -> Value {
    json!({"level": "personal", "text": text, "date": "2026-11-08", "sources": ["conversation"]})
}

/// An index line shows its entry's date, and a person's words rank above an
/// observation, even one kept after them.
#[test]
fn the_index_puts_the_persons_words_first_each_line_dated() -> TestResult {
    let memory = Memory::new("runx-memory-index")?;
    let said = memory.remember(words("I take my tea black."))?;
    let observed = memory.observe("The build runs on Node 26.", "2026-11-09")?;
    let (index, omitted) = memory.index(None)?;
    assert!(!omitted);
    assert_eq!(handles(&index), [said.as_str(), observed.as_str()]);
    assert_eq!(
        index[0]["summary"],
        format!("{said} 2026-11-08 I take my tea black.")
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
        json!({"handle": kept, "text": "I take my coffee with milk.", "date": "2026-11-09", "sources": ["conversation"]}),
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
            json!({"level": "brand", "brand": name, "text": text, "date": "2026-11-09", "sources": ["fixture"]}),
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
    fs::write(
        memory.root.join("inputs-index.json"),
        json!({"levels": [{"level": "personal"}]}).to_string(),
    )?;
    let counts = memory.root.join("counts.json");
    let output = crate::support::unsigned_runx_command_at(&memory.root)
        .env("RUNX_CWD", &memory.root)
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
    assert_eq!(entries(&ran["result"]["entries"])?.len(), 2, "{ran}");
    let counted: Value = serde_json::from_slice(&fs::read(&counts)?)?;
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
        json!({"level": "personal", "text": observed, "date": "2026-10-20", "sources": ["crm:morgan"]})
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
