// Module rationale: harness replay owns fixture loading,
// adapter invocation, receipt assertion, and graph replay sealing as one
// deterministic proof path until MCP replay creates a separate module boundary.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::{
    ClosureDisposition, ExecutionEvent, JsonObject, JsonValue, Receipt, ResolutionRequest,
    ResolutionResponse, ResolutionResponseActor,
};
use runx_parser::{SkillRunnerDefinition, SkillRunnerManifest};
use thiserror::Error;

use super::super::graph::bind_run_inputs;
use super::assertions::{assert_exchanges_requested, assert_expectations, status_from_disposition};
use super::fixtures::{
    HarnessExpectedStatus, HarnessFixture, HarnessFixtureError, HarnessFixtureKind,
    load_harness_fixture,
};
use crate::RuntimeError;
use crate::adapter::{InvocationOutput, SkillAdapter};
use crate::effects::RuntimeEffectRegistry;
use crate::execution::output_projection::project_step_claim;
use crate::execution::run_record::RunLog;
use crate::execution::runner::{GraphRun, RunClock, Runtime, RuntimeOptions, StepRun};
use crate::host::Host;
use crate::http::HarnessExchanges;
use crate::kernel::store::KernelStore;
use crate::paths::RUNX_CWD_ENV;
use crate::receipts::signing::strip_receipt_signing_env;
use crate::receipts::{
    RunScope, RuntimeReceiptSignatureConfig, RuntimeReceiptSignaturePolicy,
    StepReceiptWithDisposition, step_receipt_with_declared_claim_and_policy,
};

/// What a harness replay runs with. Each case runs as a run of its own, in a
/// kernel of its own, built from these options once the case is isolated.
#[derive(Clone, Debug)]
pub struct HarnessOptions {
    pub env: BTreeMap<String, String>,
    pub receipt_signature: RuntimeReceiptSignatureConfig,
    pub effects: RuntimeEffectRegistry,
    pub clock: RunClock,
}

impl HarnessOptions {
    /// A replay at the harness's fixed instant, sealed under the public
    /// fixture identity so its receipts are deterministic. The environment's
    /// signing variables are dropped: a replay proves only that a case ran.
    pub fn new(mut env: BTreeMap<String, String>) -> Result<Self, RuntimeError> {
        strip_receipt_signing_env(&mut env);
        Ok(Self {
            env,
            receipt_signature: RuntimeReceiptSignatureConfig::fixture().map_err(|error| {
                RuntimeError::ReceiptInvalid {
                    message: error.to_string(),
                }
            })?,
            effects: RuntimeEffectRegistry::default(),
            clock: RunClock::Fixed(crate::time::DEFAULT_CREATED_AT.to_owned()),
        })
    }

    #[must_use]
    pub fn signature_policy(&self) -> RuntimeReceiptSignaturePolicy<'_> {
        self.receipt_signature.signature_policy()
    }

    /// The case's run: an evaluation recorded in the case's own kernel.
    fn runtime(
        self,
        run_id: &str,
        fixture: &HarnessFixture,
        target: &Path,
    ) -> Result<RuntimeOptions, RuntimeError> {
        let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(&self.env, target))
            .map_err(|error| {
                RuntimeError::effect_state("opening a harness case kernel", error.to_string())
            })?;
        let run = RunLog::evaluation(
            store,
            run_id,
            &fixture.name,
            &target.to_string_lossy(),
            &self.clock.now(),
        )?;
        Ok(RuntimeOptions {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            clock: self.clock,
            env: self.env,
            receipt_signature: self.receipt_signature,
            effects: self.effects,
            run,
        })
    }
}

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
    /// The HTTP exchanges the case declares that the run never requested.
    pub unrequested: Vec<String>,
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
        source: runx_receipts::ReceiptDecodeError,
    },
    #[error(transparent)]
    ReceiptStore(#[from] crate::receipts::store::ReceiptStoreError),
}

#[derive(Debug, Error)]
#[error(
    "{message}; receipt={receipt_id}; disposition={disposition}; reason={reason}; skill_value={skill_value}; skill_failure={skill_failure}"
)]
pub struct HarnessExpectationFailure {
    pub message: String,
    pub receipt_id: String,
    pub disposition: String,
    pub reason: String,
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
    run_harness_fixture_with_adapter(
        fixture_path,
        crate::execution::skill_front::ExecutableAdapter,
        HarnessOptions::new(env)?,
    )
}

pub fn run_harness_fixture_with_adapter<A>(
    fixture_path: impl AsRef<Path>,
    adapter: A,
    options: HarnessOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError>
where
    A: SkillAdapter,
{
    let (fixture, target_path) = load_harness_case(fixture_path.as_ref())?;
    run_harness_case_with_adapter(fixture, target_path, adapter, options)
}

/// A fixture file's case and the skill or graph it targets.
pub(crate) fn load_harness_case(
    fixture_path: &Path,
) -> Result<(HarnessFixture, PathBuf), HarnessReplayError> {
    let fixture = load_harness_fixture(fixture_path)?;
    let target_path = resolve_target_path(fixture_path, &fixture.target)?;
    Ok((fixture, target_path))
}

/// Replay a case through the tools `runx harness` runs, without asserting
/// its expectations: for a generator that rewrites the expectations this run
/// produces, so they are what `runx harness` seals. Every other caller
/// asserts them through [`run_harness_fixture`].
pub fn replay_harness_fixture(
    fixture_path: impl AsRef<Path>,
    env: BTreeMap<String, String>,
) -> Result<HarnessReplayOutput, HarnessReplayError> {
    let (fixture, target_path) = load_harness_case(fixture_path.as_ref())?;
    replay_harness_case(
        fixture,
        target_path,
        crate::execution::skill_front::ExecutableAdapter,
        HarnessOptions::new(env)?,
    )
}

/// Run one case, a fixture file's or a runner's inline case, against the
/// skill or graph at `target_path`, and assert what it expects. Both kinds
/// run here, through one engine.
pub(crate) fn run_harness_case_with_adapter<A>(
    fixture: HarnessFixture,
    target_path: PathBuf,
    adapter: A,
    options: HarnessOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError>
where
    A: SkillAdapter,
{
    let receipt_signature = options.receipt_signature.clone();
    let output = replay_harness_case(fixture, target_path, adapter, options)?;
    assert_expectations(&output, receipt_signature.signature_policy())
        .map_err(|error| expectation_error_with_output(error, &output))?;
    Ok(output)
}

fn replay_harness_case<A>(
    mut fixture: HarnessFixture,
    target_path: PathBuf,
    adapter: A,
    options: HarnessOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError>
where
    A: SkillAdapter,
{
    // Each case runs in a home and a project directory of its own, held
    // until the case is done: its profiles, instances, custody and usage
    // never reach the operator or another case.
    let case_root = tempfile::Builder::new()
        .prefix("runx-harness-case-")
        .tempdir()
        .map_err(|error| {
            RuntimeError::effect_state("isolating a harness case", error.to_string())
        })?;
    crate::paths::isolate(&mut fixture.env, case_root.path());
    // The host's workspace policy is the host's: a case's environment never
    // sets it, so a publisher's harness cannot lift a refusal the host made.
    let policy = crate::execution_environment::RUNX_HOSTED_WORKSPACE_POLICY_JSON_ENV;
    match options.env.get(policy) {
        Some(host) => fixture.env.insert(policy.to_owned(), host.clone()),
        None => fixture.env.remove(policy),
    };
    let mut options = options;
    let exchanges = wire_operations(&mut fixture, &target_path, &mut options)?;
    seed_harness_receipts(&fixture, &target_path, &options)?;
    let replayed = match fixture.kind {
        HarnessFixtureKind::Skill => run_skill_fixture(&fixture, target_path, adapter, options),
        HarnessFixtureKind::Graph => run_graph_fixture(&fixture, &target_path, adapter, options),
    };
    // However the case ends, even in a refusal a front accepts as expected,
    // it proves only the requests its run made.
    let unrequested = exchanges.unrequested().map_err(|error| {
        RuntimeError::effect_state("reading the harness exchanges", error.to_string())
    })?;
    match replayed {
        Ok(mut output) => {
            output.unrequested = unrequested;
            Ok(output)
        }
        Err(error) => {
            assert_exchanges_requested(&unrequested)?;
            Err(error)
        }
    }
}

fn seed_harness_receipts(
    fixture: &HarnessFixture,
    target_path: &Path,
    options: &HarnessOptions,
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
        receipts.push(runx_receipts::decode_receipt(&contents).map_err(|source| {
            HarnessReplayError::SetupReceiptInvalid {
                path: path.clone(),
                source,
            }
        })?);
    }

    let mut env = options.env.clone();
    env.extend(fixture.env.clone());
    crate::services::VerifiedReceiptStore::resolve(&env, target_path)?.write_all(&receipts)?;
    Ok(())
}

/// Stand the case's fixtures in beneath operation steps: its exact HTTP
/// answers, its provider fixture, and the sessions and credentials it runs
/// under. Returns the exchanges, which record what the run asked of them.
fn wire_operations(
    fixture: &mut HarnessFixture,
    target_path: &Path,
    options: &mut HarnessOptions,
) -> Result<Arc<HarnessExchanges>, HarnessReplayError> {
    // The target is the skill's directory or a file in it. Connectors are
    // reached from there: from above it, a bundled package is outside its
    // own plugin.
    let skill_dir = if target_path.is_dir() {
        target_path
    } else {
        target_path.parent().unwrap_or(target_path)
    };
    let scope = crate::ConnectorScope::at(skill_dir)
        .map_err(|error| RuntimeError::effect_state("resolving harness connector scope", error))?
        .in_project(crate::paths::RunxPaths::resolve(&options.env, skill_dir).connectors());
    let invalid_provider = |message: String| {
        HarnessFixtureError::Parser(runx_parser::harness_fixture::HarnessFixtureError::Invalid {
            field: "caller.provider_responses.grant".to_owned(),
            message,
        })
    };
    let transport = super::transport::harness_transport(&fixture.caller, &scope, skill_dir)
        .map_err(invalid_provider)?;
    options.effects =
        super::transport::effects_over(&options.effects, transport.clone(), &fixture.caller)
            .map_err(|error| RuntimeError::effect_state("wiring the harness transport", error))?;
    fixture
        .env
        .extend(super::transport::provider_env(&fixture.caller));
    let mut env = options.env.clone();
    env.extend(fixture.env.clone());
    resolve_fixture_path(&mut env, skill_dir)?;
    super::transport::seed_provider_grant(&fixture.caller, &scope, &env, skill_dir)
        .map_err(invalid_provider)?;
    seed_harness_instances(fixture, target_path, skill_dir, options, &transport)?;
    Ok(transport.exchanges)
}

/// Confirm and select the instances the case runs under, in the case's own
/// kernel and home, with the fixture environment resolved as the run
/// resolves it: each session as `runx connector use <connector> --session`
/// would, and each credential bound to the variable the case sets, as
/// `runx connector use <connector>` would with that variable set. They are
/// confirmed from `skill_dir`, the directory of a graph case's file.
fn seed_harness_instances(
    fixture: &HarnessFixture,
    target_path: &Path,
    skill_dir: &Path,
    options: &HarnessOptions,
    transport: &super::provider_responses::HarnessTransport,
) -> Result<(), HarnessReplayError> {
    let setup = &fixture.setup;
    if setup.sessions.is_empty() && setup.credentials.is_empty() {
        return Ok(());
    }
    let failed = |error: String| RuntimeError::effect_state("setting up a harness instance", error);
    let mut env = options.env.clone();
    env.extend(fixture.env.clone());
    resolve_fixture_path(&mut env, skill_dir)?;
    let paths = crate::paths::RunxPaths::resolve(&env, skill_dir);
    let workspace = crate::services::WorkspaceEnv::new(env.clone(), skill_dir.to_path_buf())
        .map_err(|error| failed(error.to_string()))?;
    let instances = crate::Instances::new(
        crate::KernelStore::for_project(&paths).map_err(|error| failed(error.to_string()))?,
    );
    let scope = crate::ConnectorScope::at(skill_dir)
        .map_err(|error| failed(error.to_string()))?
        .in_project(paths.connectors());
    // A case names a connector as its skill's `connectors:` block keys it, a
    // connector id or a qualified ref, and it resolves as the skill's
    // operations resolve it, pinned manifest included.
    let requirements = match fixture.kind {
        HarnessFixtureKind::Skill => {
            crate::skill_package::load_validated_skill_package(target_path)?
                .manifest()
                .map(|manifest| manifest.connectors.clone())
                .unwrap_or_default()
        }
        HarnessFixtureKind::Graph => BTreeMap::new(),
    };
    let connector = |key: &str| {
        match requirements
            .get(key)
            .filter(|requirement| requirement.connector_from.is_none())
        {
            Some(requirement) => scope.requirement(key, requirement),
            None => scope.connector(key),
        }
        .map_err(|error| failed(error.to_string()))
    };
    for key in &setup.sessions {
        let connector = connector(key)?;
        crate::kernel::setup::confirm_session_over(
            &instances,
            &connector,
            &[],
            transport,
            &env,
            skill_dir,
        )
        .map_err(failed)?;
    }
    for key in &setup.credentials {
        let connector = connector(key)?;
        let manifest = &connector.manifest;
        // The one declared credential whose variable the case itself sets.
        let mut set = manifest
            .auth
            .credentials
            .iter()
            .filter(|(_, declaration)| fixture.env.contains_key(declaration.delivery.env.as_str()))
            .map(|(name, _)| name);
        let (Some(name), None) = (set.next(), set.next()) else {
            return Err(failed(format!(
                "the case sets the variable of exactly one of connector {}'s credentials",
                connector.connector_ref
            ))
            .into());
        };
        crate::kernel::setup::bind_environment(
            &instances,
            &connector,
            name,
            &[],
            runx_contracts::JsonObject::new(),
            &workspace,
        )
        .map_err(failed)?;
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
        reason: output
            .receipt
            .seal
            .reason
            .map_or("none", runx_contracts::SealReason::label)
            .to_owned(),
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

fn run_skill_fixture<A>(
    fixture: &HarnessFixture,
    skill_dir: PathBuf,
    adapter: A,
    options: HarnessOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError>
where
    A: SkillAdapter,
{
    let target = skill_fixture_target(fixture, skill_dir, &options)?;
    match crate::input_contract::materialize_complete_runner_inputs(
        &target.runner.inputs,
        &fixture.inputs,
    ) {
        Ok(inputs) => run_graph_skill_fixture(fixture, target, inputs, adapter, options),
        Err(error) => seal_admission_failure(
            fixture,
            &target.runner.name,
            &error.into_runtime_error(),
            &options,
        ),
    }
}

/// A case whose inputs the runner's contract refuses seals the refusal
/// without running any step.
fn seal_admission_failure(
    fixture: &HarnessFixture,
    runner_name: &str,
    error: &RuntimeError,
    options: &HarnessOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError> {
    let output = InvocationOutput::runtime_failure(
        JsonValue::Object(error.public_failure_projection()),
        error.to_string(),
        0,
    );
    let projection = project_step_claim(JsonObject::new(), None);
    let run_id = harness_run_id(&fixture.name);
    let receipt = step_receipt_with_declared_claim_and_policy(
        StepReceiptWithDisposition {
            scope: RunScope {
                run_id: &run_id,
                graph_name: &fixture.name,
            },
            step_id: runner_name,
            attempt: 1,
            output: &output,
            created_at: &options.clock.now(),
            disposition: ClosureDisposition::Failed,
            reason: Some(runx_contracts::SealReason::InputContractInvalid),
        },
        &projection.outputs,
        options.signature_policy(),
    )?;
    Ok(HarnessReplayOutput {
        fixture: fixture.clone(),
        status: status_from_disposition(&receipt.seal.disposition),
        receipt,
        step_receipts: Vec::new(),
        steps: Vec::new(),
        skill_output: Some(output),
        replayed_answers: JsonObject::new(),
        unanswered: Vec::new(),
        unrequested: Vec::new(),
    })
}

// Function rationale: the fixture graph turn keeps materialize,
// run, and act-receipt minting in one path so it seals under the same instant and
// signature policy as the production and inline fronts.
fn run_graph_skill_fixture<A>(
    fixture: &HarnessFixture,
    target: SkillFixtureTarget,
    inputs: JsonObject,
    adapter: A,
    mut options: HarnessOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError>
where
    A: SkillAdapter,
{
    overlay_harness_env(&mut options, &target.env);
    let run_id = options
        .env
        .entry(crate::execution::runner::RUNX_RUN_ID_ENV.to_owned())
        .or_insert_with(|| harness_run_id(&fixture.name))
        .clone();
    let graph = bind_run_inputs(
        target.runner.graph.clone(),
        &inputs,
        fixture.caller.act.as_ref(),
        &run_id,
    )?;
    let runtime = Runtime::new(
        adapter,
        options.runtime(&run_id, fixture, &target.skill_dir)?,
    );
    let mut host = FixtureHost::new(fixture);
    let graph_run = runtime.run_graph_for_harness(&target.skill_dir, graph, &mut host)?;
    let (replayed_answers, unanswered) = host.finish();
    let mut output = replay_output_from_graph(fixture, graph_run, replayed_answers, unanswered);
    if output.skill_output.is_none() {
        output.skill_output = output
            .steps
            .iter()
            .rev()
            .find(|run| run.outcome.succeeded())
            .or_else(|| output.steps.last())
            .map(|run| {
                if run.outcome.succeeded() {
                    InvocationOutput::runtime_success(JsonValue::Object(run.contract.clone()), 0)
                } else {
                    InvocationOutput::runtime_failure(
                        JsonValue::Object(run.contract.clone()),
                        run.outcome
                            .failure_message()
                            .unwrap_or_else(|| "graph harness step failed".to_owned()),
                        0,
                    )
                }
            });
    }
    Ok(output)
}

/// The runner a skill case runs and what it runs with.
struct SkillFixtureTarget {
    runner: SkillRunnerDefinition,
    env: BTreeMap<String, String>,
    skill_dir: PathBuf,
}

fn skill_fixture_target(
    fixture: &HarnessFixture,
    skill_dir: PathBuf,
    options: &HarnessOptions,
) -> Result<SkillFixtureTarget, HarnessReplayError> {
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
    let credentials = super::credential_variables(&skill_dir, &env)?;
    super::isolate_harness_environment(&mut env, &credentials);
    env.extend(fixture.env.clone());
    resolve_fixture_path(&mut env, &skill_dir)?;
    crate::services::merge_inferred_tool_roots(&mut env, &skill_dir);
    Ok(SkillFixtureTarget {
        runner,
        env,
        skill_dir,
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

fn run_graph_fixture<A>(
    fixture: &HarnessFixture,
    graph_path: &Path,
    adapter: A,
    mut options: HarnessOptions,
) -> Result<HarnessReplayOutput, HarnessReplayError>
where
    A: SkillAdapter,
{
    let skill_dir = graph_path.parent().unwrap_or(graph_path);
    let credentials = super::credential_variables(skill_dir, &options.env)?;
    super::isolate_harness_environment(&mut options.env, &credentials);
    overlay_harness_env(&mut options, &fixture.env);
    // Harness graph replays need a deterministic run_id so per-run governance
    // can resolve one, mirroring the production graph runner. Derived from the
    // graph so receipts stay reproducible; an explicit fixture env value still
    // wins.
    let run_id = options
        .env
        .entry(crate::execution::runner::RUNX_RUN_ID_ENV.to_owned())
        .or_insert_with(|| {
            harness_run_id(
                graph_path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("graph"),
            )
        })
        .clone();
    let runtime = Runtime::new(adapter, options.runtime(&run_id, fixture, graph_path)?);
    let mut host = FixtureHost::new(fixture);
    let graph_run = runtime.run_graph_file_for_harness(graph_path, &mut host)?;
    let (replayed_answers, unanswered) = host.finish();
    let output = replay_output_from_graph(fixture, graph_run, replayed_answers, unanswered);
    Ok(output)
}

struct FixtureHost<'a> {
    fixture: &'a HarnessFixture,
    /// The outputs scripted turns end with, by address.
    worked: JsonObject,
    replayed_answers: JsonObject,
    unanswered: Vec<String>,
}

impl<'a> FixtureHost<'a> {
    fn new(fixture: &'a HarnessFixture) -> Self {
        Self {
            fixture,
            worked: JsonObject::new(),
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
            // A case decides an approval by the address of the request it
            // names: the step that asks at the root, `<child>#<step>` below.
            ResolutionRequest::Approval { id, .. } => {
                let response = self
                    .fixture
                    .caller
                    .approvals
                    .get(id.as_str())
                    .map(|approved| ResolutionResponse {
                        actor: ResolutionResponseActor::Human,
                        payload: JsonValue::Bool(*approved),
                    });
                if response.is_none() {
                    self.unanswered.push(id.to_string());
                }
                Ok(response)
            }
            // A case scripts an act by its address, or by its id in its own
            // run at any depth.
            ResolutionRequest::AgentAct { id, .. } => {
                let address = id.as_str();
                let answers = &self.fixture.caller.answers;
                let id = if answers.contains_key(address) {
                    address
                } else {
                    crate::execution::run_record::local_request_id(address)
                };
                let response = self
                    .worked
                    .remove(address)
                    .or_else(|| answers.get(id).cloned())
                    .map(|answer| ResolutionResponse {
                        actor: ResolutionResponseActor::Agent,
                        payload: answer,
                    });
                if response.is_none() {
                    self.unanswered.push(id.to_owned());
                }
                if let Some(response) = &response {
                    self.replayed_answers
                        .insert(id.to_owned(), response.payload.clone());
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

    /// A scripted turn makes its calls, then ends with its output.
    fn drive_turn(
        &mut self,
        turn: &mut dyn crate::execution::runner::turn::Turn,
    ) -> Result<(), RuntimeError> {
        let fixture = self.fixture;
        let address = turn.invocation().id.as_str().to_owned();
        let answered = fixture.caller.answers.contains_key(&address);
        if let Some(output) =
            super::turns::work_scripted_turn(turn, self, &fixture.caller.turns, answered)?
        {
            self.worked.insert(address, output);
        }
        Ok(())
    }
}

fn replay_output_from_graph(
    fixture: &HarnessFixture,
    graph_run: GraphRun,
    replayed_answers: JsonObject,
    unanswered: Vec<String>,
) -> HarnessReplayOutput {
    let result = crate::execution::runner::graph_terminal_result(&graph_run).ok();
    let skill_output = result.map(|result| {
        if graph_run.state.status == runx_core::state_machine::GraphStatus::Succeeded {
            InvocationOutput::runtime_success(result, 0)
        } else {
            InvocationOutput::runtime_failure(
                result,
                crate::execution::runner::graph_failure(&graph_run),
                0,
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
        unrequested: Vec::new(),
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

fn overlay_harness_env(options: &mut HarnessOptions, env: &BTreeMap<String, String>) {
    options.env.extend(env.clone());
}

/// A harness case's run id: fixed by its fixture, so its receipts reproduce.
fn harness_run_id(name: &str) -> String {
    format!("harness-{name}")
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
    use super::{HarnessOptions, overlay_harness_env, resolve_fixture_path};
    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn overlay_harness_env_preserves_operator_env_and_allows_fixture_override()
    -> Result<(), crate::RuntimeError> {
        let mut options = HarnessOptions::new(BTreeMap::from([
            ("OPERATOR_ENV".to_owned(), "preserved".to_owned()),
            ("FIXTURE_OVERRIDE".to_owned(), "operator".to_owned()),
        ]))?;
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
        Ok(())
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
