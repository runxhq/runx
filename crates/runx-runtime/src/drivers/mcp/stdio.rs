//! A stdio MCP server the runtime starts for a connector: its owned process
//! tree, a bounded drain of what it writes to stderr, and the client transport
//! on its stdio, framed and bounded as every runx MCP peer reads.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::adapters::mcp::{StdioJsonRpcTransport, TransportErrorState};
use crate::process::{OwnedTokioProcess, TokioProcessSpec, spawn_tokio_process};

// A tool result may carry a fully serialized native data result plus JSON-RPC
// framing. Keep that envelope bounded without capping valid operator results
// at a small size.
const MAX_CLIENT_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CLIENT_STDERR_BYTES: usize = 256 * 1024;

/// A stdio MCP server the runtime started: its process tree, held while a
/// session speaks to it and stopped when dropped.
pub(crate) struct StdioServerProcess {
    child: OwnedTokioProcess,
    _stderr_drain: Option<StderrDrain>,
    _active_process: crate::interrupt::ActiveProcessGroup,
}

impl Drop for StdioServerProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// The client transport on a started stdio server: newline-delimited and
/// bounded as every runx MCP client reads.
pub(crate) type StdioClientTransport = StdioJsonRpcTransport<
    tokio::process::ChildStdout,
    tokio::process::ChildStdin,
    rmcp::RoleClient,
>;

/// Start `command` with `args` in `cwd` and exactly `env`, inside the tokio
/// runtime that will drive its session.
pub(crate) fn start_stdio_server(
    command: &str,
    args: &[String],
    cwd: PathBuf,
    env: BTreeMap<String, String>,
) -> Result<(StdioServerProcess, StdioClientTransport), String> {
    let mut child = spawn_tokio_process(
        TokioProcessSpec::new("MCP server", command, cwd)
            .args(args.to_vec())
            .env(env),
    )
    .map_err(|error| error.to_string())?;
    let Some(process_id) = child.id() else {
        let _ = child.start_kill();
        return Err("the MCP server's process id is unavailable".to_owned());
    };
    let active_process = crate::interrupt::ActiveProcessGroup::register(process_id);
    let stderr_drain = child.take_stderr().map(spawn_stderr_drain);
    let (Some(stdout), Some(stdin)) = (child.take_stdout(), child.take_stdin()) else {
        let _ = child.start_kill();
        return Err("the MCP server's stdio is unavailable".to_owned());
    };
    let transport = StdioJsonRpcTransport::new(
        stdout,
        stdin,
        MAX_CLIENT_RESPONSE_BYTES,
        TransportErrorState::default(),
    );
    Ok((
        StdioServerProcess {
            child,
            _stderr_drain: stderr_drain,
            _active_process: active_process,
        },
        transport,
    ))
}

/// Keeps reading a server's stderr so it never blocks on a full pipe, and
/// retains only a bounded tail of it.
struct StderrDrain {
    _state: Arc<Mutex<StderrDiagnostic>>,
    _task: tokio::task::JoinHandle<()>,
}

impl StderrDrain {
    #[cfg(test)]
    async fn finish(self) -> Option<StderrDiagnosticSnapshot> {
        let Self {
            _state: state,
            _task: task,
        } = self;
        let _ = task.await;
        state.lock().ok().map(|diagnostic| diagnostic.snapshot())
    }
}

#[derive(Default)]
struct StderrDiagnostic {
    read_total: u64,
    tail: VecDeque<u8>,
}

impl StderrDiagnostic {
    fn push(&mut self, chunk: &[u8]) {
        self.read_total = self
            .read_total
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if chunk.len() >= MAX_CLIENT_STDERR_BYTES {
            self.tail.clear();
            self.tail.extend(
                chunk[chunk.len() - MAX_CLIENT_STDERR_BYTES..]
                    .iter()
                    .copied(),
            );
            return;
        }
        let excess = self
            .tail
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(MAX_CLIENT_STDERR_BYTES);
        for _ in 0..excess {
            let _ = self.tail.pop_front();
        }
        self.tail.extend(chunk.iter().copied());
    }

    #[cfg(test)]
    fn snapshot(&self) -> StderrDiagnosticSnapshot {
        StderrDiagnosticSnapshot {
            read_total: self.read_total,
            retained_bytes: self.tail.len(),
        }
    }
}

#[cfg(test)]
struct StderrDiagnosticSnapshot {
    read_total: u64,
    retained_bytes: usize,
}

fn spawn_stderr_drain<R>(mut stderr: R) -> StderrDrain
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let state = Arc::new(Mutex::new(StderrDiagnostic::default()));
    let drain_state = Arc::clone(&state);
    let task = tokio::spawn(async move {
        let mut sink = [0_u8; 8192];
        loop {
            match tokio::io::AsyncReadExt::read(&mut stderr, &mut sink).await {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    if let Ok(mut diagnostic) = drain_state.lock() {
                        diagnostic.push(&sink[..read]);
                    }
                }
            }
        }
    });
    StderrDrain {
        _state: state,
        _task: task,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::AsyncWriteExt;

    use super::{MAX_CLIENT_STDERR_BYTES, spawn_stderr_drain};

    #[test]
    // Function rationale: the stderr-drain test drives a bounded
    // async pipe end-to-end to prove bytes are drained beyond the retained tail.
    fn stderr_drain_continues_after_retained_tail_limit() -> Result<(), String> {
        tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|error| format!("tokio runtime is available: {error}"))?
            .block_on(async move {
                let (mut writer, reader) = tokio::io::duplex(4096);
                let drain = spawn_stderr_drain(reader);
                let total_bytes = MAX_CLIENT_STDERR_BYTES + (64 * 1024);
                let chunk = vec![b'x'; 8192];
                let snapshot = tokio::time::timeout(Duration::from_secs(2), async move {
                    let mut remaining = total_bytes;
                    while remaining > 0 {
                        let write_len = remaining.min(chunk.len());
                        writer
                            .write_all(&chunk[..write_len])
                            .await
                            .map_err(|error| {
                                format!("stderr writer accepts drained bytes: {error}")
                            })?;
                        remaining -= write_len;
                    }
                    drop(writer);
                    drain
                        .finish()
                        .await
                        .ok_or_else(|| "stderr diagnostic snapshot is available".to_owned())
                })
                .await
                .map_err(|_| {
                    "stderr drain kept consuming after the retained tail filled".to_owned()
                })??;

                assert_eq!(
                    snapshot.read_total,
                    u64::try_from(total_bytes)
                        .map_err(|error| format!("test byte count fits in u64: {error}"))?
                );
                assert_eq!(snapshot.retained_bytes, MAX_CLIENT_STDERR_BYTES);
                Ok(())
            })
    }
}
