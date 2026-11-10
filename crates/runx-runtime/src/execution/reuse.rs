//! Reuse by reference (reuse.md, D32): the index of the runs a call that
//! asked to reuse a result computed, the newest source of each identity at
//! its head. Only a call that asked indexes what it runs, so a plain call
//! pays nothing. A source stays findable for as long as its record exists.

use std::collections::BTreeMap;

use runx_contracts::input_digest::{StepGiven, step_input_digest};
use runx_contracts::{CallAct, JsonObject, RunImplementation};
use serde::{Deserialize, Serialize};

use super::workflow::LoadedStepSkill;
use crate::kernel::store::{Append, Expected, KernelStore, KernelStoreError, StreamEvent};

/// The newest source of one reuse identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum IndexEvent {
    Indexed(Source),
}

impl StreamEvent for IndexEvent {
    const KIND: &'static str = "reuse";

    fn event_type(&self) -> &'static str {
        "indexed"
    }
}

/// A run that computed a result a later call may reuse: the run, its
/// receipt, and when it sealed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub run_id: String,
    pub receipt_id: String,
    pub sealed_at: String,
}

/// What makes two calls ask for the same result: the method, the closure
/// it runs over, the implementation that runs it, the act it's given, and
/// what it's given.
#[derive(Serialize)]
struct Identity<'a> {
    method: &'a str,
    closure_digest: &'a str,
    implementation: &'a RunImplementation,
    #[serde(skip_serializing_if = "Option::is_none")]
    act: Option<&'a CallAct>,
    input_digest: &'a str,
}

/// The reuse identity of a call of `skill` given `inputs` and `act`.
///
/// # Errors
///
/// Returns why it can't be made: the package can't be read again, or its
/// closure isn't fully bound, which a run that reached this call's
/// admission has already refused; or the method runs JavaScript on a worker
/// this build doesn't know the sources of.
pub(crate) fn identity(
    skill: &LoadedStepSkill,
    inputs: &JsonObject,
    act: Option<&CallAct>,
    env: &BTreeMap<String, String>,
) -> Result<String, String> {
    let method = &skill.method.name;
    let loaded = crate::skill_package::load_validated_skill_package(&skill.directory)
        .map_err(|error| error.to_string())?;
    let closure_digest =
        crate::skill_package::verify_loaded_execution_binding(loaded, method, env, None, None)
            .map_err(|error| error.to_string())?
            .closure_digest
            .ok_or_else(|| format!("the closure of {method} isn't fully bound"))?;
    let implementation =
        crate::implementation::expected(&skill.method.workflow).ok_or_else(|| {
            format!("{method} runs JavaScript, and this build doesn't know its worker's sources")
        })?;
    let input_digest = step_input_digest(&StepGiven {
        inputs,
        act,
        context: &[],
        ask_reason: None,
        environment: None,
    })
    .map_err(|error| error.to_string())?;
    runx_contracts::canonical_json::canonical_digest(&Identity {
        method,
        closure_digest: &closure_digest,
        implementation: &implementation,
        act,
        input_digest: &input_digest,
    })
    .map_err(|error| error.to_string())
}

/// The newest source of `identity`, when one is indexed.
pub(crate) fn head(
    store: &KernelStore,
    identity: &str,
) -> Result<Option<Source>, KernelStoreError> {
    Ok(store
        .latest::<IndexEvent>(identity)?
        .map(|recorded| match recorded.event {
            IndexEvent::Indexed(source) => source,
        }))
}

/// Make `source` the newest source of `identity`. Two sources sealing at
/// once each try once more on the head the other wrote, and either standing
/// as the newest is right.
pub(crate) fn index(
    store: &KernelStore,
    identity: &str,
    source: Source,
) -> Result<(), KernelStoreError> {
    let event = IndexEvent::Indexed(source);
    let append = |store: &KernelStore| -> Result<(), KernelStoreError> {
        let version = store.version::<IndexEvent>(identity)?;
        let expected = if version == 0 {
            Expected::Absent
        } else {
            Expected::Version(version)
        };
        store
            .append(vec![Append::kernel(identity, expected, &event)?])
            .map(|_| ())
    };
    match append(store) {
        Err(KernelStoreError::Conflict { .. }) => append(store),
        settled => settled,
    }
}
