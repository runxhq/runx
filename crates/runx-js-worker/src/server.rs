use std::io::{BufReader, BufWriter};
use std::thread;

use thiserror::Error;

use crate::engine::{EngineError, evaluate};
use crate::protocol::{
    JAVASCRIPT_STACK_BYTES, MAX_FRAME_BYTES, PROTOCOL_VERSION, ProtocolError, WorkerDisposition,
    WorkerFailureCode, WorkerInvocationRequest, WorkerRequest, WorkerResponse, read_frame,
    write_frame,
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
                    disposition: WorkerDisposition::Discard,
                },
                MAX_FRAME_BYTES,
            )?;
            return Ok(());
        }
    }
    while let Some(request) = read_frame::<WorkerRequest>(&mut reader, MAX_FRAME_BYTES)? {
        let WorkerRequest::Invoke(request) = request else {
            send_failure(
                &mut writer,
                None,
                WorkerFailureCode::InvalidProtocol,
                "worker handshake may occur only once",
                WorkerDisposition::Discard,
            )?;
            break;
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
                WorkerDisposition::Discard,
            )?;
            break;
        }

        // This thread exists solely to give Boa the bounded 4 MiB stack that
        // the process main thread cannot acquire retroactively. The immediate
        // join is intentional: one worker process executes one invocation.
        let evaluation = thread::Builder::new()
            .name("runx-js-invocation".to_owned())
            .stack_size(JAVASCRIPT_STACK_BYTES)
            .spawn(move || {
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
            })
            .map_err(WorkerServerError::Thread)?;
        let response = evaluation
            .join()
            .map_err(|_| WorkerServerError::InvocationThreadPanicked)?;
        write_frame(&mut writer, &response, MAX_FRAME_BYTES)?;
    }

    Ok(())
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
        disposition: WorkerDisposition::Reuse,
    }
}

fn send_failure(
    writer: &mut impl std::io::Write,
    invocation_id: Option<String>,
    code: WorkerFailureCode,
    message: &str,
    disposition: WorkerDisposition,
) -> Result<(), WorkerServerError> {
    write_frame(
        writer,
        &WorkerResponse::Failure {
            protocol_version: PROTOCOL_VERSION,
            invocation_id,
            code,
            message: message.to_owned(),
            disposition,
        },
        MAX_FRAME_BYTES,
    )?;
    Ok(())
}
