use std::io::{BufReader, BufWriter};
use std::sync::mpsc;
use std::thread;

use thiserror::Error;

use crate::engine::{EngineError, evaluate, script};
use crate::protocol::{
    JAVASCRIPT_STACK_BYTES, MAX_FRAME_BYTES, MAX_OUTPUT_PART_BYTES, PROTOCOL_VERSION,
    ProtocolError, ScriptEnding, ScriptPassRequest, ScriptSettleRequest, WorkerFailureCode,
    WorkerFate, WorkerInvocationRequest, WorkerRequest, WorkerResponse, read_frame, write_frame,
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

/// Serve the host's requests until it closes stdin. `allocated` reads how
/// much of the worker's memory is in use, so a script's heap is collected
/// when it fills.
pub fn serve(allocated: fn() -> usize) -> Result<(), WorkerServerError> {
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
    // An output a script returned past what its answer carries stays until
    // the next request that isn't for one of its parts.
    let mut returned: Option<Returned> = None;
    while let Some(request) = read_frame::<WorkerRequest>(&mut reader, MAX_FRAME_BYTES)? {
        if let WorkerRequest::OutputPart(part) = request {
            let Some(json) = returned
                .as_ref()
                .filter(|returned| {
                    returned.invocation_id == part.invocation_id
                        && part.protocol_version == PROTOCOL_VERSION
                })
                .and_then(|returned| output_part(&returned.json, part.offset))
            else {
                send_failure(
                    &mut writer,
                    Some(part.invocation_id),
                    WorkerFailureCode::InvalidRequest,
                    "no output a script on this worker returned has a part there",
                    WorkerFate::Discard,
                )?;
                break;
            };
            write_frame(
                &mut writer,
                &WorkerResponse::OutputPart {
                    protocol_version: PROTOCOL_VERSION,
                    invocation_id: part.invocation_id,
                    json,
                },
                MAX_FRAME_BYTES,
            )?;
            continue;
        }
        returned = None;
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
            let output_bytes = script.output_bytes;
            let response = script.settle(*settle)?;
            if !waiting(&response)
                && let Some(script) = live.take()
            {
                script.end()?;
            }
            let (response, kept) = spooled(response, output_bytes);
            returned = kept;
            write_frame(&mut writer, &response, MAX_FRAME_BYTES)?;
            continue;
        }
        if let Some(script) = live.take() {
            script.end()?;
        }
        let request = match request {
            WorkerRequest::Invoke(request) => request,
            WorkerRequest::Settle(_) | WorkerRequest::OutputPart(_) => continue,
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
                let output_bytes = request.limits.output_bytes;
                let (response, script) = LiveScriptThread::start(*request, allocated)?;
                live = script;
                let (response, kept) = spooled(response, output_bytes);
                returned = kept;
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

/// The JSON of an output a script returned past what its answer carries,
/// kept for the host to read in parts.
struct Returned {
    invocation_id: String,
    json: String,
}

/// `response`, with an output its script returned past `output_bytes` of
/// JSON named by its size alone, and that JSON kept for the host to read in
/// parts. The script's thread has ended, and its heap with it, so the worker
/// holds the output's value and its JSON, written once at its exact size,
/// and never its JavaScript as well.
fn spooled(response: WorkerResponse, output_bytes: usize) -> (WorkerResponse, Option<Returned>) {
    let WorkerResponse::Passed {
        protocol_version,
        invocation_id,
        calls,
        ending: ScriptEnding::Returned { output },
    } = response
    else {
        return (response, None);
    };
    let mut counted = Counted(0);
    let json = serde_json::to_writer(&mut counted, &output)
        .and_then(|()| {
            if counted.0 <= output_bytes {
                return Ok(None);
            }
            let mut json = Vec::with_capacity(counted.0);
            serde_json::to_writer(&mut json, &output)?;
            Ok(Some(json))
        })
        .map_err(|error| error.to_string())
        .and_then(|json| {
            json.map(String::from_utf8)
                .transpose()
                .map_err(|error| error.to_string())
        });
    match json {
        Ok(None) => (
            WorkerResponse::Passed {
                protocol_version,
                invocation_id,
                calls,
                ending: ScriptEnding::Returned { output },
            },
            None,
        ),
        Ok(Some(json)) => {
            drop(output);
            let size_bytes = u64::try_from(json.len()).unwrap_or(u64::MAX);
            (
                WorkerResponse::Passed {
                    protocol_version,
                    invocation_id: invocation_id.clone(),
                    calls,
                    ending: ScriptEnding::ReturnedInParts { size_bytes },
                },
                Some(Returned {
                    invocation_id,
                    json,
                }),
            )
        }
        Err(message) => (
            engine_failure(
                invocation_id,
                &EngineError::new(WorkerFailureCode::OutputRejected, message),
            ),
            None,
        ),
    }
}

/// A writer that keeps nothing and counts what it's given: an output's JSON
/// measured without being held.
struct Counted(usize);

impl std::io::Write for Counted {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The part of `json` from byte `offset`: at most [`MAX_OUTPUT_PART_BYTES`],
/// ending on a character. None when `offset` starts no part.
fn output_part(json: &str, offset: u64) -> Option<String> {
    let start = usize::try_from(offset).ok()?;
    if start >= json.len() || !json.is_char_boundary(start) {
        return None;
    }
    let mut end = start.saturating_add(MAX_OUTPUT_PART_BYTES).min(json.len());
    while !json.is_char_boundary(end) {
        end -= 1;
    }
    Some(json[start..end].to_owned())
}

/// A live script: the thread that owns its JavaScript context, which is
/// bound to the thread it was built on, and the channels it takes
/// settlements and gives answers by.
struct LiveScriptThread {
    invocation_id: String,
    /// The most of an output its answer carries; past it, parts.
    output_bytes: usize,
    settlements: mpsc::Sender<ScriptSettleRequest>,
    answers: mpsc::Receiver<WorkerResponse>,
    thread: thread::JoinHandle<()>,
}

impl LiveScriptThread {
    /// Run a pass on a thread of its own, with Boa's bounded stack, and keep
    /// the script there while it waits on its calls.
    fn start(
        request: ScriptPassRequest,
        allocated: fn() -> usize,
    ) -> Result<(WorkerResponse, Option<Self>), WorkerServerError> {
        let invocation_id = request.invocation_id.clone();
        let output_bytes = request.limits.output_bytes;
        let (settlements, settled) = mpsc::channel::<ScriptSettleRequest>();
        let (answer, answers) = mpsc::channel::<WorkerResponse>();
        let thread = thread::Builder::new()
            .name("runx-js-script".to_owned())
            .stack_size(JAVASCRIPT_STACK_BYTES)
            .spawn(move || {
                let (live, response) = script_pass(request, allocated);
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
            output_bytes,
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
    allocated: fn() -> usize,
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
        allocated,
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

#[cfg(test)]
mod tests {
    use super::output_part;
    use crate::protocol::MAX_OUTPUT_PART_BYTES;

    /// An output's parts are each at most the bound, each ends on a
    /// character, and in order they are the output: a three-byte character
    /// straddles every part's end at the bound here.
    #[test]
    fn an_outputs_parts_end_on_characters_within_the_bound_and_make_it_whole() {
        let json = format!("\"{}\"", "\u{20ac}".repeat(1_500_000));
        let mut whole = String::new();
        let mut parts = 0;
        while whole.len() < json.len() {
            let offset = u64::try_from(whole.len()).unwrap_or(u64::MAX);
            let part = output_part(&json, offset).unwrap_or_default();
            assert!(!part.is_empty(), "no part at {offset}");
            assert!(part.len() <= MAX_OUTPUT_PART_BYTES, "{} bytes", part.len());
            whole.push_str(&part);
            parts += 1;
        }
        assert_eq!(whole, json);
        assert_eq!(parts, 3);
        // No part starts inside a character, nor past the end.
        assert_eq!(output_part(&json, 2), None);
        assert_eq!(
            output_part(&json, u64::try_from(json.len()).unwrap_or(u64::MAX)),
            None
        );
    }
}
