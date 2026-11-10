use std::io::{BufReader, BufWriter};
use std::sync::mpsc;
use std::thread;

use thiserror::Error;

use crate::engine::{EngineError, evaluate, script};
use crate::protocol::{
    JAVASCRIPT_STACK_BYTES, MAX_FRAME_BYTES, PROTOCOL_VERSION, ProtocolError, ScriptEnding,
    ScriptPassRequest, ScriptSettleRequest, WorkerFailureCode, WorkerFate, WorkerInvocationRequest,
    WorkerRequest, WorkerResponse, read_frame, write_frame,
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
                    source_digest: env!("RUNX_JS_WORKER_SOURCE_DIGEST").into(),
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
    // A script a pass left waiting stays live until the next request that
    // isn't its settlement.
    let mut live: Option<LiveScriptThread> = None;
    while let Some(request) = read_frame::<WorkerRequest>(&mut reader, MAX_FRAME_BYTES)? {
        if let WorkerRequest::Settle(settle) = request {
            let Some(script) = live.as_ref().filter(|script| {
                script.invocation_id == settle.invocation_id
                    && settle.protocol_version == PROTOCOL_VERSION
            }) else {
                send_failure(
                    &mut writer,
                    Some(settle.invocation_id),
                    WorkerFailureCode::InvalidRequest,
                    "no live script on this worker waits for that settlement",
                    WorkerFate::Discard,
                )?;
                break;
            };
            let response = script.settle(*settle)?;
            if !waiting(&response)
                && let Some(script) = live.take()
            {
                script.end()?;
            }
            write_frame(&mut writer, &response, MAX_FRAME_BYTES)?;
            continue;
        }
        if let Some(script) = live.take() {
            script.end()?;
        }
        let request = match request {
            WorkerRequest::Invoke(request) => request,
            WorkerRequest::Settle(_) => continue,
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
                let (response, script) = LiveScriptThread::start(*request)?;
                live = script;
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

/// A live script: the thread that owns its JavaScript context, which is
/// bound to the thread it was built on, and the channels it takes
/// settlements and gives answers by.
struct LiveScriptThread {
    invocation_id: String,
    settlements: mpsc::Sender<ScriptSettleRequest>,
    answers: mpsc::Receiver<WorkerResponse>,
    thread: thread::JoinHandle<()>,
}

impl LiveScriptThread {
    /// Run a pass on a thread of its own, with Boa's bounded stack, and keep
    /// the script there while it waits on its calls.
    fn start(
        request: ScriptPassRequest,
    ) -> Result<(WorkerResponse, Option<Self>), WorkerServerError> {
        let invocation_id = request.invocation_id.clone();
        let (settlements, settled) = mpsc::channel::<ScriptSettleRequest>();
        let (answer, answers) = mpsc::channel::<WorkerResponse>();
        let thread = thread::Builder::new()
            .name("runx-js-script".to_owned())
            .stack_size(JAVASCRIPT_STACK_BYTES)
            .spawn(move || {
                let (live, response) = script_pass(request);
                if answer.send(response).is_err() {
                    return;
                }
                let Some((mut script, invocation_id)) = live else {
                    return;
                };
                while let Ok(settle) = settled.recv() {
                    let response = match script.settle(&settle.settled, settle.now) {
                        Ok(passed) => passed_response(invocation_id.clone(), passed),
                        Err(error) => engine_failure(invocation_id.clone(), &error),
                    };
                    let still = waiting(&response);
                    if answer.send(response).is_err() || !still {
                        return;
                    }
                }
            })
            .map_err(WorkerServerError::Thread)?;
        let response = answers
            .recv()
            .map_err(|_| WorkerServerError::InvocationThreadPanicked)?;
        let live = Self {
            invocation_id,
            settlements,
            answers,
            thread,
        };
        if waiting(&response) {
            return Ok((response, Some(live)));
        }
        live.end()?;
        Ok((response, None))
    }

    fn settle(&self, request: ScriptSettleRequest) -> Result<WorkerResponse, WorkerServerError> {
        self.settlements
            .send(request)
            .map_err(|_| WorkerServerError::InvocationThreadPanicked)?;
        self.answers
            .recv()
            .map_err(|_| WorkerServerError::InvocationThreadPanicked)
    }

    /// Let the script go: its thread ends with its context.
    fn end(self) -> Result<(), WorkerServerError> {
        drop(self.settlements);
        self.thread
            .join()
            .map_err(|_| WorkerServerError::InvocationThreadPanicked)
    }
}

fn waiting(response: &WorkerResponse) -> bool {
    matches!(
        response,
        WorkerResponse::Passed {
            ending: ScriptEnding::Waiting,
            ..
        }
    )
}

/// One script pass from the beginning, answered with the calls it made and
/// how it stands, and the script itself while it waits.
fn script_pass(
    request: ScriptPassRequest,
) -> (Option<(script::LiveScript, String)>, WorkerResponse) {
    let ScriptPassRequest {
        protocol_version: _,
        invocation_id,
        entry_module,
        export_name,
        modules,
        inputs,
        references,
        limits,
        settled,
        now,
    } = request;
    match script::start(script::ScriptPass {
        entry_module: &entry_module,
        export_name: &export_name,
        modules: &modules,
        inputs,
        references: &references,
        limits,
        settled: &settled,
        now,
    }) {
        Ok((live, passed)) => (
            live.map(|live| (live, invocation_id.clone())),
            passed_response(invocation_id, passed),
        ),
        Err(error) => (None, engine_failure(invocation_id, &error)),
    }
}

fn passed_response(invocation_id: String, passed: script::Passed) -> WorkerResponse {
    WorkerResponse::Passed {
        protocol_version: PROTOCOL_VERSION,
        invocation_id,
        calls: passed.calls,
        ending: passed.ending,
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
