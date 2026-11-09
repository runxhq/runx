use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::Criterion;
use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use runx_parser::{parse_method_manifest_yaml, validate_method_manifest};
use runx_runtime::adapters::javascript::JavaScriptAdapter;
use runx_runtime::{KernelStore, NoopHost, RunLog, Runtime, RuntimeOptions};
use tempfile::TempDir;

use super::{bool_field, u64_field, wrapped_data};

const SMALL_BYTES: usize = 256 * 1024;
const LARGE_BYTES: usize = 8 * 1024 * 1024;
const PAGE_BYTES: u64 = 64 * 1024;

#[allow(clippy::expect_used)]
pub(super) fn register(c: &mut Criterion) {
    crate::record_journey(
        "7",
        artifacts_journey().expect("the artifacts journey must read and copy each artifact"),
    )
    .expect("the artifacts journey must be recorded");
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
    reference: String,
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
        let reference = admit(&calls, &admit_input)?;
        Ok(Self {
            calls,
            admit_input,
            reference,
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
                        ("ref".to_owned(), JsonValue::String(self.reference.clone())),
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

/// Native tool calls as every caller makes them: each a one-step workflow on a
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
        let workflow = validate_method_manifest(
            "artifact-io",
            parse_method_manifest_yaml(&serde_json::to_string(&serde_json::json!({
                "minimum_runx": "0.12.0",
                "methods": {"call": {
                    "steps": [{"id": "call", "tool": tool, "inputs": inputs}],
                }},
            }))?)?,
        )?
        .methods
        .remove("call")
        .map(|method| method.workflow)
        .ok_or("the artifact call has its method")?;
        let mut run = self
            .runtime
            .for_run(Self::evaluation(&self.store, &run_id)?)
            .run_workflow_with_host(self.root(), workflow, &mut NoopHost)?;
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
    wrapped_data(calls.call("artifact.admit", input.clone())?, "artifact")?
        .remove("ref")
        .and_then(|value| match value {
            JsonValue::String(value) => Some(value),
            _ => None,
        })
        .ok_or_else(|| std::io::Error::other("artifact admission omitted its reference").into())
}

/// Journey 7's sizes.
const JOURNEY_SIZES: [(&str, usize); 3] = [
    ("1KiB", 1024),
    ("1MiB", 1024 * 1024),
    ("64MiB", 64 * 1024 * 1024),
];

/// performance.md's journey 7, artifacts, counted at each size. A page read
/// is an `artifact.read` of 64 KiB, and what it reads from the store past
/// the bytes its page holds is past its range. The store checks a snapshot
/// in chunks of 64 KiB, its unit of verification, so a page reads the chunks
/// its range covers: these pages start on a chunk, and read none past their
/// range, while a page that starts or ends inside a chunk reads up to that
/// chunk past each end, and the budget holds no more than that. A byte's
/// copies are counted at each point from the store to a worker: copied into
/// the store, read from it, and handed to the worker in a page, as a module
/// reads a JSON array artifact page by page; over the artifact's bytes, a
/// bounded number of copies reads the same at every size.
fn artifacts_journey() -> Result<JsonValue, Box<dyn Error>> {
    let reads = ToolCalls::new()?;
    let worker = PagedWorker::new()?;
    let mut copies_per_byte = JsonObject::new();
    let mut beyond_range = 0_u64;
    let mut sizes = JsonObject::new();
    for (label, bytes) in JOURNEY_SIZES {
        let pages = page_reads(&reads, bytes)?;
        beyond_range = beyond_range.max(pages.beyond_range);
        let copied = worker.count(bytes)?;
        copies_per_byte.insert(label.to_owned(), JsonValue::Number(copied.per_byte()));
        sizes.insert(
            label.to_owned(),
            JsonValue::Object(JsonObject::from([
                ("pages".to_owned(), count(pages.pages)),
                (
                    "page_store_bytes_read".to_owned(),
                    count(pages.store_bytes_read),
                ),
                (
                    "page_bytes_returned".to_owned(),
                    count(pages.bytes_returned),
                ),
                (
                    "most_page_bytes_read_beyond_range".to_owned(),
                    count(pages.beyond_range),
                ),
                ("artifact_bytes".to_owned(), count(copied.bytes)),
                ("bytes_admitted".to_owned(), count(copied.admitted)),
                ("store_bytes_read".to_owned(), count(copied.store_read)),
                ("bytes_to_worker".to_owned(), count(copied.to_worker)),
            ])),
        );
    }
    Ok(JsonValue::Object(JsonObject::from([
        (
            "copies_per_byte".to_owned(),
            JsonValue::Object(copies_per_byte),
        ),
        (
            "page_bytes_read_beyond_range".to_owned(),
            count(beyond_range),
        ),
        ("sizes".to_owned(), JsonValue::Object(sizes)),
    ])))
}

fn count(value: u64) -> JsonValue {
    JsonValue::Number(JsonNumber::U64(value))
}

struct PageReads {
    pages: u64,
    store_bytes_read: u64,
    bytes_returned: u64,
    beyond_range: u64,
}

/// An artifact of `bytes` read whole in 64 KiB pages, each counted.
fn page_reads(calls: &ToolCalls, bytes: usize) -> Result<PageReads, Box<dyn Error>> {
    fs::write(calls.root().join("journey.txt"), vec![b'x'; bytes])?;
    let reference = admit(
        calls,
        &JsonObject::from([
            (
                "path".to_owned(),
                JsonValue::String("journey.txt".to_owned()),
            ),
            (
                "media_type".to_owned(),
                JsonValue::String("text/plain".to_owned()),
            ),
        ]),
    )?;
    let mut read = PageReads {
        pages: 0,
        store_bytes_read: 0,
        bytes_returned: 0,
        beyond_range: 0,
    };
    let mut offset = 0_u64;
    loop {
        let before = runx_runtime::work_counts::snapshot();
        let page = wrapped_data(
            calls.call(
                "artifact.read",
                JsonObject::from([
                    ("ref".to_owned(), JsonValue::String(reference.clone())),
                    ("offset".to_owned(), count(offset)),
                    ("max_bytes".to_owned(), count(PAGE_BYTES)),
                    ("encoding".to_owned(), JsonValue::String("utf8".to_owned())),
                ]),
            )?,
            "artifact_page",
        )?;
        let after = runx_runtime::work_counts::snapshot();
        let next = u64_field(&page, "next_offset")?;
        let length = next.saturating_sub(offset);
        let store = after.artifact_store_bytes_read - before.artifact_store_bytes_read;
        read.pages += 1;
        read.store_bytes_read += store;
        read.bytes_returned += length;
        read.beyond_range = read.beyond_range.max(store.saturating_sub(length));
        offset = next;
        if bool_field(&page, "eof")? {
            break;
        }
    }
    if offset != bytes as u64 {
        return Err(
            std::io::Error::other("the journey's page reads did not cover the artifact").into(),
        );
    }
    Ok(read)
}

struct Copied {
    bytes: u64,
    admitted: u64,
    store_read: u64,
    to_worker: u64,
}

impl Copied {
    fn per_byte(&self) -> JsonNumber {
        let copies = self.admitted + self.store_read + self.to_worker;
        if copies.is_multiple_of(self.bytes) {
            JsonNumber::U64(copies / self.bytes)
        } else {
            JsonNumber::F64(copies as f64 / self.bytes as f64)
        }
    }
}

/// The method that counts `records.json`'s records, as the package declares
/// it.
const COUNT_PROFILE: &str = "minimum_runx: 0.12.0\nmethods:\n  count:\n    steps:\n      - id: count\n        tool:\n          module: count.mjs\n          paged_input: {path: source, media_type: application/json, framing: json_array}\n        inputs:\n          source: records.json\n        outputs:\n          records: {type: number}\n";

/// A module that counts a JSON array artifact's records, page by page, as a
/// package's own worker step reads an artifact.
struct PagedWorker {
    directory: TempDir,
    store: Arc<KernelStore>,
    runtime: Runtime<JavaScriptAdapter>,
    runs: AtomicU64,
}

impl PagedWorker {
    fn new() -> Result<Self, Box<dyn Error>> {
        let directory = TempDir::new()?;
        let package = directory.path().join("package");
        fs::create_dir_all(&package)?;
        fs::create_dir_all(directory.path().join("workspace"))?;
        fs::write(
            package.join("SKILL.md"),
            "---\nname: artifact-journey\ndescription: Count a JSON array artifact's records.\n---\n# Artifact journey\n",
        )?;
        fs::write(
            package.join("count.mjs"),
            "export default ({ runx_page: page }) => {\n  const seen = (page.state?.seen ?? 0) + page.records.length;\n  if (!page.eof) return { runx_page: { state: { seen } } };\n  return { runx_page: { state: { seen }, done: true }, records: seen };\n};\n",
        )?;
        fs::write(package.join("run.yaml"), COUNT_PROFILE)?;
        let store = KernelStore::open(&directory.path().join(".runx"))?;
        // The records are the workspace's, beside the package that reads them.
        let env = BTreeMap::from([(
            "RUNX_CWD".to_owned(),
            directory
                .path()
                .join("workspace")
                .to_string_lossy()
                .into_owned(),
        )]);
        let runtime = Runtime::new(
            JavaScriptAdapter::default(),
            RuntimeOptions::new(
                env,
                crate::fixture_signature(),
                ToolCalls::evaluation(&store, "run_artifact_journey")?,
            ),
        );
        Ok(Self {
            directory,
            store,
            runtime,
            runs: AtomicU64::new(0),
        })
    }

    /// A JSON array of about `bytes`, counted record by record, and what
    /// each copy point copied.
    fn count(&self, bytes: usize) -> Result<Copied, Box<dyn Error>> {
        let record = "x".repeat((bytes / 16).clamp(8, 1000));
        let mut array = String::from("[");
        let mut records = 0_u64;
        while array.len() + record.len() + 4 < bytes {
            if records > 0 {
                array.push(',');
            }
            array.push('"');
            array.push_str(&record);
            array.push('"');
            records += 1;
        }
        array.push(']');
        fs::write(self.directory.path().join("workspace/records.json"), &array)?;
        let workflow = validate_method_manifest(
            "artifact-journey",
            parse_method_manifest_yaml(COUNT_PROFILE)?,
        )?
        .methods
        .remove("count")
        .map(|method| method.workflow)
        .ok_or("the journey's method")?;
        let run_id = format!(
            "run_artifact_journey_{}",
            self.runs.fetch_add(1, Ordering::Relaxed)
        );
        let before = runx_runtime::work_counts::snapshot();
        let mut run = self
            .runtime
            .for_run(ToolCalls::evaluation(&self.store, &run_id)?)
            .run_workflow_with_host(
                &self.directory.path().join("package"),
                workflow,
                &mut NoopHost,
            )?;
        let after = runx_runtime::work_counts::snapshot();
        let step = run.steps.pop().ok_or("the count step did not run")?;
        let counted = step.contract.get("records").and_then(JsonValue::as_u64);
        if !step.outcome.succeeded() || counted != Some(records) {
            return Err(std::io::Error::other(format!(
                "the journey's worker counted {counted:?} of {records} records: {}",
                step.outcome.failure_message().unwrap_or_default()
            ))
            .into());
        }
        Ok(Copied {
            bytes: array.len() as u64,
            admitted: after.artifact_bytes_admitted - before.artifact_bytes_admitted,
            store_read: after.artifact_store_bytes_read - before.artifact_store_bytes_read,
            to_worker: after.artifact_bytes_to_worker - before.artifact_bytes_to_worker,
        })
    }
}
