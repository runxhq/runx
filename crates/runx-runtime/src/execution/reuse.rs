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
/// what it's given, the context it gives its child among it.
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
    context: &[runx_contracts::ContextEntry],
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
        context,
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

/// What a call that asked to reuse a result was answered: its decision, the
/// identity the child it runs is indexed under, and, on a hit, the source it
/// settles with in place of a child.
#[derive(Debug)]
pub(crate) struct Decided {
    pub decision: runx_contracts::ReuseDecision,
    pub identity: Option<String>,
    pub source: Option<Source>,
}

/// What a call that asked to reuse a result is answered, given its
/// `identity` or why it has none. A call whose identity can't be made
/// recomputes as no identity: it runs as a plain call would, and indexes
/// nothing. A source that isn't eligible recomputes as ineligible, one whose
/// reads this run may not make again, as `allowed` decides, as not allowed,
/// and one `current` gives a reason for, for that reason (reuse.md, checks
/// 1, 2 and 3). A call that recomputes runs its child, indexed under its
/// identity. A current source is reused: the call settles with it, starts no
/// child and indexes nothing.
pub(crate) fn decide(
    store: &std::sync::Arc<KernelStore>,
    identity: Result<String, String>,
    workflow: &runx_parser::Workflow,
    allowed: impl FnOnce(&Source) -> bool,
    current: impl FnOnce(&Source) -> Option<runx_contracts::RecomputeReason>,
) -> Result<Decided, super::run_record::RunRecordError> {
    use runx_contracts::RecomputeReason;
    let recompute = |reason, identity| Decided {
        decision: runx_contracts::ReuseDecision::Recomputed { reason },
        identity,
        source: None,
    };
    let Ok(identity) = identity else {
        return Ok(recompute(RecomputeReason::NoIdentity, None));
    };
    let reason = match head(store, &identity)? {
        None => RecomputeReason::NoCandidate,
        Some(source) if !eligible(store, &source.run_id, workflow)? => RecomputeReason::Ineligible,
        Some(source) if !allowed(&source) => RecomputeReason::NotAllowed,
        Some(source) => match current(&source) {
            Some(reason) => reason,
            None => {
                return Ok(Decided {
                    decision: runx_contracts::ReuseDecision::Reused {
                        from: source.receipt_id.as_str().into(),
                    },
                    identity: None,
                    source: Some(source),
                });
            }
        },
    };
    Ok(recompute(reason, Some(identity)))
}

/// The child a call that asked to reuse a result would start, as this run
/// would give it now: the run it's called from, the package it runs, the
/// act it's stated, and the run id, authority, environment and chain it
/// would run under, with the provenance its steps would inherit from the
/// call and whether its host's model runs on this machine.
pub(crate) struct Today<'a> {
    pub options: &'a crate::execution::engine::RuntimeOptions,
    pub local_artifacts: &'a crate::services::LocalArtifactService,
    pub skill: &'a LoadedStepSkill,
    pub act: Option<&'a CallAct>,
    pub current_context: Vec<runx_contracts::ContextEntry>,
    pub run_id: String,
    pub authority: Option<crate::RunAuthority>,
    pub env: BTreeMap<String, String>,
    pub chain: Vec<crate::identity::ChainLink>,
    pub inherited: BTreeMap<String, Vec<runx_contracts::ProvenanceEntry>>,
    pub reads_local_only: bool,
}

impl Today<'_> {
    /// The reach the child's tools have: its authority's.
    fn reach(&self) -> runx_contracts::Reach {
        self.authority
            .as_ref()
            .map_or_else(runx_contracts::Reach::default, |authority| authority.reach)
    }

    /// `step` of the child, as an effect is asked about it.
    fn request<'s>(
        &'s self,
        step: &'s runx_parser::WorkflowStep,
    ) -> crate::effects::EffectStepRequest<'s> {
        crate::effects::EffectStepRequest {
            step,
            target: crate::effects::ResolvedEffectTarget::default(),
            inputs: &step.inputs,
            env: &self.env,
            workflow_dir: &self.skill.directory,
            run_id: self.options.run.run_id(),
            run_authority: self.authority.as_ref(),
            held_by_effects: false,
            chain: &self.chain,
            clock: &self.options.clock,
            reconciliation: None,
        }
    }
}

/// What a call asked of reuse, at the time it asks.
pub(crate) struct Asked<'a> {
    pub max_age_seconds: u64,
    pub model: Option<&'a str>,
    pub now: &'a str,
}

/// Whether the child `today` describes may make again every read the source
/// `run_id` of `workflow` made (reuse.md, check 2): each operation read as
/// admission authorizes it, with nothing charged or counted, and each file
/// read within the reach its step would have. Anything that can't be
/// established, a read that can't be rebuilt included, isn't allowed.
/// Nothing is read, so a hit or a miss can't say what changed.
pub(crate) fn allowed(
    store: &std::sync::Arc<KernelStore>,
    run_id: &str,
    workflow: &runx_parser::Workflow,
    today: &Today<'_>,
) -> bool {
    let Ok(reads) = reads(store, run_id, workflow, today.options.workspace.as_ref()) else {
        return false;
    };
    reads.files_allowed(today.authority.as_ref(), &today.env, &today.skill.directory)
        && reads
            .operations
            .iter()
            .all(|step| today.options.effects.authorize(today.request(step)).is_ok())
}

/// Why the source `run_id` of `workflow` recomputes for the call `asked`,
/// whose child `today` describes, or none when it's current (reuse.md,
/// check 3). Its turns' model is the one the call named, when it named one;
/// it's no older than the call allows, counted from when it started; and
/// what each of its steps and turns was given, and what each read of the
/// workspace or of runx's own storage returned, is what it would be now. A
/// provider's read is current for as long as the call allows its result to
/// be old. Anything that can't be established, or can't run now,
/// recomputes as changed.
pub(crate) fn current(
    store: &std::sync::Arc<KernelStore>,
    run_id: &str,
    workflow: &runx_parser::Workflow,
    today: &Today<'_>,
    asked: &Asked<'_>,
) -> Option<runx_contracts::RecomputeReason> {
    use runx_contracts::RecomputeReason;
    let Ok(log) = super::run_record::RunLog::open(std::sync::Arc::clone(store), run_id) else {
        return Some(RecomputeReason::Changed);
    };
    let Ok(Some(record)) = log.load() else {
        return Some(RecomputeReason::Changed);
    };
    if !made_by(&record, asked.model) {
        return Some(RecomputeReason::OtherModel);
    }
    if !young_enough(&record.created_at, asked.now, asked.max_age_seconds) {
        return Some(RecomputeReason::TooOld);
    }
    match unchanged(&log, &record, workflow, today) {
        Ok(true) => None,
        Ok(false) | Err(_) => Some(RecomputeReason::Changed),
    }
}

/// Whether `model` made the run `record` holds, when a call names one: it
/// has a turn, and runx's managed agent stated `model` answered each.
fn made_by(record: &super::run_record::RunRecord, model: Option<&str>) -> bool {
    let Some(model) = model else {
        return true;
    };
    let mut turns = record
        .answers
        .iter()
        .filter(|answer| !answer.human)
        .peekable();
    turns.peek().is_some()
        && turns.all(|answer| super::outcome::stated_model(&answer.value) == Ok(Some(model)))
}

/// Whether a run that started at `started` is at most `max_age_seconds` old
/// `now`. A time that can't be read isn't.
fn young_enough(started: &str, now: &str, max_age_seconds: u64) -> bool {
    let (Some(started), Some(now)) = (crate::time::millis_of(started), crate::time::millis_of(now))
    else {
        return false;
    };
    let max = i64::try_from(max_age_seconds)
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000));
    now.checked_sub(started)
        .zip(max)
        .is_some_and(|(age, max)| age <= max)
}

/// The native reads whose result depends on the reach they ran under, not
/// only on the paths they're given: a file listing or search, which skips
/// what reach keeps out as it walks, and a git history read that shows more
/// than one named file, which leaves out what reach keeps out. For each,
/// the reach under its root is one of its inputs.
fn filters_by_reach(tool: &str, input: &JsonObject) -> bool {
    match tool {
        "fs.list" | "fs.search" | "git.log" | "git.diff" => true,
        "git.show" => input
            .get("path")
            .and_then(runx_contracts::JsonValue::as_str)
            .is_none(),
        _ => false,
    }
}

/// Whether what each step of the source `record`, whose log is `log`, was
/// given, and what each of its reads of the workspace and of runx's own
/// storage returned, is what it would be for the child `today` describes:
/// each step's `input_digest` and each turn's, worked out again from what
/// its earlier steps returned, the act and the environment, and each such
/// read made again.
fn unchanged(
    log: &super::run_record::RunLog,
    record: &super::run_record::RunRecord,
    workflow: &runx_parser::Workflow,
    today: &Today<'_>,
) -> Result<bool, crate::RuntimeError> {
    use super::run_record::{CallOutcome, Origin, Replayed, call_step_id};
    use runx_contracts::CallTarget;
    use runx_parser::{StepKind, ToolTarget};
    let Origin::Child(started) = &record.origin else {
        return Ok(false);
    };
    // A child whose package's environment doesn't resolve now can't run.
    if crate::execution_environment::resolve_environment(
        &today.skill.environment,
        &today.env,
        &today.skill.directory,
    )
    .is_err()
    {
        return Ok(false);
    }
    let bound = super::workflow::bind_run_inputs(
        workflow.clone(),
        &record.inputs,
        record.act.as_ref(),
        &today.run_id,
        today.options.workspace.as_ref(),
    )?;
    let runs = record
        .facts
        .iter()
        .filter_map(|fact| match fact {
            Replayed::Step(step) => Some(step.as_ref().clone()),
            Replayed::Skipped(_) => None,
        })
        .collect::<Vec<_>>();
    let positions = runs
        .iter()
        .enumerate()
        .map(|(position, run)| (run.step_id.clone(), position))
        .collect::<BTreeMap<_, _>>();
    let prior = super::workflow_index::PriorRunIndex::from_positions(&runs, &positions);
    let native_read = |step: &runx_parser::WorkflowStep, tool: &str, recorded: &JsonObject| {
        native_read_unchanged(step, tool, recorded, started.reach, today)
    };
    for run in &runs {
        let defined = bound
            .steps
            .iter()
            .find(|defined| defined.id == run.step_id)
            .ok_or_else(|| crate::RuntimeError::StepMissing {
                step_id: run.step_id.clone(),
            })?;
        let step = runx_parser::WorkflowStep {
            inputs: super::workflow::materialize_step_invocation_inputs_with_index(
                defined, &prior,
            )?,
            ..defined.clone()
        };
        let context_entries =
            super::workflow::materialize_step_context_entries_with_index(defined, &prior)?;
        let ask_reason = super::workflow::materialize_ask_reason_with_index(defined, &prior)?;
        let given = super::engine::input_digest::of(
            defined,
            &super::engine::input_digest::Given {
                inputs: &step.inputs,
                act: None,
                context: &context_entries,
                ask_reason: ask_reason.as_deref(),
            },
            &today.env,
            &today.skill.directory,
        );
        if given.is_none() || given != run.input_digest {
            return Ok(false);
        }
        let same = match &defined.kind {
            StepKind::Agent(_) => {
                turn_unchanged(log, record, &step, &prior, context_entries, today)?
            }
            StepKind::Tool(ToolTarget::Catalog(tool)) => native_read(&step, tool, &run.contract)?,
            StepKind::Operation(_) => storage_read_unchanged(&step, run, today)?,
            _ => true,
        };
        if !same {
            return Ok(false);
        }
        let connectors = match &defined.kind {
            StepKind::Agent(agent) => Some(&agent.connectors),
            StepKind::Script(script) => Some(&script.connectors),
            _ => None,
        };
        let (calls, _) = log.step_calls(&run.step_id, run.attempt)?;
        for call in &calls {
            let made = |kind, inputs| runx_parser::WorkflowStep {
                id: call_step_id(&run.step_id, call.index),
                kind,
                artifacts: None,
                outputs: None,
                inputs,
                bindings: Vec::new(),
                act: None,
                context_skills: Vec::new(),
                when: None,
                guard: Vec::new(),
                retry_unknown_outcomes: false,
                effects: defined.effects,
                reach: defined.reach,
            };
            let same = match (&call.target, &call.outcome) {
                (CallTarget::Tool { tool }, CallOutcome::Completed(ran)) => {
                    let kind = StepKind::Tool(ToolTarget::Catalog(tool.as_str().to_owned()));
                    native_read(
                        &made(kind, call.input.clone()),
                        tool.as_str(),
                        &ran.contract,
                    )?
                }
                (
                    CallTarget::Operation {
                        requirement,
                        operation,
                    },
                    CallOutcome::Completed(ran),
                ) => {
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
                    .map_err(|reason| crate::RuntimeError::StepFailed {
                        step_id: call_step_id(&run.step_id, call.index),
                        reason,
                    })?;
                    storage_read_unchanged(&made(StepKind::Operation(target), inputs), ran, today)?
                }
                // A read the source made that didn't complete can't be
                // shown to come out the same.
                (CallTarget::Tool { .. }, _) => false,
                (CallTarget::Operation { .. }, _) => false,
                _ => true,
            };
            if !same {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Whether the agent step `step`, given what it was given from `prior` and
/// `context_entries`, would ask its host what the source `record` shows each
/// answer to its turn was asked, by the turn's `input_digest`.
fn turn_unchanged(
    log: &super::run_record::RunLog,
    record: &super::run_record::RunRecord,
    step: &runx_parser::WorkflowStep,
    prior: &super::workflow_index::PriorRunIndex<'_>,
    context_entries: Vec<runx_contracts::ContextEntry>,
    today: &Today<'_>,
) -> Result<bool, crate::RuntimeError> {
    let mut provenance =
        super::workflow::materialize_step_invocation_provenance_with_index(step, prior)?;
    provenance.extend(today.inherited.get(&step.id).into_iter().flatten().cloned());
    let given = runx_contracts::ArtifactLabels::derived_from([
        &today
            .local_artifacts
            .labels_of(&runx_contracts::JsonValue::Object(step.inputs.clone())),
        &super::workflow::bound_step_labels(step, prior),
    ]);
    let package = match &today.skill.admitted {
        Some(admitted) => admitted.get(&today.skill.directory)?,
        None => today.options.package(&today.skill.directory)?,
    };
    let (request, _) = super::engine::ask_turn(super::engine::TurnAsk {
        step,
        workflow_dir: &today.skill.directory,
        run_authority: today.authority.as_ref(),
        env: &today.env,
        current_context: &today.current_context,
        historical_context: &today.options.historical_context,
        admitted_at: &today.options.clock.now(),
        package: &package,
        act: today.act,
        address: format!("{}/{}", today.run_id, step.id),
        inputs: step.inputs.clone(),
        provenance,
        context_entries,
        given: &given,
        reads_local_only: today.reads_local_only,
    })?;
    let runx_contracts::ResolutionRequest::Turn { invocation, .. } = &request else {
        return Ok(false);
    };
    let asked = runx_contracts::input_digest::turn_input_digest(invocation).map_err(|error| {
        crate::RuntimeError::EngineInvariant {
            context: "digesting a turn's request",
            message: error.to_string(),
        }
    })?;
    let address = log.address(&step.id);
    let mut answers = record
        .answers
        .iter()
        .filter(|answer| answer.request_id == address)
        .peekable();
    Ok(answers.peek().is_some()
        && answers.all(|answer| answer.input_digest.as_deref() == Some(asked.as_str())))
}

/// Whether the native read `step` of `tool` returns now what it returned
/// to the source, `recorded`: under the reach the child would give its step,
/// and, for a tool whose result depends on reach, only when that reach is
/// the one the source's step had, whose run held `reach_then`.
fn native_read_unchanged(
    step: &runx_parser::WorkflowStep,
    tool: &str,
    recorded: &JsonObject,
    reach_then: runx_contracts::Reach,
    today: &Today<'_>,
) -> Result<bool, crate::RuntimeError> {
    if crate::tool_catalogs::native::reads_only_inputs(tool) {
        return Ok(true);
    }
    let reach_now = today.reach().narrowed(step.reach);
    if filters_by_reach(tool, &step.inputs) && reach_then.narrowed(step.reach) != reach_now {
        return Ok(false);
    }
    let now = super::engine::read_now(
        today.options,
        today.local_artifacts,
        &today.env,
        &today.skill.directory,
        step,
        reach_now,
    )?;
    let digest = |output: &JsonObject| {
        runx_contracts::input_digest::output_digest(output).map_err(|error| {
            crate::RuntimeError::EngineInvariant {
                context: "digesting a read's output",
                message: error.to_string(),
            }
        })
    };
    Ok(digest(&now)? == digest(recorded)?)
}

/// Ask the operation owner to compare the read `ran` recorded: its confirmed
/// binding, implementation and presented credential first, then local state.
/// A provider result keeps its caller's age bound only under identical pins.
/// Missing identity evidence and unsupported reads always recompute.
fn storage_read_unchanged(
    step: &runx_parser::WorkflowStep,
    ran: &crate::StepRun,
    today: &Today<'_>,
) -> Result<bool, crate::RuntimeError> {
    let Some(runx_contracts::JsonValue::Object(packet)) = ran.output("provider_operation") else {
        return Ok(false);
    };
    let field = |name: &str| packet.get(name).and_then(runx_contracts::JsonValue::as_str);
    let Some(identity) = ran.operation_read_identity.as_deref() else {
        return Ok(false);
    };
    let Some(then) = field("result_digest") else {
        return Ok(false);
    };
    today
        .options
        .effects
        .read_unchanged(today.request(step), identity, then)
        .map_err(|error| crate::RuntimeError::StepFailed {
            step_id: step.id.clone(),
            reason: error.to_string(),
        })
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

/// A tool's read of the workspace's files: how its step narrows its run's
/// reach, the tool, and the input it was given.
struct FileRead {
    narrowing: Option<runx_contracts::Reach>,
    tool: String,
    input: JsonObject,
}

impl Reads {
    /// Whether each tool that read files would be let read under what it was
    /// given again, within the reach its step has under `authority`, the
    /// authority its run would hold, by the check the tool itself makes
    /// before it reads.
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
                        narrowing: defined.reach,
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
                        narrowing: defined.reach,
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
    // An index the run's sources re-derive: a cut that loses it loses a
    // reuse, never a result, so it's ordered.
    match store.append_as(
        vec![Append::kernel(
            identity,
            expected,
            &IndexEvent::Indexed(source),
        )?],
        crate::kernel::store::Durability::Ordered,
    ) {
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
        let decided = decide(
            &store,
            Err("this build doesn't know its worker's sources".to_owned()),
            &workflow,
            |_| true,
            |_| None,
        )
        .expect("a call that can't be pinned still runs");
        assert_eq!(
            decided.decision,
            runx_contracts::ReuseDecision::Recomputed {
                reason: runx_contracts::RecomputeReason::NoIdentity,
            }
        );
        assert_eq!(decided.identity, None, "nothing is indexed for it");
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
