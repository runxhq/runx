use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::Path;

use criterion::Criterion;
use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use runx_parser::{ArtifactPageFraming, ArtifactPageSource, WorkerTool};
use runx_runtime::adapters::javascript::JavaScriptAdapter;
use runx_runtime::{CredentialDelivery, Executable, SkillAdapter, SkillInvocation};
use tempfile::TempDir;

use super::{output_object, u64_field};

const SMALL_RECORDS: usize = 1_500;
const REGULAR_RECORDS: usize = 4_000;
const LARGE_RECORDS: usize = 12_000;

pub(super) fn register(c: &mut Criterion) {
    register_selection(c, "twitter_archive_selection", REGULAR_RECORDS);
    register_selection(c, "twitter_archive_selection_scale_small", SMALL_RECORDS);
    register_selection(c, "twitter_archive_selection_scale_large", LARGE_RECORDS);
}

#[allow(clippy::expect_used)]
fn register_selection(c: &mut Criterion, name: &'static str, record_count: usize) {
    c.bench_function(name, move |b| {
        let fixture =
            TwitterFixture::new(record_count).expect("Twitter benchmark fixture must load");
        fixture
            .select()
            .expect("Twitter benchmark smoke selection must succeed");
        super::super::record_resource_metric(
            name,
            super::super::session_metric(fixture.adapter.session_stats()),
        )
        .expect("Twitter benchmark resource metric must persist");
        b.iter(|| {
            black_box(
                fixture
                    .select()
                    .expect("Twitter selection benchmark sample must succeed"),
            )
        })
    });
}

struct TwitterFixture {
    _directory: TempDir,
    adapter: JavaScriptAdapter,
    invocation: SkillInvocation,
    record_count: usize,
}

impl TwitterFixture {
    fn new(record_count: usize) -> Result<Self, Box<dyn Error>> {
        let directory = TempDir::new()?;
        let root = fs::canonicalize(twitter_package_path())?;
        fs::create_dir_all(directory.path().join("archive"))?;
        fs::write(
            directory.path().join("archive/tweets.js"),
            twitter_archive(record_count),
        )?;
        let invocation = SkillInvocation {
            historical_context: Vec::new(),
            skill_name: "twitter".to_owned(),
            step_id: None,
            executable: Executable::Worker(WorkerTool {
                module: "twitter-selection.mjs".to_owned(),
                export: Some("selectArchivePage".to_owned()),
                timeout_seconds: None,
                pages: Some(ArtifactPageSource {
                    path_from: "archive_file".to_owned(),
                    path_scope_from: Some("archive_base".to_owned()),
                    media_type: "application/javascript".to_owned(),
                    framing: ArtifactPageFraming::JsonArray,
                    page_bytes: 512 * 1024,
                }),
                environment: Default::default(),
            }),
            outputs: None,
            requirements: Default::default(),
            artifacts: None,
            allowed_tools: None,
            inputs: selection_inputs(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: root,
            env: BTreeMap::from([(
                "RUNX_CWD".to_owned(),
                directory.path().to_string_lossy().into_owned(),
            )]),
            credential_delivery: CredentialDelivery::none(),
        };
        Ok(Self {
            _directory: directory,
            adapter: JavaScriptAdapter::new_session(),
            invocation,
            record_count,
        })
    }

    fn select(&self) -> Result<u64, Box<dyn Error>> {
        let output = output_object(self.adapter.invoke(self.invocation.clone())?)?;
        let draft = output
            .get("twitter_selection_draft")
            .and_then(JsonValue::as_object)
            .ok_or_else(|| std::io::Error::other("Twitter selection omitted its draft"))?;
        let scanned = u64_field(draft, "scanned")?;
        if scanned != self.record_count as u64 {
            return Err(
                std::io::Error::other("Twitter selection did not scan the full archive").into(),
            );
        }
        Ok(scanned)
    }
}

fn selection_inputs() -> JsonObject {
    let predicate = JsonObject::from([
        ("is_retweet".to_owned(), JsonValue::Bool(true)),
        (
            "rt_of".to_owned(),
            JsonValue::String("RunxProof".to_owned()),
        ),
    ]);
    JsonObject::from([
        (
            "archive_file".to_owned(),
            JsonValue::String("archive/tweets.js".to_owned()),
        ),
        (
            "archive_base".to_owned(),
            JsonValue::String("workspace".to_owned()),
        ),
        (
            "objective".to_owned(),
            JsonValue::String("Measure governed archive selection.".to_owned()),
        ),
        (
            "principal".to_owned(),
            JsonValue::String("account:@performance".to_owned()),
        ),
        ("predicate".to_owned(), JsonValue::Object(predicate.clone())),
        (
            "max_acts".to_owned(),
            JsonValue::Number(JsonNumber::U64(100)),
        ),
        (
            "selection_plan".to_owned(),
            JsonValue::Object(JsonObject::from([
                ("decision".to_owned(), JsonValue::String("ready".to_owned())),
                ("target".to_owned(), JsonValue::String("posts".to_owned())),
                ("predicate".to_owned(), JsonValue::Object(predicate)),
                ("blockers".to_owned(), JsonValue::Array(Vec::new())),
            ])),
        ),
    ])
}

fn twitter_package_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/twitter")
}

fn twitter_archive(count: usize) -> String {
    let padding = "x".repeat(720);
    let mut archive = String::with_capacity(count.saturating_mul(900));
    archive.push_str("window.YTD.tweets.part0 = [\n");
    for index in 0..count {
        if index > 0 {
            archive.push_str(",\n");
        }
        let text = if index % 2_000 == 0 {
            format!("RT @RunxProof: selected-{index} {padding}")
        } else {
            format!("ordinary-{index} {padding}")
        };
        archive.push_str(
            &serde_json::json!({
                "tweet": {
                    "id_str": index.to_string(),
                    "full_text": text,
                    "favorite_count": "0",
                    "retweet_count": "0",
                    "created_at": "Mon Jan 01 00:00:00 +0000 2024"
                }
            })
            .to_string(),
        );
    }
    archive.push_str("\n];\n");
    archive
}
