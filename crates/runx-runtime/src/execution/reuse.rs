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
/// Returns why it can't be made: the package can't be read again, its
/// closure isn't fully bound, or the method runs JavaScript on a worker this
/// build doesn't know the sources of. Such a call is never failed for asking:
/// [`decide`] runs it as a plain call would.
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

/// What a call that asked to reuse a result is answered, given its
/// `identity` or why it has none, and the identity its child is indexed
/// under. A call whose identity can't be made recomputes as no identity: it
/// runs as a plain call would, and indexes nothing. A source that isn't
/// eligible recomputes as ineligible, and one whose reads this run may not
/// make again, as `allowed` decides, recomputes as not allowed (reuse.md,
/// checks 1 and 2). Until check 3 compares what each step was given, an
/// allowed source isn't shown current, so it recomputes as changed.
pub(crate) fn decide(
    store: &std::sync::Arc<KernelStore>,
    identity: Result<String, String>,
    workflow: &runx_parser::Workflow,
    allowed: impl FnOnce(&Source) -> bool,
) -> Result<(runx_contracts::ReuseDecision, Option<String>), super::run_record::RunRecordError> {
    use runx_contracts::RecomputeReason;
    let recomputed = |reason| runx_contracts::ReuseDecision::Recomputed { reason };
    let Ok(identity) = identity else {
        return Ok((recomputed(RecomputeReason::NoIdentity), None));
    };
    let reason = match head(store, &identity)? {
        None => RecomputeReason::NoCandidate,
        Some(source) if !eligible(store, &source.run_id, workflow)? => RecomputeReason::Ineligible,
        Some(source) if !allowed(&source) => RecomputeReason::NotAllowed,
        Some(_) => RecomputeReason::Changed,
    };
    Ok((recomputed(reason), Some(identity)))
}

/// What a source read that a run reusing it would read again: each read
/// through an operation, as the step it ran as with the inputs it was
/// called with, and each read of the workspace's files, as the tool and the
/// input its record shows.
pub(crate) struct Reads {
    /// Each operation read, its step's `inputs` what it was called with.
    pub operations: Vec<runx_parser::WorkflowStep>,
    files: Vec<FileRead>,
}

/// A tool's read of the workspace's files: how its method and step narrow
/// a run's reach, the tool, and the input it was given.
struct FileRead {
    narrowing: Option<runx_contracts::Reach>,
    tool: String,
    input: JsonObject,
}

impl Reads {
    /// Whether each tool that read files would be let read under what it was
    /// given again, within the reach its step has under `authority`, by the
    /// check the tool itself makes before it reads.
    pub(crate) fn files_allowed(
        &self,
        authority: Option<&crate::RunAuthority>,
        env: &BTreeMap<String, String>,
        skill_directory: &std::path::Path,
    ) -> bool {
        let reach =
            authority.map_or_else(runx_contracts::Reach::default, |authority| authority.reach);
        self.files.iter().all(|read| {
            file_read_allowed(read, reach.narrowed(read.narrowing), env, skill_directory)
        })
    }
}

/// Whether `read` passes the check its tool makes before it reads, under
/// `reach`: its root, unless it's the skill's own directory, and each path
/// it names under the root. A tool that reads beyond the paths it's given
/// checks only its root, so only its root is checked. A root that no longer
/// exists isn't refused: what it read changed, which the current check
/// answers, and authority didn't refuse it.
fn file_read_allowed(
    read: &FileRead,
    reach: runx_contracts::Reach,
    env: &BTreeMap<String, String>,
    skill_directory: &std::path::Path,
) -> bool {
    use crate::services::{ReadArgument, admit_read};
    use std::path::Path;
    let text = |name: &str| {
        read.input
            .get(name)
            .and_then(runx_contracts::JsonValue::as_str)
    };
    if text("path_scope").unwrap_or("workspace") != "skill" {
        let named = Path::new(text("repo_root").unwrap_or("."));
        let workspace = crate::RunxPaths::resolve(env, skill_directory)
            .workspace()
            .to_path_buf();
        let resolved = std::fs::canonicalize(workspace).and_then(|workspace| {
            let root = if named.is_absolute() {
                named.to_path_buf()
            } else {
                workspace.join(named)
            };
            Ok((std::fs::canonicalize(root)?, workspace))
        });
        if let Ok((root, workspace)) = resolved {
            let root = ReadArgument::Root {
                named,
                root: &root,
                workspace: &workspace,
            };
            if admit_read(reach, root).is_err() {
                return false;
            }
        }
    }
    let named = match read.tool.as_str() {
        "fs.read" | "git.show" => text("path").into_iter().collect(),
        "fs.list" | "fs.search" => vec![text("path").unwrap_or(".")],
        "fs.read_bundle" => read
            .input
            .get("paths")
            .and_then(runx_contracts::JsonValue::as_array)
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(runx_contracts::JsonValue::as_str)
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    named
        .iter()
        .all(|path| admit_read(reach, ReadArgument::Named(Path::new(path))).is_ok())
}

/// What the source `run_id` of `workflow` read: each operation its steps
/// and their calls read through, rebuilt as its run made it, from the inputs
/// it was given, the act it was stated and what each earlier step returned,
/// and the reach of each step that read files.
///
/// # Errors
///
/// Returns why a read can't be rebuilt, which a reuse check takes as a read
/// it can't authorize.
pub(crate) fn reads(
    store: &std::sync::Arc<KernelStore>,
    run_id: &str,
    workflow: &runx_parser::Workflow,
    workspace: Option<&crate::paths::RunxPaths>,
) -> Result<Reads, crate::RuntimeError> {
    use super::run_record::{Replayed, RunLog, call_step_id};
    use runx_parser::{StepKind, ToolTarget};
    let log = RunLog::open(std::sync::Arc::clone(store), run_id)?;
    let record = log
        .load()?
        .ok_or_else(|| crate::RuntimeError::EngineInvariant {
            context: "reading a reuse source",
            message: format!("run {run_id} has no record"),
        })?;
    let bound = super::workflow::bind_run_inputs(
        workflow.clone(),
        &record.inputs,
        record.act.as_ref(),
        run_id,
        workspace,
    )?;
    let runs = record
        .facts
        .iter()
        .filter_map(|fact| match fact {
            Replayed::Step(step) => Some(step.as_ref().clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let positions = runs
        .iter()
        .enumerate()
        .map(|(position, run)| (run.step_id.clone(), position))
        .collect::<BTreeMap<_, _>>();
    let prior = super::workflow_index::PriorRunIndex::from_positions(&runs, &positions);
    let narrowing = |step: &runx_parser::WorkflowStep| match (bound.reach, step.reach) {
        (Some(method), Some(step)) => Some(method.min(step)),
        (method, step) => method.or(step),
    };
    let reads_files =
        |tool: &str| crate::tool_catalogs::native::workspace_lock_mode(tool).is_some();
    let mut reads = Reads {
        operations: Vec::new(),
        files: Vec::new(),
    };
    for run in &runs {
        let defined = bound
            .steps
            .iter()
            .find(|defined| defined.id == run.step_id)
            .ok_or_else(|| crate::RuntimeError::StepMissing {
                step_id: run.step_id.clone(),
            })?;
        let connectors = match &defined.kind {
            StepKind::Operation(_) => {
                let inputs = super::workflow::materialize_step_invocation_inputs_with_index(
                    defined, &prior,
                )?;
                reads.operations.push(runx_parser::WorkflowStep {
                    inputs,
                    ..defined.clone()
                });
                None
            }
            StepKind::Tool(ToolTarget::Catalog(tool)) => {
                if reads_files(tool) {
                    reads.files.push(FileRead {
                        narrowing: narrowing(defined),
                        tool: tool.clone(),
                        input: super::workflow::materialize_step_invocation_inputs_with_index(
                            defined, &prior,
                        )?,
                    });
                }
                None
            }
            StepKind::Agent(agent) => Some(&agent.connectors),
            StepKind::Script(script) => Some(&script.connectors),
            _ => None,
        };
        let (calls, _) = log.step_calls(&run.step_id, run.attempt)?;
        for call in calls {
            match &call.target {
                runx_contracts::CallTarget::Operation {
                    requirement,
                    operation,
                } => {
                    let declared = connectors
                        .and_then(|connectors| connectors.get(requirement.as_str()))
                        .cloned();
                    let (target, inputs) = super::engine::turn::operation_call_step(
                        requirement.as_str(),
                        operation.as_str(),
                        &call.input,
                        declared,
                        None,
                    )
                    .map_err(|message| crate::RuntimeError::StepFailed {
                        step_id: call_step_id(&run.step_id, call.index),
                        reason: message,
                    })?;
                    reads.operations.push(runx_parser::WorkflowStep {
                        id: call_step_id(&run.step_id, call.index),
                        kind: StepKind::Operation(target),
                        inputs,
                        bindings: Vec::new(),
                        act: None,
                        when: None,
                        guard: Vec::new(),
                        retry_unknown_outcomes: false,
                        ..defined.clone()
                    });
                }
                runx_contracts::CallTarget::Tool { tool } if reads_files(tool.as_str()) => {
                    reads.files.push(FileRead {
                        narrowing: narrowing(defined),
                        tool: tool.as_str().to_owned(),
                        input: call.input.clone(),
                    });
                }
                _ => {}
            }
        }
    }
    Ok(reads)
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
/// started. No person answered it, except to approve spending on a metered
/// read, and runx's managed agent answered each turn from a fresh context,
/// stating the model that answered.
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
        || !record.answers.iter().all(answer_allowed)
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
        StepKind::Tool(ToolTarget::Catalog(tool)) => native_read_is_seen(tool),
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

/// Whether runx sees all a native `tool` does: it only reads, and in
/// runx's own process, not through a host process it starts.
fn native_read_is_seen(tool: &str) -> bool {
    crate::tool_catalogs::native::only_reads(tool)
        && !crate::tool_catalogs::native::runs_a_process(tool)
}

/// Whether runx sees all `call` did.
fn call_is_seen(call: &super::run_record::RecordedCall) -> bool {
    match &call.target {
        runx_contracts::CallTarget::Tool { tool } => native_read_is_seen(tool.as_str()),
        runx_contracts::CallTarget::Read { .. } => true,
        runx_contracts::CallTarget::Operation { .. } => matches!(
            &call.outcome,
            super::run_record::CallOutcome::Completed(run) if read_runx_made(run)
        ),
        _ => false,
    }
}

/// The transports runx carries out itself, by the names an operation's
/// packet gives them: HTTP and a web read runx makes, its store, its
/// artifacts, and runx Connect. A process runx starts, an MCP server that
/// may be one, and any transport added later aren't among them until named.
const RUNX_TRANSPORTS: [&str; 5] = [
    "local_http",
    "local_web",
    "local_store",
    "local_artifact",
    "runx_connect",
];

/// Whether the operation `run` made only read, and runx read it itself.
fn read_runx_made(run: &crate::StepRun) -> bool {
    match run.output("provider_operation") {
        Some(runx_contracts::JsonValue::Object(packet)) => packet_read_runx_made(packet),
        _ => false,
    }
}

/// Whether an operation's `packet` shows read access through a transport
/// runx carries out itself.
fn packet_read_runx_made(packet: &runx_contracts::JsonObject) -> bool {
    let field = |name: &str| packet.get(name).and_then(runx_contracts::JsonValue::as_str);
    field("access") == Some("read")
        && field("transport").is_some_and(|transport| RUNX_TRANSPORTS.contains(&transport))
}

/// The gate a person approves spending on a metered read at.
const SPEND_GATE: &str = "provider_usage";

/// Whether `answer` leaves its source eligible: runx's managed agent gave
/// it, or a person approved spending on a metered read, since approving
/// spend isn't asking. Any other answer from a person is something runx
/// can't see again.
fn answer_allowed(answer: &super::run_record::Answer) -> bool {
    if answer.human {
        answer.gate.as_deref() == Some(SPEND_GATE)
    } else {
        answered_by_runx(answer)
    }
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

    use super::{
        Source, answer_allowed, decide, head, index_at, native_read_is_seen, packet_read_runx_made,
    };
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

    /// Asking for reuse never fails a call that would succeed without it: a
    /// call whose identity can't be made, such as one that runs JavaScript on
    /// a worker this build doesn't know the sources of, has no source to find,
    /// runs as a plain call would, and indexes nothing.
    #[test]
    fn a_call_whose_identity_cant_be_made_runs_and_indexes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let store = KernelStore::open(root.path()).unwrap();
        let workflow = crate::execution::workflow::testing::method_workflow(
            "unpinned",
            "minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs: {value: x}\n",
        )
        .unwrap();
        let (decision, identity) = decide(
            &store,
            Err("this build doesn't know its worker's sources".to_owned()),
            &workflow,
            |_| true,
        )
        .expect("a call that can't be pinned still runs");
        assert_eq!(
            decision,
            runx_contracts::ReuseDecision::Recomputed {
                reason: runx_contracts::RecomputeReason::NoIdentity,
            }
        );
        assert_eq!(identity, None, "nothing is indexed for it");
    }

    /// A native tool that runs a host process to read isn't seen, as a
    /// process-backed read isn't: capturing a CLI's help runs the CLI.
    #[test]
    fn a_native_read_through_a_host_process_is_not_seen() {
        assert!(native_read_is_seen("fs.read"));
        assert!(!native_read_is_seen("cli.capture_help"));
    }

    /// An operation's read counts only through a transport runx carries out
    /// itself, named one by one: a process it starts, an MCP server that may
    /// be one, and any transport added later aren't, until they're named.
    #[test]
    fn a_read_counts_only_through_a_transport_runx_carries_out() {
        let packet = |access: &str, transport: &str| {
            serde_json::from_value::<runx_contracts::JsonObject>(
                serde_json::json!({"access": access, "transport": transport}),
            )
            .unwrap()
        };
        for transport in [
            "local_http",
            "local_web",
            "local_store",
            "local_artifact",
            "runx_connect",
        ] {
            assert!(
                packet_read_runx_made(&packet("read", transport)),
                "{transport}"
            );
        }
        for transport in ["local_process", "local_mcp", "local_carrier_pigeon"] {
            assert!(
                !packet_read_runx_made(&packet("read", transport)),
                "{transport}"
            );
        }
        assert!(!packet_read_runx_made(&packet("mutate", "local_store")));
    }

    /// A person's approval to spend on a metered read leaves its source
    /// eligible, since approving spend isn't asking; any other answer a person
    /// gave, to a question or a write's approval, doesn't. runx's managed agent
    /// answers, stating its model and a fresh context.
    #[test]
    fn a_persons_approval_to_spend_on_a_read_leaves_a_source_eligible() {
        let answer = |human: bool, gate: Option<&str>, value: serde_json::Value| {
            crate::execution::run_record::Answer {
                request_id: "read".to_owned(),
                request_digest: "sha256:request".to_owned(),
                value: serde_json::from_value(value).unwrap(),
                human,
                input_digest: None,
                gate: gate.map(str::to_owned),
            }
        };
        let approved = serde_json::json!({"approved": true});
        assert!(answer_allowed(&answer(
            true,
            Some("provider_usage"),
            approved.clone()
        )));
        assert!(!answer_allowed(&answer(
            true,
            Some("write"),
            approved.clone()
        )));
        assert!(!answer_allowed(&answer(true, None, approved)));
        assert!(answer_allowed(&answer(
            false,
            None,
            serde_json::json!({"ending": "completed", "model": "fixture-model", "requirements": ["fresh_context"]}),
        )));
    }
}
