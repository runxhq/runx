// Module rationale: the skill front owns run preparation, domain-act frame
// construction, and shared sealed-output projection for every runner.
//! The skill front: compiles a skill-run request into a run of the runner's
//! graph and seals it through the shared act engine (`execution::runner`),
//! which owns admit -> execute -> seal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{ClosureDisposition, JsonNumber, JsonObject, JsonValue};
use runx_parser::{ActDeclaration, SkillRunnerDefinition, SkillRunnerManifest};
use serde::Serialize;
use thiserror::Error;

use crate::RuntimeError;
use crate::adapter::InvocationOutput;
use crate::effects::RuntimeEffectRegistry;
use crate::execution::orchestrator::{RunResult, RunStatus, SkillRunRequest};
use crate::paths::RUNX_RECEIPT_DIR_ENV;
use crate::receipts::signing::strip_receipt_signing_env;
use crate::receipts::store::ReceiptStoreError;
use crate::receipts::{
    DomainActFrame, RunScope, RuntimeReceiptSignatureConfig, StepSeal, StepSealClosure, seal_step,
};
use crate::services::{ReceiptServices, WorkspaceEnv};

mod executable_adapter;
mod graph;
#[cfg(feature = "process-tool")]
mod inline_harness;
mod resolution_answers;
pub(crate) mod runner_manifest;
mod turn_session;

pub(crate) use self::executable_adapter::ExecutableAdapter;
pub(crate) use self::graph::graph_domain_act_receipt;
#[cfg(feature = "process-tool")]
pub(crate) use self::inline_harness::run_package_harness_with_effects;
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
    /// Resolution values seeded for a single fresh run, with human approvals
    /// kept distinct from agent answers. `None` reads the request's
    /// `answers_path`.
    pub(crate) seeded_answers: Option<resolution_answers::ResolutionAnswers>,
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
    pub(crate) answers_path: Option<&'a Path>,
    pub(crate) managed_agent: crate::execution::orchestrator::ManagedAgentPolicy,
    /// Digests a caller pins; each must be the one the run started under.
    pub(crate) package_digest: Option<&'a str>,
    pub(crate) execution_closure_digest: Option<&'a str>,
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
            skill_path: PathBuf::from(&paused.skill_ref),
            receipt_dir: continuation.receipt_dir.map(Path::to_path_buf),
            run_id: Some(paused.run_id),
            answers_path: continuation.answers_path.map(Path::to_path_buf),
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
        )?;
    }
    crate::execution_environment::resolve_declared_environment(
        &manifest.execution_requirements(runner),
        &skill_env,
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
/// preparation before admission.
pub(super) fn seal_skill_preflight_refusal(
    request: &SkillRunRequest,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    failure: JsonObject,
) -> Result<String, SkillRunError> {
    let workspace =
        WorkspaceEnv::new(request.env.clone(), request.cwd.clone()).map_err(RuntimeError::from)?;
    let receipts = ReceiptServices::from_env_or_local_development(workspace.env())
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
    let mut receipt_metadata = failure.clone();
    receipt_metadata.remove("accepted_schema");
    let output = InvocationOutput::runtime_failure(
        JsonValue::Object(failure),
        message.clone(),
        0,
        JsonObject::new(),
    );
    let receipt = seal_refusal(
        RunScope {
            run_id: &run_id,
            graph_name: &manifest.skill,
        },
        runner,
        receipts.signature_config(),
        workspace.env(),
        &output,
        StepSealClosure {
            disposition: ClosureDisposition::Blocked,
            reason_code: "preflight_refused".to_owned(),
            summary: message,
        },
        receipt_metadata,
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
    let receipts = ReceiptServices::from_env_or_local_development(raw_workspace.env())
        .map_err(|error| SkillRunError::Invalid(error.to_string()))?;
    restore_continuation_inputs(&mut request, &raw_workspace)?;
    crate::input_contract::apply_defaults(&runner.inputs, &mut request.inputs);
    let materialize = if request.answers_path.is_some() {
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
            let receipt_id = seal_skill_preflight_refusal(&request, manifest, runner, failure)?;
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
    let answered = request.answers_path.is_some();
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

/// Aggregate result of running a package's inline and conventional fixture
/// cases. Mirrors the publish-harness summary the registry records: a status,
/// counts, per-case assertion failures, case names, sealed receipt ids, and how
/// many `kind: graph` fixtures proved the skill composes into a graph.
#[derive(Clone, Debug, Serialize)]
pub struct PackageHarnessReport {
    pub status: &'static str,
    pub case_count: usize,
    pub assertion_error_count: usize,
    pub assertion_errors: Vec<String>,
    pub case_names: Vec<String>,
    pub receipt_ids: Vec<String>,
    pub graph_case_count: usize,
}

impl PackageHarnessReport {
    #[cfg(feature = "process-tool")]
    fn not_declared() -> Self {
        Self {
            status: "not_declared",
            case_count: 0,
            assertion_error_count: 0,
            assertion_errors: Vec::new(),
            case_names: Vec::new(),
            receipt_ids: Vec::new(),
            graph_case_count: 0,
        }
    }
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
    let output = JsonObject::from([
        (
            "schema".to_owned(),
            JsonValue::String(SKILL_RUN_SCHEMA.to_owned()),
        ),
        (
            "status".to_owned(),
            JsonValue::String("needs_agent".to_owned()),
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

/// Build the domain act frame from a declared `act:` block, the trusted run
/// inputs, the model's authored reason source, and the real governed effect.
// Function rationale: act-frame construction is intentionally one
// branch table so each declared field, input fallback, and governed-effect
// reference is visible in one receipt-shaping pass.
fn build_domain_act_frame(
    act: &ActDeclaration,
    inputs: &runx_contracts::JsonObject,
    reason_source: &JsonValue,
    governed_effect: Option<&JsonValue>,
    authority_grant_refs: Vec<runx_contracts::Reference>,
) -> Option<DomainActFrame> {
    use runx_contracts::{ActForm, DecisionChoice, Reference, ReferenceType};

    // A declared field may be a static literal (`form: review`) or driver-pinned
    // from an input (`form_from: act_form` names the input key). The driver-pinned
    // input wins, so one generic skill serves every beat.
    let resolve = |from_key: Option<&str>, literal: Option<&str>| -> Option<String> {
        from_key
            .and_then(|key| inputs.get(key))
            .and_then(JsonValue::as_str)
            .or(literal)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };

    let form = match resolve(act.form_from.as_deref(), act.form.as_deref())
        .as_deref()
        .unwrap_or("observation")
    {
        "revision" => ActForm::Revision,
        "reply" => ActForm::Reply,
        "review" => ActForm::Review,
        "verification" => ActForm::Verification,
        _ => ActForm::Observation,
    };
    let purpose = resolve(act.purpose_from.as_deref(), act.purpose.as_deref())?;
    let legitimacy = resolve(act.legitimacy_from.as_deref(), act.legitimacy.as_deref())
        .unwrap_or_else(|| "Held the declared authority for this act".to_owned());

    // The human reason text. `reason_from_input` seals the calling operator's own
    // line from a trusted input (the operator holds the context and voice);
    // `reason_from` reads a declared step's authored output. The parser holds the
    // two mutually exclusive.
    let reason = act
        .reason_from_input
        .as_deref()
        .and_then(|key| inputs.get(key))
        .and_then(JsonValue::as_str)
        .or_else(|| {
            act.reason_from
                .as_deref()
                .and_then(|key| reason_source.as_object().and_then(|object| object.get(key)))
                .and_then(JsonValue::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(|| purpose.clone(), str::to_owned);

    // Resolve a trusted input value (a uri) named by the act mapping into a ref.
    let input_ref = |map_key: Option<&str>, reference_type: ReferenceType| -> Option<Reference> {
        let uri = inputs.get(map_key?).and_then(JsonValue::as_str)?.trim();
        (!uri.is_empty()).then(|| Reference::with_uri(reference_type, uri.to_owned()))
    };

    let decision_choice = act
        .decision_from
        .as_deref()
        .and_then(|key| inputs.get(key))
        .and_then(JsonValue::as_str)
        .and_then(map_decision_choice)
        .unwrap_or(DecisionChoice::Close);

    let reference_type = match act.effect_type.as_deref().unwrap_or("artifact") {
        "act" => ReferenceType::Act,
        "tracking_item" => ReferenceType::TrackingItem,
        "receipt" => ReferenceType::Receipt,
        "event" => ReferenceType::Event,
        "thread" => ReferenceType::Thread,
        "comment" => ReferenceType::Comment,
        "change_request" => ReferenceType::ChangeRequest,
        "repository" => ReferenceType::Repository,
        "external_url" => ReferenceType::ExternalUrl,
        _ => ReferenceType::Artifact,
    };
    let prefix = resolve(
        act.effect_prefix_from.as_deref(),
        act.effect_prefix.as_deref(),
    )
    .unwrap_or_default();
    let effect_ref = |id: &str| {
        let id = id.trim();
        (!id.is_empty())
            .then(|| Reference::with_uri(reference_type.clone(), format!("{prefix}{id}")))
    };
    let mut artifact_refs = Vec::new();
    if let Some(reference) = act
        .effect_from_input
        .as_deref()
        .and_then(|key| inputs.get(key))
        .and_then(JsonValue::as_str)
        .and_then(effect_ref)
    {
        artifact_refs.push(reference);
    }
    if let Some(reference) = governed_effect
        .and_then(|effect| {
            let field = resolve(act.effect_field_from.as_deref(), act.effect_from.as_deref())?;
            effect
                .as_object()
                .and_then(|object| object.get(field.as_str()))
                .and_then(JsonValue::as_str)
                .and_then(effect_ref)
        })
        .filter(|reference| !artifact_refs.contains(reference))
    {
        artifact_refs.push(reference);
    }

    Some(DomainActFrame {
        form,
        purpose: purpose.into(),
        legitimacy: legitimacy.into(),
        summary: reason.clone().into(),
        target_refs: input_ref(act.target_from.as_deref(), ReferenceType::TrackingItem)
            .into_iter()
            .collect(),
        artifact_refs,
        decision_choice,
        decision_summary: reason.into(),
        actor_ref: input_ref(act.actor_from.as_deref(), ReferenceType::Principal)
            .unwrap_or_else(|| Reference::runx(ReferenceType::Principal, "local_runtime")),
        authority_grant_refs,
        authority_scope_refs: input_ref(act.authority_from.as_deref(), ReferenceType::Grant)
            .into_iter()
            .collect(),
        authority_terms: Vec::new(),
        authority_attenuation: None,
        previous: input_ref(act.previous_from.as_deref(), ReferenceType::Receipt),
    })
}

/// Map a driver-pinned decision word onto the receipt's `DecisionChoice`.
fn map_decision_choice(value: &str) -> Option<runx_contracts::DecisionChoice> {
    use runx_contracts::DecisionChoice;
    match value.trim().to_ascii_lowercase().as_str() {
        "decline" | "reject" | "rejected" | "deny" | "denied" => Some(DecisionChoice::Decline),
        "close" | "accept" | "accepted" | "approve" | "approved" | "paid" | "settle"
        | "settled" => Some(DecisionChoice::Close),
        "continue" | "claim" | "claimed" | "deliver" | "delivered" => {
            Some(DecisionChoice::Continue)
        }
        "defer" | "deferred" => Some(DecisionChoice::Defer),
        "escalate" | "escalated" => Some(DecisionChoice::Escalate),
        "monitor" | "monitored" => Some(DecisionChoice::Monitor),
        _ => None,
    }
}

/// Seal a run that was refused before any step ran.
fn seal_refusal(
    scope: RunScope<'_>,
    runner: &SkillRunnerDefinition,
    signature_config: &RuntimeReceiptSignatureConfig,
    env: &BTreeMap<String, String>,
    output: &InvocationOutput,
    closure: StepSealClosure,
    receipt_metadata: JsonObject,
) -> Result<runx_contracts::Receipt, SkillRunError> {
    Ok(seal_step(
        StepSeal {
            scope,
            step_id: &runner.name,
            attempt: 1,
            output,
            claim: &JsonObject::new(),
            projection_refs: Default::default(),
            created_at: &crate::time::now_iso8601(),
            authority: crate::receipts::SealAuthority::default(),
            operator_refs: super::prepared_skill::prepared_receipt_references(env),
            child_receipts: &[],
            descendant_receipts: &[],
            closure: Some(closure),
            receipt_metadata: Some(receipt_metadata),
        },
        signature_config.signature_policy(),
    )?)
}

fn sealed_output(
    manifest: &SkillRunnerManifest,
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
        "skill_name".to_owned(),
        JsonValue::String(manifest.skill.clone()),
    );
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
    if let Some(observations) = skill_output
        .metadata
        .get(crate::adapter::CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA)
    {
        output.insert(
            crate::adapter::CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA.to_owned(),
            observations.clone(),
        );
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
}

fn closure_output(seal: &runx_contracts::Seal) -> JsonObject {
    let mut closure = JsonObject::new();
    closure.insert(
        "disposition".to_owned(),
        JsonValue::String(seal.disposition.label().to_owned()),
    );
    closure.insert(
        "reason_code".to_owned(),
        JsonValue::String(seal.reason_code.to_string()),
    );
    closure.insert(
        "summary".to_owned(),
        JsonValue::String(seal.summary.to_string()),
    );
    closure.insert(
        "closed_at".to_owned(),
        JsonValue::String(seal.closed_at.to_string()),
    );
    closure
}

fn invalid(message: impl Into<String>) -> SkillRunError {
    SkillRunError::Invalid(message.into())
}

#[cfg(test)]
mod domain_act_frame_tests {
    use super::*;

    fn act(value: serde_json::Value) -> Result<ActDeclaration, serde_json::Error> {
        serde_json::from_value(value)
    }

    fn inputs(value: serde_json::Value) -> Result<JsonObject, serde_json::Error> {
        serde_json::from_value(value)
    }

    #[test]
    fn operator_reason_input_seals_the_operator_line() -> Result<(), Box<dyn std::error::Error>> {
        let frame = build_domain_act_frame(
            &act(serde_json::json!({
                "purpose_from": "act_purpose",
                "reason_from_input": "line",
            }))?,
            &inputs(serde_json::json!({
                "act_purpose": "settle the accepted claim",
                "line": "Paid @worker for the accepted delivery on #120.",
            }))?,
            &JsonValue::Null,
            None,
            Vec::new(),
        )
        .ok_or_else(|| std::io::Error::other("frame"))?;
        assert_eq!(
            frame.summary.as_str(),
            "Paid @worker for the accepted delivery on #120."
        );
        Ok(())
    }

    #[test]
    fn absent_operator_line_falls_back_to_purpose() -> Result<(), Box<dyn std::error::Error>> {
        let frame = build_domain_act_frame(
            &act(serde_json::json!({
                "purpose_from": "act_purpose",
                "reason_from_input": "line",
            }))?,
            &inputs(serde_json::json!({
                "act_purpose": "settle the accepted claim",
            }))?,
            &JsonValue::Null,
            None,
            Vec::new(),
        )
        .ok_or_else(|| std::io::Error::other("frame"))?;
        assert_eq!(frame.summary.as_str(), "settle the accepted claim");
        Ok(())
    }
}
