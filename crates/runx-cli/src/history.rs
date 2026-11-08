use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::cli_args;
use runx_contracts::RunRead;
use runx_runtime::run_record::{
    RunFilter, RunListing, RunRecordError, RunState, list_runs, read_run,
};

// Module rationale: the native history command keeps
// parsing, rendering, and CLI tests together.
#[derive(Debug)]
pub enum HistoryCliError {
    InvalidArgs(String),
    Runs(RunRecordError),
    Serialize(serde_json::Error),
}

impl fmt::Display for HistoryCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgs(message) => formatter.write_str(message),
            Self::Runs(error) => write!(formatter, "{error}"),
            Self::Serialize(error) => write!(formatter, "failed to serialize history: {error}"),
        }
    }
}

impl std::error::Error for HistoryCliError {}

impl HistoryCliError {
    /// The code a `--json` failure carries.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidArgs(_) => "invalid_args",
            Self::Runs(RunRecordError::CursorRunMismatch { .. }) => "cursor_run_mismatch",
            Self::Runs(RunRecordError::InvalidCursor) => "invalid_cursor",
            Self::Runs(_) | Self::Serialize(_) => "history_error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryCliResult {
    pub output: String,
    pub error_is_usage: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ParsedHistoryArgs {
    receipt_dir: Option<PathBuf>,
    query: Option<String>,
    filter: RunFilter,
    /// The one positional, when there is exactly one: a run it names is read.
    run_id: Option<String>,
    after: Option<String>,
    json: bool,
}

pub fn run_history_command(
    args: &[OsString],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<HistoryCliResult, HistoryCliError> {
    let parsed = parse_history_args(args)?;
    if let Some(read) = read_named_run(&parsed, env, cwd)? {
        let output = if parsed.json {
            format!(
                "{}\n",
                serde_json::to_string_pretty(&read).map_err(HistoryCliError::Serialize)?
            )
        } else {
            render_run(&read)
        };
        return Ok(successful_result(output));
    }
    if parsed.after.is_some() {
        return Err(invalid_args(
            "--after continues one run's read: runx history <run-id> --after <cursor>",
        ));
    }
    let listing = list_runs(env, cwd, &parsed.filter).map_err(HistoryCliError::Runs)?;
    let output = if parsed.json {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&listing).map_err(HistoryCliError::Serialize)?
        )
    } else {
        render_history(
            &listing,
            parsed.query.as_deref(),
            parsed.receipt_dir.as_deref(),
        )
    };
    Ok(successful_result(output))
}

/// The run the one positional names, read after `--after`; `None` when it
/// names no run of this workspace, so the positional searches instead.
fn read_named_run(
    parsed: &ParsedHistoryArgs,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Option<RunRead>, HistoryCliError> {
    let Some(run_id) = parsed.run_id.as_deref() else {
        return Ok(None);
    };
    match read_run(env, cwd, run_id, parsed.after.as_deref()) {
        Ok(_) if parsed.filter.has_list_filters() => {
            Err(invalid_args("history <run-id> takes no list filters"))
        }
        Ok(read) => Ok(Some(read)),
        Err(RunRecordError::UnknownRun { .. } | RunRecordError::InvalidId { .. }) => Ok(None),
        Err(error) => Err(HistoryCliError::Runs(error)),
    }
}

fn successful_result(output: String) -> HistoryCliResult {
    HistoryCliResult {
        output,
        error_is_usage: false,
    }
}

// Function rationale: the public history CLI flag grammar
// lives in one parser.
fn parse_history_args(args: &[OsString]) -> Result<ParsedHistoryArgs, HistoryCliError> {
    if args.first().and_then(|arg| arg.to_str()) != Some("history") {
        return Err(HistoryCliError::InvalidArgs(
            "internal error: history dispatcher received non-history command".to_owned(),
        ));
    }
    let mut parsed = ParsedHistoryArgs::default();
    let mut positionals = Vec::new();
    let mut index = 1;
    while index < args.len() {
        let token = cli_args::os_arg(args, index, "history").map_err(invalid_args)?;
        // `-j` is the one short flag; anything else without `--` searches.
        if token != "-j" && !token.starts_with("--") {
            positionals.push(token.to_owned());
            index += 1;
            continue;
        }
        let (flag, inline_value) = cli_args::split_flag(token);
        match flag {
            "--json" | "-j" => {
                if inline_value.is_some() {
                    return Err(invalid_args(format!("{flag} does not take a value")));
                }
                parsed.json = true;
                index += 1;
            }
            "--receipt-dir" | "--skill" | "--state" | "--since" | "--until" | "--limit"
            | "--after" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                match flag {
                    "--receipt-dir" => parsed.receipt_dir = Some(PathBuf::from(value)),
                    "--skill" => parsed.filter.skill = Some(value),
                    "--state" => parsed.filter.state = Some(value),
                    "--since" => parsed.filter.since = Some(value),
                    "--until" => parsed.filter.until = Some(value),
                    "--after" => parsed.after = Some(value),
                    _ => {
                        parsed.filter.limit = Some(value.parse().map_err(|_| {
                            invalid_args(format!("invalid --limit value '{value}'"))
                        })?);
                    }
                }
                index = next_index;
            }
            _ => {
                return Err(HistoryCliError::InvalidArgs(format!(
                    "unknown history flag {flag}"
                )));
            }
        }
    }
    parsed.run_id = match positionals.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    };
    parsed.query = (!positionals.is_empty()).then(|| positionals.join(" "));
    parsed.filter.query = parsed.query.clone();
    Ok(parsed)
}

fn render_history(listing: &RunListing, query: Option<&str>, receipt_dir: Option<&Path>) -> String {
    if listing.runs.is_empty() && listing.unreadable.is_empty() {
        if let Some(query) = query {
            return format!(
                "\n  No runs matched {query}.\n  Try runx history to see every local run.\n\n"
            );
        }
        return "\n  No runs yet. Try a live run first:\n  runx skill <skill-dir> --json\n\n"
            .to_owned();
    }
    let paused = listing
        .runs
        .iter()
        .filter(|run| run.state == RunState::Paused)
        .count();
    let mut counts = vec![format!("{} run(s)", listing.runs.len())];
    if paused > 0 {
        counts.push(format!("{paused} paused"));
    }
    if !listing.unreadable.is_empty() {
        counts.push(format!("{} unreadable", listing.unreadable.len()));
    }
    let mut lines = vec![
        String::new(),
        format!("  history  {}", counts.join(", ")),
        String::new(),
    ];
    for run in &listing.runs {
        let status = run
            .outcome
            .as_ref()
            .map_or(run.state.label(), |outcome| outcome.label());
        lines.push(format!("  {status}  {}  {}", run.skill, run.run_id));
        if let Some(receipt_id) = &run.receipt_id {
            lines.push(format!("     receipt  {receipt_id}"));
        }
        if let Some(request_id) = &run.pending_request_id {
            let resume =
                crate::resume::render_skill_resume_command(crate::resume::SkillResumeCommand {
                    run_id: &run.run_id,
                    receipt_dir,
                    answers_path: None,
                });
            lines.push(format!("     waits on  {request_id}"));
            lines.push(format!("     next  {resume}"));
        }
    }
    for run in &listing.unreadable {
        lines.push(format!("  unreadable  {}", run.run_id));
        lines.push(format!("     why  {}", run.reason));
    }
    lines.push(String::new());
    lines.push(if paused == 0 {
        "  next  runx verify <receipt-id> --json".to_owned()
    } else {
        "  next  pipe one answers object into one of the commands above".to_owned()
    });
    lines.push(String::new());
    lines.join("\n")
}

fn render_run(read: &RunRead) -> String {
    let run = &read.run;
    let mut lines = vec![
        String::new(),
        format!(
            "  {}  {}  {}",
            run.outcome
                .as_ref()
                .map_or(run.state.label(), |outcome| outcome.label()),
            run.skill,
            run.run_id
        ),
    ];
    if let Some(receipt_id) = &run.receipt_id {
        lines.push(format!("     receipt  {receipt_id}"));
    }
    for waited in &read.waiting {
        lines.push(format!("     waits on  {}", waited.address));
    }
    if run.driven {
        lines.push("     driven by a running process".to_owned());
    }
    lines.push(format!("     {} event(s) read", read.events.len()));
    lines.push(String::new());
    lines.join("\n")
}

fn invalid_args(message: impl Into<String>) -> HistoryCliError {
    HistoryCliError::InvalidArgs(message.into())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;

    use super::*;

    #[test]
    fn parses_history_args_without_comparing_against_runtime_constants() -> Result<(), io::Error> {
        let parsed = parse_history_args(&[
            "history".into(),
            "sourcey".into(),
            "--skill".into(),
            "source".into(),
            "--state=paused".into(),
            "--limit".into(),
            "5".into(),
            "--json".into(),
        ])
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert_eq!(parsed.query.as_deref(), Some("sourcey"));
        assert_eq!(parsed.filter.skill.as_deref(), Some("source"));
        assert_eq!(parsed.filter.state.as_deref(), Some("paused"));
        assert_eq!(parsed.filter.limit, Some(5));
        assert!(parsed.json);
        Ok(())
    }

    #[test]
    fn short_json_flag_is_a_flag_not_a_query() -> Result<(), io::Error> {
        let parsed = parse_history_args(&["history".into(), "-j".into()])
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(parsed.json);
        assert_eq!(parsed.query, None);
        Ok(())
    }

    #[test]
    fn history_lists_no_runs_in_an_empty_workspace() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let mut env = BTreeMap::new();
        env.insert("RUNX_CWD".to_owned(), temp.to_string_lossy().to_string());
        let result = run_history_command(&["history".into(), "--json".into()], &env, &temp)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let listing: serde_json::Value = serde_json::from_str(&result.output)
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert_eq!(listing["runs"], serde_json::json!([]));
        let human = run_history_command(&["history".into()], &env, &temp)
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(human.output.contains("No runs yet"));
        Ok(())
    }

    #[test]
    fn history_shows_each_run_it_cannot_read_and_why() {
        let listing = RunListing {
            projector_id: "runx-runtime.run-record.v1".to_owned(),
            runs: Vec::new(),
            unreadable: vec![runx_contracts::UnreadableRun {
                run_id: "run_retired".into(),
                updated_at: "2026-11-09T00:00:00.000Z".into(),
                reason: "kernel store event could not be encoded or decoded: missing field `skill`"
                    .into(),
            }],
        };
        let human = render_history(&listing, None, None);
        assert!(human.contains("history  0 run(s), 1 unreadable"), "{human}");
        assert!(human.contains("unreadable  run_retired"), "{human}");
        assert!(
            human.contains("why  kernel store event could not be"),
            "{human}"
        );
    }

    #[test]
    fn one_positional_naming_a_run_reads_it_and_otherwise_searches() -> Result<(), io::Error> {
        let parsed = parse_history_args(&[
            "history".into(),
            "run_1".into(),
            "--after".into(),
            "c".into(),
        ])
        .map_err(|error| io::Error::other(error.to_string()))?;
        assert_eq!(parsed.run_id.as_deref(), Some("run_1"));
        assert_eq!(parsed.after.as_deref(), Some("c"));
        let searched = parse_history_args(&["history".into(), "two".into(), "words".into()])
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert_eq!(searched.run_id, None);

        let temp = tempfile_dir()?;
        let mut env = BTreeMap::new();
        env.insert("RUNX_CWD".to_owned(), temp.to_string_lossy().to_string());
        let unknown = run_history_command(
            &["history".into(), "run_none".into(), "--json".into()],
            &env,
            &temp,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
        let listing: serde_json::Value = serde_json::from_str(&unknown.output)
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert_eq!(
            listing["runs"],
            serde_json::json!([]),
            "an unknown run id searches"
        );
        assert!(matches!(
            run_history_command(
                &[
                    "history".into(),
                    "run_none".into(),
                    "--after".into(),
                    "c".into()
                ],
                &env,
                &temp
            ),
            Err(HistoryCliError::InvalidArgs(_))
        ));
        Ok(())
    }

    fn tempfile_dir() -> Result<PathBuf, io::Error> {
        let path = std::env::temp_dir().join(format!(
            "runx-cli-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| io::Error::other(error.to_string()))?
                .as_nanos()
        ));
        fs::create_dir_all(&path)?;
        Ok(path)
    }
}
