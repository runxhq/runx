//! What a wake's run is told (§6), as context entries (E2), never as inputs:
//! the loop itself, as the person wrote it, and what woke it, as evidence
//! from outside.

use runx_contracts::ContextEntry;
use serde_json::{Value, json};

use crate::LoopError;
use crate::file::digest;

/// The producer every loop entry names.
const PRODUCER: &str = "runx/loops";

/// One entry: `data` under `kind`, labelled as `purpose` and `trust`.
fn entry(
    kind: &str,
    summary: &str,
    data: Value,
    (purpose, trust): (&str, &str),
    source: &str,
    created_at: &str,
) -> Result<ContextEntry, LoopError> {
    let bytes =
        serde_json::to_vec(&data).map_err(|error| LoopError::json("a loop's context", &error))?;
    let content = digest(&bytes);
    serde_json::from_value(json!({
        "schema": "runx.context_entry.v1",
        "type": kind,
        "summary": summary,
        "data": data,
        "labels": { "purpose": purpose, "trust": trust, "sharing": "configured_model" },
        "sources": [source],
        "origin": {
            "artifact_id": digest(format!("{kind}\0{source}\0{content}").as_bytes()),
            "step_id": null,
            "producer": { "skill": PRODUCER, "method": "wake" },
            "created_at": created_at,
            "digest": content,
            "size_bytes": bytes.len(),
        },
    }))
    .map_err(|error| LoopError::json("a loop's context", &error))
}

/// The loop, as the person wrote it: its name and brief, the time now with
/// its offset, when it last settled with an answer, whether this wake is
/// late, and how the person answered its latest proposals, in their words. A
/// mandate the person confirmed.
pub(crate) fn wake(
    name: &str,
    brief: &str,
    revision: &str,
    now: &str,
    (last_settled_at, late): (Option<&str>, Option<Value>),
    answers: &[crate::state::Answer],
    (date, start, end): (&str, &str, &str),
) -> Result<ContextEntry, LoopError> {
    entry(
        "runx.loop.wake",
        &format!("Loop {name}: its brief, the time now, its day and when it last settled"),
        json!({
            "loop": name,
            "brief": brief,
            "revision": revision,
            "now": now,
            // The loop's day in its own zone, so a method counts what it did
            // today with no zone of its own.
            "day": {"date": date, "start": start, "end": end},
            "last_settled_at": last_settled_at,
            "late": late,
            "answers": answers,
        }),
        ("mandate", "person_confirmed"),
        &format!("runx:loop:{name}@{revision}"),
        now,
    )
}

/// What woke the loop: each event with its kind, identity and content. What
/// arrived from outside, a feed's event or a webhook's payload, is evidence,
/// never anyone's instruction.
pub(crate) fn batch(
    name: &str,
    events: Vec<Value>,
    (from, through): (u64, u64),
    now: &str,
) -> Result<ContextEntry, LoopError> {
    entry(
        "runx.loop.batch",
        &format!("What woke loop {name}: {} event(s)", events.len()),
        json!({ "loop": name, "from": from, "through": through, "events": events }),
        ("observation", "untrusted_evidence"),
        &format!("runx:loop:{name}/inbox@{from}..{through}"),
        now,
    )
}
