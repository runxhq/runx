use std::io::{BufReader, BufWriter};
use std::thread;

use thiserror::Error;

use crate::engine::{EngineError, evaluate, script};
use crate::protocol::{
    JAVASCRIPT_STACK_BYTES, MAX_FRAME_BYTES, PROTOCOL_VERSION, ProtocolError, ScriptPassRequest,
    WorkerFailureCode, WorkerFate, WorkerInvocationRequest, WorkerRequest, WorkerResponse,
    read_frame, write_frame,
};

#[derive(Debug, Error)]
pub enum WorkerServerError {
    #[error("worker limits could not be installed: {0}")]
    Limits(String),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("worker request thread could not be created: {0}")]
    Thread(std::io::Error),
    #[error("worker invocation thread panicked")]
    InvocationThreadPanicked,
}

pub fn serve() -> Result<(), WorkerServerError> {
    crate::limits::install().map_err(|error| WorkerServerError::Limits(error.to_string()))?;
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    let Some(request) = read_frame::<WorkerRequest>(&mut reader, MAX_FRAME_BYTES)? else {
        return Ok(());
    };
    match request {
        WorkerRequest::Hello { protocol_version } if protocol_version == PROTOCOL_VERSION => {
            write_frame(
                &mut writer,
                &WorkerResponse::Ready {
                    protocol_version: PROTOCOL_VERSION,
                },
                MAX_FRAME_BYTES,
            )?;
        }
        _ => {
            write_frame(
                &mut writer,
                &WorkerResponse::Failure {
                    protocol_version: PROTOCOL_VERSION,
                    invocation_id: None,
                    code: WorkerFailureCode::InvalidProtocol,
                    message: "worker protocol handshake mismatch".to_owned(),
                    fate: WorkerFate::Discard,
                },
                MAX_FRAME_BYTES,
            )?;
            return Ok(());
        }
    }
    while let Some(request) = read_frame::<WorkerRequest>(&mut reader, MAX_FRAME_BYTES)? {
        let request = match request {
            WorkerRequest::Invoke(request) => request,
            WorkerRequest::Pass(request) => {
                if request.protocol_version != PROTOCOL_VERSION {
                    send_failure(
                        &mut writer,
                        Some(request.invocation_id),
                        WorkerFailureCode::InvalidProtocol,
                        "worker invocation protocol mismatch",
                        WorkerFate::Discard,
                    )?;
                    break;
                }
                let response = on_invocation_thread(move || script_pass(*request))?;
                write_frame(&mut writer, &response, MAX_FRAME_BYTES)?;
                continue;
            }
            WorkerRequest::Hello { .. } => {
                send_failure(
                    &mut writer,
                    None,
                    WorkerFailureCode::InvalidProtocol,
                    "worker handshake may occur only once",
                    WorkerFate::Discard,
                )?;
                break;
            }
        };
        let WorkerInvocationRequest {
            protocol_version,
            invocation_id,
            entry_module,
            export_name,
            modules,
            inputs,
            environment,
            limits,
        } = *request;
        if protocol_version != PROTOCOL_VERSION {
            send_failure(
                &mut writer,
                Some(invocation_id),
                WorkerFailureCode::InvalidProtocol,
                "worker invocation protocol mismatch",
                WorkerFate::Discard,
            )?;
            break;
        }

        let response = on_invocation_thread(move || {
            match evaluate(
                &entry_module,
                &export_name,
                &modules,
                inputs,
                environment,
                limits,
            ) {
                Ok(output) => WorkerResponse::Result {
                    protocol_version: PROTOCOL_VERSION,
                    invocation_id,
                    output,
                },
                Err(error) => engine_failure(invocation_id, &error),
            }
        })?;
        write_frame(&mut writer, &response, MAX_FRAME_BYTES)?;
    }

    Ok(())
}

/// Run `work` on a thread of its own. The thread exists solely to give Boa
/// the bounded 4 MiB stack that the process main thread cannot acquire
/// retroactively; the immediate join is intentional, since one worker
/// process executes one invocation at a time.
fn on_invocation_thread(
    work: impl FnOnce() -> WorkerResponse + Send + 'static,
) -> Result<WorkerResponse, WorkerServerError> {
    thread::Builder::new()
        .name("runx-js-invocation".to_owned())
        .stack_size(JAVASCRIPT_STACK_BYTES)
        .spawn(work)
        .map_err(WorkerServerError::Thread)?
        .join()
        .map_err(|_| WorkerServerError::InvocationThreadPanicked)
}

/// One script pass, answered with the calls it made and how it ended.
fn script_pass(request: ScriptPassRequest) -> WorkerResponse {
    let ScriptPassRequest {
        protocol_version: _,
        invocation_id,
        entry_module,
        export_name,
        modules,
        inputs,
        limits,
        settled,
        now,
    } = request;
    match script::pass(script::ScriptPass {
        entry_module: &entry_module,
        export_name: &export_name,
        modules: &modules,
        inputs,
        limits,
        settled: &settled,
        now,
    }) {
        Ok(passed) => WorkerResponse::Passed {
            protocol_version: PROTOCOL_VERSION,
            invocation_id,
            calls: passed.calls,
            ending: passed.ending,
        },
        Err(error) => engine_failure(invocation_id, &error),
    }
}

fn engine_failure(invocation_id: String, error: &EngineError) -> WorkerResponse {
    WorkerResponse::Failure {
        protocol_version: PROTOCOL_VERSION,
        invocation_id: Some(invocation_id),
        code: error.code,
        message: error.message.clone(),
        // Each invocation receives a fresh Boa context and module loader. A
        // typed module/execution failure therefore invalidates only this
        // invocation; process retirement is reserved for protocol or process
        // lifecycle failures.
        fate: WorkerFate::Reuse,
    }
}

fn send_failure(
    writer: &mut impl std::io::Write,
    invocation_id: Option<String>,
    code: WorkerFailureCode,
    message: &str,
    fate: WorkerFate,
) -> Result<(), WorkerServerError> {
    write_frame(
        writer,
        &WorkerResponse::Failure {
            protocol_version: PROTOCOL_VERSION,
            invocation_id,
            code,
            message: message.to_owned(),
            fate,
        },
        MAX_FRAME_BYTES,
    )?;
    Ok(())
}
