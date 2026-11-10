//! What a script step's replay costs, measured on the worker process over
//! its own protocol: a script making its calls one after another, driven
//! three ways.
//!
//! - **Replayed:** a pass from the beginning after every settlement, which is
//!   how a step ran before the live worker. Its cost grows with the square of
//!   the calls.
//! - **Live:** one pass, then each settlement delivered to the script where
//!   it stands, which is how a step runs now.
//! - **Recovered:** one pass replaying the whole record, which is what a
//!   crash costs and what bounds a step's `max_calls`. It is measured at a
//!   thousand calls and at the kernel's ceiling, `MAX_SCRIPT_CALLS`.
//!
//! Each is reported per call. The kernel's own cost per call (admission,
//! custody, the record) is the same whichever way the script is driven, so it
//! isn't measured here, and neither is starting the worker. Run it with
//! `cargo bench -p runx-js-worker --bench script_replay`.

use std::collections::BTreeMap;
use std::io::{BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use runx_contracts::javascript_worker::{
    InvocationLimits, MAX_FRAME_BYTES, PROTOCOL_VERSION, ScriptCallOutcome, ScriptEnding,
    ScriptPassRequest, ScriptSettleRequest, ScriptSettlement, WorkerRequest, WorkerResponse,
    read_frame, write_frame,
};

/// A batch's size, as the script step's contract measures it.
const BATCH: u32 = 1_000;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Worker {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Worker {
    fn start() -> Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_runx-js-worker"))
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
                references: Vec::new(),
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

fn replayed(calls: u32, record: &[ScriptSettlement]) -> Result<Duration> {
    let mut worker = Worker::start()?;
    let started = Instant::now();
    let mut ending = ScriptEnding::Waiting;
    for settled in 0..=record.len() {
        ending = worker.pass(calls, &record[..settled])?;
    }
    let elapsed = started.elapsed();
    returned(&ending, calls)?;
    Ok(elapsed)
}

fn live(calls: u32, record: &[ScriptSettlement]) -> Result<Duration> {
    let mut worker = Worker::start()?;
    let started = Instant::now();
    let mut ending = worker.pass(calls, &[])?;
    for settlement in record {
        ending = worker.settle(settlement.clone())?;
    }
    let elapsed = started.elapsed();
    returned(&ending, calls)?;
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

type Drive = fn(u32, &[ScriptSettlement]) -> Result<Duration>;

/// A drive that went wrong measures nothing: say why, and end the bench.
#[allow(clippy::print_stderr)]
fn unmeasured(error: &dyn std::error::Error) -> Duration {
    eprintln!("script_replay: {error}");
    std::process::exit(2)
}

fn script_replay(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("script_replay");
    group.sample_size(10);
    let ceiling = runx_contracts::javascript_worker::MAX_SCRIPT_CALLS;
    let drives: [(&str, Drive, u32); 4] = [
        ("replayed", replayed, BATCH),
        ("live", live, BATCH),
        ("recovered", recovered, BATCH),
        ("recovered", recovered, ceiling),
    ];
    for (name, drive, calls) in drives {
        let settled = record(calls);
        group.throughput(Throughput::Elements(u64::from(calls)));
        group.bench_with_input(BenchmarkId::new(name, calls), &settled, |bench, settled| {
            // Each sample starts its own worker, outside what it measures.
            bench.iter_custom(|samples| {
                (0..samples)
                    .map(|_| {
                        drive(calls, settled).unwrap_or_else(|error| unmeasured(error.as_ref()))
                    })
                    .sum()
            });
        });
    }
    group.finish();
}

criterion_group!(benches, script_replay);
criterion_main!(benches);
