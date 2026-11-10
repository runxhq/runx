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
        let history = kept
            .iter()
            .enumerate()
            .map(|(second, (key, text))| HistoryEvent {
                stream: format!("{PACKAGE}/observed:personal"),
                key: (*key).to_owned(),
                event_type: "memory.observed".to_owned(),
                data: serde_json::from_value(json!({
                    "text": text, "summary": text, "date": "2026-11-01", "sources": ["fixture"],
                }))
                .unwrap_or_default(),
                standing: false,
                valid_until: None,
                searchable: Some(vec!["text".to_owned(), "summary".to_owned()]),
                committed_at: format!("2026-11-01T00:00:0{second}.000Z"),
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
