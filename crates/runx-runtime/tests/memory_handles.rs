#![cfg(feature = "probe-fixtures")]

//! memory's short handles, over keys laid down to share their start: an index
//! line shows the shortest start of its entry's key, at least 8 characters,
//! that no other line of its stream shares, and `read` resolves the short
//! handle a header and a line make within its stream, refusing one that
//! begins no key, or more than one, with the full handles it could mean.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_runtime::probe_fixtures::HistoryEvent;
use runx_runtime::{LocalOrchestrator, SkillRunRequest};
use serde_json::{Value, json};
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// How the store names a workspace's own memory package's streams.
const PACKAGE: &str = "project:memory";

/// A workspace holding a copy of `runx/memory`, with a home of its own.
struct Memory {
    directory: TempDir,
    env: BTreeMap<String, String>,
}

impl Memory {
    fn new() -> TestResult<Self> {
        let directory = TempDir::new()?;
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
        ]);
        crate::support::insert_test_signing_env(&mut env);
        Ok(Self { directory, env })
    }

    /// An observation under each of `keys`, oldest first, each shown by its
    /// own words.
    fn observe(&self, kept: &[(&str, &str)]) -> TestResult {
        self.observe_lines(
            &kept
                .iter()
                .map(|(key, text)| ((*key).to_owned(), (*text).to_owned(), (*text).to_owned()))
                .collect::<Vec<_>>(),
        )
    }

    /// An observation of each `(key, text, summary)`, oldest first, a second
    /// apart.
    fn observe_lines(&self, kept: &[(String, String, String)]) -> TestResult {
        let history = kept
            .iter()
            .enumerate()
            .map(|(second, (key, text, summary))| HistoryEvent {
                stream: format!("{PACKAGE}/observed:personal"),
                key: key.clone(),
                event_type: "memory.observed".to_owned(),
                data: serde_json::from_value(json!({
                    "text": text, "summary": summary, "date": "2026-11-01", "sources": ["fixture"],
                }))
                .unwrap_or_default(),
                standing: false,
                valid_until: None,
                searchable: Some(vec!["text".to_owned(), "summary".to_owned()]),
                committed_at: format!("2026-11-01T00:{:02}:{:02}.000Z", second / 60, second % 60),
                writer_package: PACKAGE.to_owned(),
            })
            .collect::<Vec<_>>();
        runx_runtime::probe_fixtures::lay_down_private_history(&self.env, &history)?;
        Ok(())
    }

    /// `method` run under a term that reads the personal streams: whether
    /// it completed, and its output.
    fn run(&self, method: &str, inputs: Value) -> TestResult<(bool, Value)> {
        let authority = serde_json::from_value(json!({
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
        // The local effects, so the store's operations are admitted.
        let orchestrator = LocalOrchestrator::with_effects_and_environment(
            runx_runtime::RuntimeEffectRegistry::local()?,
            self.env.clone(),
        );
        let result = orchestrator.run_skill_with_binding(
            &SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: Some(authority),
                act: None,
                skill_path: self.directory.path().join("skills/memory"),
                receipt_dir: None,
                run_id: None,
                answers: None,
                inputs: serde_json::from_value(inputs)?,
                env: self.env.clone(),
                cwd: self.directory.path().to_path_buf(),
                managed_agent: Default::default(),
            },
            Some(method),
            None,
            None,
        )?;
        Ok((result.succeeded(), serde_json::to_value(&result.output)?))
    }
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

fn full(key: &str) -> String {
    format!("memory:observed:personal#{key}@1")
}

/// Two keys that start alike for 9 characters show 10 each, one apart from
/// them shows 8, and `read` resolves each line's short handle to its entry.
/// A start that two keys share, or that no key begins, is refused, the shared
/// one naming both full handles.
#[test]
fn a_lines_key_is_the_start_no_other_line_shares_and_read_resolves_it() -> TestResult {
    let memory = Memory::new()?;
    let first = format!("012345678a{}", "0".repeat(54));
    let second = format!("012345678b{}", "0".repeat(54));
    let apart = format!("fedcba98{}", "0".repeat(56));
    memory.observe(&[
        (&first, "The first twin."),
        (&second, "The second twin."),
        (&apart, "Apart from both."),
    ])?;
    let (completed, index) = memory.run("index", json!({"levels": [{"level": "personal"}]}))?;
    assert!(completed, "{index}");
    let entries = &index["result"]["entries"];
    assert_eq!(
        entries[0]["summary"],
        "memory:observed:personal\nfedcba98@1 2026-11-01 Apart from both.\n012345678b@1 2026-11-01 The second twin.\n012345678a@1 2026-11-01 The first twin.",
        "{index}"
    );
    for (short, key) in [
        ("012345678a", &first),
        ("012345678b", &second),
        ("fedcba98", &apart),
    ] {
        let (completed, read) = memory.run("read", json!({"handle": full(short)}))?;
        assert!(completed, "{read}");
        assert_eq!(
            read["result"]["entry"]["data"]["handle"],
            full(key),
            "{read}"
        );
    }
    let (completed, shared) = memory.run("read", json!({"handle": full("01234567")}))?;
    let refusal = shared.to_string();
    assert!(
        !completed && refusal.contains(&full(&first)) && refusal.contains(&full(&second)),
        "{refusal}"
    );
    let (completed, none) = memory.run("read", json!({"handle": full("99999999")}))?;
    assert!(
        !completed && none.to_string().contains("names no entry"),
        "{none}"
    );
    Ok(())
}

/// A page of keys too large to reach the index inline arrives as a reference
/// to the artifact its call recorded, and the index reads it whole: 150
/// observations of 900 characters each make a first page of 100 keys past
/// the inline bound, and the index still shows every line its budget holds.
#[test]
fn a_page_past_the_inline_bound_is_read_whole() -> TestResult {
    let memory = Memory::new()?;
    let kept = (0..150)
        .map(|number| {
            (
                runx_contracts::sha256_prefixed(format!("note {number}").as_bytes())
                    .trim_start_matches("sha256:")
                    .to_owned(),
                format!("Note {number}: {}", "a long observation. ".repeat(45)),
                format!("Note {number}."),
            )
        })
        .collect::<Vec<_>>();
    memory.observe_lines(&kept)?;
    let (completed, index) = memory.run("index", json!({"levels": [{"level": "personal"}]}))?;
    assert!(completed, "{index}");
    let shown = index["result"]["entries"][0]["data"]["entries"]
        .as_array()
        .map_or(0, Vec::len);
    assert!(
        shown > 100,
        "{shown} lines, from more than a page: {}",
        index["result"]["entries"][0]["summary"]
    );
    assert_eq!(index["result"]["entries_omitted"], true);
    Ok(())
}

/// An index entry's origin states its data's size, the UTF-8 bytes of its
/// JSON, and the digest of its summary, which names each line's key and
/// version: two-, three- and four-byte characters counted as UTF-8 counts
/// them.
#[test]
fn an_index_entry_states_its_datas_size_and_its_summarys_digest() -> TestResult {
    let memory = Memory::new()?;
    let keys = ["a", "b", "c"].map(|name| {
        runx_contracts::sha256_prefixed(name.as_bytes())
            .trim_start_matches("sha256:")
            .to_owned()
    });
    memory.observe(&[
        (&keys[0], "Café on the corner opens at seven."),
        (&keys[1], "The ticket costs 12 €."),
        (&keys[2], "The team's mascot is 🦘."),
    ])?;
    let (completed, index) = memory.run("index", json!({"levels": [{"level": "personal"}]}))?;
    assert!(completed, "{index}");
    let entry = &index["result"]["entries"][0];
    assert_eq!(
        entry["origin"]["size_bytes"],
        serde_json::to_string(&entry["data"])?.len(),
        "{entry}"
    );
    let summary = entry["summary"].as_str().ok_or("an entry has a summary")?;
    assert_eq!(
        entry["origin"]["digest"],
        runx_contracts::sha256_prefixed(summary.as_bytes()),
        "{entry}"
    );
    Ok(())
}
