use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::Criterion;
use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use runx_parser::{parse_graph_yaml, validate_graph};
use runx_runtime::adapters::javascript::JavaScriptAdapter;
use runx_runtime::{KernelStore, NoopHost, RunLog, Runtime, RuntimeOptions};
use tempfile::TempDir;

use super::{bool_field, u64_field, wrapped_data};

const SMALL_BYTES: usize = 256 * 1024;
const LARGE_BYTES: usize = 8 * 1024 * 1024;
const PAGE_BYTES: u64 = 64 * 1024;

pub(super) fn register(c: &mut Criterion) {
    register_admission(c, "artifact_admission", LARGE_BYTES);
    register_continuation(c, "artifact_page_continuation", LARGE_BYTES);
    register_continuation(c, "artifact_page_continuation_scale_small", SMALL_BYTES);
    register_continuation(c, "artifact_page_continuation_scale_large", LARGE_BYTES);
}

#[allow(clippy::expect_used)]
fn register_admission(c: &mut Criterion, name: &'static str, bytes: usize) {
    c.bench_function(name, move |b| {
        let fixture = ArtifactFixture::new(bytes).expect("artifact benchmark fixture must load");
        b.iter(|| {
            black_box(
                fixture
                    .admit()
                    .expect("artifact admission benchmark sample must succeed"),
            )
        })
    });
}

#[allow(clippy::expect_used)]
fn register_continuation(c: &mut Criterion, name: &'static str, bytes: usize) {
    c.bench_function(name, move |b| {
        let fixture = ArtifactFixture::new(bytes).expect("artifact benchmark fixture must load");
        fixture
            .read_all()
            .expect("artifact benchmark smoke read must succeed");
        b.iter(|| {
            black_box(
                fixture
                    .read_all()
                    .expect("artifact continuation benchmark sample must succeed"),
            )
        })
    });
}

struct ArtifactFixture {
    calls: ToolCalls,
    admit_input: JsonObject,
    artifact_ref: String,
    bytes: u64,
}

impl ArtifactFixture {
    fn new(bytes: usize) -> Result<Self, Box<dyn Error>> {
        let calls = ToolCalls::new()?;
        fs::write(calls.root().join("artifact.txt"), vec![b'x'; bytes])?;
        let admit_input = JsonObject::from([
            (
                "path".to_owned(),
                JsonValue::String("artifact.txt".to_owned()),
            ),
            (
                "media_type".to_owned(),
                JsonValue::String("text/plain".to_owned()),
            ),
        ]);
        let artifact_ref = admit(&calls, &admit_input)?;
        Ok(Self {
            calls,
            admit_input,
            artifact_ref,
            bytes: bytes as u64,
        })
    }

    fn admit(&self) -> Result<String, Box<dyn Error>> {
        admit(&self.calls, &self.admit_input)
    }

    fn read_all(&self) -> Result<u64, Box<dyn Error>> {
        let mut offset = 0_u64;
        let mut pages = 0_u64;
        loop {
            let page = wrapped_data(
                self.calls.call(
                    "artifact.read",
                    JsonObject::from([
                        (
                            "artifact_ref".to_owned(),
                            JsonValue::String(self.artifact_ref.clone()),
                        ),
                        (
                            "offset".to_owned(),
                            JsonValue::Number(JsonNumber::U64(offset)),
                        ),
                        (
                            "max_bytes".to_owned(),
                            JsonValue::Number(JsonNumber::U64(PAGE_BYTES)),
                        ),
                        ("encoding".to_owned(), JsonValue::String("utf8".to_owned())),
                    ]),
                )?,
                "artifact_page",
            )?;
            let next = u64_field(&page, "next_offset")?;
            if next <= offset || next.saturating_sub(offset) > PAGE_BYTES {
                return Err(
                    std::io::Error::other("artifact page continuation was not bounded").into(),
                );
            }
            offset = next;
            pages = pages.saturating_add(1);
            if bool_field(&page, "eof")? {
                break;
            }
        }
        if offset != self.bytes {
            return Err(
                std::io::Error::other("artifact continuation did not consume the source").into(),
            );
        }
        Ok(pages)
    }
}

/// Native tool calls as every caller makes them: each a one-step graph on a
/// run of its own, admitted, dispatched and sealed.
struct ToolCalls {
    directory: TempDir,
    store: Arc<KernelStore>,
    runtime: Runtime<JavaScriptAdapter>,
    runs: AtomicU64,
}

impl ToolCalls {
    fn new() -> Result<Self, Box<dyn Error>> {
        let directory = TempDir::new()?;
        let store = KernelStore::open(&directory.path().join(".runx"))?;
        let env = BTreeMap::from([(
            "RUNX_CWD".to_owned(),
            directory.path().to_string_lossy().into_owned(),
        )]);
        let runtime = Runtime::new(
            JavaScriptAdapter::default(),
            RuntimeOptions::new(
                env,
                crate::fixture_signature(),
                Self::evaluation(&store, "run_artifact_io")?,
            ),
        );
        Ok(Self {
            directory,
            store,
            runtime,
            runs: AtomicU64::new(0),
        })
    }

    fn root(&self) -> &std::path::Path {
        self.directory.path()
    }

    fn evaluation(store: &Arc<KernelStore>, run_id: &str) -> Result<RunLog, Box<dyn Error>> {
        Ok(RunLog::evaluation(
            Arc::clone(store),
            run_id,
            "artifact-io",
            run_id,
            "2026-07-20T00:00:00Z",
        )?)
    }

    /// The step contract `tool` produced from `inputs`.
    fn call(&self, tool: &str, inputs: JsonObject) -> Result<JsonObject, Box<dyn Error>> {
        let run_id = format!("run_artifact_{}", self.runs.fetch_add(1, Ordering::Relaxed));
        let graph = validate_graph(parse_graph_yaml(&serde_json::to_string(
            &serde_json::json!({
                "name": "artifact-io",
                "steps": [{"id": "call", "tool": tool, "inputs": inputs}],
            }),
        )?)?)?;
        let mut run = self
            .runtime
            .for_run(Self::evaluation(&self.store, &run_id)?)
            .run_graph_with_host(self.root(), graph, &mut NoopHost)?;
        let step = run
            .steps
            .pop()
            .ok_or_else(|| std::io::Error::other("the tool step did not run"))?;
        if !step.outcome.succeeded() {
            return Err(std::io::Error::other(
                step.outcome
                    .failure_message()
                    .unwrap_or_else(|| format!("{tool} failed")),
            )
            .into());
        }
        Ok(step.contract)
    }
}

fn admit(calls: &ToolCalls, input: &JsonObject) -> Result<String, Box<dyn Error>> {
    wrapped_data(
        calls.call("artifact.admit", input.clone())?,
        "local_artifact",
    )?
    .remove("artifact_ref")
    .and_then(|value| match value {
        JsonValue::String(value) => Some(value),
        _ => None,
    })
    .ok_or_else(|| std::io::Error::other("artifact admission omitted its reference").into())
}
