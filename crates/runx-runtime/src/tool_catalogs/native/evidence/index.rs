use std::collections::BTreeSet;

use runx_contracts::{JsonNumber, JsonObject, JsonValue, canonical_stable_json, sha256_prefixed};

use super::EvidenceIndexInput;
use super::source::{IndexedSource, unwrap_source_packet};
use crate::RuntimeError;

const TOOL: &str = "evidence.index_sources";
pub(super) fn build(
    inputs: &EvidenceIndexInput,
    observed_at: &str,
) -> Result<JsonValue, RuntimeError> {
    let objective = inputs.objective.trim().to_owned();
    let supplied = inputs.source_packets.as_slice();
    let limits = Limits {
        max_sources: inputs.max_sources,
        max_source_characters: inputs.max_source_characters,
        max_total_characters: inputs.max_total_characters,
    };
    let mut blockers = initial_blockers(&objective, supplied.len(), limits.max_sources);
    let (sources, indexed_characters) = index_packets(
        supplied,
        limits.max_sources,
        limits.max_source_characters,
        observed_at,
        &mut blockers,
    );
    if indexed_characters > limits.max_total_characters {
        blockers.push(format!(
            "indexed text exceeds {} characters",
            limits.max_total_characters
        ));
    }
    let sources = deduplicate(sources);
    let canonical = canonical_stable_json(&JsonValue::Array(
        sources.iter().map(IndexedSource::index_material).collect(),
    ))
    .map_err(|error| invalid(format!("source index canonicalization failed: {error}")))?;

    Ok(wrap_index(index_fields(
        objective,
        &sources,
        blockers,
        sha256_prefixed(canonical.as_bytes()),
        limits.report(supplied.len(), sources.len(), indexed_characters),
    )))
}

#[derive(Clone, Copy)]
struct Limits {
    max_sources: u64,
    max_source_characters: u64,
    max_total_characters: u64,
}

impl Limits {
    fn report(self, supplied: usize, indexed: usize, characters: u64) -> JsonValue {
        JsonValue::Object(JsonObject::from([
            ("max_sources".to_owned(), number(self.max_sources)),
            (
                "max_source_characters".to_owned(),
                number(self.max_source_characters),
            ),
            (
                "max_total_characters".to_owned(),
                number(self.max_total_characters),
            ),
            ("supplied_sources".to_owned(), number(supplied as u64)),
            ("indexed_sources".to_owned(), number(indexed as u64)),
            ("indexed_characters".to_owned(), number(characters)),
        ]))
    }
}

fn initial_blockers(objective: &str, supplied: usize, max_sources: u64) -> Vec<String> {
    let mut blockers = Vec::new();
    if objective.is_empty() {
        blockers.push("objective is missing".to_owned());
    }
    if supplied == 0 {
        blockers.push("source_packets is empty".to_owned());
    }
    if supplied as u64 > max_sources {
        blockers.push(format!("source_packets exceeds {max_sources}"));
    }
    blockers
}

fn index_packets(
    supplied: &[JsonValue],
    max_sources: u64,
    max_source_characters: u64,
    observed_at: &str,
    blockers: &mut Vec<String>,
) -> (Vec<IndexedSource>, u64) {
    let mut sources = Vec::new();
    let mut characters = 0_u64;
    for (index, raw) in supplied.iter().take(max_sources as usize).enumerate() {
        match IndexedSource::from_packet(
            unwrap_source_packet(raw),
            observed_at,
            max_source_characters,
        ) {
            Ok(source) => {
                characters = characters.saturating_add(source.character_count());
                sources.push(source);
            }
            Err(local) => blockers.push(format!("source_packets[{index}]: {}", local.join(", "))),
        }
    }
    (sources, characters)
}

fn index_fields(
    objective: String,
    sources: &[IndexedSource],
    blockers: Vec<String>,
    index_digest: String,
    limits: JsonValue,
) -> JsonObject {
    let decision = if blockers.is_empty() {
        "ready"
    } else {
        "needs_more_evidence"
    };
    JsonObject::from([
        (
            "decision".to_owned(),
            JsonValue::String(decision.to_owned()),
        ),
        ("objective".to_owned(), JsonValue::String(objective)),
        (
            "sources".to_owned(),
            JsonValue::Array(sources.iter().map(IndexedSource::as_json).collect()),
        ),
        (
            "source_digests".to_owned(),
            JsonValue::Array(sources.iter().map(IndexedSource::digest_json).collect()),
        ),
        (
            "source_evidence".to_owned(),
            JsonValue::Array(sources.iter().map(IndexedSource::evidence_json).collect()),
        ),
        ("index_digest".to_owned(), JsonValue::String(index_digest)),
        (
            "blockers".to_owned(),
            JsonValue::Array(blockers.into_iter().map(JsonValue::String).collect()),
        ),
        ("limits".to_owned(), limits),
    ])
}

fn wrap_index(index: JsonObject) -> JsonValue {
    JsonValue::Object(JsonObject::from([(
        "source_index".to_owned(),
        JsonValue::Object(index),
    )]))
}

fn deduplicate(sources: Vec<IndexedSource>) -> Vec<IndexedSource> {
    let mut seen = BTreeSet::new();
    sources
        .into_iter()
        .filter(|source| seen.insert(source.digest().to_owned()))
        .collect()
}

fn number(value: u64) -> JsonValue {
    JsonValue::Number(JsonNumber::U64(value))
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::failed(TOOL.to_owned(), message.into())
}
