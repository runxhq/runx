use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::hint::black_box;

use criterion::Criterion;
use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use runx_runtime::RuntimeEffectRegistry;
use runx_runtime::adapters::agent_loop::ToolExecutor;
use runx_runtime::adapters::agent_tools::RuntimeToolExecutor;
use tempfile::TempDir;

use super::{bool_field, record_native_metric, u64_field, wrapped_data};

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
        record_native_metric(name, &fixture.executor)
            .expect("artifact benchmark resource metric must persist");
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
        record_native_metric(name, &fixture.executor)
            .expect("artifact benchmark resource metric must persist");
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
    _directory: TempDir,
    executor: RuntimeToolExecutor,
    admit_input: JsonValue,
    artifact_ref: String,
    bytes: u64,
}

impl ArtifactFixture {
    fn new(bytes: usize) -> Result<Self, Box<dyn Error>> {
        let directory = TempDir::new()?;
        fs::write(directory.path().join("artifact.txt"), vec![b'x'; bytes])?;
        let executor = RuntimeToolExecutor::new(
            BTreeMap::from([(
                "RUNX_CWD".to_owned(),
                directory.path().to_string_lossy().into_owned(),
            )]),
            directory.path().to_path_buf(),
            RuntimeEffectRegistry::default(),
            "2026-07-20T00:00:00Z",
            ["artifact.admit".to_owned(), "artifact.read".to_owned()],
            Vec::new(),
        );
        let admit_input = JsonValue::Object(JsonObject::from([
            (
                "path".to_owned(),
                JsonValue::String("artifact.txt".to_owned()),
            ),
            (
                "media_type".to_owned(),
                JsonValue::String("text/plain".to_owned()),
            ),
        ]));
        let artifact_ref = admit(&executor, &admit_input)?;
        Ok(Self {
            _directory: directory,
            executor,
            admit_input,
            artifact_ref,
            bytes: bytes as u64,
        })
    }

    fn admit(&self) -> Result<String, Box<dyn Error>> {
        admit(&self.executor, &self.admit_input)
    }

    fn read_all(&self) -> Result<u64, Box<dyn Error>> {
        let mut offset = 0_u64;
        let mut pages = 0_u64;
        loop {
            let page = wrapped_data(
                self.executor.execute(
                    "artifact.read",
                    &JsonValue::Object(JsonObject::from([
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
                    ])),
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

fn admit(executor: &RuntimeToolExecutor, input: &JsonValue) -> Result<String, Box<dyn Error>> {
    wrapped_data(executor.execute("artifact.admit", input)?, "local_artifact")?
        .remove("artifact_ref")
        .and_then(|value| match value {
            JsonValue::String(value) => Some(value),
            _ => None,
        })
        .ok_or_else(|| std::io::Error::other("artifact admission omitted its reference").into())
}
