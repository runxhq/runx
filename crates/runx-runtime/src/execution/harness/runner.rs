// Module rationale: harness replay owns fixture loading,
// adapter invocation, receipt assertion, and graph replay sealing as one
// deterministic proof path until MCP replay creates a separate module boundary.

mod dispositions;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use dispositions::{
    agent_answer_disposition, agent_task_output, disposition_from_expected_status,
    disposition_suffix, named_reason_code, process_reason_code, required_string_metadata,
    skill_output_object, string_metadata,
};
use runx_contracts::{
    ClosureDisposition, ExecutionEvent, JsonObject, JsonValue, Receipt, ResolutionRequest,
    ResolutionResponse, ResolutionResponseActor,
};
use runx_core::state_machine::StepAdmissionWitness;
use runx_parser::{SkillRunnerDefinition, SkillRunnerManifest};
use thiserror::Error;

use super::super::graph::materialize_graph_parameter_inputs;
use super::assertions::{assert_expectations, status_from_disposition};
use super::fixtures::{
    HarnessExpectedStatus, HarnessFixture, HarnessFixtureError, HarnessFixtureKind,
    load_harness_fixture,
};
use crate::RuntimeError;
use crate::adapter::{EphemeralValue, InvocationOutput, SkillAdapter, SkillInvocation};
use crate::agent_contract::{
    agent_output_contract_payload, verified_agent_metadata_with_artifacts,
};
use crate::agent_invocation::{
    AgentActInvocationSourceType, agent_act_invocation_id, agent_act_resolution_request,
};
use crate::effects::RuntimeEffectRegistry;
use crate::execution::output_projection::project_step_claim;
use crate::execution::runner::{GraphRun, Runtime, RuntimeOptions, StepRun};
use crate::host::Host;
use crate::output_contract::{
    attach_verified_metadata, project_declared_output_claim,
    verified_runner_metadata_with_artifacts,
};
use crate::receipts::paths::RUNX_CWD_ENV;
use crate::receipts::{
    GraphClosure, StepReceiptWithDisposition, graph_receipt_with_disposition_and_policy,
    step_receipt_with_declared_claim_and_policy, step_receipt_with_disposition_and_policy,
};

#[derive(Clone, Debug)]
pub struct HarnessReplayOutput {
    pub fixture: HarnessFixture,
    pub status: HarnessExpectedStatus,
    pub receipt: Receipt,
    pub step_receipts: Vec<Receipt>,
    pub steps: Vec<StepRun>,
    pub skill_output: Option<InvocationOutput>,
    pub replayed_answers: JsonObject,
    /// The requests the run made that the case did not answer, by id.
    pub unanswered: Vec<String>,
}

#[derive(Debug, Error)]
pub enum HarnessReplayError {
    #[error(transparent)]
    Fixture(#[from] HarnessFixtureError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("harness fixture target {target} has no parent directory")]
    TargetWithoutParent { target: PathBuf },
    #[error("harness expectation mismatch at {field}: expected {expected}, actual {actual}")]
    Mismatch {
        field: String,
        expected: String,
        actual: String,
    },
    // Boxed: the seven diagnostic strings would otherwise dominate the size of
    // every Result carrying this enum (clippy::result_large_err).
    #[error(transparent)]
    ExpectationFailed(Box<HarnessExpectationFailure>),
    #[error("receipt digest failed: {message}")]
    ReceiptDigest { message: String },
    #[error("receipt proof failed for {receipt_id}: {findings}")]
    ReceiptProofInvalid {
        receipt_id: String,
        findings: String,
    },
    #[error("invalid harness replay metadata at {field}: {message}")]
    InvalidReplayMetadata { field: String, message: String },
    #[error("invalid harness fixture environment {name}: {message}")]
    InvalidFixtureEnvironment { name: String, message: String },
    #[error("harness setup receipt path escaped its skill package: {path}")]
    SetupReceiptPathEscape { path: PathBuf },
    #[error("failed to read harness setup receipt {path}: {source}")]
    SetupReceiptRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid harness setup receipt {path}: {source}")]
    SetupReceiptInvalid {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error(transparent)]
    ReceiptStore(#[from] crate::receipts::store::ReceiptStoreError),
    #[error(
        "native cli-tool harness replay is unavailable because runx-runtime was built without the cli-tool feature"
    )]
    CliToolFeatureDisabled,
}

#[derive(Debug, Error)]
#[error(
    "{message}; receipt={receipt_id}; disposition={disposition}; reason={reason_code}; summary={summary}; skill_value={skill_value}; skill_failure={skill_failure}"
)]
pub struct HarnessExpectationFailure {
    pub message: String,
    pub receipt_id: String,
    pub disposition: String,
    pub reason_code: String,
    pub summary: String,
    pub skill_value: String,
    pub skill_failure: String,
}

impl From<crate::execution::skill_front::SkillRunError> for HarnessReplayError {
    fn from(error: crate::execution::skill_front::SkillRunError) -> Self {
        use crate::execution::skill_front::SkillRunError;
        match error {
            SkillRunError::Runtime(error) => HarnessReplayError::Runtime(error),
            SkillRunError::PreflightRefused { source, .. } => HarnessReplayError::Runtime(*source),
            other => HarnessReplayError::Runtime(RuntimeError::ReceiptInvalid {
                message: other.to_string(),
            }),
        }
    }
}

pub fn run_harness_fixture(
    fixture_path: impl AsRef<Path>,
    env: BTreeMap<String, String>,
) -> Result<HarnessReplayOutput, HarnessReplayError> {
    #[cfg(feature = "cli-tool")]
    {
        run_harness_fixture_with_adapter(
            fixture_path,
            crate::execution::skill_front::SkillSourceAdapter::default(),
            fixture_runtime_options_from_env(env)?,
        )
    }
    #[cfg(not(feature = "cli-tool"))]
    {
        let _ = fixture_path;
        let _ = env;
        Err(HarnessReplayError::CliToolFeatureDisabled)
    }
}

#[cfg(feature = "cli-tool")]
fn fixture_runtime_options_from_env(
    env: BTreeMap<String, String>,
) -> Result<RuntimeOptions, HarnessReplayError> {
    Ok(RuntimeOptions {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        created_at: crate::time::DEFAULT_CREATED_AT.to_owned(),
        ..RuntimeOptions::from_env_or_local_development(env)?
    })
}

pub fn run_harness_fixture_with_adapter<A>(
    fixture_path: impl AsRef<Path>,
    adapter: A,
    mut options: RuntimeOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError>
where
    A: SkillAdapter,
{
    let fixture_path = fixture_path.as_ref();
    let mut fixture = load_harness_fixture(fixture_path)?;
    let target_path = resolve_target_path(fixture_path, &fixture.target)?;
    // Each case runs against a kernel of its own, held until the case is
    // done: its instances, custody and usage never reach the operator's
    // workspace or another case.
    let case_root = tempfile::Builder::new()
        .prefix("runx-harness-case-")
        .tempdir()
        .map_err(|error| {
            RuntimeError::effect_state("isolating a harness case", error.to_string())
        })?;
    fixture.env.insert(
        crate::receipts::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
        case_root
            .path()
            .join("project")
            .to_string_lossy()
            .into_owned(),
    );
    #[cfg(feature = "async-http")]
    wire_operations(&mut fixture, &target_path, &mut options, case_root.path())?;
    seed_harness_receipts(&fixture, &target_path, &options)?;
    let receipt_signature = options.receipt_signature.clone();
    let output = match fixture.kind {
        HarnessFixtureKind::Skill | HarnessFixtureKind::Agent => {
            run_skill_fixture(&fixture, target_path, adapter, options)?
        }
        HarnessFixtureKind::AgentStep => run_agent_task_fixture(&fixture, options)?,
        HarnessFixtureKind::Graph if is_fixture_replay_graph(&fixture) => {
            run_graph_replay_fixture(&fixture, options)?
        }
        HarnessFixtureKind::Graph => run_graph_fixture(&fixture, &target_path, adapter, options)?,
    };
    assert_expectations(&output, receipt_signature.signature_policy())
        .map_err(|error| expectation_error_with_output(error, &output))?;
    Ok(output)
}

fn seed_harness_receipts(
    fixture: &HarnessFixture,
    target_path: &Path,
    options: &RuntimeOptions,
) -> Result<(), HarnessReplayError> {
    if fixture.setup.receipts.is_empty() {
        return Ok(());
    }

    let loaded = crate::load_validated_skill_package(target_path)?;
    let package_root = fs::canonicalize(&loaded.package_root).map_err(|source| {
        HarnessReplayError::SetupReceiptRead {
            path: loaded.package_root.clone(),
            source,
        }
    })?;
    let mut receipts = Vec::with_capacity(fixture.setup.receipts.len());
    for relative in &fixture.setup.receipts {
        let candidate = package_root.join(relative);
        let path = fs::canonicalize(&candidate).map_err(|source| {
            HarnessReplayError::SetupReceiptRead {
                path: candidate.clone(),
                source,
            }
        })?;
        if !path.starts_with(&package_root) {
            return Err(HarnessReplayError::SetupReceiptPathEscape { path });
        }
        let contents = fs::read(&path).map_err(|source| HarnessReplayError::SetupReceiptRead {
            path: path.clone(),
            source,
        })?;
        receipts.push(
            serde_json::from_slice::<Receipt>(&contents).map_err(|source| {
                HarnessReplayError::SetupReceiptInvalid {
                    path: path.clone(),
                    source,
                }
            })?,
        );
    }

    let mut env = options.env.clone();
    env.extend(fixture.env.clone());
    crate::services::VerifiedReceiptStore::resolve(&env, target_path)?.write_all(&receipts)?;
    Ok(())
}

/// Stand the case's fixtures in beneath operation steps: its exact HTTP
/// answers, its provider fixture, and the sessions and credentials it runs
/// under.
#[cfg(feature = "async-http")]
fn wire_operations(
    fixture: &mut HarnessFixture,
    target_path: &Path,
    options: &mut RuntimeOptions,
    case_root: &Path,
) -> Result<(), HarnessReplayError> {
    let http_responses = runx_parser::harness_fixture::parse_harness_http_responses(
        fixture.caller.get("http_responses"),
        "caller.http_responses",
    )
    .map_err(HarnessFixtureError::Parser)?;
    let http_exchanges = runx_parser::harness_fixture::parse_harness_http_exchanges(
        fixture.caller.get("http_exchanges"),
        "caller.http_exchanges",
    )
    .map_err(HarnessFixtureError::Parser)?;
    let provider_responses = runx_parser::harness_fixture::parse_harness_provider_responses(
        fixture.caller.get("provider_responses"),
        "caller.provider_responses",
    )
    .map_err(HarnessFixtureError::Parser)?;
    let mcp_calls = runx_parser::harness_fixture::parse_harness_mcp_calls(
        fixture.caller.get("mcp_calls"),
        "caller.mcp_calls",
    )
    .map_err(HarnessFixtureError::Parser)?;
    let transport = super::http_responses::harness_transport(
        &http_responses,
        &http_exchanges,
        provider_responses.as_ref(),
    );
    options.effects =
        super::http_responses::effects_over(&options.effects, transport.clone(), &mcp_calls)
            .map_err(|error| RuntimeError::effect_state("wiring the harness transport", error))?;
    if let Some(responses) = provider_responses.as_ref() {
        fixture.env.insert(
            crate::HOSTED_API_BASE_URL_ENV.to_owned(),
            super::HARNESS_PROVIDER_BASE_URL.to_owned(),
        );
        fixture.env.insert(
            crate::HOSTED_API_TOKEN_ENV.to_owned(),
            super::HARNESS_PROVIDER_TOKEN.to_owned(),
        );
        fixture
            .env
            .extend(super::injected_grant_env(responses).map_err(|message| {
                HarnessFixtureError::Parser(
                    runx_parser::harness_fixture::HarnessFixtureError::Invalid {
                        field: "caller.provider_responses.principal_id".to_owned(),
                        message,
                    },
                )
            })?);
    }
    seed_harness_sessions(fixture, target_path, options, &transport)?;
    seed_harness_credentials(fixture, target_path, options, case_root)
}

/// Confirm and select the session instances the case runs under, as
/// `runx connector use` would, in the case's own kernel and
/// with the fixture environment resolved as the run resolves it.
#[cfg(feature = "async-http")]
fn seed_harness_sessions(
    fixture: &HarnessFixture,
    target_path: &Path,
    options: &RuntimeOptions,
    transport: &super::provider_responses::HarnessTransport,
) -> Result<(), HarnessReplayError> {
    if fixture.setup.sessions.is_empty() {
        return Ok(());
    }
    let mut env = options.env.clone();
    env.extend(fixture.env.clone());
    resolve_fixture_path(&mut env, target_path)?;
    let setup = |error: String| RuntimeError::effect_state("confirming a harness session", error);
    let instances = crate::Instances::new(
        crate::KernelStore::for_workspace(&env, target_path)
            .map_err(|error| setup(error.to_string()))?,
    );
    let scope =
        crate::ConnectorScope::for_skill(target_path).map_err(|error| setup(error.to_string()))?;
    for connector in &fixture.setup.sessions {
        let connector = scope
            .connector(connector.as_str())
            .map_err(|error| setup(error.to_string()))?;
        crate::kernel::setup::confirm_session_over(
            &instances,
            &connector,
            transport,
            &env,
            target_path,
        )
        .map_err(setup)?;
    }
    Ok(())
}

/// Store and select the credentials the case runs under, as `runx
/// credential set` would. They live in a runx home inside the case's root,
/// so no secret reaches the operator's.
#[cfg(feature = "async-http")]
fn seed_harness_credentials(
    fixture: &mut HarnessFixture,
    target_path: &Path,
    options: &RuntimeOptions,
    case_root: &Path,
) -> Result<(), HarnessReplayError> {
    if fixture.setup.credentials.is_empty() {
        return Ok(());
    }
    let setup = |error: String| RuntimeError::effect_state("storing a harness credential", error);
    fixture.env.insert(
        "RUNX_HOME".to_owned(),
        case_root.join("home").to_string_lossy().into_owned(),
    );
    let mut env = options.env.clone();
    env.extend(fixture.env.clone());
    resolve_fixture_path(&mut env, target_path)?;
    let workspace = crate::services::WorkspaceEnv::new(env.clone(), target_path.to_path_buf())
        .map_err(|error| setup(error.to_string()))?;
    let instances = crate::Instances::new(
        crate::KernelStore::for_workspace(&env, target_path)
            .map_err(|error| setup(error.to_string()))?,
    );
    let scope =
        crate::ConnectorScope::for_skill(target_path).map_err(|error| setup(error.to_string()))?;
    for connector in &fixture.setup.credentials {
        let connector = scope
            .connector(connector.as_str())
            .map_err(|error| setup(error.to_string()))?;
        let manifest = &connector.manifest;
        // The one declared credential whose variable the case itself sets.
        let mut set = manifest
            .auth
            .credentials
            .iter()
            .filter_map(|(name, declaration)| {
                fixture
                    .env
                    .get(declaration.delivery.env.as_str())
                    .map(|secret| (name, secret))
            });
        let (Some((name, secret)), None) = (set.next(), set.next()) else {
            return Err(setup(format!(
                "the case sets the variable of exactly one of connector {}'s credentials",
                manifest.id
            ))
            .into());
        };
        crate::kernel::setup::set_credential(
            &instances,
            &connector,
            manifest.id.as_str(),
            name,
            secret,
            &[],
            &workspace,
        )
        .map_err(setup)?;
    }
    Ok(())
}

fn expectation_error_with_output(
    error: HarnessReplayError,
    output: &HarnessReplayOutput,
) -> HarnessReplayError {
    HarnessReplayError::ExpectationFailed(Box::new(HarnessExpectationFailure {
        message: error.to_string(),
        receipt_id: output.receipt.id.to_string(),
        disposition: format!("{:?}", output.receipt.seal.disposition),
        reason_code: output.receipt.seal.reason_code.to_string(),
        summary: truncate_diagnostic(&output.receipt.seal.summary),
        skill_value: output
            .skill_output
            .as_ref()
            .map(|skill_output| truncate_diagnostic(&skill_output.rendered_value()))
            .unwrap_or_default(),
        skill_failure: output
            .skill_output
            .as_ref()
            .and_then(InvocationOutput::failure_message)
            .map(|message| truncate_diagnostic(&message))
            .unwrap_or_default(),
    }))
}

fn truncate_diagnostic(value: &str) -> String {
    const LIMIT: usize = 800;
    let trimmed = value.trim();
    if trimmed.len() <= LIMIT {
        return trimmed.to_owned();
    }
    format!(
        "{}...[truncated]",
        crate::bytes::truncate_utf8_bytes(trimmed, LIMIT)
    )
}

fn run_agent_task_fixture(
    fixture: &HarnessFixture,
    options: RuntimeOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError> {
    let replay_name = fixture.runner.as_deref().unwrap_or(&fixture.name);
    let request_id = format!("agent_task.{replay_name}.output");
    let output = agent_task_output(fixture, &request_id)?;
    let replayed_answers = replayed_fixture_answer(fixture, &request_id);
    let disposition = fixture
        .expect
        .status
        .as_ref()
        .map(disposition_from_expected_status)
        .unwrap_or_else(|| {
            if output.succeeded() {
                ClosureDisposition::Closed
            } else {
                ClosureDisposition::Failed
            }
        });
    let claim = skill_output_object(&output);
    let receipt = step_receipt_with_disposition_and_policy(
        StepReceiptWithDisposition {
            graph_name: &fixture.name,
            step_id: &fixture.name,
            attempt: 1,
            output: &output,
            created_at: &options.created_at,
            disposition: disposition.clone(),
            reason_code: process_reason_code(&disposition),
            summary: format!("agent-task {} completed", fixture.name),
        },
        &claim,
        options.signature_policy(),
    )?;
    Ok(HarnessReplayOutput {
        fixture: fixture.clone(),
        status: status_from_disposition(&receipt.seal.disposition),
        receipt,
        step_receipts: Vec::new(),
        steps: Vec::new(),
        skill_output: Some(output),
        replayed_answers,
        unanswered: Vec::new(),
    })
}

#[derive(Clone, Debug)]
struct GraphReplayStep {
    step_id: String,
    task: String,
    request_id: String,
}

fn is_fixture_replay_graph(fixture: &HarnessFixture) -> bool {
    string_metadata(fixture, "graph_shape") == Some("fixture_replay")
}

// Function rationale: graph replay receipt assembly keeps
// step runs, closure disposition, and parent receipt sealing in one invariant.
fn run_graph_replay_fixture(
    fixture: &HarnessFixture,
    options: RuntimeOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError> {
    let mut runs = Vec::new();
    let mut replayed_answers = JsonObject::new();
    for replay_step in graph_replay_steps(fixture)? {
        let output = agent_task_output(fixture, &replay_step.request_id)?;
        replayed_answers.extend(replayed_fixture_answer(fixture, &replay_step.request_id));
        let disposition = if output.succeeded() {
            ClosureDisposition::Closed
        } else {
            ClosureDisposition::Deferred
        };
        let outputs = skill_output_object(&output);
        let receipt = step_receipt_with_disposition_and_policy(
            StepReceiptWithDisposition {
                graph_name: &fixture.name,
                step_id: &replay_step.step_id,
                attempt: 1,
                output: &output,
                created_at: &options.created_at,
                disposition: disposition.clone(),
                reason_code: process_reason_code(&disposition),
                summary: if output.succeeded() {
                    format!("agent-task {} replayed", replay_step.task)
                } else {
                    output
                        .failure_message()
                        .unwrap_or_else(|| "agent-task replay failed".to_owned())
                },
            },
            &outputs,
            options.signature_policy(),
        )?;
        let succeeded = output.succeeded();
        let admission_witness =
            StepAdmissionWitness::local_runtime(&replay_step.step_id, receipt.id.as_str());
        runs.push(StepRun {
            step_id: replay_step.step_id,
            attempt: 1,
            skill: replay_step.task.clone(),
            runner: Some(replay_step.task),
            fanout_group: None,
            contract: outputs,
            ephemeral_contract: EphemeralValue::default(),
            outcome: output.into(),
            receipt,
            nested_receipts: Vec::new(),
            admission_witness,
        });
        if !succeeded {
            break;
        }
    }
    if runs.is_empty() {
        return Err(HarnessReplayError::InvalidReplayMetadata {
            field: "metadata.graph_replay_steps".to_owned(),
            message: "at least one replay step is required".to_owned(),
        });
    }
    let disposition = fixture
        .expect
        .status
        .as_ref()
        .map(disposition_from_expected_status)
        .unwrap_or_else(|| {
            if runs.iter().all(|run| run.outcome.succeeded()) {
                ClosureDisposition::Closed
            } else {
                ClosureDisposition::Deferred
            }
        });
    let receipt = graph_receipt_with_disposition_and_policy(
        &fixture.name,
        &mut runs,
        &[],
        &options.created_at,
        GraphClosure {
            disposition: disposition.clone(),
            reason_code: named_reason_code(&fixture.name, &disposition),
            summary: format!("graph {} replayed through fixture harness", fixture.name),
        },
        RuntimeEffectRegistry::default(),
        options.signature_policy(),
    )?;
    let step_receipts = runs
        .iter()
        .map(|run| run.receipt.clone())
        .collect::<Vec<_>>();
    let skill_output = runs
        .iter()
        .rev()
        .find(|run| run.outcome.succeeded())
        .or_else(|| runs.last())
        .map(|run| {
            if run.outcome.succeeded() {
                InvocationOutput::runtime_success(
                    JsonValue::Object(run.contract.clone()),
                    0,
                    run.outcome.metadata.clone(),
                )
            } else {
                InvocationOutput::runtime_failure(
                    JsonValue::Object(run.contract.clone()),
                    run.outcome
                        .failure_message()
                        .unwrap_or_else(|| "fixture replay step failed".to_owned()),
                    0,
                    run.outcome.metadata.clone(),
                )
            }
        });
    Ok(HarnessReplayOutput {
        fixture: fixture.clone(),
        status: status_from_disposition(&receipt.seal.disposition),
        receipt,
        step_receipts,
        steps: runs,
        skill_output,
        replayed_answers,
        unanswered: Vec::new(),
    })
}

fn graph_replay_steps(
    fixture: &HarnessFixture,
) -> Result<Vec<GraphReplayStep>, HarnessReplayError> {
    let Some(JsonValue::Array(raw_steps)) = fixture.metadata.get("graph_replay_steps") else {
        return Err(HarnessReplayError::InvalidReplayMetadata {
            field: "metadata.graph_replay_steps".to_owned(),
            message: "array is required for fixture replay graphs".to_owned(),
        });
    };
    raw_steps
        .iter()
        .enumerate()
        .map(|(index, raw_step)| {
            let JsonValue::Object(step) = raw_step else {
                return Err(HarnessReplayError::InvalidReplayMetadata {
                    field: format!("metadata.graph_replay_steps.{index}"),
                    message: "object is required".to_owned(),
                });
            };
            let step_id = required_string_metadata(
                step,
                &format!("metadata.graph_replay_steps.{index}.step_id"),
                "step_id",
            )?;
            let task = required_string_metadata(
                step,
                &format!("metadata.graph_replay_steps.{index}.task"),
                "task",
            )?;
            Ok(GraphReplayStep {
                request_id: format!("agent_task.{task}.output"),
                step_id,
                task,
            })
        })
        .collect()
}

fn run_skill_fixture<A>(
    fixture: &HarnessFixture,
    skill_dir: PathBuf,
    adapter: A,
    options: RuntimeOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError>
where
    A: SkillAdapter,
{
    let (skill_name, runner, mut invocation) =
        skill_fixture_invocation(fixture, skill_dir, &options)?;
    let admitted_inputs = crate::input_contract::materialize_complete_runner_inputs(
        &runner.inputs,
        &invocation.inputs,
    );
    let outcome = match admitted_inputs {
        Ok(inputs) => {
            invocation.inputs = inputs;
            if invocation.source.source_type == runx_parser::SourceKind::Graph {
                if is_fixture_replay_graph(fixture) {
                    return run_graph_replay_fixture(fixture, options);
                }
                return run_graph_skill_fixture(fixture, runner, invocation, adapter, options);
            }
            run_skill_invocation(fixture, &runner, invocation, adapter)?
        }
        Err(error) => skill_fixture_admission_failure(error.into_runtime_error()),
    };
    let SkillFixtureInvocationOutcome {
        output: skill_output,
        claim_payload,
        disposition,
        reason_code,
        summary,
        replayed_answers,
    } = outcome;
    let claim = if skill_output.succeeded() {
        project_declared_output_claim(
            &runner.name,
            &claim_payload,
            runner.source.outputs.as_ref(),
            runner.artifacts.as_ref(),
        )?
    } else {
        JsonObject::new()
    };
    let projection = project_step_claim(claim);
    let receipt = step_receipt_with_declared_claim_and_policy(
        StepReceiptWithDisposition {
            graph_name: &fixture.name,
            step_id: &skill_name,
            attempt: 1,
            output: &skill_output,
            created_at: &options.created_at,
            disposition: disposition.clone(),
            reason_code,
            summary,
        },
        &projection.outputs,
        projection.refs,
        options.signature_policy(),
    )?;
    Ok(HarnessReplayOutput {
        fixture: fixture.clone(),
        status: status_from_disposition(&receipt.seal.disposition),
        receipt,
        step_receipts: Vec::new(),
        steps: Vec::new(),
        skill_output: Some(skill_output),
        replayed_answers,
        unanswered: Vec::new(),
    })
}

fn skill_fixture_admission_failure(error: RuntimeError) -> SkillFixtureInvocationOutcome {
    let message = error.to_string();
    let claim_payload = JsonValue::Object(error.public_failure_projection());
    SkillFixtureInvocationOutcome {
        output: InvocationOutput::runtime_failure(
            claim_payload.clone(),
            message,
            0,
            JsonObject::new(),
        ),
        claim_payload,
        disposition: ClosureDisposition::Failed,
        reason_code: "input_contract_invalid".to_owned(),
        summary: "runner input contract rejected the harness case".to_owned(),
        replayed_answers: JsonObject::new(),
    }
}

// Function rationale: the fixture graph turn keeps materialize,
// run, and act-receipt minting in one path so it seals under the same instant and
// signature policy as the production and inline fronts.
fn run_graph_skill_fixture<A>(
    fixture: &HarnessFixture,
    runner: SkillRunnerDefinition,
    invocation: SkillInvocation,
    adapter: A,
    mut options: RuntimeOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError>
where
    A: SkillAdapter,
{
    let graph = invocation
        .source
        .graph
        .clone()
        .ok_or_else(|| RuntimeError::UnsupportedSource {
            source_kind: "graph runner without source.graph".to_owned(),
        })?;
    let graph = materialize_graph_parameter_inputs(graph, &invocation.inputs);
    overlay_harness_env(&mut options, &invocation.env);
    let run_id = options
        .env
        .entry(crate::execution::runner::RUNX_RUN_ID_ENV.to_owned())
        .or_insert_with(|| format!("harness-{}", fixture.name))
        .clone();
    // Capture the deterministic seal inputs before `options` moves into the
    // runtime, so an `act:`-declaring runner can mint its domain receipt from the
    // same instant and signature policy the graph trace sealed under.
    let created_at = options.created_at.clone();
    let signature_config = options.receipt_signature.clone();
    let runtime = Runtime::new(adapter, options);
    let mut host = FixtureHost::new(fixture);
    let graph_run = runtime.run_graph_for_harness(&invocation.skill_directory, graph, &mut host)?;
    let (replayed_answers, unanswered) = host.finish();
    // When the runner declares an `act:` block, seal the turn's primary receipt as
    // its domain act through the SAME production minting entry, so the fixture and
    // production/inline paths emit identical receipts. The graph trace receipt and
    // per-step receipts remain as the execution trace. Borrow `graph_run` before
    // `replay_output_from_graph` consumes it by value.
    let minted = crate::execution::skill_front::graph_domain_act_receipt(
        &runner,
        &invocation.inputs,
        &graph_run,
        &run_id,
        &created_at,
        &signature_config,
    )?;
    let mut output = replay_output_from_graph(fixture, graph_run, replayed_answers, unanswered);
    if let Some(domain_receipt) = minted {
        output.receipt = domain_receipt;
    }
    if output.skill_output.is_none() {
        output.skill_output = output
            .steps
            .iter()
            .rev()
            .find(|run| run.outcome.succeeded())
            .or_else(|| output.steps.last())
            .map(|run| {
                if run.outcome.succeeded() {
                    InvocationOutput::runtime_success(
                        JsonValue::Object(run.contract.clone()),
                        0,
                        run.outcome.metadata.clone(),
                    )
                } else {
                    InvocationOutput::runtime_failure(
                        JsonValue::Object(run.contract.clone()),
                        run.outcome
                            .failure_message()
                            .unwrap_or_else(|| "graph harness step failed".to_owned()),
                        0,
                        run.outcome.metadata.clone(),
                    )
                }
            });
    }
    Ok(output)
}

fn skill_fixture_invocation(
    fixture: &HarnessFixture,
    skill_dir: PathBuf,
    options: &RuntimeOptions,
) -> Result<(String, SkillRunnerDefinition, SkillInvocation), HarnessReplayError> {
    let loaded = crate::load_validated_skill_package(&skill_dir)?;
    let manifest = loaded
        .manifest()
        .ok_or_else(|| RuntimeError::UnsupportedRunnerSelection {
            runner: fixture
                .runner
                .clone()
                .unwrap_or_else(|| "default".to_owned()),
        })?;
    let runner = select_harness_runner(manifest, fixture.runner.as_deref())?.clone();
    let mut env = options.env.clone();
    super::isolate_harness_environment(&mut env, &super::credential_variables(&skill_dir)?);
    env.extend(fixture.env.clone());
    resolve_fixture_path(&mut env, &skill_dir)?;
    crate::services::merge_inferred_tool_roots(&mut env, &skill_dir);
    let skill_name = runner.name.clone();
    let invocation = SkillInvocation {
        historical_context: Vec::new(),
        skill_name: skill_name.clone(),
        step_id: None,
        source: runner.source.clone(),
        requirements: manifest.execution_requirements(&runner),
        artifacts: runner.artifacts.clone(),
        allowed_tools: runner.allowed_tools.clone(),
        inputs: fixture.inputs.clone(),
        resolved_inputs: JsonObject::new(),
        current_context: Vec::new(),
        provenance: Vec::new(),
        skill_directory: skill_dir,
        env,
        credential_delivery: crate::credentials::CredentialDelivery::none(),
    };
    Ok((skill_name, runner, invocation))
}

struct SkillFixtureInvocationOutcome {
    output: InvocationOutput,
    claim_payload: JsonValue,
    disposition: ClosureDisposition,
    reason_code: String,
    summary: String,
    replayed_answers: JsonObject,
}

fn run_skill_invocation<A>(
    fixture: &HarnessFixture,
    runner: &SkillRunnerDefinition,
    invocation: SkillInvocation,
    adapter: A,
) -> Result<SkillFixtureInvocationOutcome, HarnessReplayError>
where
    A: SkillAdapter,
{
    let skill_name = invocation.skill_name.clone();
    let raw_output = invocation.source.outputs.clone();
    let skill_directory = invocation.skill_directory.clone();
    let invocation_env = invocation.env.clone();
    let (output, claim_payload, disposition, reason_code, summary, replayed_answers) =
        match invocation.source.source_type.as_str() {
            "agent" | "agent-task" => {
                let source_type = AgentActInvocationSourceType::from_contract_value(
                    invocation.source.source_type.as_str(),
                )
                .ok_or_else(|| RuntimeError::UnsupportedAdapter {
                    adapter_type: invocation.source.source_type.as_str().to_owned(),
                })?;
                let resolution_request = agent_act_resolution_request(&invocation, source_type)?;
                let request_id = agent_act_invocation_id(&invocation, source_type);
                let (mut output, disposition, reason_code, summary) =
                    replay_agent_skill_fixture(fixture, &request_id)?;
                let claim_payload = match &resolution_request {
                    ResolutionRequest::AgentAct { .. } => {
                        agent_output_contract_payload(&output.value)
                    }
                    _ => {
                        return Err(RuntimeError::ReceiptInvalid {
                            message: "agent harness produced a non-agent request".to_owned(),
                        }
                        .into());
                    }
                };
                if output.succeeded() {
                    let metadata = verified_agent_metadata_with_artifacts(
                        &resolution_request,
                        &output.value,
                        runner.artifacts.as_ref(),
                        &skill_directory,
                        &invocation_env,
                    )?;
                    for (key, value) in metadata {
                        if output.metadata.insert(key.clone(), value).is_some() {
                            return Err(RuntimeError::ReceiptInvalid {
                                message: format!(
                                    "agent harness produced duplicate runtime metadata {key:?}"
                                ),
                            }
                            .into());
                        }
                    }
                }
                (
                    output,
                    claim_payload,
                    disposition,
                    reason_code,
                    summary,
                    replayed_fixture_answer(fixture, &request_id),
                )
            }
            _ => {
                let mut output = adapter.invoke(invocation)?;
                if output.succeeded() {
                    let metadata = verified_runner_metadata_with_artifacts(
                        &skill_name,
                        &output.value,
                        raw_output.as_ref(),
                        runner.artifacts.as_ref(),
                        &skill_directory,
                        &invocation_env,
                    )?;
                    attach_verified_metadata(&mut output, metadata)?;
                }
                let disposition = if output.succeeded() {
                    ClosureDisposition::Closed
                } else {
                    ClosureDisposition::Failed
                };
                let reason_code = process_reason_code(&disposition);
                let summary = format!("step {skill_name} completed");
                let claim_payload = output.value.clone();
                (
                    output,
                    claim_payload,
                    disposition,
                    reason_code,
                    summary,
                    JsonObject::new(),
                )
            }
        };
    Ok(SkillFixtureInvocationOutcome {
        output,
        claim_payload,
        disposition,
        reason_code,
        summary,
        replayed_answers,
    })
}

fn select_harness_runner<'a>(
    manifest: &'a SkillRunnerManifest,
    requested_runner: Option<&str>,
) -> Result<&'a SkillRunnerDefinition, HarnessReplayError> {
    if let Some(runner) = requested_runner {
        return manifest.runners.get(runner).ok_or_else(|| {
            RuntimeError::UnsupportedRunnerSelection {
                runner: runner.to_owned(),
            }
            .into()
        });
    }
    let defaults = manifest
        .runners
        .values()
        .filter(|runner| runner.default)
        .collect::<Vec<_>>();
    match defaults.as_slice() {
        [runner] => Ok(*runner),
        [] if manifest.runners.len() == 1 => manifest.runners.values().next().ok_or_else(|| {
            RuntimeError::UnsupportedRunnerSelection {
                runner: "default".to_owned(),
            }
            .into()
        }),
        [] => Err(RuntimeError::UnsupportedRunnerSelection {
            runner: "default".to_owned(),
        }
        .into()),
        _ => Err(RuntimeError::UnsupportedRunnerSelection {
            runner: "default".to_owned(),
        }
        .into()),
    }
}

fn replay_agent_skill_fixture(
    fixture: &HarnessFixture,
    request_id: &str,
) -> Result<(InvocationOutput, ClosureDisposition, String, String), HarnessReplayError> {
    let mut metadata = JsonObject::new();
    metadata.insert(
        "agent_request_id".to_owned(),
        JsonValue::String(request_id.to_owned()),
    );
    let Some(answer) = fixture_answer(fixture, "answers", request_id, request_id) else {
        return Ok((
            InvocationOutput::runtime_failure(
                JsonValue::Null,
                format!("missing replay answer for {request_id}"),
                0,
                metadata,
            ),
            ClosureDisposition::Deferred,
            "agent_act_deferred".to_owned(),
            format!("agent act {request_id} is awaiting replay answer"),
        ));
    };
    let disposition = agent_answer_disposition(answer)?;
    let succeeded = disposition == ClosureDisposition::Closed;
    let output = if succeeded {
        InvocationOutput::runtime_success(answer.clone(), 0, metadata)
    } else {
        InvocationOutput::runtime_failure(
            answer.clone(),
            format!("agent act closed with {}", disposition_suffix(&disposition)),
            0,
            metadata,
        )
    };
    Ok((
        output,
        disposition.clone(),
        format!("agent_act_{}", disposition_suffix(&disposition)),
        format!("agent act closed with {}", disposition_suffix(&disposition)),
    ))
}

fn run_graph_fixture<A>(
    fixture: &HarnessFixture,
    graph_path: &Path,
    adapter: A,
    mut options: RuntimeOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError>
where
    A: SkillAdapter,
{
    let skill_dir = graph_path.parent().unwrap_or(graph_path);
    super::isolate_harness_environment(&mut options.env, &super::credential_variables(skill_dir)?);
    overlay_harness_env(&mut options, &fixture.env);
    // Harness graph replays need a deterministic run_id so per-run governance
    // can resolve one, mirroring the production graph runner. Derived from the
    // graph so receipts stay reproducible; an explicit fixture env value still
    // wins.
    options
        .env
        .entry(crate::execution::runner::RUNX_RUN_ID_ENV.to_owned())
        .or_insert_with(|| {
            let stem = graph_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("graph");
            format!("harness-{stem}")
        });
    let runtime = Runtime::new(adapter, options);
    let mut host = FixtureHost::new(fixture);
    let graph_run = runtime.run_graph_file_for_harness(graph_path, &mut host)?;
    let (replayed_answers, unanswered) = host.finish();
    let output = replay_output_from_graph(fixture, graph_run, replayed_answers, unanswered);
    Ok(output)
}

struct FixtureHost<'a> {
    fixture: &'a HarnessFixture,
    replayed_answers: JsonObject,
    unanswered: Vec<String>,
}

impl<'a> FixtureHost<'a> {
    fn new(fixture: &'a HarnessFixture) -> Self {
        Self {
            fixture,
            replayed_answers: JsonObject::new(),
            unanswered: Vec::new(),
        }
    }

    /// The answers the run replayed and the requests the case left open.
    fn finish(self) -> (JsonObject, Vec<String>) {
        (self.replayed_answers, self.unanswered)
    }
}

impl Host for FixtureHost<'_> {
    fn report(&mut self, _event: ExecutionEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError> {
        match request {
            ResolutionRequest::Approval { id, gate } => {
                let response = fixture_approval_response(self.fixture, &id, &gate.id)?;
                if response.is_none() {
                    self.unanswered.push(gate.id.to_string());
                }
                if response.is_some()
                    && fixture_answer(self.fixture, "approvals", &gate.id, &id).is_none()
                    && let Some((key, answer)) =
                        fixture_answer_entry(self.fixture, "answers", &id, &gate.id)
                {
                    self.replayed_answers.insert(key.to_owned(), answer.clone());
                }
                Ok(response)
            }
            ResolutionRequest::AgentAct { id, .. } => {
                let response = fixture_agent_act_response(self.fixture, id.as_str())?;
                if response.is_none() {
                    self.unanswered.push(id.to_string());
                }
                if let Some(response) = &response {
                    self.replayed_answers
                        .insert(id.to_string(), response.payload.clone());
                }
                Ok(response)
            }
            ResolutionRequest::Input { id, .. } | ResolutionRequest::Retry { id, .. } => {
                self.unanswered.push(id.to_string());
                Ok(None)
            }
        }
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }
}

fn fixture_agent_act_response(
    fixture: &HarnessFixture,
    request_id: &str,
) -> Result<Option<ResolutionResponse>, RuntimeError> {
    let Some(answer) = fixture_answer(fixture, "answers", request_id, request_id) else {
        return Ok(None);
    };
    Ok(Some(ResolutionResponse {
        actor: ResolutionResponseActor::Agent,
        payload: answer.clone(),
    }))
}

fn fixture_approval_response(
    fixture: &HarnessFixture,
    request_id: &str,
    gate_id: &str,
) -> Result<Option<ResolutionResponse>, RuntimeError> {
    let Some(answer) = fixture_answer(fixture, "approvals", gate_id, request_id)
        .or_else(|| fixture_answer(fixture, "answers", request_id, gate_id))
    else {
        return Ok(None);
    };
    let approved = fixture_bool_answer(answer, request_id, gate_id)?;
    Ok(Some(ResolutionResponse {
        actor: fixture_answer_actor(answer, request_id, gate_id)?,
        payload: JsonValue::Bool(approved),
    }))
}

fn fixture_answer<'a>(
    fixture: &'a HarnessFixture,
    group: &str,
    primary_key: &str,
    secondary_key: &str,
) -> Option<&'a JsonValue> {
    fixture_answer_entry(fixture, group, primary_key, secondary_key).map(|(_, answer)| answer)
}

fn fixture_answer_entry<'a>(
    fixture: &'a HarnessFixture,
    group: &str,
    primary_key: &str,
    secondary_key: &str,
) -> Option<(&'a str, &'a JsonValue)> {
    let answers = fixture.caller.get(group).and_then(JsonValue::as_object)?;
    answers
        .get_key_value(primary_key)
        .or_else(|| answers.get_key_value(secondary_key))
        .map(|(key, value)| (key.as_str(), value))
}

fn fixture_bool_answer(
    answer: &JsonValue,
    request_id: &str,
    gate_id: &str,
) -> Result<bool, RuntimeError> {
    match answer {
        JsonValue::Bool(value) => Ok(*value),
        JsonValue::Object(object) => match object.get("approved").or_else(|| object.get("payload"))
        {
            Some(JsonValue::Bool(value)) => Ok(*value),
            Some(_) | None => Err(invalid_fixture_answer(request_id, gate_id)),
        },
        JsonValue::Null | JsonValue::Number(_) | JsonValue::String(_) | JsonValue::Array(_) => {
            Err(invalid_fixture_answer(request_id, gate_id))
        }
    }
}

fn fixture_answer_actor(
    answer: &JsonValue,
    request_id: &str,
    gate_id: &str,
) -> Result<ResolutionResponseActor, RuntimeError> {
    let Some(actor) = answer.as_object().and_then(|object| object.get("actor")) else {
        return Ok(ResolutionResponseActor::Human);
    };
    match actor {
        JsonValue::String(value) if value == "human" => Ok(ResolutionResponseActor::Human),
        JsonValue::String(value) if value == "agent" => Ok(ResolutionResponseActor::Agent),
        _ => Err(RuntimeError::ReceiptInvalid {
            message: format!(
                "harness fixture approval answer for request {request_id} gate {gate_id} has invalid actor"
            ),
        }),
    }
}

fn invalid_fixture_answer(request_id: &str, gate_id: &str) -> RuntimeError {
    RuntimeError::ReceiptInvalid {
        message: format!(
            "harness fixture approval answer for request {request_id} gate {gate_id} must be a boolean or object with a boolean approved field"
        ),
    }
}

fn replayed_fixture_answer(fixture: &HarnessFixture, request_id: &str) -> JsonObject {
    fixture_answer(fixture, "answers", request_id, request_id)
        .map(|answer| JsonObject::from([(request_id.to_owned(), answer.clone())]))
        .unwrap_or_default()
}

fn replay_output_from_graph(
    fixture: &HarnessFixture,
    graph_run: GraphRun,
    replayed_answers: JsonObject,
    unanswered: Vec<String>,
) -> HarnessReplayOutput {
    let result = crate::execution::runner::graph_run_result(&graph_run).ok();
    let skill_output = result.map(|result| {
        if graph_run.state.status == runx_core::state_machine::GraphStatus::Succeeded {
            InvocationOutput::runtime_success(result, 0, JsonObject::new())
        } else {
            InvocationOutput::runtime_failure(
                result,
                format!("graph {} did not succeed", graph_run.graph.name),
                0,
                JsonObject::new(),
            )
        }
    });
    let step_receipts = graph_run
        .steps
        .iter()
        .map(|step| step.receipt.clone())
        .collect::<Vec<_>>();
    HarnessReplayOutput {
        fixture: fixture.clone(),
        status: status_from_disposition(&graph_run.receipt.seal.disposition),
        receipt: graph_run.receipt,
        step_receipts,
        steps: graph_run.steps,
        skill_output,
        replayed_answers,
        unanswered,
    }
}

fn resolve_target_path(fixture_path: &Path, target: &str) -> Result<PathBuf, HarnessReplayError> {
    let Some(parent) = fixture_path.parent() else {
        return Err(HarnessReplayError::TargetWithoutParent {
            target: fixture_path.to_path_buf(),
        });
    };
    let unresolved = parent.join(target);
    fs::canonicalize(&unresolved).map_err(|source| {
        RuntimeError::io(
            format!("resolving harness target {}", unresolved.display()),
            source,
        )
        .into()
    })
}

fn overlay_harness_env(options: &mut RuntimeOptions, env: &BTreeMap<String, String>) {
    options.env.extend(env.clone());
}

fn resolve_fixture_path(
    env: &mut BTreeMap<String, String>,
    skill_dir: &Path,
) -> Result<(), HarnessReplayError> {
    if let Some(value) = env.get("PATH") {
        let resolved = std::env::split_paths(value)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    skill_dir.join(path)
                }
            })
            .collect::<Vec<_>>();
        let joined = std::env::join_paths(resolved).map_err(|error| {
            HarnessReplayError::InvalidFixtureEnvironment {
                name: "PATH".to_owned(),
                message: error.to_string(),
            }
        })?;
        env.insert("PATH".to_owned(), joined.to_string_lossy().into_owned());
    }
    if let Some(value) = env.get(RUNX_CWD_ENV) {
        let path = Path::new(value);
        if !path.is_absolute() {
            env.insert(
                RUNX_CWD_ENV.to_owned(),
                skill_dir.join(path).to_string_lossy().into_owned(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{overlay_harness_env, resolve_fixture_path};
    use crate::effects::RuntimeEffectRegistry;
    use crate::execution::runner::RuntimeOptions;
    use crate::receipts::RuntimeReceiptSignatureConfig;
    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn overlay_harness_env_preserves_operator_env_and_allows_fixture_override() {
        let mut options = RuntimeOptions {
            run_authority: None,
            current_context: Vec::new(),
            historical_context: Vec::new(),
            created_at: "2026-05-18T00:00:00Z".to_owned(),
            env: BTreeMap::from([
                ("OPERATOR_ENV".to_owned(), "preserved".to_owned()),
                ("FIXTURE_OVERRIDE".to_owned(), "operator".to_owned()),
            ]),
            receipt_signature: RuntimeReceiptSignatureConfig::local_development(),
            effects: RuntimeEffectRegistry::default(),
        };
        let fixture_env = BTreeMap::from([
            ("FIXTURE_OVERRIDE".to_owned(), "fixture".to_owned()),
            ("FIXTURE_ONLY".to_owned(), "fixture".to_owned()),
        ]);

        overlay_harness_env(&mut options, &fixture_env);

        assert_eq!(
            options.env.get("OPERATOR_ENV"),
            Some(&"preserved".to_owned())
        );
        assert_eq!(
            options.env.get("FIXTURE_OVERRIDE"),
            Some(&"fixture".to_owned())
        );
        assert_eq!(options.env.get("FIXTURE_ONLY"), Some(&"fixture".to_owned()));
    }

    #[test]
    fn fixture_path_entries_resolve_from_the_owning_skill_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut env = BTreeMap::from([
            (
                "PATH".to_owned(),
                std::env::join_paths(["fixtures/bin", "/usr/bin"])?
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("RUNX_CWD".to_owned(), "../..".to_owned()),
        ]);

        resolve_fixture_path(&mut env, Path::new("/workspace/skills/github-sync"))?;

        let paths = std::env::split_paths(env.get("PATH").ok_or("PATH was not retained")?)
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                Path::new("/workspace/skills/github-sync/fixtures/bin").to_path_buf(),
                Path::new("/usr/bin").to_path_buf(),
            ]
        );
        assert_eq!(
            env.get("RUNX_CWD"),
            Some(&"/workspace/skills/github-sync/../..".to_owned())
        );
        Ok(())
    }
}
