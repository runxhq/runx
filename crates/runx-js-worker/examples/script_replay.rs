//! What a script step's replay costs, measured on the worker process over
//! its own protocol: a script making `CALLS` calls one after another, driven
//! three ways.
//!
//! - **Replayed:** a pass from the beginning after every settlement, which is
//!   how a step ran before the live worker. Its cost grows with the square of
//!   the calls.
//! - **Live:** one pass, then each settlement delivered to the script where
//!   it stands, which is how a step runs now.
//! - **Recovered:** one pass replaying the whole record, which is what a
//!   crash costs and what bounds a step's `max_calls`. It is measured at
//!   `CALLS` and at the kernel's ceiling, `MAX_SCRIPT_CALLS`.
//!
//! The kernel's own cost per call (admission, custody, the record) is the
//! same whichever way the script is driven, so it isn't measured here. Build
//! the worker and run it optimized, the worker beside it:
//! `cargo build --release -p runx-js-worker --bins --examples`, then
//! `target/release/examples/script_replay`.

// The measurement's report is what it prints.
#![allow(clippy::print_stdout)]

use std::collections::BTreeMap;
use std::io::{BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use runx_contracts::javascript_worker::{
    InvocationLimits, MAX_FRAME_BYTES, PROTOCOL_VERSION, ScriptCallOutcome, ScriptEnding,
    ScriptPassRequest, ScriptSettleRequest, ScriptSettlement, WorkerRequest, WorkerResponse,
    read_frame, write_frame,
};

const CALLS: u32 = 1_000;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Worker {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Worker {
    fn start() -> Result<Self> {
        let mut child = Command::new(worker()?)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = BufWriter::new(child.stdin.take().ok_or("worker stdin")?);
        let output = BufReader::new(child.stdout.take().ok_or("worker stdout")?);
        let mut worker = Self {
            child,
            input,
            output,
        };
        match worker.exchange(&WorkerRequest::Hello {
            protocol_version: PROTOCOL_VERSION,
        })? {
            WorkerResponse::Ready { .. } => Ok(worker),
            other => Err(format!("the worker didn't greet: {other:?}").into()),
        }
    }

    fn exchange(&mut self, request: &WorkerRequest) -> Result<WorkerResponse> {
        write_frame(&mut self.input, request, MAX_FRAME_BYTES)?;
        self.input.flush()?;
        read_frame(&mut self.output, MAX_FRAME_BYTES)?
            .ok_or_else(|| "the worker closed its output".into())
    }

    fn pass(&mut self, calls: u32, settled: &[ScriptSettlement]) -> Result<ScriptEnding> {
        let response = self.exchange(&WorkerRequest::Pass(Box::new(ScriptPassRequest {
            protocol_version: PROTOCOL_VERSION,
            invocation_id: "measure".to_owned(),
            entry_module: "main.mjs".to_owned(),
            export_name: "default".to_owned(),
            modules: BTreeMap::from([("main.mjs".to_owned(), script(calls))]),
            inputs: serde_json::json!({}),
            limits: InvocationLimits {
                cpu_milliseconds: runx_contracts::javascript_worker::MAX_CPU_MILLISECONDS,
                ..InvocationLimits::default()
            },
            settled: settled.to_vec(),
            now: 0,
        })))?;
        ending(response)
    }

    fn settle(&mut self, settlement: ScriptSettlement) -> Result<ScriptEnding> {
        let response = self.exchange(&WorkerRequest::Settle(Box::new(ScriptSettleRequest {
            protocol_version: PROTOCOL_VERSION,
            invocation_id: "measure".to_owned(),
            settled: vec![settlement],
            now: 0,
        })))?;
        ending(response)
    }
}

/// The worker built beside this example: `<target>/<profile>/runx-js-worker`.
fn worker() -> Result<std::path::PathBuf> {
    let current = std::env::current_exe()?;
    let profile = current
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("the example runs from its target's examples directory")?;
    let worker = profile.join(format!("runx-js-worker{}", std::env::consts::EXE_SUFFIX));
    if !worker.is_file() {
        return Err(format!("build the worker first: {} is missing", worker.display()).into());
    }
    Ok(worker)
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ending(response: WorkerResponse) -> Result<ScriptEnding> {
    match response {
        WorkerResponse::Passed { ending, .. } => Ok(ending),
        other => Err(format!("the worker answered {other:?}").into()),
    }
}

fn script(calls: u32) -> String {
    format!(
        "export default async function (ctx) {{ let total = 0; for (let call = 0; call < {calls}; call++) total += (await ctx.tool(\"count\", {{ call }})).one; return {{ total }}; }}"
    )
}

fn record(calls: u32) -> Vec<ScriptSettlement> {
    (0..calls)
        .map(|position| ScriptSettlement {
            position,
            outcome: ScriptCallOutcome::Resolved {
                value: serde_json::json!({"one": 1}),
            },
        })
        .collect()
}

fn returned(ending: &ScriptEnding, calls: u32) -> Result<()> {
    match ending {
        ScriptEnding::Returned { output } if output["total"].as_f64() == Some(f64::from(calls)) => {
            Ok(())
        }
        other => Err(format!("the script didn't return its total: {other:?}").into()),
    }
}

fn replayed(record: &[ScriptSettlement]) -> Result<Duration> {
    let mut worker = Worker::start()?;
    let started = Instant::now();
    let mut ending = ScriptEnding::Waiting;
    for settled in 0..=record.len() {
        ending = worker.pass(CALLS, &record[..settled])?;
    }
    let elapsed = started.elapsed();
    returned(&ending, CALLS)?;
    Ok(elapsed)
}

fn live(record: &[ScriptSettlement]) -> Result<Duration> {
    let mut worker = Worker::start()?;
    let started = Instant::now();
    let mut ending = worker.pass(CALLS, &[])?;
    for settlement in record {
        ending = worker.settle(settlement.clone())?;
    }
    let elapsed = started.elapsed();
    returned(&ending, CALLS)?;
    Ok(elapsed)
}

fn recovered(calls: u32, record: &[ScriptSettlement]) -> Result<Duration> {
    let mut worker = Worker::start()?;
    let started = Instant::now();
    let ending = worker.pass(calls, record)?;
    let elapsed = started.elapsed();
    returned(&ending, calls)?;
    Ok(elapsed)
}

/// The middle of three runs.
fn median(mut run: impl FnMut() -> Result<Duration>) -> Result<Duration> {
    let mut runs = [run()?, run()?, run()?];
    runs.sort();
    Ok(runs[1])
}

fn report(name: &str, calls: u32, elapsed: Duration) {
    let total = elapsed.as_secs_f64() * 1_000.0;
    println!(
        "{name:<10} {calls:>6} calls: {total:>10.1} ms, {:>8.3} ms per call",
        total / f64::from(calls)
    );
}

fn main() -> Result<()> {
    let settled = record(CALLS);
    let frame = serde_json::to_vec(&settled)?.len();
    println!("a record of {CALLS} settlements is {frame} bytes in a pass's frame");
    report("replayed", CALLS, replayed(&settled)?);
    report("live", CALLS, median(|| live(&settled))?);
    report("recovered", CALLS, median(|| recovered(CALLS, &settled))?);
    let ceiling = runx_parser::MAX_SCRIPT_CALLS;
    let full = record(ceiling);
    report("recovered", ceiling, median(|| recovered(ceiling, &full))?);
    Ok(())
}
