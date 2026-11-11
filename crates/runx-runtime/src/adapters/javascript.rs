mod bundle;
mod paged;
mod supervisor;

use std::sync::Arc;
use std::time::Instant;

use runx_contracts::javascript_worker::{
    MAX_WORKER_POOL_SIZE, ScriptCallOutcome, ScriptEnding, ScriptInputReference, ScriptSettlement,
};
use runx_contracts::{JsonObject, JsonValue};

use self::bundle::validated_module;
use self::supervisor::JavaScriptWorkerSupervisor;
use crate::RuntimeError;
use crate::adapter::{InvocationOutput, InvocationStatus, SkillAdapter, SkillInvocation};
use crate::adapter_pipeline::AdapterProjection;
use crate::observations::Observations;

const WORKER_PATH_ENV: &str = "RUNX_JS_WORKER_PATH";

/// One explicit deterministic-JavaScript session. Clones share a bounded lazy
/// worker pool so warm sequential work reuses one process while concurrent
/// branches receive independent wall-time kill boundaries. Independent
/// adapters never share failure or lifecycle state.
#[derive(Clone)]
pub struct JavaScriptAdapter {
    supervisor: Arc<JavaScriptWorkerSupervisor>,
    max_concurrency: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JavaScriptSessionStats {
    pub spawned_process_count: u64,
    pub peak_in_flight: usize,
}

pub(crate) struct PreparedJavaScriptInvocation {
    entry_module: String,
    export_name: String,
    modules: std::collections::BTreeMap<String, String>,
    environment: std::collections::BTreeMap<String, String>,
    worker_path: Option<String>,
    limits: runx_contracts::javascript_worker::InvocationLimits,
}

/// A script step's module bundle, prepared once and passed again after each
/// settlement.
pub(crate) struct PreparedScript {
    entry_module: String,
    export_name: String,
    modules: std::collections::BTreeMap<String, String>,
    worker_path: Option<String>,
    limits: runx_contracts::javascript_worker::InvocationLimits,
}

/// What a script pass or settlement came to: the calls made since the last,
/// in position order, and how the script stands.
pub(crate) struct ScriptPassed {
    pub(crate) calls: Vec<runx_contracts::javascript_worker::ScriptCall>,
    pub(crate) ending: runx_contracts::javascript_worker::ScriptEnding,
}

/// A script waiting on its calls, live on its worker.
pub(crate) struct LiveScript<'a> {
    script: supervisor::LiveScript<'a>,
}

impl LiveScript<'_> {
    /// Deliver further settlements, in the order they settled.
    pub(crate) fn settle(
        &mut self,
        settled: Vec<runx_contracts::javascript_worker::ScriptSettlement>,
        now: i64,
    ) -> Result<ScriptPassed, RuntimeError> {
        crate::work_counts::count_many(
            crate::work_counts::Work::ScriptSettlementDelivered,
            settled.len(),
        );
        // Settlements reach the script in frames, as a replay's do, while it
        // waits on them: a batch read at once settles together, and no
        // frame holds more than a bounded share of it.
        let mut frames = settlement_frames(settled)?.into_iter();
        let mut passed = ScriptPassed {
            calls: Vec::new(),
            ending: ScriptEnding::Waiting,
        };
        while passed.ending == ScriptEnding::Waiting {
            let Some(frame) = frames.next() else {
                break;
            };
            let more = script_passed(self.script.settle(frame, now)?)?;
            passed.calls.extend(more.calls);
            passed.ending = more.ending;
        }
        whole(&mut self.script, passed)
    }
}

/// `passed`, with an output its script returned past what an answer carries
/// read from its worker in parts and given whole, as an output within it is.
fn whole(
    script: &mut supervisor::LiveScript<'_>,
    passed: ScriptPassed,
) -> Result<ScriptPassed, RuntimeError> {
    let ScriptEnding::ReturnedInParts { size_bytes } = passed.ending else {
        return Ok(passed);
    };
    let size = usize::try_from(size_bytes).map_err(|_| RuntimeError::JavaScriptWorker {
        message: format!("a script's output of {size_bytes} bytes is past what this host holds"),
    })?;
    let mut json = String::with_capacity(size);
    while json.len() < size {
        let offset = u64::try_from(json.len()).unwrap_or(u64::MAX);
        let part = script.output_part(offset)?;
        if part.is_empty() || json.len().saturating_add(part.len()) > size {
            return Err(RuntimeError::JavaScriptWorker {
                message: format!(
                    "a part of a script's output at byte {offset} doesn't fit its {size_bytes} bytes"
                ),
            });
        }
        json.push_str(&part);
    }
    let output = serde_json::from_str(&json)
        .map_err(|source| RuntimeError::json("reading a script's output from its parts", source))?;
    Ok(ScriptPassed {
        calls: passed.calls,
        ending: ScriptEnding::Returned { output },
    })
}

/// The most of a script's settlements one frame to its worker holds, in
/// bytes of their JSON, live or in a replay: a settlement past it, a large
/// read, takes a frame alone.
pub(crate) const SETTLEMENT_FRAME_BYTES: usize = 2 * 1024 * 1024;

/// `settled` in the frames it reaches the worker in, in order.
fn settlement_frames(
    settled: Vec<ScriptSettlement>,
) -> Result<Vec<Vec<ScriptSettlement>>, RuntimeError> {
    let mut frames = Vec::new();
    let mut frame = Vec::new();
    let mut bytes = 0_usize;
    for settlement in settled {
        let size = serde_json::to_vec(&settlement)
            .map_err(|source| RuntimeError::json("measuring a script's settlement", source))?
            .len();
        if !frame.is_empty() && bytes.saturating_add(size) > SETTLEMENT_FRAME_BYTES {
            frames.push(std::mem::take(&mut frame));
            bytes = 0;
        }
        bytes = bytes.saturating_add(size);
        frame.push(settlement);
    }
    if !frame.is_empty() {
        frames.push(frame);
    }
    Ok(frames)
}

fn script_passed(result: supervisor::WorkerInvocationResult) -> Result<ScriptPassed, RuntimeError> {
    match result {
        supervisor::WorkerInvocationResult::Passed { calls, ending } => {
            Ok(ScriptPassed { calls, ending })
        }
        supervisor::WorkerInvocationResult::Failure { message, .. } => {
            Err(RuntimeError::JavaScriptWorker { message })
        }
        supervisor::WorkerInvocationResult::Success(_) => Err(RuntimeError::JavaScriptWorker {
            message: "the worker answered a script with a tool's result".to_owned(),
        }),
        supervisor::WorkerInvocationResult::OutputPart(_) => Err(RuntimeError::JavaScriptWorker {
            message: "the worker answered a script's pass with a part of an output".to_owned(),
        }),
    }
}

impl PreparedJavaScriptInvocation {
    fn with_inputs(
        &self,
        inputs: &JsonObject,
    ) -> Result<supervisor::WorkerInvocation, RuntimeError> {
        let inputs = serde_json::to_value(inputs)
            .map_err(|source| RuntimeError::json("serializing JavaScript inputs", source))?;
        Ok(supervisor::WorkerInvocation {
            entry_module: self.entry_module.clone(),
            export_name: self.export_name.clone(),
            modules: self.modules.clone(),
            inputs,
            environment: self.environment.clone(),
            worker_path: self.worker_path.clone(),
            limits: self.limits,
        })
    }
}

impl JavaScriptAdapter {
    #[must_use]
    pub fn new_session() -> Self {
        Self::with_max_concurrency(1)
    }

    #[must_use]
    pub fn with_max_concurrency(max_concurrency: usize) -> Self {
        let max_concurrency = max_concurrency.clamp(1, MAX_WORKER_POOL_SIZE);
        Self {
            supervisor: Arc::new(JavaScriptWorkerSupervisor::new(max_concurrency)),
            max_concurrency,
        }
    }

    /// The digest of the sources the worker this session runs `env`'s
    /// JavaScript on reports it was built from: an idle worker's, or one
    /// started now, which stays in the session for the next call to it.
    pub(crate) fn worker_digest(
        &self,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<String, RuntimeError> {
        self.supervisor
            .worker_digest(env.get(WORKER_PATH_ENV).map(String::as_str))
    }

    #[must_use]
    pub fn spawned_process_count(&self) -> u64 {
        self.supervisor.spawn_count()
    }

    #[must_use]
    pub fn session_stats(&self) -> JavaScriptSessionStats {
        JavaScriptSessionStats {
            spawned_process_count: self.supervisor.spawn_count(),
            peak_in_flight: self.supervisor.peak_in_flight(),
        }
    }

    /// Prepare `request` to run from `package`, the validated package its
    /// run admitted: its modules are that package's, never reread.
    pub(crate) fn prepare_invocation(
        &self,
        request: &SkillInvocation,
        package: &crate::LoadedSkillPackage,
    ) -> Result<PreparedJavaScriptInvocation, RuntimeError> {
        validate_pure_javascript_boundary(request)?;
        validated_module(request, package)
    }

    pub(crate) fn invoke_prepared(
        &self,
        prepared: &PreparedJavaScriptInvocation,
        inputs: &JsonObject,
    ) -> Result<InvocationOutput, RuntimeError> {
        let started = Instant::now();
        let outcome = self.supervisor.invoke(prepared.with_inputs(inputs)?)?;
        project_worker_outcome(started, outcome)
    }

    /// Prepare script step `script` to run from `package`, the validated
    /// package its run admitted.
    pub(crate) fn prepare_script(
        &self,
        script: &runx_parser::ScriptStep,
        package: &crate::LoadedSkillPackage,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<PreparedScript, RuntimeError> {
        bundle::validated_script(script, package, env.get(WORKER_PATH_ENV).cloned())
    }

    /// Run `prepared` from the beginning against what its calls `settled`,
    /// in the order they settled; a `ctx.now()` past the record reads `now`.
    /// The script comes back live, to take each further settlement where it
    /// stands rather than from the beginning.
    ///
    /// The pass starts with the script's inputs, those given by reference in
    /// place, and what its record read of the clock. The record's other
    /// settlements follow in further frames, in the order they settled, at
    /// most [`SETTLEMENT_FRAME_BYTES`] of them a frame and a larger one alone,
    /// while the script waits on them: no frame holds more than a bounded
    /// share of the record, however long it is.
    pub(crate) fn start_script(
        &self,
        prepared: &PreparedScript,
        given: (&JsonObject, Vec<ScriptInputReference>),
        settled: Vec<ScriptSettlement>,
        now: i64,
    ) -> Result<(LiveScript<'_>, ScriptPassed), RuntimeError> {
        let (inputs, references) = given;
        let inputs = serde_json::to_value(inputs)
            .map_err(|source| RuntimeError::json("serializing a script's inputs", source))?;
        crate::work_counts::count(crate::work_counts::Work::ScriptPass);
        crate::work_counts::count_many(
            crate::work_counts::Work::ScriptSettlementDelivered,
            settled.len(),
        );
        let (clock, settled): (Vec<_>, Vec<_>) = settled
            .into_iter()
            .partition(|settlement| matches!(settlement.outcome, ScriptCallOutcome::Now { .. }));
        let (mut script, result) =
            self.supervisor
                .start_script(supervisor::ScriptPassInvocation {
                    entry_module: prepared.entry_module.clone(),
                    export_name: prepared.export_name.clone(),
                    modules: prepared.modules.clone(),
                    inputs,
                    references,
                    worker_path: prepared.worker_path.clone(),
                    limits: prepared.limits,
                    settled: clock,
                    now,
                })?;
        let mut passed = script_passed(result)?;
        let mut frames = settlement_frames(settled)?.into_iter();
        while passed.ending == ScriptEnding::Waiting {
            let Some(frame) = frames.next() else {
                break;
            };
            let more = script_passed(script.settle(frame, now)?)?;
            passed.calls.extend(more.calls);
            passed.ending = more.ending;
        }
        // A script that ended with settlements left is given none of them,
        // as the worker would have been: one for a call it never made means
        // the replay went another way.
        if matches!(
            passed.ending,
            ScriptEnding::Returned { .. }
                | ScriptEnding::ReturnedInParts { .. }
                | ScriptEnding::Threw { .. }
        ) && let Some(position) = frames
            .flatten()
            .map(|settlement| settlement.position)
            .find(|position| !passed.calls.iter().any(|call| call.position == *position))
        {
            passed.ending = ScriptEnding::Diverged { position };
        }
        let passed = whole(&mut script, passed)?;
        Ok((LiveScript { script }, passed))
    }

    /// Run `request` from `package`, the validated package its run admitted.
    pub(crate) fn invoke_with_artifacts(
        &self,
        request: SkillInvocation,
        package: &crate::LoadedSkillPackage,
        local_artifacts: &crate::services::LocalArtifactService,
    ) -> Result<InvocationOutput, RuntimeError> {
        if worker(&request)?.paged_input.is_some() {
            return paged::invoke(self, request, package, local_artifacts);
        }
        let prepared = self.prepare_invocation(&request, package)?;
        self.invoke_prepared(&prepared, &request.inputs)
    }
}

impl Default for JavaScriptAdapter {
    fn default() -> Self {
        Self::new_session()
    }
}

impl std::fmt::Debug for JavaScriptAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JavaScriptAdapter")
            .field("max_concurrency", &self.max_concurrency)
            .field("spawned_process_count", &self.spawned_process_count())
            .finish_non_exhaustive()
    }
}

/// The adapter alone, outside any run, runs the package as it is now.
impl SkillAdapter for JavaScriptAdapter {
    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        validate_pure_javascript_boundary(&request)?;
        let package = crate::load_validated_skill_package(&request.skill_directory)?;
        self.invoke_with_artifacts(
            request,
            &package,
            &crate::services::LocalArtifactService::default(),
        )
    }
}

fn project_worker_outcome(
    started: Instant,
    outcome: supervisor::WorkerInvocationOutcome,
) -> Result<InvocationOutput, RuntimeError> {
    match outcome.result {
        supervisor::WorkerInvocationResult::Success(output) => {
            let value = serde_json::from_value(output).map_err(|source| {
                RuntimeError::json("converting JavaScript worker output", source)
            })?;
            Ok(
                AdapterProjection::from_duration_ms(elapsed_ms(started)).runtime_output(
                    InvocationStatus::Success,
                    value,
                    None,
                    worker_observations(outcome.execution_boundary),
                ),
            )
        }
        supervisor::WorkerInvocationResult::Failure { message, .. } => Ok(
            AdapterProjection::from_duration_ms(elapsed_ms(started)).runtime_output(
                InvocationStatus::Failure,
                JsonValue::Null,
                Some(message),
                worker_observations(outcome.execution_boundary),
            ),
        ),
        supervisor::WorkerInvocationResult::Passed { .. }
        | supervisor::WorkerInvocationResult::OutputPart(_) => {
            Err(RuntimeError::JavaScriptWorker {
                message: "the worker answered a tool with a script's answer".to_owned(),
            })
        }
    }
}

/// The worker tool an invocation runs.
pub(super) fn worker(request: &SkillInvocation) -> Result<&runx_parser::WorkerTool, RuntimeError> {
    request
        .executable
        .worker()
        .ok_or_else(|| RuntimeError::UnsupportedExecutable {
            kind: request.executable.kind().to_owned(),
        })
}

fn validate_pure_javascript_boundary(request: &SkillInvocation) -> Result<(), RuntimeError> {
    worker(request)?;
    if !request.credential_delivery.secret_env().is_empty() {
        return Err(RuntimeError::InvalidProcessInvocation {
            message: "worker tools cannot receive credentials; route provider access through a connector operation"
                .to_owned(),
        });
    }
    Ok(())
}

/// Where a worker invocation ran.
fn worker_observations(boundary: runx_contracts::ExecutionBoundaryKind) -> Observations {
    Observations {
        boundary: Some(boundary),
        ..Observations::default()
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
