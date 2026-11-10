//! performance.md's journey 5, memory, counted: the index and search over a
//! personal memory of 100, 10,000 and 100,000 entries. Each size is laid down
//! in a fresh home through the store's own append, a third of them the
//! person's words, a tenth corrected, a twentieth disabled and some
//! observations superseded, placed by their distance from the newest so the
//! newest pages read alike at every size. Then `runx/memory#index` and
//! `#search` run as any caller runs them, and the store counts what they read.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use runx_runtime::probe_fixtures::HistoryEvent;
use runx_runtime::work_counts::{WorkCounts, snapshot};
use tempfile::TempDir;

const SIZES: [(&str, usize); 3] = [("100", 100), ("10000", 10_000), ("100000", 100_000)];
/// How the store names a workspace's own memory package's streams.
const PACKAGE: &str = "project:memory";
/// The most rows a page of a key listing steps for the keys it keeps: its
/// largest page and the one more that says there's another.
const ROWS_A_PAGE_KEEPS: u64 = 101;
/// The streams a personal index or search reads: the person's words and the
/// observations.
const STREAMS: u64 = 2;

#[allow(clippy::expect_used)]
pub(super) fn register() {
    crate::record_journey(
        "5",
        memory_journey().expect("the memory journey must index and search each size"),
    )
    .expect("the memory journey must be recorded");
}

fn memory_journey() -> Result<JsonValue, Box<dyn Error>> {
    let mut rows_visited = JsonObject::new();
    let mut rows_its_budget_reads = JsonObject::new();
    let mut search_pages_per_stream = 0_u64;
    let mut search_history_rows_read = 0_u64;
    let mut sizes = JsonObject::new();
    for (label, entries) in SIZES {
        let memory = Memory::new()?;
        memory.lay_down(entries)?;
        let (index, counted) = memory.run(
            "index",
            serde_json::json!({"levels": [{"level": "personal"}]}),
        )?;
        let shown = entries_of(&index)?;
        if shown == 0 {
            return Err(std::io::Error::other(format!(
                "the index of {entries} entries showed none: {index}"
            ))
            .into());
        }
        let pages = counted.provider_calls;
        rows_visited.insert(label.to_owned(), count(counted.store_key_rows_visited));
        rows_its_budget_reads.insert(
            label.to_owned(),
            count(pages * ROWS_A_PAGE_KEEPS + counted.store_key_rows_passed_over),
        );
        let (found, searched) = memory.run(
            "search",
            serde_json::json!({"levels": [{"level": "personal"}], "query": "lighthouse"}),
        )?;
        if entries_of(&found)? == 0 {
            return Err(std::io::Error::other(format!(
                "a search of {entries} entries found no lighthouse: {found}"
            ))
            .into());
        }
        search_pages_per_stream =
            search_pages_per_stream.max(searched.store_search_pages.div_ceil(STREAMS));
        search_history_rows_read = search_history_rows_read.max(searched.store_event_rows_read);
        sizes.insert(
            label.to_owned(),
            JsonValue::Object(JsonObject::from([
                ("index_lines".to_owned(), count(shown)),
                (
                    "index_omitted".to_owned(),
                    JsonValue::Bool(index["entries_omitted"] == true),
                ),
                ("index_pages".to_owned(), count(pages)),
                (
                    "index_rows_visited".to_owned(),
                    count(counted.store_key_rows_visited),
                ),
                (
                    "index_rows_passed_over".to_owned(),
                    count(counted.store_key_rows_passed_over),
                ),
                (
                    "search_pages".to_owned(),
                    count(searched.store_search_pages),
                ),
                (
                    "search_history_rows_read".to_owned(),
                    count(searched.store_event_rows_read),
                ),
            ])),
        );
    }
    Ok(JsonValue::Object(JsonObject::from([
        (
            "index_rows_visited".to_owned(),
            JsonValue::Object(rows_visited),
        ),
        (
            "index_rows_its_budget_reads".to_owned(),
            JsonValue::Object(rows_its_budget_reads),
        ),
        (
            "search_pages_per_stream".to_owned(),
            count(search_pages_per_stream),
        ),
        (
            "search_history_rows_read".to_owned(),
            count(search_history_rows_read),
        ),
        ("sizes".to_owned(), JsonValue::Object(sizes)),
    ])))
}

fn count(value: u64) -> JsonValue {
    JsonValue::Number(JsonNumber::U64(value))
}

fn entries_of(output: &serde_json::Value) -> Result<u64, Box<dyn Error>> {
    let entries = &output["entries"];
    let list = entries.get("data").unwrap_or(entries);
    Ok(list
        .as_array()
        .ok_or_else(|| format!("no entries in {output}"))?
        .len() as u64)
}

/// A workspace holding a copy of `runx/memory`, with a home of its own.
struct Memory {
    directory: TempDir,
    env: BTreeMap<String, String>,
}

impl Memory {
    fn new() -> Result<Self, Box<dyn Error>> {
        let directory = TempDir::new()?;
        let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/memory");
        copy_tree(&package, &directory.path().join("skills/memory"))?;
        fs::create_dir_all(directory.path().join("home"))?;
        let mut env = std::env::vars().collect::<BTreeMap<_, _>>();
        env.insert(
            "RUNX_CWD".to_owned(),
            directory.path().to_string_lossy().into_owned(),
        );
        env.insert(
            "RUNX_HOME".to_owned(),
            directory.path().join("home").to_string_lossy().into_owned(),
        );
        Ok(Self { directory, env })
    }

    /// `entries` entries on `said:personal` and `observed:personal`, oldest
    /// first, each event at its own time.
    fn lay_down(&self, entries: usize) -> Result<(), Box<dyn Error>> {
        let mut history = Vec::with_capacity(entries + entries / 5);
        for index in 0..entries {
            // Its distance from the newest entry, which places it.
            let distance = entries - 1 - index;
            let said = distance.is_multiple_of(3);
            let stream = format!(
                "{PACKAGE}/{}:personal",
                if said { "said" } else { "observed" }
            );
            let key = format!("{index:064x}");
            let text = if distance % 50 == 1 {
                format!("Entry {index} mentions a lighthouse on the point.")
            } else {
                format!("Entry {index} is one of the probe's entries.")
            };
            let at = |half: u64| committed_at(index as u64, half);
            let mut event = |event_type: &str, data: serde_json::Value, half: u64| {
                history.push(HistoryEvent {
                    stream: stream.clone(),
                    key: key.clone(),
                    event_type: event_type.to_owned(),
                    data: serde_json::from_value(data).unwrap_or_default(),
                    standing: false,
                    valid_until: None,
                    committed_at: at(half),
                    writer_package: PACKAGE.to_owned(),
                });
            };
            let fact =
                serde_json::json!({"text": text, "date": "2026-01-01", "sources": ["probe"]});
            let confirmed = |mut fact: serde_json::Value| {
                fact["confirmation"] = serde_json::json!({"approved": true});
                fact
            };
            if said {
                event("memory.remembered", confirmed(fact.clone()), 0);
                if distance % 10 == 3 {
                    let mut corrected = fact.clone();
                    corrected["text"] = serde_json::json!(format!("Entry {index}, corrected."));
                    event("memory.corrected", confirmed(corrected), 1);
                }
            } else {
                event("memory.observed", fact.clone(), 0);
                if distance % 10 == 5 {
                    let mut superseding = fact.clone();
                    superseding["text"] =
                        serde_json::json!(format!("Entry {index}, observed again."));
                    event("memory.observed", superseding, 1);
                }
            }
            if distance % 20 == 7 {
                event(
                    "memory.disabled",
                    confirmed(serde_json::json!({"reason": "The probe disables a twentieth."})),
                    2,
                );
            }
        }
        runx_runtime::probe_fixtures::lay_down_private_history(&self.env, &history)?;
        Ok(())
    }

    /// `method` run as any caller runs it, under a term on the personal
    /// streams, with its output and what the run counted.
    fn run(
        &self,
        method: &str,
        inputs: serde_json::Value,
    ) -> Result<(serde_json::Value, WorkCounts), Box<dyn Error>> {
        let authority = serde_json::from_value(serde_json::json!({
            "delegations": [{"connector": "private", "terms": [{
                "verbs": ["read"],
                "bounds": {
                    "operations": ["private/*"],
                    "scopes": ["store.read"],
                    "targets": [format!("{PACKAGE}/said:*"), format!("{PACKAGE}/observed:*")],
                    "packages": [PACKAGE],
                },
            }]}],
        }))?;
        let skill_path = self.directory.path().join("skills/memory");
        let request = runx_runtime::orchestrator::SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: Some(authority),
            act: None,
            skill_path: skill_path.clone(),
            receipt_dir: None,
            run_id: None,
            answers: None,
            inputs: serde_json::from_value(inputs)?,
            env: self.env.clone(),
            cwd: self.directory.path().to_path_buf(),
            managed_agent: Default::default(),
        };
        let orchestrator = runx_runtime::LocalOrchestrator::with_effects_and_environment(
            runx_runtime::RuntimeEffectRegistry::local()?,
            self.env.clone(),
        );
        let entry = runx_runtime::skill_front::PreparedEntryProvenance {
            kind: "explicit_path".to_owned(),
            reference: None,
            source: skill_path.to_string_lossy().into_owned(),
            source_label: skill_path.to_string_lossy().into_owned(),
            skill_id: None,
            version: None,
            digest: None,
            package_digest: None,
            execution_closure_digest: None,
            trust_tier: None,
        };
        let before = snapshot();
        let mut prepared = orchestrator.prepare_skill(request, Some(method), entry)?;
        prepared.bind_context()?;
        let result = orchestrator.run_prepared_skill(&prepared)?;
        let after = snapshot();
        if !result.succeeded() {
            return Err(std::io::Error::other(format!(
                "memory#{method} didn't complete: {}",
                serde_json::to_string(&result.output)?
            ))
            .into());
        }
        // The method's result, as the run's output holds it.
        let output = serde_json::to_value(&result.output)?;
        Ok((output["result"].clone(), between(before, after)))
    }
}

fn between(before: WorkCounts, after: WorkCounts) -> WorkCounts {
    WorkCounts {
        provider_calls: after.provider_calls - before.provider_calls,
        store_key_rows_visited: after.store_key_rows_visited - before.store_key_rows_visited,
        store_key_rows_passed_over: after.store_key_rows_passed_over
            - before.store_key_rows_passed_over,
        store_search_pages: after.store_search_pages - before.store_search_pages,
        store_event_rows_read: after.store_event_rows_read - before.store_event_rows_read,
        ..after
    }
}

/// The `index`-th entry's time, a second apart, and a version's within it.
fn committed_at(index: u64, version: u64) -> String {
    let seconds = index;
    let day = 1 + seconds / 86_400;
    let hour = (seconds / 3_600) % 24;
    let minute = (seconds / 60) % 60;
    let second = seconds % 60;
    format!(
        "2026-01-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        version * 100
    )
}

fn copy_tree(from: &Path, to: &PathBuf) -> Result<(), Box<dyn Error>> {
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
