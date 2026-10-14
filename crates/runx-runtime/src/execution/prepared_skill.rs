// Module rationale: prepared requests keep digest construction,
// drift guards, approval evidence, and their security fixtures co-located.
//! Digest-bound preparation for operator-approved skill execution.
//!
//! The public report is deliberately safe to print or serialize. The owned
//! request and canonical digest preimage are private so raw input bodies and
//! credential material cannot leak through the approval surface.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{
    ExecutionBoundaryKind, JsonObject, JsonValue, Reference, ReferenceType, sha256_prefixed,
};
use runx_parser::{SkillRunnerDefinition, SkillRunnerManifest, StepKind};
use serde::{Deserialize, Serialize};

use super::operator_context::{
    SkillOperatorContextChain, SkillOperatorContextNode, SkillOperatorContextOptions,
    load_skill_operator_context_chain_from_package,
};
use super::orchestrator::ManagedAgentPolicy;
use super::orchestrator::SkillRunRequest;
use super::skill_front::SkillRunError;
use super::skill_front::runner_manifest::selected_runner;
use crate::RuntimeError;

pub const PREPARED_SKILL_REPORT_SCHEMA: &str = "runx.prepared_skill_run.v1";
pub(crate) const PREPARED_CONTEXT_DIGEST_ENV: &str = "RUNX_INTERNAL_PREPARED_CONTEXT_DIGEST";
pub(crate) const PREPARED_ARTIFACT_GUARDS_ENV: &str = "RUNX_INTERNAL_PREPARED_ARTIFACT_GUARDS";

pub(crate) fn strip_untrusted_prepared_env(env: &mut BTreeMap<String, String>) {
    for name in [PREPARED_CONTEXT_DIGEST_ENV, PREPARED_ARTIFACT_GUARDS_ENV] {
        env.remove(name);
    }
}

/// The operator context a run was prepared with: its digest, which every act
/// the run seals carries as its `context_ref`, and the artifacts it read,
/// each checked again where the run uses it. The run's record keeps it, so a
/// resumed run is held to the context it started with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedContext {
    pub digest: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub artifact_guards: BTreeMap<String, String>,
}

impl PreparedContext {
    /// The context a prepared run's environment carries, if it was prepared.
    pub(crate) fn from_env(env: &BTreeMap<String, String>) -> Result<Option<Self>, RuntimeError> {
        let Some(digest) = env.get(PREPARED_CONTEXT_DIGEST_ENV) else {
            return Ok(None);
        };
        let artifact_guards = match env.get(PREPARED_ARTIFACT_GUARDS_ENV) {
            Some(encoded) => serde_json::from_str(encoded)
                .map_err(|source| RuntimeError::json("parsing prepared artifact guards", source))?,
            None => BTreeMap::new(),
        };
        Ok(Some(Self {
            digest: digest.clone(),
            artifact_guards,
        }))
    }

    /// Carry `context` into a run's environment, in place of whatever the
    /// environment held.
    pub(crate) fn apply(
        context: Option<&Self>,
        env: &mut BTreeMap<String, String>,
    ) -> Result<(), RuntimeError> {
        strip_untrusted_prepared_env(env);
        if let Some(context) = context {
            let guards = serde_json::to_string(&context.artifact_guards).map_err(|source| {
                RuntimeError::json("serializing prepared artifact guards", source)
            })?;
            env.insert(
                PREPARED_CONTEXT_DIGEST_ENV.to_owned(),
                context.digest.clone(),
            );
            env.insert(PREPARED_ARTIFACT_GUARDS_ENV.to_owned(), guards);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparedSkillRunStatus {
    Ready,
    Blocked,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedEntryProvenance {
    pub kind: String,
    pub reference: Option<String>,
    pub source: String,
    pub source_label: String,
    pub skill_id: Option<String>,
    pub version: Option<String>,
    pub digest: Option<String>,
    pub package_digest: Option<String>,
    pub execution_closure_digest: Option<String>,
    pub trust_tier: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedInputSummary {
    pub name: String,
    pub value_type: String,
    pub canonical_bytes: usize,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedRequestSummary {
    pub skill_path: PathBuf,
    pub cwd: PathBuf,
    pub runner: String,
    pub receipt_dir: Option<PathBuf>,
    pub run_id: Option<String>,
    pub inputs: Vec<PreparedInputSummary>,
    pub managed_agent: ManagedAgentPolicy,
    pub entry: PreparedEntryProvenance,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedGovernanceSummary {
    pub declared_steps: usize,
    pub conditional_steps: usize,
    pub tool_refs: Vec<String>,
    pub authority_scopes: Vec<String>,
    pub execution_boundaries: Vec<ExecutionBoundaryKind>,
    pub gates: Vec<String>,
    pub idempotency_keys: Vec<String>,
    pub managed_agent_acts: usize,
    pub managed_agent_enabled: bool,
    pub managed_agent_max_rounds: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedTraceEntry {
    pub node_path: String,
    pub stage: String,
    pub outcome: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PreparedSkillRunReport {
    pub schema: String,
    pub status: PreparedSkillRunStatus,
    pub digest: String,
    pub request: PreparedRequestSummary,
    pub governance: PreparedGovernanceSummary,
    pub chain: Option<SkillOperatorContextChain>,
    pub trace: Vec<PreparedTraceEntry>,
    pub blocked_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal_receipt_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedArtifactGuard {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Clone)]
pub struct PreparedSkillRun {
    request: SkillRunRequest,
    package_digest: String,
    selected_runner: String,
    manifest: SkillRunnerManifest,
    runner: SkillRunnerDefinition,
    report: PreparedSkillRunReport,
    guards: Vec<PreparedArtifactGuard>,
    context_bound: bool,
}

impl std::fmt::Debug for PreparedSkillRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedSkillRun")
            .field("package_digest", &self.package_digest)
            .field("selected_runner", &self.selected_runner)
            .field("report", &self.report)
            .field("guard_count", &self.guards.len())
            .field("context_bound", &self.context_bound)
            .finish_non_exhaustive()
    }
}

impl PreparedSkillRun {
    #[must_use]
    pub fn report(&self) -> &PreparedSkillRunReport {
        &self.report
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.report.digest
    }

    #[must_use]
    pub(crate) fn package_digest(&self) -> &str {
        &self.package_digest
    }

    #[must_use]
    pub(crate) fn execution_closure_digest(&self) -> Option<&str> {
        self.report
            .request
            .entry
            .execution_closure_digest
            .as_deref()
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.report.status == PreparedSkillRunStatus::Ready
    }

    /// Bind the exact prepared context without fabricating action approval.
    ///
    /// Consequential actions are approved by their owning graph gate or
    /// effect boundary after the exact action is known. Preparation binds
    /// context and artifact drift only.
    pub fn bind_context(&mut self) -> Result<(), SkillRunError> {
        self.bind_prepared_context()
    }

    fn bind_prepared_context(&mut self) -> Result<(), SkillRunError> {
        if !self.is_ready() {
            return Err(SkillRunError::Invalid(
                self.report
                    .blocked_reason
                    .clone()
                    .unwrap_or_else(|| "prepared skill run is blocked".to_owned()),
            ));
        }
        let artifact_guards = self
            .guards
            .iter()
            .map(|guard| {
                let path = fs::canonicalize(&guard.path).unwrap_or_else(|_| guard.path.clone());
                (path.to_string_lossy().into_owned(), guard.sha256.clone())
            })
            .collect::<BTreeMap<_, _>>();
        PreparedContext::apply(
            Some(&PreparedContext {
                digest: self.report.digest.clone(),
                artifact_guards,
            }),
            &mut self.request.env,
        )?;
        self.context_bound = true;
        Ok(())
    }

    #[must_use]
    pub const fn is_context_bound(&self) -> bool {
        self.context_bound
    }

    pub(crate) fn request(&self) -> &SkillRunRequest {
        &self.request
    }

    pub(crate) fn selected_runner(&self) -> &str {
        &self.selected_runner
    }

    pub(crate) fn manifest(&self) -> &SkillRunnerManifest {
        &self.manifest
    }

    pub(crate) fn runner(&self) -> &SkillRunnerDefinition {
        &self.runner
    }

    pub(crate) fn verify_artifacts(
        &self,
    ) -> Result<std::sync::Arc<crate::LoadedSkillPackage>, SkillRunError> {
        for guard in &self.guards {
            let content = fs::read(&guard.path).map_err(|source| {
                RuntimeError::io(
                    format!("verifying prepared artifact {}", guard.path.display()),
                    source,
                )
            })?;
            let actual = sha256_prefixed(&content);
            if actual != guard.sha256 {
                return Err(SkillRunError::Invalid(format!(
                    "prepared artifact drift at {}: expected {}, actual {}",
                    guard.path.display(),
                    guard.sha256,
                    actual
                )));
            }
        }
        let loaded = std::sync::Arc::new(crate::load_validated_skill_package(
            &self.report.request.skill_path,
        )?);
        crate::skill_package::verify_loaded_execution_binding(
            loaded.clone(),
            &self.selected_runner,
            &self.request.env,
            Some(&self.package_digest),
            self.execution_closure_digest(),
        )
        .map(|_| loaded)
        .map_err(|message| {
            SkillRunError::Invalid(format!(
                "skill execution binding drift at execution boundary: {message}"
            ))
        })
    }
}

#[derive(Serialize)]
struct PreparedContextPreimage<'a> {
    schema: &'static str,
    skill_path: &'a Path,
    cwd: &'a Path,
    runner: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    answers: Option<&'a runx_contracts::ResumeDocument>,
    inputs: &'a BTreeMap<String, JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    act: Option<&'a runx_contracts::CallAct>,
    run_authority: Option<&'a crate::RunAuthority>,
    managed_agent: &'a ManagedAgentPolicy,
    entry: &'a PreparedEntryProvenance,
    chain: Option<&'a SkillOperatorContextChain>,
    blocked_reason: Option<&'a str>,
}

pub fn prepare_skill_run(
    request: SkillRunRequest,
    selected_runner_name: Option<&str>,
    entry: PreparedEntryProvenance,
) -> Result<PreparedSkillRun, SkillRunError> {
    prepare_skill_run_with_effects(
        request,
        selected_runner_name,
        entry,
        &crate::RuntimeEffectRegistry::default(),
    )
}

pub(crate) fn prepare_skill_run_with_effects(
    mut request: SkillRunRequest,
    selected_runner_name: Option<&str>,
    mut entry: PreparedEntryProvenance,
    effects: &crate::RuntimeEffectRegistry,
) -> Result<PreparedSkillRun, SkillRunError> {
    strip_untrusted_prepared_env(&mut request.env);
    let loaded = std::sync::Arc::new(crate::load_validated_skill_package(&request.skill_path)?);
    let manifest = loaded.manifest().cloned().ok_or_else(|| {
        SkillRunError::Invalid(format!(
            "skill package {} does not declare run.yaml runners",
            loaded.directory.display()
        ))
    })?;
    let package_digest = loaded.package.package_digest.clone();
    let skill_dir = loaded.directory.clone();
    let runner = selected_runner(&manifest, selected_runner_name)?.clone();
    let execution_closure_digest = crate::skill_package::verify_loaded_execution_binding(
        loaded.clone(),
        &runner.name,
        &request.env,
        entry.package_digest.as_deref(),
        entry.execution_closure_digest.as_deref(),
    )
    .map_err(|error| SkillRunError::Invalid(error.to_string()))?;
    if entry.execution_closure_digest.is_none() {
        entry.execution_closure_digest = execution_closure_digest.clone();
    }
    let workspace = crate::services::WorkspaceEnv::new(request.env.clone(), request.cwd.clone())
        .map_err(RuntimeError::from)?;
    super::skill_front::restore_continuation_inputs(&mut request, &workspace)?;
    crate::input_contract::apply_defaults(&runner.inputs, &mut request.inputs);
    let input_failure = match crate::input_contract::materialize_present_runner_inputs(
        &runner.inputs,
        &request.inputs,
    ) {
        Ok(inputs) => {
            request.inputs = inputs;
            None
        }
        Err(error) => Some(error.into_runtime_error()),
    };
    let request_summary = request_summary(&request, &skill_dir, &runner.name, entry);
    let context = input_failure.as_ref().map_or_else(
        || resolve_prepared_context(&request, &loaded, &runner, effects),
        |error| {
            blocked_prepared_context(
                vec![PreparedTraceEntry {
                    node_path: "entry".to_owned(),
                    stage: "resolve_runner".to_owned(),
                    outcome: "resolved".to_owned(),
                    detail: format!("selected runner {}", runner.name),
                }],
                "entry",
                "validate_inputs",
                error.to_string(),
            )
        },
    );
    let governance = prepared_governance(&request, &context);
    let digest = prepared_context_digest(&request, &request_summary, &context)?;
    let refusal_receipt_id = if context.status == PreparedSkillRunStatus::Blocked {
        let mut failure = input_failure.as_ref().map_or_else(
            || {
                JsonObject::from([
                    (
                        "code".to_owned(),
                        JsonValue::String("prepared_context_blocked".to_owned()),
                    ),
                    (
                        "message".to_owned(),
                        JsonValue::String(
                            context
                                .blocked_reason
                                .clone()
                                .unwrap_or_else(|| "skill preparation was blocked".to_owned()),
                        ),
                    ),
                ])
            },
            RuntimeError::public_failure_projection,
        );
        failure.insert(
            "prepared_context_digest".to_owned(),
            JsonValue::String(digest.clone()),
        );
        if let Some(stage) = context.trace.last().map(|entry| entry.stage.clone()) {
            failure.insert("stage".to_owned(), JsonValue::String(stage));
        }
        // Inputs that failed their contract are refused as that; any other
        // blocked preparation is a preflight refusal.
        let reason = if input_failure.is_some() {
            runx_contracts::SealReason::InputContractInvalid
        } else {
            runx_contracts::SealReason::PreflightRefused
        };
        Some(super::skill_front::seal_skill_preflight_refusal(
            &request, &manifest, &runner, failure, reason,
        )?)
    } else {
        None
    };
    let guards = context
        .chain
        .as_ref()
        .map(artifact_guards)
        .unwrap_or_default();
    Ok(PreparedSkillRun {
        request,
        package_digest,
        selected_runner: runner.name.clone(),
        manifest,
        runner,
        report: PreparedSkillRunReport {
            schema: PREPARED_SKILL_REPORT_SCHEMA.to_owned(),
            status: context.status,
            digest,
            request: request_summary,
            governance,
            chain: context.chain,
            trace: context.trace,
            blocked_reason: context.blocked_reason,
            refusal_receipt_id,
        },
        guards,
        context_bound: false,
    })
}

struct PreparedContextResolution {
    status: PreparedSkillRunStatus,
    chain: Option<SkillOperatorContextChain>,
    trace: Vec<PreparedTraceEntry>,
    blocked_reason: Option<String>,
}

fn prepared_governance(
    request: &SkillRunRequest,
    context: &PreparedContextResolution,
) -> PreparedGovernanceSummary {
    let mut governance = context
        .chain
        .as_ref()
        .map(governance_summary)
        .unwrap_or_default();
    governance.managed_agent_enabled = request.managed_agent.is_inline();
    governance.managed_agent_max_rounds = request.managed_agent.max_rounds();
    governance
}

fn prepared_context_digest(
    request: &SkillRunRequest,
    summary: &PreparedRequestSummary,
    context: &PreparedContextResolution,
) -> Result<String, SkillRunError> {
    // Receipt storage and generated run identity are execution bookkeeping, so
    // the same semantic run has the same prepared-context identity.
    let preimage = PreparedContextPreimage {
        schema: PREPARED_SKILL_REPORT_SCHEMA,
        skill_path: &summary.skill_path,
        cwd: &summary.cwd,
        runner: &summary.runner,
        answers: request.answers.as_ref(),
        inputs: &request.inputs,
        act: request.act.as_ref(),
        run_authority: request.run_authority.as_ref(),
        managed_agent: &request.managed_agent,
        entry: &summary.entry,
        chain: context.chain.as_ref(),
        blocked_reason: context.blocked_reason.as_deref(),
    };
    let bytes = serde_json::to_vec(&preimage)
        .map_err(|source| RuntimeError::json("serializing prepared skill digest", source))?;
    Ok(sha256_prefixed(&bytes))
}

fn resolve_prepared_context(
    request: &SkillRunRequest,
    loaded: &crate::LoadedSkillPackage,
    runner: &SkillRunnerDefinition,
    effects: &crate::RuntimeEffectRegistry,
) -> PreparedContextResolution {
    let trace = vec![PreparedTraceEntry {
        node_path: "entry".to_owned(),
        stage: "resolve_runner".to_owned(),
        outcome: "resolved".to_owned(),
        detail: format!("selected runner {}", runner.name),
    }];
    let missing = crate::input_contract::missing_required(&runner.inputs, &request.inputs);
    if !missing.is_empty() {
        let reason = format!("missing required inputs: {}", missing.join(", "));
        return blocked_prepared_context(trace, "entry", "validate_inputs", reason);
    }

    match load_skill_operator_context_chain_from_package(
        loaded,
        Some(&runner.name),
        SkillOperatorContextOptions::new(request.env.clone(), request.cwd.clone())
            .with_effects(effects.clone()),
    ) {
        Ok(chain) => ready_prepared_context(trace, chain),
        Err(blocked) => blocked_prepared_context(
            trace,
            &blocked.node_path,
            "expand_chain",
            blocked.error.to_string(),
        ),
    }
}

fn ready_prepared_context(
    mut trace: Vec<PreparedTraceEntry>,
    chain: SkillOperatorContextChain,
) -> PreparedContextResolution {
    trace.push(PreparedTraceEntry {
        node_path: "entry".to_owned(),
        stage: "expand_chain".to_owned(),
        outcome: "resolved".to_owned(),
        detail: format!("expanded {} nodes", chain.node_count),
    });
    PreparedContextResolution {
        status: PreparedSkillRunStatus::Ready,
        chain: Some(chain),
        trace,
        blocked_reason: None,
    }
}

fn blocked_prepared_context(
    mut trace: Vec<PreparedTraceEntry>,
    node_path: &str,
    stage: &str,
    reason: String,
) -> PreparedContextResolution {
    trace.push(PreparedTraceEntry {
        node_path: node_path.to_owned(),
        stage: stage.to_owned(),
        outcome: "blocked".to_owned(),
        detail: reason.clone(),
    });
    PreparedContextResolution {
        status: PreparedSkillRunStatus::Blocked,
        chain: None,
        trace,
        blocked_reason: Some(reason),
    }
}

/// The operator context the run was prepared with, which every act the run
/// seals carries as its `context_ref`.
pub(crate) fn prepared_context_ref(env: &BTreeMap<String, String>) -> Option<Reference> {
    let digest = env.get(PREPARED_CONTEXT_DIGEST_ENV)?;
    Some(Reference {
        reference_type: ReferenceType::Artifact,
        uri: format!("runx:artifact:operator_context:{digest}").into(),
        provider: Some("runx".to_owned().into()),
        locator: Some(digest.clone().into()),
        label: None,
        observed_at: None,
        proof_kind: None,
    })
}

pub(crate) fn verify_prepared_artifact_at_use(
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Result<(), RuntimeError> {
    let Some(PreparedContext {
        artifact_guards: guards,
        ..
    }) = PreparedContext::from_env(env)?
    else {
        return Ok(());
    };
    let canonical = fs::canonicalize(path).map_err(|source| {
        RuntimeError::io(
            format!("canonicalizing prepared artifact {}", path.display()),
            source,
        )
    })?;
    let key = canonical.to_string_lossy();
    let Some(expected) = guards.get(key.as_ref()) else {
        return Ok(());
    };
    let content = fs::read(&canonical).map_err(|source| {
        RuntimeError::io(
            format!("verifying prepared artifact {} at use", canonical.display()),
            source,
        )
    })?;
    let actual = sha256_prefixed(&content);
    if &actual != expected {
        return Err(RuntimeError::SkillFailed {
            skill_name: "prepared-run".to_owned(),
            message: format!(
                "prepared artifact drift at use boundary {}: expected {}, actual {}",
                canonical.display(),
                expected,
                actual
            ),
        });
    }
    Ok(())
}

fn request_summary(
    request: &SkillRunRequest,
    skill_dir: &Path,
    runner: &str,
    mut entry: PreparedEntryProvenance,
) -> PreparedRequestSummary {
    if entry.kind.is_empty() {
        entry.kind = "local_path".to_owned();
    }
    if entry.source.is_empty() {
        entry.source = "local-path".to_owned();
    }
    if entry.source_label.is_empty() {
        entry.source_label = skill_dir.to_string_lossy().into_owned();
    }
    PreparedRequestSummary {
        skill_path: skill_dir.to_path_buf(),
        cwd: request.cwd.clone(),
        runner: runner.to_owned(),
        receipt_dir: request.receipt_dir.clone(),
        run_id: request.run_id.clone(),
        inputs: request
            .inputs
            .iter()
            .map(|(name, value)| {
                let bytes = serde_json::to_vec(value).unwrap_or_default();
                PreparedInputSummary {
                    name: name.clone(),
                    value_type: json_type(value).to_owned(),
                    canonical_bytes: bytes.len(),
                    sha256: sha256_prefixed(&bytes),
                }
            })
            .collect(),
        managed_agent: request.managed_agent.clone(),
        entry,
    }
}

fn json_type(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

fn governance_summary(chain: &SkillOperatorContextChain) -> PreparedGovernanceSummary {
    let mut summary = PreparedGovernanceSummary::default();
    summarize_node(&chain.entry, &mut summary);
    summary.tool_refs.sort();
    summary.tool_refs.dedup();
    summary.authority_scopes.sort();
    summary.authority_scopes.dedup();
    summary.execution_boundaries.sort();
    summary.execution_boundaries.dedup();
    summary.gates.sort();
    summary.gates.dedup();
    summary.idempotency_keys.sort();
    summary.idempotency_keys.dedup();
    summary
}

fn summarize_node(node: &SkillOperatorContextNode, summary: &mut PreparedGovernanceSummary) {
    summary
        .authority_scopes
        .extend(node.runner.scopes.iter().cloned());
    summary
        .execution_boundaries
        .extend(node.tools.iter().map(|tool| tool.execution_boundary.kind));
    for step in &node.steps {
        let definition = &step.definition;
        if definition.when.is_some() {
            summary.conditional_steps += 1;
        } else {
            summary.declared_steps += 1;
        }
        summary.tool_refs.extend(step.tool_refs.iter().cloned());
        if let Some(observation) = step.execution_boundary {
            summary.execution_boundaries.push(observation.kind);
        }
        summary
            .authority_scopes
            .extend(definition.scopes.iter().cloned());
        match &definition.kind {
            StepKind::Ask(ask) => summary.gates.push(ask.gate.clone()),
            StepKind::Agent(_) => summary.managed_agent_acts += 1,
            _ => {}
        }
        // An operation call's key, as its step writes it.
        if let Some(idempotency_key) = definition
            .operation()
            .and(definition.inputs.get("idempotency_key"))
            .and_then(JsonValue::as_str)
        {
            summary.idempotency_keys.push(idempotency_key.to_owned());
        }
        for child in &step.children {
            summarize_node(child, summary);
        }
    }
}

fn artifact_guards(chain: &SkillOperatorContextChain) -> Vec<PreparedArtifactGuard> {
    let mut guards = BTreeMap::<PathBuf, String>::new();
    collect_node_guards(&chain.entry, &mut guards);
    guards
        .into_iter()
        .map(|(path, sha256)| PreparedArtifactGuard { path, sha256 })
        .collect()
}

fn collect_node_guards(node: &SkillOperatorContextNode, guards: &mut BTreeMap<PathBuf, String>) {
    if let Some(path) = &node.skill_markdown.path {
        guards.insert(path.clone(), node.skill_markdown.sha256.clone());
    }
    let manifest_path = node.package.directory.join(runx_parser::PROFILE_FILE);
    if let Ok(content) = fs::read(&manifest_path) {
        guards.insert(manifest_path, sha256_prefixed(&content));
    }
    for tool in &node.tools {
        if let (Some(path), Some(sha256)) = (&tool.path, &tool.sha256) {
            guards.insert(path.clone(), sha256.clone());
        }
    }
    for step in &node.steps {
        for context in &step.context_skills {
            if let (Some(path), Some(digest)) = (
                context.artifact.get("path").and_then(JsonValue::as_str),
                context
                    .artifact
                    .get("manual_sha256")
                    .and_then(JsonValue::as_str),
            ) {
                guards.insert(PathBuf::from(path), digest.to_owned());
            }
            if let (Some(path), Some(digest)) = (
                context
                    .artifact
                    .get("profile_path")
                    .and_then(JsonValue::as_str),
                context
                    .artifact
                    .get("profile_sha256")
                    .and_then(JsonValue::as_str),
            ) {
                guards.insert(PathBuf::from(path), digest.to_owned());
            }
        }
        for child in &step.children {
            collect_node_guards(child, guards);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use tempfile::tempdir;

    use super::*;
    use crate::RunStatus;

    fn write_manual(directory: &Path, name: &str, body: &str) -> Result<(), Box<dyn Error>> {
        fs::create_dir_all(directory)?;
        fs::write(
            directory.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Test skill for prepared execution.\n---\n\n{body}\n"
            ),
        )?;
        Ok(())
    }

    fn write_skill(directory: &Path, inputs: &str, body: &str) -> Result<(), Box<dyn Error>> {
        write_manual(directory, "prepared", body)?;
        fs::write(
            directory.join("run.yaml"),
            format!(
                "skill: prepared\nrunners:\n  main:\n    default: true\n{inputs}    steps:\n      - id: review\n        agent: {{ role: reviewer, task: review }}\n        outputs:\n          result: object\n"
            ),
        )?;
        Ok(())
    }

    fn request(path: &Path) -> SkillRunRequest {
        SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: path.to_path_buf(),
            receipt_dir: None,
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            // Anchor the runx home inside the test dir: without this the
            // agent path discovers the developer's real ~/.runx agent
            // credentials and resolves inline against the live provider.
            env: BTreeMap::from([("RUNX_HOME".to_owned(), path.to_string_lossy().into_owned())]),
            cwd: path.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        }
    }

    #[test]
    fn prepared_skill_digest_is_deterministic_and_binds_inputs() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        write_skill(temp.path(), "", "# Prepared")?;
        let first = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;
        let second = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;
        assert!(first.is_ready());
        assert_eq!(first.digest(), second.digest());
        let mut changed = request(temp.path());
        changed
            .inputs
            .insert("prompt".to_owned(), JsonValue::String("changed".to_owned()));
        let changed = prepare_skill_run(changed, None, PreparedEntryProvenance::default())?;
        assert_ne!(first.digest(), changed.digest());
        Ok(())
    }

    #[test]
    fn prepared_skill_rejects_execution_package_drift() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        write_skill(temp.path(), "", "# Prepared")?;
        let error = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance {
                package_digest: Some("sha256:not-the-package".to_owned()),
                ..PreparedEntryProvenance::default()
            },
        )
        .err()
        .ok_or("mismatched execution package digest did not fail closed")?;

        assert!(error.to_string().contains("skill package digest mismatch"));
        Ok(())
    }

    #[test]
    fn prepared_skill_rejects_execution_closure_drift() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        write_skill(temp.path(), "", "# Prepared")?;
        let error = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance {
                execution_closure_digest: Some("sha256:not-the-closure".to_owned()),
                ..PreparedEntryProvenance::default()
            },
        )
        .err()
        .ok_or("mismatched execution closure digest did not fail closed")?;

        assert!(
            error
                .to_string()
                .contains("skill execution closure digest mismatch")
        );
        Ok(())
    }

    #[test]
    fn prepared_skill_rechecks_sibling_closure_at_execution_boundary() -> Result<(), Box<dyn Error>>
    {
        let temp = tempdir()?;
        let entry = temp.path().join("twitter");
        let child = temp.path().join("data-store");
        write_manual(&entry, "twitter", "# Twitter")?;
        write_manual(&child, "data-store", "# Data store")?;
        fs::write(
            child.join("run.yaml"),
            "skill: data-store\nrunners:\n  read:\n    default: true\n    steps:\n      - id: read\n        tool: { module: run.mjs }\n        outputs:\n          version:\n            type: integer\n            required: true\n",
        )?;
        fs::write(
            child.join("run.mjs"),
            "export default () => ({ version: 1 });\n",
        )?;
        fs::write(
            entry.join("run.yaml"),
            "skill: twitter\nrunners:\n  inspect:\n    default: true\n    result_from: [store]\n    steps:\n      - id: store\n        skill: ../data-store\n        runner: read\n",
        )?;
        let closure = crate::skill_package::inspect_loaded_execution_closure_binding(
            crate::load_validated_skill_package(&entry)?.into(),
            "inspect",
            &BTreeMap::new(),
        )?;
        assert!(closure.fully_bound);
        let prepared = prepare_skill_run(
            request(&entry),
            Some("inspect"),
            PreparedEntryProvenance {
                execution_closure_digest: Some(closure.digest),
                ..PreparedEntryProvenance::default()
            },
        )?;

        fs::write(
            child.join("run.mjs"),
            "export default () => ({ version: 2 });\n",
        )?;
        let error = prepared
            .verify_artifacts()
            .err()
            .ok_or("sibling package drift did not invalidate the bound closure")?;
        assert!(
            error
                .to_string()
                .contains("skill execution binding drift at execution boundary")
        );
        assert!(
            error
                .to_string()
                .contains("skill execution closure digest mismatch")
        );
        Ok(())
    }

    #[test]
    fn bound_continuation_rechecks_execution_closure_without_reapproval()
    -> Result<(), Box<dyn Error>> {
        use crate::execution::orchestrator::LocalOrchestrator;

        let temp = tempdir()?;
        write_skill(temp.path(), "", "# Prepared")?;
        let error = LocalOrchestrator::default()
            .run_skill_with_binding(
                &request(temp.path()),
                Some("main"),
                None,
                Some("sha256:not-the-closure"),
            )
            .err()
            .ok_or("resumed continuation discarded its execution closure binding")?;
        assert!(
            error
                .to_string()
                .contains("skill execution closure digest mismatch")
        );
        Ok(())
    }

    #[test]
    fn bound_continuation_persists_closure_for_resume() -> Result<(), Box<dyn Error>> {
        use crate::execution::orchestrator::LocalOrchestrator;

        let temp = tempdir()?;
        write_skill(temp.path(), "", "# Prepared")?;
        let loaded = crate::load_validated_skill_package(temp.path())?;
        let package_digest = loaded.package.package_digest.clone();
        let closure = crate::skill_package::inspect_loaded_execution_closure_binding(
            loaded.into(),
            "main",
            &BTreeMap::new(),
        )?;
        assert!(closure.fully_bound);
        let closure = closure.digest;
        let receipt_dir = temp.path().join("receipts");
        let mut bound_request = request(temp.path());
        bound_request.receipt_dir = Some(receipt_dir.clone());

        let result = LocalOrchestrator::default().run_skill_with_binding(
            &bound_request,
            Some("main"),
            Some(&package_digest),
            Some(&closure),
        )?;
        assert_eq!(result.status, RunStatus::NeedsAgent);
        let run_id = result
            .output
            .as_object()
            .and_then(|output| output.get("run_id"))
            .and_then(runx_contracts::JsonValue::as_str)
            .ok_or("the paused run has no id")?;
        let paused =
            crate::execution::run_record::paused_run(&BTreeMap::new(), temp.path(), run_id)?
                .ok_or("the bound run did not pause in its record")?;
        assert_eq!(paused.package_digest, package_digest);
        assert_eq!(paused.execution_closure_digest, closure);
        Ok(())
    }

    #[test]
    fn prepared_skill_binds_and_reports_managed_agent_consent() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        write_skill(temp.path(), "", "# Prepared")?;
        let host_driven = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;
        let mut inline_request = request(temp.path());
        inline_request.managed_agent = ManagedAgentPolicy::inline(3)?;
        let inline = prepare_skill_run(inline_request, None, PreparedEntryProvenance::default())?;

        assert_ne!(host_driven.digest(), inline.digest());
        assert_eq!(inline.report().governance.managed_agent_acts, 1);
        assert!(inline.report().governance.managed_agent_enabled);
        assert_eq!(inline.report().governance.managed_agent_max_rounds, Some(3));
        assert_eq!(
            inline.report().request.managed_agent,
            ManagedAgentPolicy::Inline { max_rounds: 3 }
        );
        Ok(())
    }

    #[test]
    fn prepared_skill_digest_ignores_receipt_storage_and_generated_run_id()
    -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        write_skill(temp.path(), "", "# Prepared")?;

        let baseline = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;
        let mut relocated = request(temp.path());
        relocated.receipt_dir = Some(temp.path().join("other-receipts"));
        relocated.run_id = Some("rx_other".to_owned());
        let relocated = prepare_skill_run(relocated, None, PreparedEntryProvenance::default())?;

        assert_eq!(baseline.digest(), relocated.digest());
        assert_ne!(
            baseline.report().request.receipt_dir,
            relocated.report().request.receipt_dir
        );
        assert_ne!(
            baseline.report().request.run_id,
            relocated.report().request.run_id
        );
        Ok(())
    }

    #[test]
    fn prepared_skill_applies_declared_input_defaults() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        write_skill(
            temp.path(),
            "    inputs:\n      data_source_ref:\n        type: string\n        required: false\n        default: local://runx/default\n",
            "# Prepared",
        )?;

        let prepared = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;

        assert_eq!(
            prepared.request().inputs.get("data_source_ref"),
            Some(&JsonValue::String("local://runx/default".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn prepared_skill_missing_input_returns_blocked_trace() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        write_skill(
            temp.path(),
            "    inputs:\n      prompt:\n        type: string\n        required: true\n",
            "# Prepared",
        )?;
        let receipt_dir = temp.path().join("receipts");
        let mut request = request(temp.path());
        request.receipt_dir = Some(receipt_dir.clone());
        let prepared = prepare_skill_run(request, None, PreparedEntryProvenance::default())?;
        assert_eq!(prepared.report().status, PreparedSkillRunStatus::Blocked);
        assert!(
            prepared
                .report()
                .blocked_reason
                .as_deref()
                .unwrap_or_default()
                .contains("prompt")
        );
        assert!(
            prepared
                .report()
                .trace
                .iter()
                .any(|entry| entry.outcome == "blocked")
        );
        let refusal_receipt_id = prepared
            .report()
            .refusal_receipt_id
            .as_deref()
            .ok_or("blocked preparation did not expose its refusal receipt")?;
        let services =
            crate::services::ReceiptServices::resolve(&crate::services::WorkspaceEnv::new(
                prepared.request().env.clone(),
                prepared.request().cwd.clone(),
            )?)?;
        let receipts = crate::receipts::store::LocalReceiptStore::new(&receipt_dir)
            .list(services.signature_config().signature_policy())?;
        assert!(
            receipts
                .iter()
                .any(|receipt| receipt.id.as_str() == refusal_receipt_id),
            "blocked preparation did not persist its refusal receipt"
        );
        Ok(())
    }

    #[test]
    fn prepared_skill_invalid_input_receipt_never_echoes_the_rejected_value()
    -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        write_skill(
            temp.path(),
            "    inputs:\n      secret_ref:\n        type: string\n        required: true\n        schema: { pattern: '^secret://' }\n",
            "# Prepared",
        )?;
        let sentinel = "PLAINTEXT-SECRET-MUST-NOT-APPEAR";
        let receipt_dir = temp.path().join("receipts");
        let mut request = request(temp.path());
        request.receipt_dir = Some(receipt_dir.clone());
        request.inputs.insert(
            "secret_ref".to_owned(),
            JsonValue::String(sentinel.to_owned()),
        );

        let prepared = prepare_skill_run(request, None, PreparedEntryProvenance::default())?;
        assert_eq!(prepared.report().status, PreparedSkillRunStatus::Blocked);
        assert!(prepared.report().refusal_receipt_id.is_some());
        let services =
            crate::services::ReceiptServices::resolve(&crate::services::WorkspaceEnv::new(
                prepared.request().env.clone(),
                prepared.request().cwd.clone(),
            )?)?;
        let receipts = crate::receipts::store::LocalReceiptStore::new(&receipt_dir)
            .list(services.signature_config().signature_policy())?;
        let public = serde_json::to_string(&(prepared.report(), receipts))?;
        assert!(!public.contains(sentinel));
        assert!(public.contains("/secret_ref"));
        assert!(public.contains("input_contract_invalid"));
        Ok(())
    }

    #[test]
    fn prepared_skill_strict_tool_resolution_blocks_with_trace() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        write_manual(temp.path(), "prepared", "# Prepared")?;
        fs::write(
            temp.path().join("run.yaml"),
            "skill: prepared\nrunners:\n  main:\n    default: true\n    result_from: [call]\n    steps:\n      - id: call\n        tool: missing.tool\n",
        )?;
        let prepared = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;
        assert_eq!(prepared.report().status, PreparedSkillRunStatus::Blocked);
        assert!(
            prepared
                .report()
                .blocked_reason
                .as_deref()
                .unwrap_or_default()
                .contains("missing.tool")
        );
        Ok(())
    }

    #[test]
    fn prepared_skill_blocks_missing_result_contract_before_mutating_tool()
    -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let sentinel = temp.path().join("tool-ran");
        write_manual(temp.path(), "prepared", "# Prepared")?;
        fs::create_dir_all(temp.path().join("tools/example/record"))?;
        fs::write(
            temp.path().join("tools/example/record/manifest.json"),
            r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.record",
  "source": {
    "command": "sh",
    "args": ["-c", "touch \"$RUNX_CWD/tool-ran\""],
    "input_mode": "none"
  }
}
"#,
        )?;
        fs::write(
            temp.path().join("run.yaml"),
            "skill: prepared\nrunners:\n  main:\n    default: true\n    result_from: [record]\n    steps:\n      - id: record\n        tool: example.record\n",
        )?;

        let prepared = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;

        assert_eq!(prepared.report().status, PreparedSkillRunStatus::Blocked);
        assert!(
            prepared
                .report()
                .blocked_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("declares no semantic output contract"))
        );
        assert!(!sentinel.exists(), "preparation executed the mutating tool");
        Ok(())
    }

    #[test]
    fn prepared_governance_consumes_typed_graph_step_contract() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        crate::kernel::connectors::plugin_with_source_connectors(temp.path(), &["github"]);
        write_manual(temp.path(), "prepared", "# Prepared")?;
        fs::write(
            temp.path().join("run.yaml"),
            r#"skill: prepared
connectors:
  github:
    operations: [branch.push]
runners:
  main:
    default: true
    result_from: [publish]
    steps:
      - id: approve-publish
        ask: { gate: release.publish.approval, reason: Approve the exact release. }
        when:
          value: $input.ready
          equals: true
      - id: push
        operation: github/branch.push
        idempotency_key: release-publish-1
        input: {repository: acme/app, branch: main, commit: "0123456789abcdef0123456789abcdef01234567"}
      - id: publish
        tool: { command: node }
        scopes:
          - release:publish
        outputs:
          result: object
"#,
        )?;

        let prepared = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;
        let governance = &prepared.report().governance;

        assert_eq!(governance.declared_steps, 2);
        assert_eq!(governance.conditional_steps, 1);
        assert_eq!(governance.authority_scopes, ["release:publish"]);
        assert_eq!(governance.gates, ["release.publish.approval"]);
        assert_eq!(governance.idempotency_keys, ["release-publish-1"]);
        assert_eq!(governance.managed_agent_acts, 0);
        Ok(())
    }

    #[test]
    fn prepared_governance_includes_terminal_runner_scopes() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        write_manual(temp.path(), "prepared", "# Prepared")?;
        fs::write(
            temp.path().join("run.yaml"),
            r#"skill: prepared
runners:
  mutate:
    default: true
    steps:
      - id: mutate
        tool: { command: example-mutator }
        scopes: [example:write]
        outputs:
          result: object
"#,
        )?;

        let prepared = prepare_skill_run(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;

        assert_eq!(
            prepared.report().governance.authority_scopes,
            ["example:write"]
        );
        Ok(())
    }

    #[test]
    fn prepared_skill_execution_matches_unprepared_and_rejects_drift() -> Result<(), Box<dyn Error>>
    {
        use crate::execution::orchestrator::LocalOrchestrator;

        let temp = tempdir()?;
        write_skill(temp.path(), "", "# Prepared")?;
        let orchestrator = LocalOrchestrator::default();
        let baseline_request = request(temp.path());
        let baseline = orchestrator.run_skill(&baseline_request)?;
        let mut prepared = orchestrator.prepare_skill(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;
        prepared.bind_context()?;
        let prepared_result = orchestrator.run_prepared_skill(&prepared)?;
        assert_eq!(baseline.status, prepared_result.status);

        fs::write(temp.path().join("SKILL.md"), "# Changed after preparation")?;
        let Err(error) = orchestrator.run_prepared_skill(&prepared) else {
            return Err("prepared artifact drift must fail closed".into());
        };
        let message = error.to_string();
        assert!(message.contains("prepared artifact drift"));
        assert!(message.contains("SKILL.md"));
        assert!(message.contains("expected sha256:"));
        assert!(message.contains("actual sha256:"));
        Ok(())
    }

    #[test]
    fn prepared_context_binding_does_not_fabricate_human_approval() -> Result<(), Box<dyn Error>> {
        use crate::execution::orchestrator::LocalOrchestrator;

        let temp = tempdir()?;
        write_skill(temp.path(), "", "# Prepared")?;
        let orchestrator = LocalOrchestrator::default();
        let mut prepared = orchestrator.prepare_skill(
            request(temp.path()),
            None,
            PreparedEntryProvenance::default(),
        )?;

        let Err(error) = orchestrator.run_prepared_skill(&prepared) else {
            return Err("unbound prepared context must fail closed".into());
        };
        assert!(error.to_string().contains("context to be bound"));

        prepared.bind_context()?;
        assert!(prepared.is_context_bound());
        let context = prepared_context_ref(&prepared.request().env)
            .ok_or("a bound prepared run has an operator context")?;
        assert_eq!(context.reference_type, ReferenceType::Artifact);
        assert!(
            context
                .uri
                .as_str()
                .starts_with("runx:artifact:operator_context:")
        );
        assert_eq!(context.label, None);
        assert_eq!(
            orchestrator.run_prepared_skill(&prepared)?.status,
            RunStatus::NeedsAgent
        );
        Ok(())
    }

    #[test]
    fn prepared_skill_execution_rejects_child_drift_at_load_boundary() -> Result<(), Box<dyn Error>>
    {
        use crate::execution::graph::{StepSkillLoadOptions, load_step_skill};

        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let child = entry.join("child");
        write_manual(&entry, "entry", "# Entry")?;
        write_manual(&child, "child", "# Child")?;
        fs::write(
            child.join("run.yaml"),
            "skill: child\nrunners:\n  child:\n    default: true\n    steps:\n      - id: child\n        agent: { role: reviewer, task: before }\n        outputs:\n          result: object\n",
        )?;
        fs::write(
            entry.join("run.yaml"),
            "skill: entry\nrunners:\n  main:\n    default: true\n    result_from: [child]\n    steps:\n      - id: child\n        skill: ./child\n        artifacts:\n          wrap_as: child_result\n",
        )?;
        let mut prepared =
            prepare_skill_run(request(&entry), None, PreparedEntryProvenance::default())?;
        prepared.bind_context()?;
        fs::write(
            child.join("run.yaml"),
            "skill: child\nrunners:\n  child:\n    default: true\n    steps:\n      - id: child\n        agent: { role: reviewer, task: after }\n        outputs:\n          result: object\n",
        )?;
        let step = &prepared.runner.graph.steps[0];
        let error = match load_step_skill(
            &entry,
            step,
            StepSkillLoadOptions {
                env: &prepared.request.env,
            },
        ) {
            Ok(_) => return Err("child drift must fail at load boundary".into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("drift at use boundary"));
        assert!(error.to_string().contains("child/run.yaml"));
        Ok(())
    }

    #[test]
    fn a_prepared_runs_operator_context_rides_on_each_governed_calls_act()
    -> Result<(), Box<dyn Error>> {
        use crate::adapter::InvocationOutput;
        use crate::receipts::{
            RuntimeReceiptSignatureConfig, SealedCall, StepSeal, StepSealClosure, seal_step,
        };
        use runx_contracts::ClosureDisposition;

        let signature = RuntimeReceiptSignatureConfig::fixture()?;

        let mut env = BTreeMap::new();
        env.insert(
            PREPARED_CONTEXT_DIGEST_ENV.to_owned(),
            "sha256:abc123".to_owned(),
        );
        let output = InvocationOutput::runtime_success(JsonValue::Object(BTreeMap::new()), 0);
        let projection =
            crate::execution::output_projection::project_step_claim(BTreeMap::new(), None);
        let seal = |call: Option<SealedCall>| {
            seal_step(
                StepSeal {
                    scope: crate::receipts::RunScope {
                        run_id: "run_prepared",
                        graph_name: "prepared",
                    },
                    step_id: "execute",
                    attempt: 1,
                    output: &output,
                    claim: &projection.outputs,
                    created_at: "2026-07-12T00:00:00Z",
                    authority: crate::receipts::SealAuthority::default(),
                    call,
                    approval_refs: Vec::new(),
                    child_receipts: &[],
                    descendant_receipts: &[],
                    closure: Some(StepSealClosure {
                        disposition: ClosureDisposition::Closed,
                        reason: None,
                    }),
                },
                signature.signature_policy(),
            )
        };
        let receipt = seal(Some(SealedCall {
            operation_intent: None,
            act: None,
            description: "Post one message.".into(),
            legitimacy: runx_contracts::TermLegitimacy::for_term("term_prepared")
                .ok_or("a term id names a legitimacy")?,
            target_ref: None,
            effects: Vec::new(),
            context_ref: prepared_context_ref(&env),
        }))?;
        assert_eq!(receipt.acts[0].intent.purpose.as_str(), "Post one message.");
        assert_eq!(
            receipt.acts[0]
                .context_ref
                .as_ref()
                .map(|context| context.uri.as_str()),
            Some("runx:artifact:operator_context:sha256:abc123")
        );
        assert!(
            receipt.acts[0]
                .artifact_refs
                .iter()
                .all(|reference| !reference.uri.as_str().contains("operator_context"))
        );
        assert!(prepared_context_ref(&BTreeMap::new()).is_none());
        // A step that makes no governed call seals no act to carry it.
        assert!(seal(None)?.acts.is_empty());
        Ok(())
    }

    #[test]
    fn prepared_skill_untrusted_env_cannot_forge_receipt_references() {
        let mut env = BTreeMap::from([(
            PREPARED_CONTEXT_DIGEST_ENV.to_owned(),
            "sha256:forged".to_owned(),
        )]);
        strip_untrusted_prepared_env(&mut env);
        assert!(prepared_context_ref(&env).is_none());
        assert!(
            !env.keys()
                .any(|key| key.starts_with("RUNX_INTERNAL_PREPARED_"))
        );
    }

    #[test]
    fn prepared_skill_unprepared_receipt_rejects_forged_internal_env() -> Result<(), Box<dyn Error>>
    {
        use crate::execution::orchestrator::LocalOrchestrator;

        let temp = tempdir()?;
        write_manual(temp.path(), "unprepared", "# Unprepared")?;
        fs::write(
            temp.path().join("run.yaml"),
            "skill: unprepared\nrunners:\n  main:\n    default: true\n    steps:\n      - id: main\n        tool: { command: \"true\", args: [] }\n        outputs:\n          result: object\n",
        )?;
        let mut request = request(temp.path());
        request.env.insert(
            PREPARED_CONTEXT_DIGEST_ENV.to_owned(),
            "sha256:forged".to_owned(),
        );
        let result = LocalOrchestrator::default().run_skill(&request)?;
        let output = serde_json::to_string(&result.output)?;
        assert!(!output.contains("operator_context"));
        assert!(!output.contains("forged"));
        Ok(())
    }
}
