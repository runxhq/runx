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

/// Make `source` the newest source of `identity`.
pub(crate) fn index(
    store: &KernelStore,
    identity: &str,
    source: Source,
) -> Result<(), KernelStoreError> {
    let read = store.version::<IndexEvent>(identity)?;
    index_at(store, identity, source, read)
}

/// Make `source` the newest source of `identity`, whose head was at version
/// `read`. Another source that sealed since leaves its own head standing, and
/// either is right as the newest, so losing that race is no failure: indexing
/// never fails the call that ran its source.
fn index_at(
    store: &KernelStore,
    identity: &str,
    source: Source,
    read: u64,
) -> Result<(), KernelStoreError> {
    let expected = if read == 0 {
        Expected::Absent
    } else {
        Expected::Version(read)
    };
    match store.append(vec![Append::kernel(
        identity,
        expected,
        &IndexEvent::Indexed(source),
    )?]) {
        Ok(_) | Err(KernelStoreError::Conflict { .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Whether the run `run_id`, a source of `workflow`, shows on positive
/// evidence that runx saw all it did and all it saw (reuse.md, check 1). It
/// sealed completed, under runx's managed driver, as one method run with no
/// child runs. Every step it could run is a native tool that only reads, a
/// worker, a turn, a script, or an operation that only read, and everything
/// its turns and scripts called is a native tool that only reads, a read of
/// a result, or an operation that only read. An operation counts only when
/// its packet shows runx made the read itself, not through a process it
/// started. No person answered it, and runx's managed agent answered each
/// turn from a fresh context, stating the model that answered.
pub(crate) fn eligible(
    store: &std::sync::Arc<KernelStore>,
    run_id: &str,
    workflow: &runx_parser::Workflow,
) -> Result<bool, super::run_record::RunRecordError> {
    use super::run_record::{Origin, Replayed, RunLog};
    let log = RunLog::open(std::sync::Arc::clone(store), run_id)?;
    let Some(record) = log.load()? else {
        return Ok(false);
    };
    if record
        .sealed
        .as_ref()
        .is_none_or(|sealed| sealed.outcome != runx_contracts::RunOutcome::Completed)
    {
        return Ok(false);
    }
    let root = match &record.origin {
        Origin::Run(started) => Some(started.driver),
        _ => match RunLog::open(
            std::sync::Arc::clone(store),
            super::run_record::root_run_id(run_id),
        )?
        .load()?
        .map(|root| root.origin)
        {
            Some(Origin::Run(started)) => Some(started.driver),
            _ => None,
        },
    };
    if root != Some(runx_contracts::RunDriver::Managed)
        || !workflow.steps.iter().all(step_is_seen)
        || !record.answers.iter().all(answered_by_runx)
    {
        return Ok(false);
    }
    // What each operation step read, what each step called, and no clock
    // read, which a result read from the time can't be reused past.
    for fact in &record.facts {
        let Replayed::Step(step) = fact else {
            continue;
        };
        let operation = workflow.steps.iter().any(|defined| {
            defined.id == step.step_id
                && matches!(defined.kind, runx_parser::StepKind::Operation(_))
        });
        if operation && !read_runx_made(step) {
            return Ok(false);
        }
        let (calls, clocks) = log.step_calls(&step.step_id, step.attempt)?;
        if !clocks.is_empty() || !calls.iter().all(call_is_seen) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether runx sees all a step of this kind does.
fn step_is_seen(step: &runx_parser::WorkflowStep) -> bool {
    use runx_parser::{StepKind, ToolTarget};
    match &step.kind {
        StepKind::Tool(ToolTarget::Catalog(tool)) => crate::tool_catalogs::native::only_reads(tool),
        // An operation step is seen by what its record shows it read.
        StepKind::Tool(ToolTarget::Worker(_))
        | StepKind::Agent(_)
        | StepKind::Script(_)
        | StepKind::Operation(_) => true,
        StepKind::Tool(ToolTarget::Process(_))
        | StepKind::Ask(_)
        | StepKind::Skill(_)
        | StepKind::Load(_) => false,
    }
}

/// Whether runx sees all `call` did.
fn call_is_seen(call: &super::run_record::RecordedCall) -> bool {
    match &call.target {
        runx_contracts::CallTarget::Tool { tool } => {
            crate::tool_catalogs::native::only_reads(tool.as_str())
        }
        runx_contracts::CallTarget::Read { .. } => true,
        runx_contracts::CallTarget::Operation { .. } => matches!(
            &call.outcome,
            super::run_record::CallOutcome::Completed(run) if read_runx_made(run)
        ),
        _ => false,
    }
}

/// Whether the operation `run` made only read, and runx read it itself: its
/// packet shows read access through a transport that isn't a process runx
/// started, a local process or an MCP server that may be one.
fn read_runx_made(run: &crate::StepRun) -> bool {
    let Some(runx_contracts::JsonValue::Object(packet)) = run.output("provider_operation") else {
        return false;
    };
    let field = |name: &str| packet.get(name).and_then(runx_contracts::JsonValue::as_str);
    field("access") == Some("read")
        && field("transport")
            .is_some_and(|transport| !matches!(transport, "local_process" | "local_mcp"))
}

/// Whether runx's managed agent gave `answer`: no person did, and it states
/// the model that answered and that the turn started from a fresh context.
fn answered_by_runx(answer: &super::run_record::Answer) -> bool {
    !answer.human
        && super::outcome::stated_model(&answer.value).is_ok_and(|model| model.is_some())
        && super::outcome::states_met(&answer.value, runx_contracts::TurnRequirement::FreshContext)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::{Source, head, index_at};
    use crate::kernel::store::KernelStore;

    fn source(run: &str) -> Source {
        Source {
            run_id: run.to_owned(),
            receipt_id: format!("sha256:{run}"),
            sealed_at: "2026-11-10T00:00:00Z".to_owned(),
        }
    }

    /// Two sources sealing at once each append against the head they read.
    /// The one that loses the race leaves the other's head standing and
    /// succeeds, so indexing never fails the call that ran its source.
    #[test]
    fn a_source_that_loses_the_race_leaves_the_others_head() {
        let root = tempfile::tempdir().unwrap();
        let store = KernelStore::open(root.path()).unwrap();
        let read = store.version::<super::IndexEvent>("identity").unwrap();
        index_at(&store, "identity", source("first"), read).unwrap();
        index_at(&store, "identity", source("second"), read)
            .expect("a source that loses the race succeeds");
        assert_eq!(head(&store, "identity").unwrap(), Some(source("first")));
    }
}
