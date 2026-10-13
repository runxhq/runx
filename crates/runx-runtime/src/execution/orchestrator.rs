// Module rationale: the canonical entrypoint keeps request/result
// types and prepared/unprepared execution dispatch in one reviewable boundary.
//! Canonical local orchestration entrypoint.
//!
//! CLI commands and TypeScript wrappers should enter local skill, graph, and
//! harness execution through this module instead of calling narrower execution
//! helpers directly.

use std::collections::BTreeMap;
use std::path::PathBuf;

use runx_contracts::{ClosureDisposition, JsonValue, Receipt};
use thiserror::Error;

use super::harness::PackageHarnessReport;
use super::harness::{HarnessReplayError, HarnessReplayOutput};
use super::prepared_skill::{
    PreparedEntryProvenance, PreparedSkillRun, prepare_skill_run_with_effects,
};
use super::skill_front::{SkillRunError, TurnSession};
use crate::effects::RuntimeEffectRegistry;

pub const DEFAULT_MANAGED_AGENT_MAX_ROUNDS: u32 = 4;
pub const MANAGED_AGENT_MAX_ROUNDS_LIMIT: u32 = 32;

/// Per-run consent for in-process model execution.
///
/// Model credentials configure availability; they never grant consent. The
/// default remains caller-mediated `needs_agent` resolution.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ManagedAgentPolicy {
    #[default]
    HostDriven,
    Inline {
        max_rounds: u32,
    },
}

impl ManagedAgentPolicy {
    pub fn inline(max_rounds: u32) -> Result<Self, String> {
        if !(1..=MANAGED_AGENT_MAX_ROUNDS_LIMIT).contains(&max_rounds) {
            return Err(format!(
                "managed-agent rounds must be between 1 and {MANAGED_AGENT_MAX_ROUNDS_LIMIT}"
            ));
        }
        Ok(Self::Inline { max_rounds })
    }

    #[must_use]
    pub const fn max_rounds(&self) -> Option<u32> {
        match self {
            Self::HostDriven => None,
            Self::Inline { max_rounds } => Some(*max_rounds),
        }
    }

    #[must_use]
    pub const fn is_inline(&self) -> bool {
        matches!(self, Self::Inline { .. })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkillRunRequest {
    /// Explicit caller context, bound to this run and inherited by acting children.
    pub current_context: Vec<runx_contracts::ContextEntry>,
    /// Retrieved cross-run evidence; context never grants authority.
    pub historical_context: Vec<runx_contracts::ContextEntry>,
    /// Actor identity and attenuated terms from the trusted host. Never taken
    /// from skill inputs, model output or retrieved context.
    pub run_authority: Option<crate::RunAuthority>,
    /// The act the invoker states for the run: why it is made. Its steps read
    /// it as `$act`, only inside an `act:`.
    pub act: Option<runx_contracts::CallAct>,
    pub skill_path: PathBuf,
    pub receipt_dir: Option<PathBuf>,
    pub run_id: Option<String>,
    pub answers_path: Option<PathBuf>,
    pub inputs: BTreeMap<String, JsonValue>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    /// Explicit consent for an in-process managed-agent loop. The default is
    /// host-driven resolution even when model credentials are configured.
    pub managed_agent: ManagedAgentPolicy,
}

/// Continue a paused run with the answers its host brings. The run's skill,
/// runner and digests come from its record.
#[derive(Clone, Debug, PartialEq)]
pub struct ResumeRequest {
    pub run_id: String,
    pub answers_path: PathBuf,
    pub receipt_dir: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    /// Explicit consent for an in-process managed agent to work the turns
    /// the run opens from here.
    pub managed_agent: ManagedAgentPolicy,
    /// Digests a caller pins; each must be the one the run started under.
    pub package_digest: Option<String>,
    pub execution_closure_digest: Option<String>,
}

/// The agent turn a paused run waits on.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnRequest {
    pub run_id: String,
    pub receipt_dir: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessRunRequest {
    pub fixture_path: PathBuf,
    pub receipt_dir: Option<PathBuf>,
}

/// Request to run every harness case owned by a skill package. `skill_path` is
/// a package directory or its `SKILL.md`; inline cases and conventional fixture
/// files share the supplied environment and receipt directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageHarnessRequest {
    pub skill_path: PathBuf,
    pub receipt_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunRequest {
    Skill(Box<SkillRunRequest>),
    Harness(HarnessRunRequest),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunResult {
    pub status: RunStatus,
    pub disposition: Option<ClosureDisposition>,
    pub output: JsonValue,
    pub receipt_refs: Vec<String>,
    pub child_receipt_refs: Vec<String>,
    pub pending_requests: Vec<JsonValue>,
    pub diagnostics: Vec<String>,
}

impl RunResult {
    /// Transport projections must not infer success from the presence or shape
    /// of JSON. The receipt disposition is assigned where execution seals.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.disposition == Some(ClosureDisposition::Closed)
    }

    #[must_use]
    pub fn needs_resolution(&self) -> bool {
        self.status == RunStatus::NeedsAgent
            || self.disposition == Some(ClosureDisposition::Deferred)
    }

    pub(crate) fn sealed(output: JsonValue, receipt: &Receipt) -> Self {
        Self {
            status: RunStatus::Sealed,
            disposition: Some(receipt.seal.disposition.clone()),
            output,
            receipt_refs: vec![receipt.id.to_string()],
            child_receipt_refs: child_receipt_refs(receipt),
            pending_requests: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunStatus {
    NeedsAgent,
    Sealed,
    Failed,
}

#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error(transparent)]
    SkillRun(#[from] SkillRunError),
    #[error(transparent)]
    Runtime(#[from] crate::RuntimeError),
    #[error(transparent)]
    Harness(#[from] HarnessReplayError),
    #[error(transparent)]
    ReceiptStore(#[from] crate::receipts::store::ReceiptStoreError),
}

#[derive(Clone, Debug, Default)]
pub struct LocalOrchestrator {
    effects: RuntimeEffectRegistry,
    environment: BTreeMap<String, String>,
}

impl LocalOrchestrator {
    #[must_use]
    pub fn with_effects(effects: RuntimeEffectRegistry) -> Self {
        Self {
            effects,
            environment: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_effects_and_environment(
        effects: RuntimeEffectRegistry,
        environment: BTreeMap<String, String>,
    ) -> Self {
        Self {
            effects,
            environment,
        }
    }

    #[must_use]
    pub fn with_environment(&self, environment: BTreeMap<String, String>) -> Self {
        Self {
            effects: self.effects.clone(),
            environment,
        }
    }

    pub fn run(&self, request: RunRequest) -> Result<RunResult, OrchestratorError> {
        match request {
            RunRequest::Skill(request) => self.run_skill(&request),
            RunRequest::Harness(request) => self.run_harness(&request),
        }
    }

    pub fn run_skill(&self, request: &SkillRunRequest) -> Result<RunResult, OrchestratorError> {
        let output = super::skill_front::execute_skill_run_with_effects(request, &self.effects)?;
        Ok(output)
    }

    pub fn run_skill_with_runner(
        &self,
        request: &SkillRunRequest,
        runner: &str,
    ) -> Result<RunResult, OrchestratorError> {
        let overrides = super::skill_front::SkillRunOverrides {
            runner: Some(runner.to_owned()),
            ..Default::default()
        };
        let output = super::skill_front::execute_skill_run_with_overrides(
            request,
            &overrides,
            &self.effects,
        )?;
        Ok(output)
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn run_skill_with_services(
        &self,
        request: &SkillRunRequest,
        runner: &str,
        javascript: crate::adapters::javascript::JavaScriptAdapter,
        package_digest: &str,
        execution_closure_digest: &str,
    ) -> Result<RunResult, OrchestratorError> {
        Ok(super::skill_front::execute_bound_skill_run_with_overrides(
            request,
            &super::skill_front::SkillRunOverrides {
                runner: Some(runner.to_owned()),
                javascript: Some(javascript),
            },
            &self.effects,
            Some(package_digest),
            Some(execution_closure_digest),
        )?)
    }

    pub fn run_skill_with_binding(
        &self,
        request: &SkillRunRequest,
        runner: Option<&str>,
        expected_package_digest: Option<&str>,
        expected_execution_closure_digest: Option<&str>,
    ) -> Result<RunResult, OrchestratorError> {
        let overrides = super::skill_front::SkillRunOverrides {
            runner: runner.map(str::to_owned),
            ..Default::default()
        };
        let output = super::skill_front::execute_bound_skill_run_with_overrides(
            request,
            &overrides,
            &self.effects,
            expected_package_digest,
            expected_execution_closure_digest,
        )?;
        Ok(output)
    }

    /// Continue a paused run with the answers its host brings.
    pub fn resume(&self, request: &ResumeRequest) -> Result<RunResult, OrchestratorError> {
        let run = super::skill_front::bind_paused_run(
            super::skill_front::Continuation {
                run_id: &request.run_id,
                receipt_dir: request.receipt_dir.as_deref(),
                env: &request.env,
                cwd: &request.cwd,
                answers_path: Some(&request.answers_path),
                managed_agent: request.managed_agent.clone(),
                package_digest: request.package_digest.as_deref(),
                execution_closure_digest: request.execution_closure_digest.as_deref(),
            },
            &self.effects,
        )?;
        Ok(run.execute()?)
    }

    /// Open the agent turns a paused run waits on, for a host that works them
    /// from outside the run.
    pub fn open_turns(&self, request: &TurnRequest) -> Result<TurnSession, OrchestratorError> {
        let run = super::skill_front::bind_paused_run(
            super::skill_front::Continuation {
                run_id: &request.run_id,
                receipt_dir: request.receipt_dir.as_deref(),
                env: &request.env,
                cwd: &request.cwd,
                answers_path: None,
                managed_agent: ManagedAgentPolicy::HostDriven,
                package_digest: None,
                execution_closure_digest: None,
            },
            &self.effects,
        )?;
        Ok(TurnSession::open(run)?)
    }

    pub fn prepare_skill(
        &self,
        request: SkillRunRequest,
        runner: Option<&str>,
        entry: PreparedEntryProvenance,
    ) -> Result<PreparedSkillRun, OrchestratorError> {
        Ok(prepare_skill_run_with_effects(
            request,
            runner,
            entry,
            &self.effects,
        )?)
    }

    pub fn run_prepared_skill(
        &self,
        prepared: &PreparedSkillRun,
    ) -> Result<RunResult, OrchestratorError> {
        if !prepared.is_ready() {
            return Err(SkillRunError::Invalid(
                prepared
                    .report()
                    .blocked_reason
                    .clone()
                    .unwrap_or_else(|| "prepared skill run is blocked".to_owned()),
            )
            .into());
        }
        if !prepared.is_context_bound() {
            return Err(SkillRunError::Invalid(
                "prepared skill run requires its context to be bound".to_owned(),
            )
            .into());
        }
        let loaded = prepared.verify_artifacts()?;
        let overrides = super::skill_front::SkillRunOverrides {
            runner: Some(prepared.selected_runner().to_owned()),
            ..Default::default()
        };
        let output = super::skill_front::execute_prepared_skill_run_with_resolved(
            super::skill_front::ResolvedSkillRun {
                loaded: &loaded,
                request: prepared.request(),
                overrides: &overrides,
                effects: &self.effects,
                skill_dir: &prepared.report().request.skill_path,
                manifest: prepared.manifest(),
                runner: prepared.runner(),
                package_digest: prepared.package_digest(),
                execution_closure_digest: prepared.execution_closure_digest(),
            },
        )?;
        Ok(output)
    }

    pub fn run_harness(&self, request: &HarnessRunRequest) -> Result<RunResult, OrchestratorError> {
        harness_result(self.run_harness_fixture(request)?)
    }

    pub fn run_package_harness(
        &self,
        request: &PackageHarnessRequest,
    ) -> Result<PackageHarnessReport, OrchestratorError> {
        Ok(super::harness::run_package_harness_with_effects(
            &request.skill_path,
            request.receipt_dir.as_deref(),
            &self.environment,
            &self.effects,
        )?)
    }

    pub fn run_harness_fixture(
        &self,
        request: &HarnessRunRequest,
    ) -> Result<HarnessReplayOutput, OrchestratorError> {
        let (mut options, receipt_dir) = standalone_harness_options(request, &self.environment)?;
        options.effects = self.effects.clone();
        // The case writes its receipts into a scratch root; they are kept
        // where the caller asked once the case passes.
        let scratch = tempfile::Builder::new()
            .prefix("runx-harness-receipts-")
            .tempdir()
            .map_err(|source| crate::RuntimeError::io("creating a harness case scratch", source))?;
        let mut case = options.clone();
        case.env.insert(
            crate::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
            scratch
                .path()
                .join("receipts")
                .to_string_lossy()
                .into_owned(),
        );
        let output = super::harness::run_harness_fixture_with_adapter(
            &request.fixture_path,
            super::skill_front::ExecutableAdapter,
            case,
        )?;
        persist_harness_receipts(&output, &options, &receipt_dir)?;
        Ok(output)
    }
}

fn standalone_harness_options(
    request: &HarnessRunRequest,
    environment: &BTreeMap<String, String>,
) -> Result<(super::harness::HarnessOptions, PathBuf), OrchestratorError> {
    use crate::paths::RUNX_RECEIPT_DIR_ENV;

    let workspace = crate::WorkspaceEnv::from_admitted(environment.clone())
        .map_err(crate::RuntimeError::from)?;
    let mut env = workspace.env().clone();
    let receipt_dir = workspace
        .paths()
        .harness_receipts(request.receipt_dir.as_deref());
    env.insert(
        RUNX_RECEIPT_DIR_ENV.to_owned(),
        receipt_dir.to_string_lossy().into_owned(),
    );
    let options = super::harness::HarnessOptions::new(env)?;
    Ok((options, receipt_dir))
}

fn persist_harness_receipts(
    output: &HarnessReplayOutput,
    options: &super::harness::HarnessOptions,
    receipt_dir: &std::path::Path,
) -> Result<(), OrchestratorError> {
    crate::services::ReceiptServices::from_signature_config(options.receipt_signature.clone())
        .write_local_receipts(
            output
                .steps
                .iter()
                .flat_map(|step| {
                    step.nested_receipts
                        .iter()
                        .chain(std::iter::once(&step.receipt))
                })
                .chain(std::iter::once(&output.receipt)),
            receipt_dir,
        )?;
    Ok(())
}

fn harness_result(output: HarnessReplayOutput) -> Result<RunResult, OrchestratorError> {
    let status = status_from_receipt(&output.receipt);
    let value = receipt_json(&output.receipt)?;
    Ok(RunResult {
        status,
        disposition: Some(output.receipt.seal.disposition.clone()),
        output: value,
        receipt_refs: vec![output.receipt.id.to_string()],
        child_receipt_refs: child_receipt_refs(&output.receipt),
        pending_requests: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn status_from_receipt(receipt: &Receipt) -> RunStatus {
    match receipt.seal.disposition {
        ClosureDisposition::Closed => RunStatus::Sealed,
        _ => RunStatus::Failed,
    }
}

fn receipt_json(receipt: &Receipt) -> Result<JsonValue, OrchestratorError> {
    let value = serde_json::to_value(receipt)
        .map_err(|source| crate::RuntimeError::json("serializing orchestrated receipt", source))?;
    serde_json::from_value(value)
        .map_err(|source| crate::RuntimeError::json("normalizing orchestrated receipt", source))
        .map_err(Into::into)
}

fn child_receipt_refs(receipt: &Receipt) -> Vec<String> {
    receipt
        .lineage
        .children
        .iter()
        .map(|reference| reference.uri.clone().into_string())
        .collect()
}
