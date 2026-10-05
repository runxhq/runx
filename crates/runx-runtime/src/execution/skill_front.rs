// Module rationale: the skill front owns source-type dispatch,
// domain-act frame construction, and shared sealed-output projection for all
// first-class skill runners.
//! The skill front: compiles a skill-run request into an execution (cli-tool,
//! agent, or graph runner) and seals it through the shared act engine. This is
//! one of the source-type "fronts" from `plans/governed-execution-layer.md`;
//! the act engine (`execution::runner`) owns admit -> execute -> seal.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{ClosureDisposition, JsonNumber, JsonObject, JsonValue, ResolutionRequest};
use runx_parser::{ActDeclaration, SkillRunnerDefinition, SkillRunnerManifest};
use serde::Serialize;
use thiserror::Error;

use crate::RuntimeError;
use crate::adapter::{InvocationOutput, SkillInvocation};
use crate::agent_invocation::{AgentActInvocationSourceType, agent_act_resolution_request};
use crate::effects::RuntimeEffectRegistry;
use crate::execution::disposition::agent_answer_disposition_or_closed;
use crate::execution::orchestrator::{RunResult, RunStatus, SkillRunRequest};
use crate::execution::output_projection::project_step_claim;
use crate::output_contract::project_declared_output_claim;
use crate::receipts::paths::RUNX_RECEIPT_DIR_ENV;
use crate::receipts::signing::strip_receipt_signing_env;
use crate::receipts::store::ReceiptStoreError;
use crate::receipts::{
    DomainActFrame, RuntimeReceiptSignatureConfig, StepSeal, StepSealClosure, seal_step,
};
use crate::services::{ReceiptServices, WorkspaceEnv};

mod agent;
#[cfg(test)]
mod credential_tests;
mod graph;
#[cfg(feature = "cli-tool")]
mod inline_harness;
mod resolution_answers;
pub(crate) mod runner_manifest;
mod source_adapter;
mod state_store;

pub use self::state_store::inspect_paused_skill_request;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingSkillRequest {
    pub id: String,
    pub value: JsonValue,
}

pub(crate) use self::graph::graph_domain_act_receipt;
#[cfg(feature = "cli-tool")]
pub(crate) use self::inline_harness::run_package_harness_with_effects;
pub(crate) use self::source_adapter::SkillSourceAdapter;

use self::agent::execute_agent_skill_run;
use self::graph::execute_graph_skill_run;
use self::runner_manifest::{execute_adapter_skill_run, runner_invocation, selected_runner};

pub use super::operator_context::{
    SkillOperatorContextChain, SkillOperatorContextContextSkill, SkillOperatorContextDocument,
    SkillOperatorContextNode, SkillOperatorContextOptions, SkillOperatorContextPackage,
    SkillOperatorContextRegistry, SkillOperatorContextRunner, SkillOperatorContextStep,
    SkillOperatorContextTerminal, SkillOperatorContextTool, load_skill_operator_context_chain,
};
pub use super::prepared_skill::{
    PREPARED_SKILL_REPORT_SCHEMA, PreparedCredentialSummary, PreparedEntryProvenance,
    PreparedGovernanceSummary, PreparedInputSummary, PreparedRequestSummary, PreparedSkillRun,
    PreparedSkillRunReport, PreparedSkillRunStatus, PreparedTraceEntry, prepare_skill_run,
};

// The canonical public boundary is a bounded result envelope. Full receipts
// and graph checkpoints remain durable in their owning stores instead of being
// copied into every CLI/SDK response.
const SKILL_RUN_SCHEMA: &str = "runx.skill_run.v1";
const GRAPH_SKILL_STATE_SCHEMA: &str = "runx.graph_skill_state.v1";

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
    /// kept distinct from agent answers. `None` keeps the `answers_path`
    /// (resume-from-checkpoint) behavior.
    pub(crate) seeded_answers: Option<resolution_answers::ResolutionAnswers>,
    /// Native host services shared for the lifetime of a local server session.
    pub(crate) javascript: Option<crate::adapters::javascript::JavaScriptAdapter>,
    pub(crate) credential_delivery: Option<crate::credentials::CredentialDelivery>,
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

#[derive(Clone, Copy)]
struct SkillExecutionContext<'a> {
    loaded: Option<&'a std::sync::Arc<crate::LoadedSkillPackage>>,
    request: &'a SkillRunRequest,
    overrides: &'a SkillRunOverrides,
    effects: &'a RuntimeEffectRegistry,
    workspace: &'a WorkspaceEnv,
    receipts: &'a ReceiptServices,
    manifest: &'a SkillRunnerManifest,
    runner: &'a SkillRunnerDefinition,
    package_digest: &'a str,
    execution_closure_digest: Option<&'a str>,
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
    let loaded = std::sync::Arc::new(crate::load_validated_skill_package(&request.skill_path)?);
    let skill_dir = loaded.directory.clone();
    let manifest = loaded.manifest().cloned().ok_or_else(|| {
        invalid(format!(
            "skill package {} does not declare X.yaml runners",
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
    execute_skill_run_with_resolved_trust(
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

pub(crate) fn execute_prepared_skill_run_with_resolved(
    resolved: ResolvedSkillRun<'_>,
) -> Result<RunResult, SkillRunError> {
    execute_skill_run_with_resolved_trust(resolved, true)
}

fn execute_skill_run_with_resolved_trust(
    resolved: ResolvedSkillRun<'_>,
    trusted_prepared: bool,
) -> Result<RunResult, SkillRunError> {
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
    let (request, workspace, receipts) = prepare_skill_execution(
        request,
        manifest,
        runner,
        package_digest,
        execution_closure_digest,
        trusted_prepared,
    )?;
    let request = &request;
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
    if let Some(delivery) = &overrides.credential_delivery {
        validate_resolved_credential(manifest, runner, delivery)?;
    } else {
        validate_declared_credential(
            manifest,
            runner,
            request.local_credential.as_ref(),
            &skill_env,
        )?;
    }
    let mut invocation = runner_invocation(
        skill_dir,
        manifest,
        runner,
        &request.inputs,
        &skill_env,
        request.local_credential.as_ref(),
    )?;
    if let Some(delivery) = &overrides.credential_delivery {
        invocation.credential_delivery = delivery.clone();
    }
    crate::execution_environment::resolve_declared_environment(
        &invocation.requirements,
        &invocation.env,
    )?;
    let context = SkillExecutionContext {
        loaded: Some(loaded),
        request,
        overrides,
        effects,
        workspace: &workspace,
        receipts: &receipts,
        manifest,
        runner,
        package_digest,
        execution_closure_digest,
    };
    if runner.source.source_type == runx_parser::SourceKind::Graph {
        return execute_graph_skill_run(&context);
    }
    if !matches!(
        runner.source.source_type,
        runx_parser::SourceKind::Agent | runx_parser::SourceKind::AgentStep
    ) {
        return execute_adapter_skill_run(&context, invocation);
    }

    execute_agent_skill_run(&context, invocation)
}

fn validate_resolved_credential(
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    delivery: &crate::credentials::CredentialDelivery,
) -> Result<(), SkillRunError> {
    let observation = delivery.public_observation();
    let requirement = runner
        .credential
        .as_ref()
        .and_then(|name| manifest.credentials.get(name));
    match (requirement, observation) {
        (None, None) if delivery.secret_env().is_empty() => Ok(()),
        (Some(requirement), Some(observation))
            if observation.provider.as_str() == requirement.provider
                && delivery.secret_env().iter().all(|(name, _)| {
                    requirement
                        .deliveries
                        .values()
                        .any(|allowed| allowed == name)
                }) =>
        {
            Ok(())
        }
        _ => Err(invalid(format!(
            "resolved credential does not satisfy runner '{}' requirement",
            runner.name
        ))),
    }
}

#[derive(Serialize)]
struct GeneratedRunIdentity<'a> {
    schema: &'static str,
    skill: &'a str,
    runner: &'a str,
    source_type: &'a str,
    request_id: Option<&'a str>,
    inputs: &'a BTreeMap<String, JsonValue>,
    package_digest: &'a str,
    execution_closure_digest: &'a str,
}

fn generated_run_id(
    segment: &str,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    request_id: Option<&str>,
    inputs: &BTreeMap<String, JsonValue>,
    package_digest: &str,
    execution_closure_digest: Option<&str>,
) -> Result<String, SkillRunError> {
    let execution_closure_digest = execution_closure_digest
        .ok_or_else(|| invalid("generated run identity requires an execution-closure digest"))?;
    let skill = manifest.skill.as_deref().unwrap_or(&runner.name);
    let identity = GeneratedRunIdentity {
        schema: "runx.skill_run_identity.v1",
        skill,
        runner: &runner.name,
        source_type: runner.source.source_type.as_str(),
        request_id,
        inputs,
        package_digest,
        execution_closure_digest,
    };
    let bytes = serde_json::to_vec(&identity)
        .map_err(|error| invalid(format!("failed to derive skill run identity: {error}")))?;
    let digest = runx_contracts::sha256_prefixed(&bytes);
    let suffix = digest
        .strip_prefix("sha256:")
        .and_then(|value| value.get(..16))
        .ok_or_else(|| invalid("failed to derive skill run identity digest"))?;
    Ok(format!("run_{}_{}", identifier_segment(segment), suffix))
}

pub(super) fn seal_skill_preflight_refusal(
    request: &SkillRunRequest,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    package_digest: &str,
    execution_closure_digest: Option<&str>,
    failure: JsonObject,
) -> Result<String, SkillRunError> {
    let workspace =
        WorkspaceEnv::new(request.env.clone(), request.cwd.clone()).map_err(RuntimeError::from)?;
    let receipts = ReceiptServices::from_env_or_local_development(workspace.env())
        .map_err(|error| SkillRunError::Invalid(error.to_string()))?;
    let run_id = match &request.run_id {
        Some(run_id) => run_id.clone(),
        None => generated_run_id(
            &runner.name,
            manifest,
            runner,
            None,
            &request.inputs,
            package_digest,
            execution_closure_digest,
        )?,
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
    let receipt = SkillSealContext::from_services(&run_id, runner, &receipts, &workspace)
        .seal_output(
            &output,
            None,
            StepSealClosure {
                disposition: ClosureDisposition::Blocked,
                reason_code: "preflight_refused".to_owned(),
                summary: message,
            },
            Some(receipt_metadata),
        )?;
    runner_manifest::write_skill_receipt(request, &workspace, &receipts, &receipt)?;
    Ok(receipt.id.to_string())
}

fn prepare_skill_execution(
    request: &SkillRunRequest,
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    package_digest: &str,
    execution_closure_digest: Option<&str>,
    trusted_prepared: bool,
) -> Result<(SkillRunRequest, WorkspaceEnv, ReceiptServices), SkillRunError> {
    let mut request = request.clone();
    let raw_workspace =
        WorkspaceEnv::new(request.env.clone(), request.cwd.clone()).map_err(RuntimeError::from)?;
    let receipts = ReceiptServices::from_env_or_local_development(raw_workspace.env())
        .map_err(|error| SkillRunError::Invalid(error.to_string()))?;
    if request.answers_path.is_some()
        && matches!(
            runner.source.source_type,
            runx_parser::SourceKind::Agent | runx_parser::SourceKind::AgentStep
        )
    {
        state_store::restore_agent_inputs(
            &mut request,
            &raw_workspace,
            &receipts,
            &runner.name,
            package_digest,
            execution_closure_digest,
        )?;
    }
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
            let receipt_id = seal_skill_preflight_refusal(
                &request,
                manifest,
                runner,
                package_digest,
                execution_closure_digest,
                failure,
            )?;
            return Err(SkillRunError::PreflightRefused {
                source: Box::new(source),
                receipt_id,
            });
        }
    };
    let mut runtime_env = request.env.clone();
    let resolved_receipt_path =
        receipts.resolve_path(&raw_workspace, request.receipt_dir.as_deref(), None);
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

fn validate_declared_credential(
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    local: Option<&crate::execution::orchestrator::LocalCredentialDescriptor>,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<(), SkillRunError> {
    let hosted = env
        .get(crate::credentials::RUNX_HOSTED_CREDENTIAL_HANDLES_JSON_ENV)
        .filter(|value| !value.trim().is_empty());
    let Some(requirement_name) = runner.credential.as_ref() else {
        if local.is_some() || hosted.is_some() {
            return Err(invalid(format!(
                "runner '{}' received a credential but declares no credential requirement",
                runner.name
            )));
        }
        return Ok(());
    };
    let requirement = manifest.credentials.get(requirement_name).ok_or_else(|| {
        invalid(format!(
            "runner '{}' references undeclared credential '{}'",
            runner.name, requirement_name
        ))
    })?;
    if let Some(local) = local {
        return validate_local_credential(local, requirement, runner, requirement_name);
    }
    if let Some(hosted) = hosted {
        return validate_hosted_credential(hosted, &requirement.provider, runner);
    }
    Err(invalid(format!(
        "runner '{}' requires credential '{}' for provider '{}'",
        runner.name, requirement_name, requirement.provider
    )))
}

fn validate_local_credential(
    local: &crate::execution::orchestrator::LocalCredentialDescriptor,
    requirement: &runx_parser::CredentialRequirement,
    runner: &SkillRunnerDefinition,
    requirement_name: &str,
) -> Result<(), SkillRunError> {
    if local.provider == requirement.provider
        && requirement.deliveries.get(&local.auth_mode) == Some(&local.env_var)
    {
        return Ok(());
    }
    Err(invalid(format!(
        "credential provision does not satisfy runner '{}' requirement '{}'",
        runner.name, requirement_name
    )))
}

fn validate_hosted_credential(
    hosted: &str,
    required_provider: &str,
    runner: &SkillRunnerDefinition,
) -> Result<(), SkillRunError> {
    let provider = crate::credentials::CredentialDelivery::hosted_handles_provider(hosted)
        .map_err(|error| {
            invalid(format!(
                "hosted credential handle admission failed: {error}"
            ))
        })?;
    if provider.as_deref() == Some(required_provider) {
        return Ok(());
    }
    Err(invalid(format!(
        "hosted credential does not satisfy runner '{}' provider '{}'",
        runner.name, required_provider
    )))
}

/// Aggregate result of running a package's inline and conventional fixture
/// cases. Mirrors the publish-harness summary the registry records: a status,
/// counts, per-case assertion failures, case names, sealed receipt ids, and how
/// many cases exercised a graph.
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
    #[cfg(feature = "cli-tool")]
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

fn agent_invocation_source_type(
    value: &str,
) -> Result<AgentActInvocationSourceType, SkillRunError> {
    AgentActInvocationSourceType::from_contract_value(value)
        .ok_or_else(|| invalid(format!("unsupported agent source type {value}")))
}

fn agent_request(
    invocation: &SkillInvocation,
    source_type: AgentActInvocationSourceType,
) -> Result<ResolutionRequest, SkillRunError> {
    agent_act_resolution_request(invocation, source_type).map_err(Into::into)
}

fn needs_agent_output(
    manifest: &SkillRunnerManifest,
    runner: &str,
    run_id: &str,
    request_id: &str,
    request: JsonValue,
) -> RunResult {
    let mut output = JsonObject::new();
    output.insert(
        "schema".to_owned(),
        JsonValue::String(SKILL_RUN_SCHEMA.to_owned()),
    );
    output.insert(
        "status".to_owned(),
        JsonValue::String("needs_agent".to_owned()),
    );
    output.insert(
        "skill_name".to_owned(),
        JsonValue::String(manifest.skill.clone().unwrap_or_else(|| "skill".to_owned())),
    );
    output.insert("runner".to_owned(), JsonValue::String(runner.to_owned()));
    output.insert("run_id".to_owned(), JsonValue::String(run_id.to_owned()));
    let pending_requests = vec![request_for_public_loop(request_id, request)];
    output.insert(
        "requests".to_owned(),
        JsonValue::Array(pending_requests.clone()),
    );
    RunResult {
        status: RunStatus::NeedsAgent,
        disposition: None,
        output: JsonValue::Object(output),
        receipt_refs: Vec::new(),
        child_receipt_refs: Vec::new(),
        pending_requests,
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

fn read_answer(
    path: &Path,
    request_id: &str,
    request: &JsonValue,
) -> Result<JsonValue, SkillRunError> {
    let raw = fs::read_to_string(path)
        .map_err(|source| RuntimeError::io(format!("reading {}", path.display()), source))?;
    let value = serde_json::from_str::<JsonValue>(&raw).map_err(|source| {
        RuntimeError::json(format!("parsing answers file {}", path.display()), source)
    })?;
    let answers = match &value {
        JsonValue::Object(object) => {
            if let Some(digests) = object.get("request_digests") {
                let digests = digests
                    .as_object()
                    .ok_or_else(|| invalid("request_digests field must be a JSON object"))?;
                let supplied = digests
                    .get(request_id)
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        invalid(format!(
                            "request_digests did not include pending request {request_id}"
                        ))
                    })?;
                let bytes = serde_json::to_vec(request).map_err(|source| {
                    RuntimeError::json("serializing pending request for digest binding", source)
                })?;
                let expected = runx_contracts::sha256_prefixed(&bytes);
                if supplied != expected {
                    return Err(invalid(format!(
                        "request digest mismatch for {request_id}: supplied {supplied}, current {expected}"
                    )));
                }
            }
            match object.get("answers") {
                Some(JsonValue::Object(nested)) => nested,
                _ => object,
            }
        }
        _ => return Err(invalid("answers file must be a JSON object")),
    };
    answers
        .get(request_id)
        .cloned()
        .ok_or_else(|| invalid(format!("answers file did not include {request_id}")))
}

#[derive(Clone, Copy)]
struct SkillSealContext<'a> {
    run_id: &'a str,
    runner: &'a SkillRunnerDefinition,
    signature_config: &'a RuntimeReceiptSignatureConfig,
    env: &'a std::collections::BTreeMap<String, String>,
}

impl<'a> SkillSealContext<'a> {
    fn from_services(
        run_id: &'a str,
        runner: &'a SkillRunnerDefinition,
        receipts: &'a ReceiptServices,
        workspace: &'a WorkspaceEnv,
    ) -> Self {
        Self {
            run_id,
            runner,
            signature_config: receipts.signature_config(),
            env: workspace.env(),
        }
    }

    fn seal_answer(
        self,
        answer: &JsonValue,
        claim_payload: &JsonValue,
        disposition: ClosureDisposition,
        metadata: JsonObject,
    ) -> Result<runx_contracts::Receipt, SkillRunError> {
        let disposition_label = disposition.label();
        let succeeded = disposition == ClosureDisposition::Closed;
        let skill_output = if succeeded {
            InvocationOutput::runtime_success(answer.clone(), 0, metadata)
        } else {
            InvocationOutput::runtime_failure(
                answer.clone(),
                format!("agent act closed with {disposition_label}"),
                0,
                metadata,
            )
        };
        self.seal_output(
            &skill_output,
            Some(claim_payload),
            StepSealClosure {
                disposition,
                reason_code: format!("agent_act_{disposition_label}"),
                summary: format!("agent act closed with {disposition_label}"),
            },
            None,
        )
    }
}

/// Build the domain act frame for a governed turn when its runner declares an
/// `act:` block: the trusted mapping from the driver's pinned beat inputs and the
/// model's reason text to the receipt's act, decision, and authority. Returns
/// `None` for runners without an `act:` block (sealed generically, exactly as
/// before). The model supplies only the reason prose; every structural field is
/// read from the runner declaration and the trusted inputs, never the model.
fn domain_act_frame(
    invocation: &SkillInvocation,
    answer: &JsonValue,
    governed_effect: Option<&JsonValue>,
) -> Option<DomainActFrame> {
    let act = invocation.source.act.as_ref()?;
    // Promote the delivered credential into the act's held authority: a governed
    // turn's receipt records the grants it actually carried, not just the
    // declared scope.
    let authority_grant_refs = invocation
        .credential_delivery
        .public_observation()
        .map(|observation| observation.credential_refs.clone())
        .unwrap_or_default();
    build_domain_act_frame(
        act,
        &invocation.inputs,
        answer,
        governed_effect,
        authority_grant_refs,
    )
}

/// The core of [`domain_act_frame`], reusable by the graph path: build the domain
/// act frame from a declared `act:` block, the trusted run inputs, the model's
/// authored reason source, and the real governed effect.
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
    use runx_contracts::{
        ActForm, AuthorityAttenuation, AuthoritySubsetProof, AuthorityTerm, DecisionChoice,
        Reference, ReferenceType,
    };

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
        "provider" | "provider_event" => ReferenceType::ProviderEvent,
        "provider_thread" => ReferenceType::ProviderThread,
        "provider_comment" => ReferenceType::ProviderComment,
        "github_issue" => ReferenceType::GithubIssue,
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

    // Charter attenuation, read from driver-pinned inputs (the model never sets
    // authority). The member's child term, the parent charter reference, and the
    // subset proof each ride a trusted input key named by the act declaration.
    // Attenuation is recorded only when both a parent and a proof are present; a
    // term without them is a root and carries no proof, as the receipt verifier
    // requires.
    let authority_terms = act
        .authority_term_from
        .as_deref()
        .and_then(|key| inputs.get(key))
        .and_then(|value| serde_json::to_value(value).ok())
        .and_then(|value| serde_json::from_value::<AuthorityTerm>(value).ok())
        .map(|term| vec![term])
        .unwrap_or_default();
    let parent_authority_ref = act
        .authority_parent_from
        .as_deref()
        .and_then(|key| inputs.get(key))
        .and_then(|value| serde_json::to_value(value).ok())
        .and_then(|value| serde_json::from_value::<Reference>(value).ok());
    let subset_proof = act
        .authority_subset_proof_from
        .as_deref()
        .and_then(|key| inputs.get(key))
        .and_then(|value| serde_json::to_value(value).ok())
        .and_then(|value| serde_json::from_value::<AuthoritySubsetProof>(value).ok());
    let authority_attenuation = match (parent_authority_ref, subset_proof) {
        (Some(parent), Some(proof)) => Some(AuthorityAttenuation {
            parent_authority_ref: Some(parent),
            subset_proof: Some(proof),
        }),
        _ => None,
    };

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
        authority_terms,
        authority_attenuation,
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

impl SkillSealContext<'_> {
    fn seal_output(
        self,
        output: &InvocationOutput,
        claim_payload: Option<&JsonValue>,
        closure: StepSealClosure,
        receipt_metadata: Option<JsonObject>,
    ) -> Result<runx_contracts::Receipt, SkillRunError> {
        let graph_name = identifier_segment(self.run_id);
        let step_id = identifier_segment(&self.runner.name);
        let claim = if output.succeeded() {
            project_declared_output_claim(
                &self.runner.name,
                claim_payload.unwrap_or(&output.value),
                self.runner.source.outputs.as_ref(),
                self.runner.artifacts.as_ref(),
            )?
        } else {
            JsonObject::new()
        };
        let mut projection = project_step_claim(claim);
        Ok(seal_step(
            StepSeal {
                graph_name: &graph_name,
                step_id: &step_id,
                attempt: 1,
                output,
                claim: &projection.outputs,
                projection_refs: std::mem::take(&mut projection.refs),
                created_at: &crate::time::now_iso8601(),
                authority_grant_refs: Vec::new(),
                authority_scope_refs: Vec::new(),
                operator_refs: super::prepared_skill::prepared_receipt_references(self.env),
                child_receipts: &[],
                descendant_receipts: &[],
                closure: Some(closure),
                receipt_metadata,
            },
            self.signature_config.signature_policy(),
        )?)
    }
}

fn answer_disposition(answer: &JsonValue) -> Result<ClosureDisposition, SkillRunError> {
    agent_answer_disposition_or_closed(answer).map_err(|error| invalid(format!("{error}")))
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
        JsonValue::String(manifest.skill.clone().unwrap_or_else(|| "skill".to_owned())),
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

fn normalize_request_id(value: &str) -> String {
    let mut normalized = String::new();
    let mut replaced = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-') {
            normalized.push(character);
            replaced = false;
        } else if !replaced {
            normalized.push('_');
            replaced = true;
        }
    }
    normalized
}

fn identifier_segment(value: &str) -> String {
    normalize_request_id(value)
        .trim_matches(['.', '_', '-'])
        .replace('.', "-")
}

fn contract_json_value(value: &impl serde::Serialize) -> Result<JsonValue, SkillRunError> {
    let value = serde_json::to_value(value)
        .map_err(|source| RuntimeError::json("serializing native skill contract value", source))?;
    serde_json::from_value(value).map_err(|source| {
        RuntimeError::json("normalizing native skill contract value", source).into()
    })
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

#[cfg(test)]
mod run_identity_tests {
    use super::*;

    fn manifest() -> Result<SkillRunnerManifest, Box<dyn std::error::Error>> {
        let raw = runx_parser::parse_runner_manifest_yaml(
            r#"
skill: identity-test
runners:
  execute:
    default: true
    type: agent-task
    agent: reviewer
    task: review
    outputs:
      result: object
"#,
        )?;
        Ok(runx_parser::validate_runner_manifest(raw)?)
    }

    #[test]
    fn generated_run_identity_binds_package_and_execution_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let manifest = manifest()?;
        let runner = manifest
            .runners
            .get("execute")
            .ok_or("missing execute runner")?;
        let inputs = BTreeMap::from([(
            "claim_id".to_owned(),
            JsonValue::String("claim-1".to_owned()),
        )]);
        let baseline = generated_run_id(
            "execute",
            &manifest,
            runner,
            None,
            &inputs,
            "sha256:package-a",
            Some("sha256:closure-a"),
        )?;
        let same = generated_run_id(
            "execute",
            &manifest,
            runner,
            None,
            &inputs,
            "sha256:package-a",
            Some("sha256:closure-a"),
        )?;
        let changed_package = generated_run_id(
            "execute",
            &manifest,
            runner,
            None,
            &inputs,
            "sha256:package-b",
            Some("sha256:closure-a"),
        )?;
        let changed_closure = generated_run_id(
            "execute",
            &manifest,
            runner,
            None,
            &inputs,
            "sha256:package-a",
            Some("sha256:closure-b"),
        )?;

        assert_eq!(baseline, same);
        assert_ne!(baseline, changed_package);
        assert_ne!(baseline, changed_closure);
        Ok(())
    }

    #[test]
    fn generated_run_identity_refuses_unbound_execution() -> Result<(), Box<dyn std::error::Error>>
    {
        let manifest = manifest()?;
        let runner = manifest
            .runners
            .get("execute")
            .ok_or("missing execute runner")?;
        let error = generated_run_id(
            "execute",
            &manifest,
            runner,
            None,
            &BTreeMap::new(),
            "sha256:package",
            None,
        )
        .err()
        .ok_or("unbound generated run identity did not fail")?;

        assert!(
            error
                .to_string()
                .contains("requires an execution-closure digest")
        );
        Ok(())
    }
}
