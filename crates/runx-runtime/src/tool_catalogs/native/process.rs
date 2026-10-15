//! Native tools for detached processes: start one under runx supervision, then
//! read its output by cursor, write to its stdin or terminal, resize its
//! terminal, stop it, and list them. A run reaches the processes its actor's
//! runs started in the project.

use runx_contracts::{JsonNumber, JsonObject, JsonValue, ProcessList, ProcessOutput, RunxProcess};

use super::argv::{self, ArgvRequest};
use super::{NativeInvocation, invalid_input};
use crate::RuntimeError;
use crate::process::detached::{Processes, Reach, Start};
use crate::process_invocation::{
    NativeCommandInvocationRequest, prepare_native_command_invocation,
};

mod capability;

pub(super) use capability::CAPABILITIES;
use capability::{
    ProcessIdInput, ProcessListInput, ProcessReadInput, ProcessResizeInput, ProcessStartInput,
    ProcessWriteInput,
};

const START: &str = "process.start";
const MAX_SECONDS: u64 = 86_400;
const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_TERMINAL_CELLS: u16 = 1_000;

fn start(
    invocation: &NativeInvocation<'_, ProcessStartInput>,
) -> Result<RunxProcess, RuntimeError> {
    let call = invocation
        .call
        .ok_or_else(|| invalid_input(START, "a process is started by a run's call"))?;
    let argv = admit(invocation)?;
    invocation
        .credential_delivery
        .reject_process_env_boundary("native process.start")
        .map_err(|error| invalid_input(START, error.to_string()))?;
    let process = prepare_native_command_invocation(NativeCommandInvocationRequest {
        command: argv.command,
        args: argv.args,
        cwd: &argv.cwd,
        workspace_root: &argv.repo_root,
        explicit_env: &argv.explicit_env,
        base_env: invocation.env,
    })?
    .into_execution_plan();
    processes(invocation, START)?
        .start(Start {
            call,
            command: process.command,
            args: process.args,
            cwd: process.cwd,
            env: process.env,
            tty: invocation.inputs.tty,
            max_seconds: invocation.inputs.max_seconds,
        })
        .map_err(|error| invalid_input(START, error.to_string()))
}

/// What `process.start` would run: the exact argv the person approving it
/// reads, where, for how long, and in what terminal.
fn start_effect(
    invocation: &NativeInvocation<'_, ProcessStartInput>,
) -> Result<Option<crate::capability::LocalEffect>, RuntimeError> {
    let argv = admit(invocation)?;
    let inputs = invocation.inputs;
    let mut payload = JsonObject::from([
        ("kind".to_owned(), JsonValue::String("process".to_owned())),
        (
            "argv".to_owned(),
            JsonValue::Array(argv.argv().into_iter().map(JsonValue::String).collect()),
        ),
        (
            "cwd".to_owned(),
            JsonValue::String(argv.cwd_relative.clone()),
        ),
        (
            "max_seconds".to_owned(),
            JsonValue::Number(JsonNumber::U64(inputs.max_seconds)),
        ),
        (
            "env_names".to_owned(),
            JsonValue::Array(
                argv.explicit_env
                    .keys()
                    .cloned()
                    .map(JsonValue::String)
                    .collect(),
            ),
        ),
    ]);
    if let Some(tty) = inputs.tty {
        payload.insert(
            "tty".to_owned(),
            JsonValue::from_serialize(&tty)
                .map_err(|error| invalid_input(START, error.to_string()))?,
        );
    }
    Ok(Some(crate::capability::LocalEffect {
        payload: JsonValue::Object(payload),
        paths: Vec::new(),
        argv: Some(argv.argv()),
    }))
}

/// A process start's argv, its limits and its terminal, admitted.
fn admit(invocation: &NativeInvocation<'_, ProcessStartInput>) -> Result<argv::Argv, RuntimeError> {
    let inputs = invocation.inputs;
    if !(1..=MAX_SECONDS).contains(&inputs.max_seconds) {
        return Err(invalid_input(
            START,
            format!("max_seconds must be 1-{MAX_SECONDS}"),
        ));
    }
    if let Some(tty) = inputs.tty {
        if !cfg!(unix) {
            return Err(invalid_input(START, "a terminal needs a Unix host"));
        }
        if !(1..=MAX_TERMINAL_CELLS).contains(&tty.rows)
            || !(1..=MAX_TERMINAL_CELLS).contains(&tty.cols)
        {
            return Err(invalid_input(
                START,
                format!("tty rows and cols must be 1-{MAX_TERMINAL_CELLS}"),
            ));
        }
    }
    argv::admit(
        START,
        invocation,
        ArgvRequest {
            repo_root: &inputs.repo_root,
            command: &inputs.command,
            args: &inputs.args,
            cwd: &inputs.cwd,
            env: &inputs.env,
        },
    )
}

fn read(
    invocation: &NativeInvocation<'_, ProcessReadInput>,
) -> Result<ProcessOutput, RuntimeError> {
    let inputs = invocation.inputs;
    processes(invocation, "process.read")?
        .read(
            &inputs.process_id,
            reach(invocation),
            inputs.after,
            usize::try_from(inputs.max_bytes).unwrap_or(usize::MAX),
        )
        .map_err(|error| invalid_input("process.read", error.to_string()))
}

fn write(
    invocation: &NativeInvocation<'_, ProcessWriteInput>,
) -> Result<RunxProcess, RuntimeError> {
    let inputs = invocation.inputs;
    if inputs.input.is_empty() || inputs.input.len() > MAX_INPUT_BYTES {
        return Err(invalid_input(
            "process.write",
            format!("input must be 1-{MAX_INPUT_BYTES} bytes"),
        ));
    }
    processes(invocation, "process.write")?
        .write(
            &inputs.process_id,
            reach(invocation),
            invocation.call.map(|call| call.key.as_str()),
            inputs.input.clone(),
        )
        .map_err(|error| invalid_input("process.write", error.to_string()))
}

fn resize(
    invocation: &NativeInvocation<'_, ProcessResizeInput>,
) -> Result<RunxProcess, RuntimeError> {
    let inputs = invocation.inputs;
    if !(1..=MAX_TERMINAL_CELLS).contains(&inputs.rows)
        || !(1..=MAX_TERMINAL_CELLS).contains(&inputs.cols)
    {
        return Err(invalid_input(
            "process.resize",
            format!("rows and cols must be 1-{MAX_TERMINAL_CELLS}"),
        ));
    }
    processes(invocation, "process.resize")?
        .resize(
            &inputs.process_id,
            reach(invocation),
            runx_contracts::TerminalSize {
                rows: inputs.rows,
                cols: inputs.cols,
            },
        )
        .map_err(|error| invalid_input("process.resize", error.to_string()))
}

fn stop(invocation: &NativeInvocation<'_, ProcessIdInput>) -> Result<RunxProcess, RuntimeError> {
    processes(invocation, "process.stop")?
        .stop(&invocation.inputs.process_id, reach(invocation))
        .map_err(|error| invalid_input("process.stop", error.to_string()))
}

fn list(invocation: &NativeInvocation<'_, ProcessListInput>) -> Result<ProcessList, RuntimeError> {
    processes(invocation, "process.list")?
        .list(reach(invocation))
        .map_err(|error| invalid_input("process.list", error.to_string()))
}

/// Writes, resizes and stops reach a process runx already supervises; the
/// floor holds only its start.
fn no_effect<I>(
    _invocation: &NativeInvocation<'_, I>,
) -> Result<Option<crate::capability::LocalEffect>, RuntimeError> {
    Ok(None)
}

fn processes<I: ?Sized>(
    invocation: &NativeInvocation<'_, I>,
    tool: &str,
) -> Result<Processes, RuntimeError> {
    Processes::of(&crate::paths::RunxPaths::resolve(
        invocation.env,
        invocation.skill_directory,
    ))
    .map_err(|error| invalid_input(tool, error.to_string()))
}

/// The processes the calling run reaches: its actor's.
fn reach<'a, I: ?Sized>(invocation: &NativeInvocation<'a, I>) -> Reach<'a> {
    Reach::Actor(invocation.call.and_then(|call| call.actor.as_deref()))
}
