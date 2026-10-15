//! `runx process`: the long-running processes the project's runs started,
//! as the person lists, reads and stops them.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::Path;

use runx_contracts::{ProcessList, ProcessOutput, ProcessState, RunxProcess};
use runx_runtime::ProcessError;

use crate::cli_args;

/// About how much output a read returns when `--max-bytes` is not given.
const READ_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub enum ProcessCliError {
    InvalidArgs(String),
    Process(ProcessError),
    Serialize(serde_json::Error),
}

impl fmt::Display for ProcessCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgs(message) => formatter.write_str(message),
            Self::Process(error) => write!(formatter, "{error}"),
            Self::Serialize(error) => write!(formatter, "failed to serialize the process: {error}"),
        }
    }
}

impl std::error::Error for ProcessCliError {}

impl ProcessCliError {
    /// The code a `--json` failure carries.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidArgs(_) => "invalid_args",
            Self::Process(ProcessError::InvalidId(_) | ProcessError::Unknown(_)) => {
                "unknown_process"
            }
            Self::Process(_) | Self::Serialize(_) => "process_error",
        }
    }
}

enum Command {
    List {
        actor: Option<String>,
    },
    Read {
        id: String,
        after: u64,
        max_bytes: usize,
    },
    Stop {
        id: String,
    },
}

pub fn run_process_command(
    args: &[OsString],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<String, ProcessCliError> {
    let (command, json) = parse(args)?;
    let process = ProcessCliError::Process;
    match command {
        Command::List { actor } => {
            let list = runx_runtime::list_processes(env, cwd, actor.as_deref()).map_err(process)?;
            render(json, &list, render_list)
        }
        Command::Read {
            id,
            after,
            max_bytes,
        } => {
            let output =
                runx_runtime::read_process(env, cwd, &id, after, max_bytes).map_err(process)?;
            render(json, &output, render_output)
        }
        Command::Stop { id } => {
            let stopped = runx_runtime::stop_process(env, cwd, &id).map_err(process)?;
            render(json, &stopped, |stopped| format!("{}\n", summary(stopped)))
        }
    }
}

fn parse(args: &[OsString]) -> Result<(Command, bool), ProcessCliError> {
    let invalid = ProcessCliError::InvalidArgs;
    let mut positionals = Vec::new();
    let mut actor = None;
    let mut after = None;
    let mut max_bytes = None;
    let mut json = false;
    let mut index = 1;
    while index < args.len() {
        let token = cli_args::os_arg(args, index, "process").map_err(invalid)?;
        if token != "-j" && !token.starts_with("--") {
            positionals.push(token.to_owned());
            index += 1;
            continue;
        }
        let (flag, inline_value) = cli_args::split_flag(token);
        match flag {
            "--json" | "-j" if inline_value.is_none() => {
                json = true;
                index += 1;
            }
            "--actor" | "--after" | "--max-bytes" => {
                let (value, next) =
                    cli_args::flag_value(args, index, flag, inline_value, "process")
                        .map_err(invalid)?;
                let number = || {
                    value
                        .parse::<u64>()
                        .map_err(|_| invalid(format!("{flag} takes a number, not {value}")))
                };
                match flag {
                    "--actor" => actor = Some(value.clone()),
                    "--after" => after = Some(number()?),
                    _ => max_bytes = Some(usize::try_from(number()?).unwrap_or(usize::MAX)),
                }
                index = next;
            }
            _ => return Err(invalid(format!("unknown process flag {flag}"))),
        }
    }
    let usage = || invalid("runx process list|read <process-id>|stop <process-id>".to_owned());
    let command = match positionals.as_slice() {
        [list] if list == "list" && after.is_none() && max_bytes.is_none() => {
            Command::List { actor }
        }
        [read, id] if read == "read" && actor.is_none() => Command::Read {
            id: id.clone(),
            after: after.unwrap_or(0),
            max_bytes: max_bytes.unwrap_or(READ_BYTES),
        },
        [stop, id]
            if stop == "stop" && actor.is_none() && after.is_none() && max_bytes.is_none() =>
        {
            Command::Stop { id: id.clone() }
        }
        _ => return Err(usage()),
    };
    Ok((command, json))
}

fn render<T: serde::Serialize>(
    json: bool,
    value: &T,
    human: impl FnOnce(&T) -> String,
) -> Result<String, ProcessCliError> {
    if json {
        return serde_json::to_string_pretty(value)
            .map(|text| format!("{text}\n"))
            .map_err(ProcessCliError::Serialize);
    }
    Ok(human(value))
}

fn render_list(list: &ProcessList) -> String {
    if list.processes.is_empty() {
        return "No processes.\n".to_owned();
    }
    list.processes
        .iter()
        .map(|process| format!("{}\n", summary(process)))
        .collect()
}

fn render_output(output: &ProcessOutput) -> String {
    let mut text = output
        .chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<String>();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if output.dropped > 0 {
        text.insert_str(
            0,
            &format!(
                "[{} bytes of older output are no longer kept]\n",
                output.dropped
            ),
        );
    }
    let state = match output.state {
        ProcessState::Starting => "starting",
        ProcessState::Running => "running",
        ProcessState::Exited => "exited",
    };
    text.push_str(&format!(
        "[{} {state}; read on with --after {}]\n",
        output.process_id, output.next
    ));
    text
}

/// One line for a process: its id, where it stands, and what it runs.
fn summary(process: &RunxProcess) -> String {
    let state = match (&process.state, &process.exit) {
        (_, Some(exit)) => {
            let reason = serde_json::to_value(exit.reason)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            match (exit.code, exit.signal, &exit.message) {
                (Some(code), _, _) => format!("{reason} ({code})"),
                (None, Some(signal), _) => format!("{reason} (signal {signal})"),
                (None, None, Some(message)) => format!("{reason}: {message}"),
                (None, None, None) => reason,
            }
        }
        (ProcessState::Running, None) => "running".to_owned(),
        (_, None) => "starting".to_owned(),
    };
    let argv = std::iter::once(process.command.as_str())
        .chain(process.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{}  {state}  {argv}", process.process_id)
}
