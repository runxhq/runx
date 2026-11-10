//! What a step or an agent turn was given, as one digest, its
//! `input_digest`: the same step given the same things in another run has
//! the same digest, so a revised run can tell a result that still stands from
//! one to make again (reuse, D32).
//!
//! A digest leaves out only the identities and stamps a run puts on what it
//! gives, each at an exact path held here. Anything else is compared, so a
//! field added to a request changes its digest by default, and a `created_at`
//! inside an entry's own data is compared like the rest of it.

use serde::Serialize;

use crate::canonical_json::{CanonicalJsonError, canonical_digest};
use crate::{CallAct, ContextEntry, JsonObject, JsonValue, TurnInvocation};

/// What a turn's request leaves out, as JSON pointers into its invocation in
/// which `*` is every element of an array: the request's own address and the
/// run and step it belongs to.
pub const TURN_EXCLUDED: &[&str] = &["/id", "/envelope/run_id", "/envelope/step_id"];

/// Where a turn's request holds context entries and provenance entries.
const TURN_CONTEXT: &[&str] = &[
    "/envelope/current_context/*",
    "/envelope/historical_context/*",
];
const TURN_PROVENANCE: &[&str] = &["/envelope/provenance/*"];

/// Where a step's given value holds its context entries.
const STEP_CONTEXT: &[&str] = &["/context/*"];

/// What a context entry leaves out: its origin's artifact, run and step, and
/// the time it was stamped. The digest of what it holds stays.
pub const CONTEXT_ENTRY_EXCLUDED: &[&str] = &[
    "/origin/artifact_id",
    "/origin/run_id",
    "/origin/step_id",
    "/origin/created_at",
];

/// What a provenance entry leaves out: the artifact and receipt it names.
pub const PROVENANCE_ENTRY_EXCLUDED: &[&str] = &["/artifact_id", "/receipt_id"];

/// What an artifact leaves out wherever it is given (an object whose
/// `schema` is `runx.artifact.v1`): where it is held, when, and what made
/// it, never the digest of what it holds.
pub const ARTIFACT_EXCLUDED: &[&str] = &["/ref", "/path", "/created_at", "/produced_by"];

/// The `schema` that marks a value as an artifact.
const ARTIFACT_SCHEMA: &str = "runx.artifact.v1";

/// What a step was given: its resolved inputs, the act its child run is
/// invoked with, the context entries it is given, the reason its ask shows,
/// and a home-keyed HMAC of the values of the environment variables it
/// declares.
#[derive(Clone, Debug, Serialize)]
pub struct StepGiven<'a> {
    pub inputs: &'a JsonObject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<&'a CallAct>,
    #[serde(skip_serializing_if = "<[ContextEntry]>::is_empty")]
    pub context: &'a [ContextEntry],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<&'a str>,
}

/// A step's `input_digest`.
///
/// # Errors
///
/// Returns an error when what the step was given can't be encoded.
pub fn step_input_digest(given: &StepGiven<'_>) -> Result<String, CanonicalJsonError> {
    let mut value = encoded(given)?;
    for entry in STEP_CONTEXT {
        exclude_under(&mut value, entry, CONTEXT_ENTRY_EXCLUDED);
    }
    digest(value)
}

/// An agent turn's `input_digest`, over its request.
///
/// # Errors
///
/// Returns an error when the request can't be encoded.
pub fn turn_input_digest(invocation: &TurnInvocation) -> Result<String, CanonicalJsonError> {
    let mut value = encoded(invocation)?;
    exclude_under(&mut value, "", TURN_EXCLUDED);
    for entry in TURN_CONTEXT {
        exclude_under(&mut value, entry, CONTEXT_ENTRY_EXCLUDED);
    }
    for entry in TURN_PROVENANCE {
        exclude_under(&mut value, entry, PROVENANCE_ENTRY_EXCLUDED);
    }
    digest(value)
}

fn encoded<T: Serialize + ?Sized>(value: &T) -> Result<JsonValue, CanonicalJsonError> {
    JsonValue::from_serialize(value).map_err(|error| CanonicalJsonError::Serialization {
        message: error.to_string(),
    })
}

fn digest(mut value: JsonValue) -> Result<String, CanonicalJsonError> {
    without_artifact_identity(&mut value);
    canonical_digest(&value)
}

/// Remove each of `excluded` from every value `at` names.
fn exclude_under(value: &mut JsonValue, at: &str, excluded: &[&str]) {
    for target in matching(value, &segments(at)) {
        for path in excluded {
            remove(target, &segments(path));
        }
    }
}

fn segments(pointer: &str) -> Vec<&str> {
    pointer.split('/').skip(1).collect()
}

/// Every value `path` names, `*` naming each element of an array.
fn matching<'v>(value: &'v mut JsonValue, path: &[&str]) -> Vec<&'v mut JsonValue> {
    let Some((first, rest)) = path.split_first() else {
        return vec![value];
    };
    match (value, *first) {
        (JsonValue::Array(items), "*") => items
            .iter_mut()
            .flat_map(|item| matching(item, rest))
            .collect(),
        (JsonValue::Object(object), key) => object
            .get_mut(key)
            .map(|child| matching(child, rest))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn remove(value: &mut JsonValue, path: &[&str]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    for parent in matching(value, parents) {
        if let JsonValue::Object(object) = parent {
            object.remove(*last);
        }
    }
}

/// Leave out every artifact's own identity, wherever it is given.
fn without_artifact_identity(value: &mut JsonValue) {
    match value {
        JsonValue::Object(object) => {
            if object
                .get("schema")
                .is_some_and(|schema| schema == &JsonValue::String(ARTIFACT_SCHEMA.to_owned()))
            {
                for path in ARTIFACT_EXCLUDED {
                    if let Some(key) = segments(path).first() {
                        object.remove(*key);
                    }
                }
            }
            object.values_mut().for_each(without_artifact_identity);
        }
        JsonValue::Array(items) => items.iter_mut().for_each(without_artifact_identity),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{StepGiven, TURN_EXCLUDED, step_input_digest, turn_input_digest};
    use crate::{ContextEntry, JsonObject, JsonValue, TurnInvocation};

    fn object(value: serde_json::Value) -> JsonObject {
        serde_json::from_value(value).unwrap()
    }

    fn entry(run: &str, created_at: &str, data: serde_json::Value) -> ContextEntry {
        serde_json::from_value(serde_json::json!({
            "schema": "runx.context_entry.v1",
            "type": "note",
            "data": data,
            "labels": {"purpose": "observation", "trust": "untrusted_evidence", "sharing": "configured_model"},
            "sources": [],
            "origin": {
                "artifact_id": format!("artifact:{run}"),
                "run_id": run,
                "step_id": "step",
                "producer": {"skill": "memory", "method": "read"},
                "created_at": created_at,
                "digest": "sha256:1",
                "size_bytes": 1,
            },
        }))
        .unwrap()
    }

    fn digest(inputs: &JsonObject, context: &[ContextEntry]) -> String {
        step_input_digest(&StepGiven {
            inputs,
            act: None,
            context,
            ask_reason: None,
            environment: None,
        })
        .unwrap()
    }

    /// The same things given in two runs digest the same: an artifact's
    /// own identity and a context entry's origin are left out, the digest
    /// of what each holds is not.
    #[test]
    fn two_runs_given_the_same_things_digest_the_same() {
        let artifact = |run: &str, digest: &str| {
            object(serde_json::json!({"page": {
                "schema": "runx.artifact.v1",
                "ref": format!("runx:artifact:{run}"),
                "path": format!("/runs/{run}/page.json"),
                "created_at": format!("2026-11-0{}T00:00:00Z", run.len()),
                "produced_by": {"run_ref": {"type": "run", "uri": format!("runx:run:{run}")}},
                "digest": digest,
                "size_bytes": 4,
                "media_type": "application/json",
                "labels": {"sharing": "configured_model"},
            }}))
        };
        let first = digest(
            &artifact("a", "sha256:same"),
            &[entry(
                "a",
                "2026-11-01T00:00:00Z",
                serde_json::json!({"n": 1}),
            )],
        );
        let second = digest(
            &artifact("bb", "sha256:same"),
            &[entry(
                "bb",
                "2026-11-02T00:00:00Z",
                serde_json::json!({"n": 1}),
            )],
        );
        assert_eq!(first, second);
        assert_ne!(
            first,
            digest(
                &artifact("a", "sha256:other"),
                &[entry(
                    "a",
                    "2026-11-01T00:00:00Z",
                    serde_json::json!({"n": 1})
                )],
            ),
            "what an artifact holds is compared"
        );
    }

    /// Only the entry's stamp is left out: a `created_at` in its own data,
    /// or in an input, is compared.
    #[test]
    fn a_created_at_in_what_was_given_is_compared() {
        let inputs = |at: &str| object(serde_json::json!({"event": {"created_at": at}}));
        assert_ne!(
            digest(&inputs("2026-11-01T00:00:00Z"), &[]),
            digest(&inputs("2026-11-02T00:00:00Z"), &[])
        );
        let data = |at: &str| {
            entry(
                "a",
                "2026-11-01T00:00:00Z",
                serde_json::json!({"created_at": at}),
            )
        };
        assert_ne!(
            digest(&JsonObject::new(), &[data("2026-11-01T00:00:00Z")]),
            digest(&JsonObject::new(), &[data("2026-11-02T00:00:00Z")])
        );
    }

    fn turn(run: &str, request: &str, inputs: serde_json::Value) -> TurnInvocation {
        serde_json::from_value(serde_json::json!({
            "id": request,
            "task": "draft",
            "allowance": {},
            "envelope": {
                "run_id": run,
                "step_id": format!("{run}/draft"),
                "skill": "cold-outreach",
                "instructions": "Write it.",
                "instructions_digest": "sha256:1",
                "inputs": inputs,
                "requirements": {"execution_boundary": {"kind": "remote_provider"}},
                "current_context": [entry(run, "2026-11-01T00:00:00Z", serde_json::json!({"n": 1}))],
                "historical_context": [],
                "provenance": [{
                    "input": "offer",
                    "output": "draft",
                    "artifact_id": format!("artifact:{run}"),
                    "receipt_id": format!("receipt:{run}"),
                }],
                "trust_boundary": "test",
            },
        }))
        .unwrap()
    }

    /// A turn's request digests the same in two runs, whatever its address
    /// and run, and differently when what it asks changes. Each path it
    /// leaves out is a field a request has, so a renamed field fails here
    /// rather than being compared unseen.
    #[test]
    fn a_turn_digests_what_it_asks_and_not_where_it_runs() {
        let offer = serde_json::json!({"offer": "A review."});
        let first = turn("run-a", "prepare/3#draft", offer.clone());
        assert_eq!(
            turn_input_digest(&first).unwrap(),
            turn_input_digest(&turn("run-b", "prepare/7#draft", offer)).unwrap()
        );
        assert_ne!(
            turn_input_digest(&first).unwrap(),
            turn_input_digest(&turn(
                "run-a",
                "prepare/3#draft",
                serde_json::json!({"offer": "A call."})
            ))
            .unwrap()
        );
        let request = JsonValue::from_serialize(&first).unwrap();
        let held = serde_json::to_value(&request).unwrap();
        for path in TURN_EXCLUDED {
            assert!(held.pointer(path).is_some(), "a request has {path}");
        }
    }
}
