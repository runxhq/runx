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

const SMALL_EVENTS: usize = 100;
const LARGE_EVENTS: usize = 1_000;
const PAGE_EVENTS: u64 = 100;
const DATA_SOURCE: &str = "benchmark://events";
const RESOURCE: &str = "benchmark_events";

pub(super) fn register(c: &mut Criterion) {
    register_continuation(c, "event_page_continuation", LARGE_EVENTS);
    register_continuation(c, "event_page_continuation_scale_small", SMALL_EVENTS);
    register_continuation(c, "event_page_continuation_scale_large", LARGE_EVENTS);
}

#[allow(clippy::expect_used)]
fn register_continuation(c: &mut Criterion, name: &'static str, event_count: usize) {
    c.bench_function(name, move |b| {
        let fixture = EventFixture::new(event_count).expect("event benchmark fixture must load");
        fixture
            .read_all()
            .expect("event benchmark smoke read must succeed");
        record_native_metric(name, &fixture.executor)
            .expect("event benchmark resource metric must persist");
        b.iter(|| {
            black_box(
                fixture
                    .read_all()
                    .expect("event continuation benchmark sample must succeed"),
            )
        })
    });
}

struct EventFixture {
    _directory: TempDir,
    executor: RuntimeToolExecutor,
    aggregate_id: String,
    event_count: usize,
}

impl EventFixture {
    fn new(event_count: usize) -> Result<Self, Box<dyn Error>> {
        let directory = TempDir::new()?;
        fs::create_dir_all(directory.path().join(".runx/data"))?;
        let data_sources = serde_json::json!({
            "data_sources": {
                (DATA_SOURCE): {
                    "adapter": "data.sqlite",
                    "database_path": ".runx/data/events.sqlite"
                }
            }
        })
        .to_string();
        let executor = RuntimeToolExecutor::new(
            BTreeMap::from([
                (
                    "RUNX_CWD".to_owned(),
                    directory.path().to_string_lossy().into_owned(),
                ),
                ("RUNX_DATA_SOURCES".to_owned(), data_sources),
            ]),
            directory.path().to_path_buf(),
            RuntimeEffectRegistry::default(),
            "2026-07-20T00:00:00Z",
            [
                "data.append_event".to_owned(),
                "data.read_events".to_owned(),
            ],
            Vec::new(),
        );
        let aggregate_id = format!("stream-{event_count}");
        for version in 0..event_count {
            let output = executor.execute(
                "data.append_event",
                &JsonValue::Object(JsonObject::from([
                    (
                        "data_source_ref".to_owned(),
                        JsonValue::String(DATA_SOURCE.to_owned()),
                    ),
                    (
                        "resource".to_owned(),
                        JsonValue::String(RESOURCE.to_owned()),
                    ),
                    (
                        "aggregate_id".to_owned(),
                        JsonValue::String(aggregate_id.clone()),
                    ),
                    (
                        "expected_version".to_owned(),
                        JsonValue::Number(JsonNumber::U64(version as u64)),
                    ),
                    (
                        "idempotency_key".to_owned(),
                        JsonValue::String(format!("event-{version}")),
                    ),
                    (
                        "event".to_owned(),
                        JsonValue::Object(JsonObject::from([
                            (
                                "type".to_owned(),
                                JsonValue::String("benchmark.event".to_owned()),
                            ),
                            (
                                "index".to_owned(),
                                JsonValue::Number(JsonNumber::U64(version as u64)),
                            ),
                        ])),
                    ),
                ])),
            )?;
            super::output_object(output)?;
        }
        Ok(Self {
            _directory: directory,
            executor,
            aggregate_id,
            event_count,
        })
    }

    fn read_all(&self) -> Result<u64, Box<dyn Error>> {
        // `after_version` selects the forward continuation contract. Omitting it is
        // intentionally a latest-tail read and therefore cannot enumerate history.
        let mut after_version = 0_u64;
        let mut events = 0_u64;
        let mut pages = 0_u64;
        loop {
            let inputs = JsonObject::from([
                (
                    "data_source_ref".to_owned(),
                    JsonValue::String(DATA_SOURCE.to_owned()),
                ),
                (
                    "resource".to_owned(),
                    JsonValue::String(RESOURCE.to_owned()),
                ),
                (
                    "aggregate_id".to_owned(),
                    JsonValue::String(self.aggregate_id.clone()),
                ),
                (
                    "limit".to_owned(),
                    JsonValue::Number(JsonNumber::U64(PAGE_EVENTS)),
                ),
                (
                    "after_version".to_owned(),
                    JsonValue::Number(JsonNumber::U64(after_version)),
                ),
            ]);
            let page = wrapped_data(
                self.executor
                    .execute("data.read_events", &JsonValue::Object(inputs))?,
                "data_operation_result",
            )?;
            let page_events = page
                .get("events")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| std::io::Error::other("event page omitted events"))?;
            if page_events.len() > PAGE_EVENTS as usize {
                return Err(std::io::Error::other("event page exceeded its declared limit").into());
            }
            events = events.saturating_add(page_events.len() as u64);
            pages = pages.saturating_add(1);
            let next = u64_field(&page, "next_after_version")?;
            let has_more = bool_field(&page, "has_more")?;
            if has_more && next <= after_version {
                return Err(std::io::Error::other("event cursor did not advance").into());
            }
            after_version = next;
            if !has_more {
                break;
            }
        }
        if events != self.event_count as u64 {
            return Err(
                std::io::Error::other("event continuation did not read every event").into(),
            );
        }
        Ok(pages)
    }
}
