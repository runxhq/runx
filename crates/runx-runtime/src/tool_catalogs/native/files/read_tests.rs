#![allow(clippy::expect_used)]

use std::fs;

use super::capability::{FileReadBundleInput, FileReadInput};
use super::test_support::{TestResult, digest, invoke};
use super::{read, read_bundle};
use crate::services::WorkspaceBounds;

/// A file of `count` numbered lines.
fn numbered(count: usize) -> String {
    (1..=count).map(|line| format!("line {line}\n")).collect()
}

#[test]
fn a_read_returns_the_lines_it_asks_for_with_the_whole_files_lines_and_digest() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let text = numbered(10);
    fs::write(workspace.path().join("notes.md"), &text)?;
    let read_lines = |inputs: serde_json::Value| {
        invoke::<FileReadInput, _>(workspace.path(), inputs, WorkspaceBounds::Agent, read)
    };

    let whole = read_lines(serde_json::json!({"path": "notes.md"}))?;
    assert_eq!(whole.contents, text);
    assert!(!whole.truncated);
    assert_eq!((whole.start_line, whole.total_lines), (1, 10));
    assert_eq!(whole.content_digest, digest(&text));

    let middle =
        read_lines(serde_json::json!({"path": "notes.md", "start_line": 4, "line_count": 3}))?;
    assert_eq!(middle.contents, "line 4\nline 5\nline 6\n");
    assert!(middle.truncated, "lines 7 to 10 follow");
    assert_eq!((middle.start_line, middle.total_lines), (4, 10));
    // A part of the file still names the whole file's digest, which a write of it binds to.
    assert_eq!(middle.content_digest, digest(&text));

    let last = read_lines(serde_json::json!({"path": "notes.md", "start_line": 9}))?;
    assert_eq!(last.contents, "line 9\nline 10\n");
    assert!(!last.truncated);

    let past = read_lines(serde_json::json!({"path": "notes.md", "start_line": 11}))?;
    assert_eq!(
        (past.contents.as_str(), past.truncated, past.total_lines),
        ("", false, 10)
    );

    for refused in [
        serde_json::json!({"path": "notes.md", "start_line": 0}),
        serde_json::json!({"path": "notes.md", "line_count": 0}),
    ] {
        assert!(read_lines(refused).is_err());
    }
    Ok(())
}

#[test]
fn a_read_over_its_byte_bound_returns_the_whole_lines_within_it_never_nothing() -> TestResult {
    let workspace = tempfile::tempdir()?;
    fs::write(workspace.path().join("notes.md"), numbered(10))?;
    // Each line is seven bytes: twenty bytes hold two whole lines.
    let bounded = invoke::<FileReadInput, _>(
        workspace.path(),
        serde_json::json!({"path": "notes.md", "max_bytes": 20}),
        WorkspaceBounds::Agent,
        read,
    )?;
    assert_eq!(bounded.contents, "line 1\nline 2\n");
    assert!(bounded.truncated);
    assert_eq!(bounded.bytes, numbered(10).len() as u64);

    // A first line longer than the bound comes back cut at a character boundary.
    fs::write(
        workspace.path().join("minified.js"),
        format!("{}\n", "é".repeat(20)),
    )?;
    let cut = invoke::<FileReadInput, _>(
        workspace.path(),
        serde_json::json!({"path": "minified.js", "max_bytes": 5}),
        WorkspaceBounds::Agent,
        read,
    )?;
    assert_eq!(cut.contents, "éé");
    assert!(cut.truncated);
    Ok(())
}

#[test]
fn a_bundle_reads_each_file_whole() -> TestResult {
    let workspace = tempfile::tempdir()?;
    fs::write(workspace.path().join("a.md"), numbered(3000))?;
    fs::write(workspace.path().join("b.md"), "one line")?;
    let bundle = invoke::<FileReadBundleInput, _>(
        workspace.path(),
        serde_json::json!({"paths": ["a.md", "b.md"]}),
        WorkspaceBounds::Agent,
        read_bundle,
    )?;
    let lines: Vec<_> = bundle
        .files
        .iter()
        .map(|file| {
            (
                file.total_lines,
                file.truncated,
                file.contents.lines().count(),
            )
        })
        .collect();
    assert_eq!(lines, [(3000, false, 3000), (1, false, 1)]);
    Ok(())
}
