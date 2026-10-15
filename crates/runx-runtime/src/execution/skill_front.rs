// Module rationale: the skill front owns run preparation and the shared
// sealed-output projection for every runner.
//! The skill front: compiles a skill-run request into a run of the runner's
//! graph and seals it through the shared act engine (`execution::runner`),
//! which owns admit -> execute -> seal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{ClosureDisposition, JsonNumber, JsonObject, JsonValue};
use runx_parser::{SkillRunnerDefinition, SkillRunnerManifest};
use thiserror::Error;

use crate::RuntimeError;
use crate::adapter::InvocationOutput;
use crate::effects::RuntimeEffectRegistry;
use crate::execution::orchestrator::{RunResult, RunStatus, SkillRunRequest};
use crate::paths::RUNX_RECEIPT_DIR_ENV;
use crate::receipts::signing::strip_receipt_signing_env;
use crate::receipts::store::ReceiptStoreError;
use crate::receipts::{
    RunScope, RuntimeReceiptSignatureConfig, StepSeal, StepSealClosure, seal_step,
};
use crate::services::{ReceiptServices, WorkspaceEnv};

mod executable_adapter;
mod graph;
mod resolution_answers;
pub(crate) mod runner_manifest;
mod turn_session;

pub(crate) use self::executable_adapter::ExecutableAdapter;
pub use self::turn_session::TurnSession;

use self::runner_manifest::selected_runner;

pub use super::operator_context::{
    SkillOperatorContextChain, SkillOperatorContextContextSkill, SkillOperatorContextDocument,
    SkillOperatorContextNode, SkillOperatorContextOptions, SkillOperatorContextPackage,
    SkillOperatorContextRegistry, SkillOperatorContextRunner, SkillOperatorContextStep,
    SkillOperatorContextTool, load_skill_operator_context_chain,
};
pub use super::prepared_skill::{
    PREPARED_SKILL_REPORT_SCHEMA, PreparedEntryProvenance, PreparedGovernanceSummary,
    PreparedInputSummary, PreparedRequestSummary, PreparedSkillRun, PreparedSkillRunReport,
    PreparedSkillRunStatus, PreparedTraceEntry, prepare_skill_run,
};

// The canonical public boundary is a bounded result envelope. Full receipts
// and the run record remain durable in their owning stores instead of being
// copied into every CLI/SDK response.
const SKILL_RUN_SCHEMA: &str = "runx.skill_run.v1";

impl From<crate::execution::run_record::RunRecordError> for SkillRunError {
    fn from(error: crate::execution::run_record::RunRecordError) -> Self {
        Self::Runtime(error.into())
    }
}

#[derive(Debug, Error)]
pub enum SkillRunError {
    #[error("skill run failed: {0}")]
    Invalid(String),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("{source}; refusal receipt: {receipt_id}")]
    PreflightRefused {
        source: Box<RuntimeError>,
        receipt_id: String,
    },
    #[error(transparent)]
    ReceiptStore(#[from] ReceiptStoreError),
}

/// Per-run choices and explicitly shared native host services. Every entry
/// point retains the same admission, continuation, and receipt semantics.
/// Omitted services use the normal local runtime defaults.
#[derive(Clone, Debug, Default)]
pub(crate) struct SkillRunOverrides {
    /// Select a runner by name instead of the manifest default.
    pub(crate) runner: Option<String>,
    /// Native host services shared for the lifetime of a local server session.
    pub(crate) javascript: Option<crate::adapters::javascript::JavaScriptAdapter>,
}

pub(crate) struct ResolvedSkillRun<'a> {
    pub(crate) loaded: &'a std::sync::Arc<crate::LoadedSkillPackage>,
    pub(crate) request: &'a SkillRunRequest,
    pub(crate) overrides: &'a SkillRunOverrides,
    pub(crate) effects: &'a RuntimeEffectRegistry,
    pub(crate) skill_dir: &'a Path,
    pub(crate) manifest: &'a SkillRunnerManifest,
    pub(crate) runner: &'a SkillRunnerDefinition,
    pub(crate) package_digest: &'a str,
    pub(crate) execution_closure_digest: Option<&'a str>,
}

/// A skill run bound to its verified package: everything a run of it needs
/// before its record is opened. One binding serves every pass a driver makes.
pub(crate) struct BoundRun {
    request: SkillRunRequest,
    overrides: SkillRunOverrides,
    /// The package directory, canonical as the loader resolved it.
    skill_dir: PathBuf,
    effects: RuntimeEffectRegistry,
    workspace: WorkspaceEnv,
    receipts: ReceiptServices,
    manifest: SkillRunnerManifest,
    runner: SkillRunnerDefinition,
    package_digest: String,
    execution_closure_digest: Option<String>,
}

pub(crate) fn execute_skill_run_with_effects(
    request: &SkillRunRequest,
    effects: &RuntimeEffectRegistry,
) -> Result<RunResult, SkillRunError> {
    execute_skill_run_with_overrides(request, &SkillRunOverrides::default(), effects)
}

pub(crate) fn execute_skill_run_with_overrides(
    request: &SkillRunRequest,
    overrides: &SkillRunOverrides,
    effects: &RuntimeEffectRegistry,
) -> Result<RunResult, SkillRunError> {
    execute_bound_skill_run_with_overrides(request, overrides, effects, None, None)
}

pub(crate) fn execute_bound_skill_run_with_overrides(
    request: &SkillRunRequest,
    overrides: &SkillRunOverrides,
    effects: &RuntimeEffectRegistry,
    expected_package_digest: Option<&str>,
    expected_execution_closure_digest: Option<&str>,
) -> Result<RunResult, SkillRunError> {
    bind_skill_run(
        request,
        overrides,
        effects,
        expected_package_digest,
        expected_execution_closure_digest,
    )?
    .execute()
}

/// The inputs command-line `texts` give the runner of `skill_path` that a run
/// with `runner` selects: each read by its declaration, never guessed. A text
/// for an input the runner does not declare is refused.
pub fn runner_inputs_from_text(
    skill_path: &Path,
    runner: Option<&str>,
    texts: &BTreeMap<String, String>,
) -> Result<JsonObject, SkillRunError> {
    let loaded = crate::load_validated_skill_package(skill_path)?;
    let manifest = loaded.manifest().ok_or_else(|| {
        invalid(format!(
            "skill package {} does not declare run.yaml runners",
            loaded.directory.display(),
        ))
    })?;
    let runner = selected_runner(manifest, runner)?;
    texts
        .iter()
        .map(|(name, text)| {
            let declared = runner.inputs.get(name).ok_or_else(|| {
                invalid(format!(
                    "runner {} declares no input {name}; it takes {}",
                    runner.name,
                    declared_names(runner)
                ))
            })?;
            declared
                .value_from_text(text)
                .map(|value| (name.clone(), value))
                .map_err(|reason| invalid(format!("input {name} {reason}")))
        })
        .collect()
}

fn declared_names(runner: &SkillRunnerDefinition) -> String {
    if runner.inputs.is_empty() {
        return "no inputs".to_owned();
    }
    runner
        .inputs
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Load the request's skill, verify it against the digests a caller pins,
/// and bind the run to it.
pub(crate) fn bind_skill_run(
    request: &SkillRunRequest,
    overrides: &SkillRunOverrides,
    effects: &RuntimeEffectRegistry,
    expected_package_digest: Option<&str>,
    expected_execution_closure_digest: Option<&str>,
) -> Result<BoundRun, SkillRunError> {
    let loaded = std::sync::Arc::new(crate::load_validated_skill_package(&request.skill_path)?);
    let skill_dir = loaded.directory.clone();
    let manifest = loaded.manifest().cloned().ok_or_else(|| {
        invalid(format!(
            "skill package {} does not declare run.yaml runners",
            skill_dir.display(),
        ))
    })?;
    let runner = selected_runner(&manifest, overrides.runner.as_deref())?.clone();
    let package_digest = loaded.package.package_digest.clone();
    let execution_closure_digest = crate::skill_package::verify_loaded_execution_binding(
        loaded.clone(),
        &runner.name,
        &request.env,
        expected_package_digest,
        expected_execution_closure_digest,
    )
    .map_err(|error| invalid(error.to_string()))?;
    bind_resolved(
        ResolvedSkillRun {
            loaded: &loaded,
            request,
            overrides,
            effects,
            skill_dir: &skill_dir,
            manifest: &manifest,
            runner: &runner,
            package_digest: &package_digest,
            execution_closure_digest: execution_closure_digest.as_deref(),
        },
        false,
    )
}

/// What continues a paused run: the run, where its receipts go, and what
/// its host brings.
pub(crate) struct Continuation<'a> {
    pub(crate) run_id: &'a str,
    pub(crate) receipt_dir: Option<&'a Path>,
    pub(crate) env: &'a BTreeMap<String, String>,
    pub(crate) cwd: &'a Path,
    pub(crate) answers: Option<&'a runx_contracts::ResumeDocument>,
    pub(crate) managed_agent: crate::execution::orchestrator::ManagedAgentPolicy,
    /// Digests a caller pins; each must be the one the run started under.
    pub(crate) package_digest: Option<&'a str>,
    pub(crate) execution_closure_digest: Option<&'a str>,
}

/// Seal run `run_id` with its host's close, from its record alone: any run
/// that is not sealed, whether it waits at an act or stopped between steps.
/// Nothing is bound first, so a package or execution closure that changed
/// since the run started never traps it open, and an effect the run left
/// unconfirmed stays unconfirmed. Its driver lock, which the caller holds,
/// is what keeps a live driver out.
pub(crate) fn close_run(continuation: Continuation<'_>) -> Result<RunResult, SkillRunError> {
    use crate::execution::run_record::{Origin, Replayed, RunLog, validate_run_id};

    let close = match continuation.answers {
        Some(document)
            if !document.answers.is_empty()
                || !document.approvals.is_empty()
                || document.standing_approvals.is_some() =>
        {
            return Err(invalid("a close comes alone, without answers or approvals"));
        }
        Some(runx_contracts::ResumeDocument {
            close: Some(close), ..
        }) => *close,
        _ => return Err(invalid("a close names its disposition")),
    };
    let run_id = validate_run_id(continuation.run_id)?;
    let workspace = WorkspaceEnv::new(continuation.env.clone(), continuation.cwd.to_path_buf())
        .map_err(RuntimeError::from)?;
    let receipts = ReceiptServices::resolve(&workspace)
        .map_err(|error| SkillRunError::Invalid(error.to_string()))?;
    let store = crate::kernel::store::KernelStore::for_project(workspace.paths())
        .map_err(crate::execution::run_record::RunRecordError::from)?;
    let log = RunLog::open(store, run_id)?;
    let record = log
        .load()?
        .ok_or_else(|| invalid(format!("run {run_id} has no record in this workspace")))?;
    // A closed run leaves no change half-applied.
    crate::filesystem::recover(log.store(), run_id)?;
    let Origin::Run(started) = &record.origin else {
        return Err(invalid("a child run closes only with its parent"));
    };
    if record.sealed.is_some() {
        return Err(invalid(format!(
            "run {run_id} is sealed; only an open run closes"
        )));
    }
    for (label, pinned, recorded) in [
        (
            "package",
            continuation.package_digest,
            &started.package_digest,
        ),
        (
            "execution closure",
            continuation.execution_closure_digest,
            &started.execution_closure_digest,
        ),
    ] {
        if let Some(pinned) = pinned
            && pinned != recorded
        {
            return Err(invalid(format!(
                "the {label} digest {pinned} is not the one run {run_id} started under ({recorded})"
            )));
        }
    }
    let steps = record
        .facts
        .iter()
        .filter_map(|fact| match fact {
            Replayed::Step(run) => Some(run.as_ref().clone()),
            Replayed::Skipped(_) => None,
        })
        .collect::<Vec<_>>();
    let receipt = crate::receipts::graph_receipt_with_closure(
        RunScope {
            run_id,
            graph_name: &started.skill,
        },
        &steps,
        &crate::time::now_iso8601(),
        crate::receipts::GraphClosure {
            disposition: close.disposition.into(),
            reason: Some(runx_contracts::SealReason::HostClosed),
        },
        receipts.signature_config().signature_policy(),
    )?;
    // A closed run's result is nothing: it stopped before it produced one.
    let result = JsonValue::Object(JsonObject::new());
    log.record_sealed(&receipt, Some(&result))?;
    let request = SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: PathBuf::from(&started.skill_ref),
        receipt_dir: continuation.receipt_dir.map(Path::to_path_buf),
        run_id: Some(run_id.to_owned()),
        answers: None,
        inputs: BTreeMap::new(),
        env: continuation.env.clone(),
        cwd: continuation.cwd.to_path_buf(),
        managed_agent: continuation.managed_agent,
    };
    for step in &steps {
        for nested in &step.nested_receipts {
            runner_manifest::write_skill_receipt(&request, &workspace, &receipts, nested)?;
        }
        runner_manifest::write_skill_receipt(&request, &workspace, &receipts, &step.receipt)?;
    }
    runner_manifest::write_skill_receipt(&request, &workspace, &receipts, &receipt)?;
    Ok(sealed_output(
        &started.skill,
        &started.runner,
        run_id,
        &InvocationOutput::runtime_success(result.clone(), 0),
        &result,
        SkillOutputDiagnostics::default(),
        &receipt,
    ))
}

/// Bind a paused run again: its skill, runner and digests come from its
/// record, never from the caller.
pub(crate) fn bind_paused_run(
    continuation: Continuation<'_>,
    effects: &RuntimeEffectRegistry,
) -> Result<BoundRun, SkillRunError> {
    let run_id = continuation.run_id;
    let paused =
        crate::execution::run_record::paused_run(continuation.env, continuation.cwd, run_id)?
            .ok_or_else(|| invalid(format!("run {run_id} is not paused")))?;
    for (label, pinned, recorded) in [
        (
            "package",
            continuation.package_digest,
            &paused.package_digest,
        ),
        (
            "execution closure",
            continuation.execution_closure_digest,
            &paused.execution_closure_digest,
        ),
    ] {
        if let Some(pinned) = pinned
            && pinned != recorded
        {
            return Err(invalid(format!(
                "the {label} digest {pinned} is not the one run {run_id} started under ({recorded})"
            )));
        }
    }
    bind_skill_run(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: PathBuf::from(&paused.skill_ref),
            receipt_dir: continuation.receipt_dir.map(Path::to_path_buf),
            run_id: Some(paused.run_id),
            answers: continuation.answers.cloned(),
            inputs: BTreeMap::new(),
            env: continuation.env.clone(),
            cwd: continuation.cwd.to_path_buf(),
            managed_agent: continuation.managed_agent,
        },
        &SkillRunOverrides {
            runner: Some(paused.runner),
            ..SkillRunOverrides::default()
        },
        effects,
        Some(&paused.package_digest),
        Some(&paused.execution_closure_digest),
    )
}

pub(crate) fn execute_prepared_skill_run_with_resolved(
    resolved: ResolvedSkillRun<'_>,
) -> Result<RunResult, SkillRunError> {
    bind_resolved(resolved, true)?.execute()
}

fn bind_resolved(
    resolved: ResolvedSkillRun<'_>,
    trusted_prepared: bool,
) -> Result<BoundRun, SkillRunError> {
    let ResolvedSkillRun {
        loaded,
        request,
        overrides,
        effects,
        skill_dir,
        manifest,
        runner,
        package_digest,
        execution_closure_digest,
    } = resolved;
    let (request, workspace, receipts) =
        prepare_skill_execution(request, manifest, runner, trusted_prepared)?;
    let skill_env = workspace.skill_env_for_skill(skill_dir);
    if !trusted_prepared {
        super::operator_context::load_skill_operator_context_chain_from_package(
            loaded,
            Some(&runner.name),
            SkillOperatorContextOptions::new(
                workspace.env().clone(),
                workspace.cwd().to_path_buf(),
            )
            .with_effects(effects.clone()),
        )
        .map_err(|blocked| blocked.error)?;
    }
    crate::execution_environment::resolve_declared_environment(
        &manifest.execution_requirements(runner),
        &skill_env,
        skill_dir,
    )?;
    Ok(BoundRun {
        request,
        overrides: overrides.clone(),
        skill_dir: skill_dir.to_path_buf(),
        effects: effects.clone(),
        workspace,
        receipts,
        manifest: manifest.clone(),
        runner: runner.clone(),
        package_digest: package_digest.to_owned(),
        execution_closure_digest: execution_closure_digest.map(str::to_owned),
    })
}

/// Seal the refusal of a run that never started: its request failed
/// preparation before admission, for `reason`.
pub(super) fn seal_skill_preflight_refusal(
    request: &SkillRunRequest,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    failure: JsonObject,
    reason: runx_contracts::SealReason,
) -> Result<String, SkillRunError> {
    let workspace =
        WorkspaceEnv::new(request.env.clone(), request.cwd.clone()).map_err(RuntimeError::from)?;
    let receipts = ReceiptServices::resolve(&workspace)
        .map_err(|error| SkillRunError::Invalid(error.to_string()))?;
    let run_id = match &request.run_id {
        Some(run_id) => crate::execution::run_record::validate_run_id(run_id)?.to_owned(),
        None => crate::execution::run_record::mint_run_id(&manifest.skill)?,
    };
    let message = failure
        .get("message")
        .and_then(JsonValue::as_str)
        .unwrap_or("skill preparation was refused")
        .to_owned();
    let output = InvocationOutput::runtime_failure(JsonValue::Object(failure), message.clone(), 0);
    let receipt = seal_refusal(
        RunScope {
            run_id: &run_id,
            graph_name: &manifest.skill,
        },
        runner,
        receipts.signature_config(),
        &output,
        StepSealClosure {
            disposition: ClosureDisposition::Blocked,
            reason: Some(reason),
        },
    )?;
    runner_manifest::write_skill_receipt(request, &workspace, &receipts, &receipt)?;
    Ok(receipt.id.to_string())
}

fn prepare_skill_execution(
    request: &SkillRunRequest,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    trusted_prepared: bool,
) -> Result<(SkillRunRequest, WorkspaceEnv, ReceiptServices), SkillRunError> {
    let mut request = request.clone();
    let raw_workspace =
        WorkspaceEnv::new(request.env.clone(), request.cwd.clone()).map_err(RuntimeError::from)?;
    let receipts = ReceiptServices::resolve(&raw_workspace)
        .map_err(|error| SkillRunError::Invalid(error.to_string()))?;
    restore_continuation_inputs(&mut request, &raw_workspace)?;
    crate::input_contract::apply_defaults(&runner.inputs, &mut request.inputs);
    let materialize = if request.answers.is_some() {
        crate::input_contract::materialize_present_runner_inputs
    } else {
        crate::input_contract::materialize_complete_runner_inputs
    };
    request.inputs = match materialize(&runner.inputs, &request.inputs) {
        Ok(inputs) => inputs,
        Err(error) => {
            let source = error.into_runtime_error();
            let mut failure = source.public_failure_projection();
            failure.insert(
                "stage".to_owned(),
                JsonValue::String("validate_inputs".to_owned()),
            );
            let receipt_id = seal_skill_preflight_refusal(
                &request,
                manifest,
                runner,
                failure,
                runx_contracts::SealReason::InputContractInvalid,
            )?;
            return Err(SkillRunError::PreflightRefused {
                source: Box::new(source),
                receipt_id,
            });
        }
    };
    let mut runtime_env = request.env.clone();
    let resolved_receipt_path = raw_workspace.receipt_path(request.receipt_dir.as_deref());
    runtime_env.insert(
        RUNX_RECEIPT_DIR_ENV.to_owned(),
        resolved_receipt_path.path.to_string_lossy().into_owned(),
    );
    strip_receipt_signing_env(&mut runtime_env);
    if !trusted_prepared {
        super::prepared_skill::strip_untrusted_prepared_env(&mut runtime_env);
    }
    let workspace =
        WorkspaceEnv::new(runtime_env, request.cwd.clone()).map_err(RuntimeError::from)?;
    Ok((request, workspace, receipts))
}

/// A request that names a recorded run continues it, under the inputs the
/// run was started with: the request may restate them but never replace them.
/// Answers continue only a recorded run.
pub(crate) fn restore_continuation_inputs(
    request: &mut SkillRunRequest,
    workspace: &WorkspaceEnv,
) -> Result<(), SkillRunError> {
    let answered = request.answers.is_some();
    let Some(run_id) = request.run_id.as_deref() else {
        return if answered {
            Err(invalid("graph continuation requires run_id"))
        } else {
            Ok(())
        };
    };
    let store = crate::kernel::store::KernelStore::for_project(workspace.paths())
        .map_err(crate::execution::run_record::RunRecordError::from)?;
    let Some(record) = crate::execution::run_record::RunLog::open(
        store,
        crate::execution::run_record::validate_run_id(run_id)?,
    )?
    .load()?
    else {
        return if answered {
            Err(invalid(format!("run {run_id} has no record to continue")))
        } else {
            Ok(())
        };
    };
    if request
        .inputs
        .iter()
        .any(|(name, value)| record.inputs.get(name) != Some(value))
    {
        return Err(invalid(
            "graph run identity cannot replace its original inputs",
        ));
    }
    request.inputs = record.inputs.into_iter().collect();
    Ok(())
}

/// A run that waits: the requests it waits on, each with its id.
fn paused_output(
    manifest: &SkillRunnerManifest,
    runner: &str,
    run_id: &str,
    waiting: &[crate::execution::run_record::PendingRequest],
) -> RunResult {
    let requests = waiting
        .iter()
        .map(|request| request_for_public_loop(&request.id, request.value.clone()))
        .collect::<Vec<_>>();
    // A run that waits only on approvals needs a person, not an agent.
    let approvals_only = !requests.is_empty()
        && requests.iter().all(|request| {
            request
                .as_object()
                .and_then(|request| request.get("kind"))
                .and_then(JsonValue::as_str)
                == Some("approval")
        });
    let status = if approvals_only {
        "needs_approval"
    } else {
        "needs_agent"
    };
    let output = JsonObject::from([
        (
            "schema".to_owned(),
            JsonValue::String(SKILL_RUN_SCHEMA.to_owned()),
        ),
        ("status".to_owned(), JsonValue::String(status.to_owned())),
        (
            "outcome".to_owned(),
            JsonValue::String("deferred".to_owned()),
        ),
        (
            "skill_name".to_owned(),
            JsonValue::String(manifest.skill.clone()),
        ),
        ("runner".to_owned(), JsonValue::String(runner.to_owned())),
        ("run_id".to_owned(), JsonValue::String(run_id.to_owned())),
        ("requests".to_owned(), JsonValue::Array(requests.clone())),
    ]);
    RunResult {
        status: RunStatus::NeedsAgent,
        disposition: None,
        output: JsonValue::Object(output),
        receipt_refs: Vec::new(),
        child_receipt_refs: Vec::new(),
        pending_requests: requests,
        diagnostics: Vec::new(),
    }
}

fn request_for_public_loop(request_id: &str, request: JsonValue) -> JsonValue {
    let mut object = match request {
        JsonValue::Object(object) => object,
        _ => JsonObject::new(),
    };
    object.insert("id".to_owned(), JsonValue::String(request_id.to_owned()));
    object
        .entry("kind".to_owned())
        .or_insert_with(|| JsonValue::String("agent_act".to_owned()));
    JsonValue::Object(object)
}

/// Seal a run that was refused before any step ran.
fn seal_refusal(
    scope: RunScope<'_>,
    runner: &SkillRunnerDefinition,
    signature_config: &RuntimeReceiptSignatureConfig,
    output: &InvocationOutput,
    closure: StepSealClosure,
) -> Result<runx_contracts::Receipt, SkillRunError> {
    Ok(seal_step(
        StepSeal {
            scope,
            step_id: &runner.name,
            attempt: 1,
            output,
            claim: &JsonObject::new(),
            created_at: &crate::time::now_iso8601(),
            authority: crate::receipts::SealAuthority::default(),
            call: None,
            approval_refs: Vec::new(),
            child_receipts: &[],
            descendant_receipts: &[],
            closure: Some(closure),
        },
        signature_config.signature_policy(),
    )?)
}

fn sealed_output(
    skill: &str,
    runner: &str,
    run_id: &str,
    skill_output: &InvocationOutput,
    result: &JsonValue,
    diagnostics: SkillOutputDiagnostics,
    receipt: &runx_contracts::Receipt,
) -> RunResult {
    let mut output = JsonObject::new();
    output.insert(
        "schema".to_owned(),
        JsonValue::String(SKILL_RUN_SCHEMA.to_owned()),
    );
    output.insert("status".to_owned(), JsonValue::String("sealed".to_owned()));
    output.insert(
        "outcome".to_owned(),
        JsonValue::String(outcome(&receipt.seal.disposition).to_owned()),
    );
    output.insert("skill_name".to_owned(), JsonValue::String(skill.to_owned()));
    output.insert("runner".to_owned(), JsonValue::String(runner.to_owned()));
    output.insert("run_id".to_owned(), JsonValue::String(run_id.to_owned()));
    output.insert(
        "receipt_id".to_owned(),
        JsonValue::String(receipt.id.to_string()),
    );
    output.insert(
        "closure".to_owned(),
        JsonValue::Object(closure_output(&receipt.seal)),
    );
    output.insert(
        "result".to_owned(),
        skill_output.ephemeral.merged_with(result),
    );
    if let Some(context) = diagnostics.context {
        output.insert("context".to_owned(), context);
    }
    if let Some(trace) = diagnostics.trace {
        output.insert("trace".to_owned(), trace);
    }
    if let Some(refusal) = diagnostics.refusal {
        output.insert("refusal".to_owned(), JsonValue::Object(refusal));
    }
    if matches!(
        receipt.seal.disposition,
        ClosureDisposition::Failed | ClosureDisposition::Killed | ClosureDisposition::TimedOut
    ) && (skill_output.exit_code().is_some() || skill_output.failure_message().is_some())
    {
        let mut error = JsonObject::new();
        error.insert(
            "exit_code".to_owned(),
            skill_output
                .exit_code()
                .map_or(JsonValue::Null, |exit_code| {
                    JsonValue::Number(JsonNumber::I64(i64::from(exit_code)))
                }),
        );
        if let Some(message) = skill_output.failure_message() {
            error.insert("message".to_owned(), JsonValue::String(message));
        }
        output.insert("error".to_owned(), JsonValue::Object(error));
    }
    RunResult::sealed(JsonValue::Object(output), receipt)
}

#[derive(Default)]
struct SkillOutputDiagnostics {
    context: Option<JsonValue>,
    trace: Option<JsonValue>,
    /// Why a guard or an authority refused the run, when one did.
    refusal: Option<JsonObject>,
}

/// What a sealed run's closure means to its caller.
fn outcome(disposition: &ClosureDisposition) -> &'static str {
    match disposition {
        ClosureDisposition::Closed => "completed",
        ClosureDisposition::Deferred => "deferred",
        ClosureDisposition::Blocked
        | ClosureDisposition::Declined
        | ClosureDisposition::Superseded => "blocked",
        ClosureDisposition::Failed | ClosureDisposition::Killed | ClosureDisposition::TimedOut => {
            "failed"
        }
    }
}

fn closure_output(seal: &runx_contracts::Seal) -> JsonObject {
    let mut closure = JsonObject::new();
    closure.insert(
        "disposition".to_owned(),
        JsonValue::String(seal.disposition.label().to_owned()),
    );
    if let Some(reason) = seal.reason {
        closure.insert(
            "reason".to_owned(),
            JsonValue::String(reason.label().to_owned()),
        );
    }
    closure
}

fn invalid(message: impl Into<String>) -> SkillRunError {
    SkillRunError::Invalid(message.into())
}

#[cfg(test)]
mod tests;
