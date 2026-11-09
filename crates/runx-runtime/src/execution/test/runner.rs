// Module rationale: test replay owns fixture loading,
// adapter invocation, receipt assertion, and workflow replay sealing as one
// deterministic proof path until MCP replay creates a separate module boundary.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::{
    ExecutionEvent, JsonObject, JsonValue, Receipt, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor, RunOutcome,
};
use runx_parser::{SkillMethodDefinition, SkillMethodManifest};
use thiserror::Error;

use super::super::workflow::bind_run_inputs;
use super::assertions::{assert_exchanges_requested, assert_expectations, status_from_outcome};
use super::fixtures::{TestExpectedStatus, TestFixture, TestFixtureError, load_test_fixture};
use crate::RuntimeError;
use crate::adapter::{InvocationOutput, SkillAdapter};
use crate::effects::RuntimeEffectRegistry;
use crate::execution::output_projection::project_step_claim;
use crate::execution::run_record::RunLog;
use crate::execution::runner::{RunClock, Runtime, RuntimeOptions, StepRun, WorkflowRun};
use crate::host::Host;
use crate::http::TestExchanges;
use crate::kernel::store::KernelStore;
use crate::paths::RUNX_CWD_ENV;
use crate::receipts::signing::strip_receipt_signing_env;
use crate::receipts::{
    RunScope, RuntimeReceiptSignatureConfig, RuntimeReceiptSignaturePolicy, StepReceiptWithOutcome,
    step_receipt_with_declared_claim_and_policy,
};

/// What a test replay runs with. Each case runs as a run of its own, in a
/// kernel of its own, built from these options once the case is isolated.
#[derive(Clone, Debug)]
pub struct TestOptions {
    pub env: BTreeMap<String, String>,
    pub receipt_signature: RuntimeReceiptSignatureConfig,
    pub effects: RuntimeEffectRegistry,
    pub clock: RunClock,
}

impl TestOptions {
    /// A replay at the test's fixed instant, sealed under the public
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
        fixture: &TestFixture,
        target: &Path,
    ) -> Result<RuntimeOptions, RuntimeError> {
        let max_concurrency =
            crate::execution::runner::host_max_concurrency(&self.env).map_err(|error| {
                RuntimeError::effect_state("reading the host's resource policy", error)
            })?;
        let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(&self.env, target))
            .map_err(|error| {
                RuntimeError::effect_state("opening a test case kernel", error.to_string())
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
            effect_gate: None,
            call_stops: std::sync::Arc::default(),
            packages: None,
            chain: Vec::new(),
            reads: crate::execution::runner::RunReads::default(),
            workspace: None,
            max_concurrency,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TestReplayOutput {
    pub fixture: TestFixture,
    pub status: TestExpectedStatus,
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
pub enum TestReplayError {
    #[error(transparent)]
    Fixture(#[from] TestFixtureError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("test fixture target {target} has no parent directory")]
    TargetWithoutParent { target: PathBuf },
    #[error("test expectation mismatch at {field}: expected {expected}, actual {actual}")]
    Mismatch {
        field: String,
        expected: String,
        actual: String,
    },
    // Boxed: the seven diagnostic strings would otherwise dominate the size of
    // every Result carrying this enum (clippy::result_large_err).
    #[error(transparent)]
    ExpectationFailed(Box<TestExpectationFailure>),
    #[error("receipt digest failed: {message}")]
    ReceiptDigest { message: String },
    #[error("receipt proof failed for {receipt_id}: {findings}")]
    ReceiptProofInvalid {
        receipt_id: String,
        findings: String,
    },
    #[error("invalid test replay metadata at {field}: {message}")]
    InvalidReplayMetadata { field: String, message: String },
    #[error("invalid test fixture environment {name}: {message}")]
    InvalidFixtureEnvironment { name: String, message: String },
    #[error("setup.authority isn't an authority this run can be given: {message}")]
    InvalidSetupAuthority { message: String },
    #[error("test setup receipt path escaped its skill package: {path}")]
    SetupReceiptPathEscape { path: PathBuf },
    #[error("failed to read test setup receipt {path}: {source}")]
    SetupReceiptRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid test setup receipt {path}: {source}")]
    SetupReceiptInvalid {
        path: PathBuf,
        source: runx_receipts::ReceiptDecodeError,
    },
    #[error(transparent)]
    ReceiptStore(#[from] crate::receipts::store::ReceiptStoreError),
}

/// The run's authority `fixture` states as `setup.authority`, in connector
/// and package names as a host states one, with each delegation's connector
/// resolved to the instance that stands for it at `skill_dir`, as the root's
/// admission resolves any run's. None when it states none.
fn stated_authority(
    fixture: &TestFixture,
    skill_dir: &Path,
    env: &BTreeMap<String, String>,
) -> Result<Option<crate::RunAuthority>, TestReplayError> {
    let Some(authority) = &fixture.setup.authority else {
        return Ok(None);
    };
    let invalid = |message: String| TestReplayError::InvalidSetupAuthority { message };
    let authority = serde_json::to_value(authority)
        .and_then(serde_json::from_value::<crate::RunAuthority>)
        .map_err(|error| invalid(error.to_string()))?;
    crate::kernel::admission::resolved_authority(authority, skill_dir, env, skill_dir)
        .map(Some)
        .map_err(invalid)
}

#[derive(Debug, Error)]
#[error(
    "{message}; receipt={receipt_id}; outcome={outcome}; reason={reason}; skill_value={skill_value}; skill_failure={skill_failure}"
)]
pub struct TestExpectationFailure {
    pub message: String,
    pub receipt_id: String,
    pub outcome: String,
    pub reason: String,
    pub skill_value: String,
    pub skill_failure: String,
}

impl From<crate::execution::skill_front::SkillRunError> for TestReplayError {
    fn from(error: crate::execution::skill_front::SkillRunError) -> Self {
        use crate::execution::skill_front::SkillRunError;
        match error {
            SkillRunError::Runtime(error) => TestReplayError::Runtime(error),
            SkillRunError::PreflightRefused { source, .. } => TestReplayError::Runtime(*source),
            other => TestReplayError::Runtime(RuntimeError::ReceiptInvalid {
                message: other.to_string(),
            }),
        }
    }
}

pub fn run_test_fixture(
    fixture_path: impl AsRef<Path>,
    env: BTreeMap<String, String>,
) -> Result<TestReplayOutput, TestReplayError> {
    run_test_fixture_with_adapter(
        fixture_path,
        crate::execution::skill_front::ExecutableAdapter,
        TestOptions::new(env)?,
    )
}

pub fn run_test_fixture_with_adapter<A>(
    fixture_path: impl AsRef<Path>,
    adapter: A,
    options: TestOptions,
) -> Result<TestReplayOutput, TestReplayError>
where
    A: SkillAdapter,
{
    let (fixture, target_path) = load_test_case(fixture_path.as_ref())?;
    run_test_case_with_adapter(fixture, target_path, adapter, options)
}

/// A test file's case and the package it runs: the package whose `tests/`
/// holds it, or the fixture package under that package's `fixtures/` it
/// names.
pub(crate) fn load_test_case(
    fixture_path: &Path,
) -> Result<(TestFixture, PathBuf), TestReplayError> {
    let fixture = load_test_fixture(fixture_path)?;
    let target_path = resolve_target_path(fixture_path, fixture.package.as_deref())?;
    Ok((fixture, target_path))
}

/// Replay a case through the tools `runx test` runs, without asserting
/// its expectations: for a generator that rewrites the expectations this run
/// produces, so they are what `runx test` seals. Every other caller
/// asserts them through [`run_test_fixture`].
pub fn replay_test_fixture(
    fixture_path: impl AsRef<Path>,
    env: BTreeMap<String, String>,
) -> Result<TestReplayOutput, TestReplayError> {
    let (fixture, target_path) = load_test_case(fixture_path.as_ref())?;
    replay_test_case(
        fixture,
        target_path,
        crate::execution::skill_front::ExecutableAdapter,
        TestOptions::new(env)?,
    )
}

/// Run one case, a fixture file's or a method's inline case, against the
/// skill or workflow at `target_path`, and assert what it expects. Both kinds
/// run here, through one engine.
pub(crate) fn run_test_case_with_adapter<A>(
    fixture: TestFixture,
    target_path: PathBuf,
    adapter: A,
    options: TestOptions,
) -> Result<TestReplayOutput, TestReplayError>
where
    A: SkillAdapter,
{
    let receipt_signature = options.receipt_signature.clone();
    let output = replay_test_case(fixture, target_path, adapter, options)?;
    assert_expectations(&output, receipt_signature.signature_policy())
        .map_err(|error| expectation_error_with_output(error, &output))?;
    Ok(output)
}

fn replay_test_case<A>(
    mut fixture: TestFixture,
    target_path: PathBuf,
    adapter: A,
    options: TestOptions,
) -> Result<TestReplayOutput, TestReplayError>
where
    A: SkillAdapter,
{
    // Each case runs in a home and a project directory of its own, held
    // until the case is done: its profiles, instances, custody and usage
    // never reach the operator or another case.
    let case_root = tempfile::Builder::new()
        .prefix("runx-test-case-")
        .tempdir()
        .map_err(|error| RuntimeError::effect_state("isolating a test case", error.to_string()))?;
    crate::paths::isolate(&mut fixture.env, case_root.path());
    stage_installed_connectors_for_case(&options.env, &fixture.env, &target_path)?;
    stage_installed_skills_for_case(&options.env, &fixture.env, &target_path)?;
    // The host's workspace policy is the host's: a case's environment never
    // sets it, so a publisher's test cannot lift a refusal the host made.
    let policy = crate::execution_environment::RUNX_HOSTED_WORKSPACE_POLICY_JSON_ENV;
    match options.env.get(policy) {
        Some(host) => fixture.env.insert(policy.to_owned(), host.clone()),
        None => fixture.env.remove(policy),
    };
    let mut options = options;
    let exchanges = wire_operations(&mut fixture, &target_path, &mut options)?;
    seed_test_receipts(&fixture, &target_path, &options)?;
    let replayed = run_skill_fixture(&fixture, target_path, adapter, options);
    // However the case ends, even in a refusal a front accepts as expected,
    // it proves only the requests its run made.
    let unrequested = exchanges.unrequested().map_err(|error| {
        RuntimeError::effect_state("reading the test exchanges", error.to_string())
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

/// A skill case has a private project for its instances and receipts. Stage
/// only the pinned installed connector versions its package binds into that
/// project before resolving its sessions; no mutable state crosses the edge.
fn stage_installed_connectors_for_case(
    source_env: &BTreeMap<String, String>,
    case_env: &BTreeMap<String, String>,
    target: &Path,
) -> Result<(), TestReplayError> {
    let source_paths = crate::paths::RunxPaths::resolve(source_env, target);
    let case_paths = crate::paths::RunxPaths::resolve(case_env, target);
    let loaded = crate::load_validated_skill_package(target)?;
    for connector in super::package::bound_installed_connectors(&loaded, &source_paths)? {
        if let crate::kernel::connectors::ConnectorOrigin::Installed { directory } =
            &connector.origin
        {
            crate::registry::write_connector_version(
                &case_paths.added_connectors(),
                &connector,
                Some(directory),
            )
            .map_err(|error| RuntimeError::effect_state("staging test connectors", error))?;
        }
    }
    Ok(())
}

/// A case runs in a project of its own, and execution reads registry skills
/// only from the project a run is in: the case's project holds the installed
/// registry skills of the project its test runs in. A package's test
/// project holds exactly its package's closure.
fn stage_installed_skills_for_case(
    source_env: &BTreeMap<String, String>,
    case_env: &BTreeMap<String, String>,
    target: &Path,
) -> Result<(), TestReplayError> {
    let source = crate::paths::RunxPaths::resolve(source_env, target);
    let case = crate::paths::RunxPaths::resolve(case_env, target);
    if source.project() != case.project() {
        crate::registry::copy_installed_skills(&source, &case)
            .map_err(|error| RuntimeError::effect_state("staging test skills", error))?;
    }
    Ok(())
}

fn seed_test_receipts(
    fixture: &TestFixture,
    target_path: &Path,
    options: &TestOptions,
) -> Result<(), TestReplayError> {
    if fixture.setup.receipts.is_empty() {
        return Ok(());
    }

    // A test's receipts are its own package's, whichever package it runs.
    let loaded = crate::load_validated_skill_package(target_path)?;
    let tests_package = if fixture.package.is_some() {
        loaded
            .package_root
            .parent()
            .and_then(crate::skill_package::find_owning_package_root)
            .unwrap_or_else(|| loaded.package_root.clone())
    } else {
        loaded.package_root.clone()
    };
    let package_root =
        fs::canonicalize(&tests_package).map_err(|source| TestReplayError::SetupReceiptRead {
            path: tests_package.clone(),
            source,
        })?;
    let mut receipts = Vec::with_capacity(fixture.setup.receipts.len());
    for relative in &fixture.setup.receipts {
        let candidate = package_root.join(relative);
        let path =
            fs::canonicalize(&candidate).map_err(|source| TestReplayError::SetupReceiptRead {
                path: candidate.clone(),
                source,
            })?;
        if !path.starts_with(&package_root) {
            return Err(TestReplayError::SetupReceiptPathEscape { path });
        }
        let contents = fs::read(&path).map_err(|source| TestReplayError::SetupReceiptRead {
            path: path.clone(),
            source,
        })?;
        receipts.push(runx_receipts::decode_receipt(&contents).map_err(|source| {
            TestReplayError::SetupReceiptInvalid {
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
    fixture: &mut TestFixture,
    target_path: &Path,
    options: &mut TestOptions,
) -> Result<Arc<TestExchanges>, TestReplayError> {
    // Connectors are reached from the package's directory: from above it, a
    // bundled package is outside its own plugin.
    let skill_dir = target_path;
    let scope = crate::ConnectorScope::in_project(
        skill_dir,
        &crate::paths::RunxPaths::resolve(&options.env, skill_dir),
    )
    .map_err(|error| RuntimeError::effect_state("resolving test connector scope", error))?;
    let invalid_provider = |message: String| {
        TestFixtureError::Parser(runx_parser::test_fixture::TestFixtureError::Invalid {
            field: "providers.hosted.grant".to_owned(),
            message,
        })
    };
    let mut env = options.env.clone();
    env.extend(fixture.env.clone());
    resolve_fixture_path(&mut env, skill_dir)?;
    let closure = granted_closure(fixture, target_path, &env)?;
    let transport =
        super::transport::test_transport(&fixture.providers, &scope, &closure, skill_dir)
            .map_err(invalid_provider)?;
    options.effects =
        super::transport::effects_over(&options.effects, transport.clone(), &fixture.providers)
            .map_err(|error| RuntimeError::effect_state("wiring the test transport", error))?;
    let provider_env = super::transport::provider_env(&fixture.providers);
    fixture.env.extend(provider_env.clone());
    env.extend(provider_env);
    super::transport::seed_provider_grant(&fixture.providers, &scope, &closure, &env, skill_dir)
        .map_err(invalid_provider)?;
    seed_test_instances(
        fixture,
        target_path,
        skill_dir,
        options,
        &transport,
        &closure,
    )?;
    Ok(transport.exchanges)
}

/// Every connector the case's run binds through its whole closure, for its
/// provider grant to reach: walked in the case's environment, so its registry
/// children are materialized where the run finds them. None without a grant,
/// or for a workflow in no package.
fn granted_closure(
    fixture: &TestFixture,
    target_path: &Path,
    env: &BTreeMap<String, String>,
) -> Result<Vec<Arc<crate::kernel::connectors::LoadedConnector>>, TestReplayError> {
    if fixture.providers.hosted.is_none() {
        return Ok(Vec::new());
    }
    let unbound = |error: crate::skill_package::SkillInspectionError| {
        RuntimeError::effect_state("binding the case's closure", error.to_string())
    };
    let loaded = crate::load_validated_skill_package(target_path)?;
    let manifest = loaded
        .manifest()
        .ok_or_else(|| RuntimeError::UnsupportedMethodSelection {
            method: fixture
                .method
                .clone()
                .unwrap_or_else(|| "default".to_owned()),
        })?;
    let method = select_test_method(manifest, fixture.method.as_deref())?
        .name
        .clone();
    crate::skill_package::case_closure_connectors(
        &loaded,
        crate::skill_package::CaseClosureTarget::Method(&method),
        env,
    )
    .map_err(|error| unbound(error).into())
}

/// Confirm and select the instances the case runs under, in the case's own
/// kernel and home, with the fixture environment resolved as the run
/// resolves it: each session as `runx connector use <connector> --session`
/// would, and each credential bound to the variable the case sets, as
/// `runx connector use <connector>` would with that variable set. They are
/// confirmed from `skill_dir`, the directory of a workflow case's file.
fn seed_test_instances(
    fixture: &TestFixture,
    target_path: &Path,
    skill_dir: &Path,
    options: &TestOptions,
    transport: &super::provider_responses::TestTransport,
    closure: &[std::sync::Arc<crate::kernel::connectors::LoadedConnector>],
) -> Result<(), TestReplayError> {
    let setup = &fixture.setup;
    if setup.sessions.is_empty() && setup.credentials.is_empty() {
        return Ok(());
    }
    let failed = |error: String| RuntimeError::effect_state("setting up a test instance", error);
    let mut env = options.env.clone();
    env.extend(fixture.env.clone());
    resolve_fixture_path(&mut env, skill_dir)?;
    let paths = crate::paths::RunxPaths::resolve(&env, skill_dir);
    let workspace = crate::services::WorkspaceEnv::new(env.clone(), skill_dir.to_path_buf())
        .map_err(|error| failed(error.to_string()))?;
    let instances = crate::Instances::new(
        crate::KernelStore::for_project(&paths).map_err(|error| failed(error.to_string()))?,
    );
    let scope = crate::ConnectorScope::in_project(skill_dir, &paths)
        .map_err(|error| failed(error.to_string()))?;
    // A case names a connector as its skill's `connectors:` block keys it, a
    // connector id or a qualified ref, and it resolves as the skill's
    // operations resolve it, pinned manifest included.
    let requirements = crate::skill_package::load_validated_skill_package(target_path)?
        .manifest()
        .map(|manifest| manifest.connectors.clone())
        .unwrap_or_default();
    let connector = |key: &str| {
        match requirements
            .get(key)
            .filter(|requirement| requirement.choose.is_none())
        {
            Some(requirement) => scope
                .requirement(key, requirement)
                .map_err(|error| error.to_string()),
            // Reached as the case's calls reach it: from a bundle's package
            // that carries it too, as a published fixture package's are.
            None => super::transport::case_connector(&scope, closure, skill_dir, key),
        }
        .map_err(failed)
    };
    // A fixture's sessions read their accounts as its calls do: through its
    // transport and its recorded process calls.
    let readers = crate::kernel::setup::AccountReaders {
        transport,
        tools: &crate::drivers::process::StandIn {
            calls: &fixture.providers.processes,
        },
    };
    for key in &setup.sessions {
        let connector = connector(key)?;
        crate::kernel::setup::confirm_session_over(
            &instances,
            &connector,
            &[],
            setup.parameters.get(key).cloned().unwrap_or_default(),
            &readers,
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
            .filter(|(_, declaration)| {
                declaration
                    .env()
                    .is_some_and(|env| fixture.env.contains_key(env.as_str()))
            })
            .map(|(name, _)| name);
        let (Some(name), None) = (set.next(), set.next()) else {
            return Err(failed(format!(
                "the case sets the variable of exactly one of connector {}'s credentials",
                connector.name
            ))
            .into());
        };
        crate::kernel::setup::bind_environment(
            &instances,
            &connector,
            name,
            &[],
            setup.parameters.get(key).cloned().unwrap_or_default(),
            &workspace,
        )
        .map_err(failed)?;
    }
    Ok(())
}

fn expectation_error_with_output(
    error: TestReplayError,
    output: &TestReplayOutput,
) -> TestReplayError {
    TestReplayError::ExpectationFailed(Box::new(TestExpectationFailure {
        message: error.to_string(),
        receipt_id: output.receipt.id.to_string(),
        outcome: format!("{:?}", output.receipt.seal.outcome),
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
    fixture: &TestFixture,
    skill_dir: PathBuf,
    adapter: A,
    options: TestOptions,
) -> Result<TestReplayOutput, TestReplayError>
where
    A: SkillAdapter,
{
    let target = skill_fixture_target(fixture, skill_dir, &options)?;
    match crate::input_contract::materialize_complete_method_inputs(
        &target.method.inputs,
        &target.method.input_constraints,
        &fixture.inputs,
    ) {
        Ok(inputs) => run_workflow_skill_fixture(fixture, target, inputs, adapter, options),
        Err(error) => seal_admission_failure(
            fixture,
            &target.method.name,
            &error.into_runtime_error(),
            &options,
        ),
    }
}

/// A case whose inputs the method's contract refuses seals the refusal
/// without running any step.
fn seal_admission_failure(
    fixture: &TestFixture,
    method_name: &str,
    error: &RuntimeError,
    options: &TestOptions,
) -> Result<TestReplayOutput, TestReplayError> {
    let output = InvocationOutput::runtime_failure(
        JsonValue::Object(error.public_failure_projection()),
        error.to_string(),
        0,
    );
    let projection = project_step_claim(JsonObject::new(), None);
    let run_id = test_run_id(&fixture.name);
    let receipt = step_receipt_with_declared_claim_and_policy(
        StepReceiptWithOutcome {
            scope: RunScope {
                run_id: &run_id,
                workflow_name: &fixture.name,
            },
            step_id: method_name,
            attempt: 1,
            output: &output,
            created_at: &options.clock.now(),
            outcome: RunOutcome::Failed,
            reason: Some(runx_contracts::SealReason::InputContractInvalid),
        },
        &projection.outputs,
        options.signature_policy(),
    )?;
    Ok(TestReplayOutput {
        fixture: fixture.clone(),
        status: status_from_outcome(&receipt.seal.outcome),
        receipt,
        step_receipts: Vec::new(),
        steps: Vec::new(),
        skill_output: Some(output),
        replayed_answers: JsonObject::new(),
        unanswered: Vec::new(),
        unrequested: Vec::new(),
    })
}

// Function rationale: the fixture workflow turn keeps materialize,
// run, and act-receipt minting in one path so it seals under the same instant and
// signature policy as the production and inline fronts.
fn run_workflow_skill_fixture<A>(
    fixture: &TestFixture,
    target: SkillFixtureTarget,
    inputs: JsonObject,
    adapter: A,
    mut options: TestOptions,
) -> Result<TestReplayOutput, TestReplayError>
where
    A: SkillAdapter,
{
    overlay_test_env(&mut options, &target.env);
    let run_id = options
        .env
        .entry(crate::execution::runner::RUNX_RUN_ID_ENV.to_owned())
        .or_insert_with(|| test_run_id(&fixture.name))
        .clone();
    let workflow = bind_run_inputs(
        target.method.workflow.clone(),
        &inputs,
        fixture.host.act.as_ref(),
        &run_id,
        None,
    )?;
    let mut options = options.runtime(&run_id, fixture, &target.skill_dir)?;
    // The run holds the authority the test states, as a host gives a run
    // one, and its method's effects and reach, as any run of it does.
    options.run_authority = crate::RunAuthority::of_method(
        stated_authority(fixture, &target.skill_dir, &options.env)?,
        &target.method.workflow,
    );
    let runtime = Runtime::new(adapter, options);
    let mut host = FixtureHost::new(fixture);
    let workflow_run = runtime.run_workflow_for_test(&target.skill_dir, workflow, &mut host)?;
    let (replayed_answers, unanswered) = host.finish();
    let mut output =
        replay_output_from_workflow(fixture, workflow_run, replayed_answers, unanswered);
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
                            .unwrap_or_else(|| "workflow test step failed".to_owned()),
                        0,
                    )
                }
            });
    }
    Ok(output)
}

/// The method a skill case runs and what it runs with.
struct SkillFixtureTarget {
    method: SkillMethodDefinition,
    env: BTreeMap<String, String>,
    skill_dir: PathBuf,
}

fn skill_fixture_target(
    fixture: &TestFixture,
    skill_dir: PathBuf,
    options: &TestOptions,
) -> Result<SkillFixtureTarget, TestReplayError> {
    let loaded = crate::load_validated_skill_package(&skill_dir)?;
    let manifest = loaded
        .manifest()
        .ok_or_else(|| RuntimeError::UnsupportedMethodSelection {
            method: fixture
                .method
                .clone()
                .unwrap_or_else(|| "default".to_owned()),
        })?;
    let method = select_test_method(manifest, fixture.method.as_deref())?.clone();
    let mut env = options.env.clone();
    let credentials = super::credential_variables(&skill_dir, &env)?;
    super::isolate_test_environment(&mut env, &credentials);
    env.extend(fixture.env.clone());
    resolve_fixture_path(&mut env, &skill_dir)?;
    crate::services::merge_inferred_tool_roots(&mut env, &skill_dir);
    Ok(SkillFixtureTarget {
        method,
        env,
        skill_dir,
    })
}

fn select_test_method<'a>(
    manifest: &'a SkillMethodManifest,
    requested_method: Option<&str>,
) -> Result<&'a SkillMethodDefinition, TestReplayError> {
    if let Some(method) = requested_method {
        return manifest.methods.get(method).ok_or_else(|| {
            RuntimeError::UnsupportedMethodSelection {
                method: method.to_owned(),
            }
            .into()
        });
    }
    manifest.default_method().ok_or_else(|| {
        RuntimeError::UnsupportedMethodSelection {
            method: "default".to_owned(),
        }
        .into()
    })
}

struct FixtureHost<'a> {
    fixture: &'a TestFixture,
    /// The outputs scripted turns end with, by address.
    worked: JsonObject,
    replayed_answers: JsonObject,
    unanswered: Vec<String>,
}

impl<'a> FixtureHost<'a> {
    fn new(fixture: &'a TestFixture) -> Self {
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
                    .host
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
            ResolutionRequest::Turn { id, .. } => {
                let address = id.as_str();
                let answers = &self.fixture.host.answers;
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
            // A purchase is bought from runx, which a case never reaches:
            // a case tests the method it sells from its own source.
            ResolutionRequest::Input { id, .. }
            | ResolutionRequest::Retry { id, .. }
            | ResolutionRequest::Purchase { id, .. } => {
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
        let answered = fixture.host.answers.contains_key(&address);
        if let Some(output) =
            super::turns::work_scripted_turn(turn, self, &fixture.host.turns, answered)?
        {
            self.worked.insert(address, output);
        }
        Ok(())
    }
}

fn replay_output_from_workflow(
    fixture: &TestFixture,
    workflow_run: WorkflowRun,
    replayed_answers: JsonObject,
    unanswered: Vec<String>,
) -> TestReplayOutput {
    let result = crate::execution::runner::workflow_terminal_result(&workflow_run).ok();
    let skill_output = result.map(|result| {
        if workflow_run.progress.state == runx_core::state_machine::WorkflowState::Succeeded {
            InvocationOutput::runtime_success(result, 0)
        } else {
            InvocationOutput::runtime_failure(
                result,
                crate::execution::runner::workflow_failure(&workflow_run),
                0,
            )
        }
    });
    let step_receipts = workflow_run
        .steps
        .iter()
        .map(|step| step.receipt.clone())
        .collect::<Vec<_>>();
    TestReplayOutput {
        fixture: fixture.clone(),
        status: status_from_outcome(&workflow_run.receipt.seal.outcome),
        receipt: workflow_run.receipt,
        step_receipts,
        steps: workflow_run.steps,
        skill_output,
        replayed_answers,
        unanswered,
        unrequested: Vec::new(),
    }
}

fn resolve_target_path(
    fixture_path: &Path,
    package: Option<&str>,
) -> Result<PathBuf, TestReplayError> {
    // `<package>/tests/<name>.yaml`
    let Some(own) = fixture_path.parent().and_then(Path::parent) else {
        return Err(TestReplayError::TargetWithoutParent {
            target: fixture_path.to_path_buf(),
        });
    };
    let unresolved = match package {
        Some(package) => own.join(package),
        None => own.to_path_buf(),
    };
    fs::canonicalize(&unresolved).map_err(|source| {
        RuntimeError::io(
            format!("resolving the test's package {}", unresolved.display()),
            source,
        )
        .into()
    })
}

fn overlay_test_env(options: &mut TestOptions, env: &BTreeMap<String, String>) {
    options.env.extend(env.clone());
}

/// A test's run id: fixed by its name, so its receipts reproduce.
fn test_run_id(name: &str) -> String {
    format!("test-{name}")
}

fn resolve_fixture_path(
    env: &mut BTreeMap<String, String>,
    skill_dir: &Path,
) -> Result<(), TestReplayError> {
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
            TestReplayError::InvalidFixtureEnvironment {
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
    use super::{
        TestOptions, overlay_test_env, resolve_fixture_path, stage_installed_connectors_for_case,
    };
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    #[test]
    fn installed_connector_is_staged_into_each_case_project()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let publisher = workspace.path().join("publisher");
        fs::create_dir_all(&publisher)?;
        fs::write(publisher.join("plugin.json"), r#"{"namespace":"acme"}"#)?;
        let connector_dir = publisher.join("connectors/ledger");
        fs::create_dir_all(&connector_dir)?;
        fs::write(
            connector_dir.join("connector.yaml"),
            r#"schema: runx.connector.v1
name: ledger
title: Ledger
description: Read a public ledger.
category: agent-tools
auth:
  credentials:
    service_token:
      kind: api_key
      fields:
        - {name: token, label: Acme service token}
      delivery: {env: ACME_SERVICE_TOKEN}
      audience: [api.example.com]
placements:
  hosted: {auth: credential, credential: service_token}
  local: {auth: [credentials]}
operations:
  entry.read:
    description: Read one ledger entry.
    verb: read
    scopes: [entry.read]
    credentials: [service_token]
    target: {template: "acme:ledger"}
    result_fields: [entry]
"#,
        )?;
        let connector = crate::ConnectorScope::at(&publisher)?.named("ledger")?;
        let source_project = workspace.path().join("source-project");
        let installed_source = crate::registry::write_connector_version(
            &source_project.join("added-connectors"),
            &connector,
            Some(&connector_dir),
        )?;
        let version = installed_source
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or("installed connector has no version")?;
        let case_project = workspace.path().join("case-project");
        let skill_dir = workspace.path().join("skills/ledger-read");
        fs::create_dir_all(&skill_dir)?;
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: ledger-read\ndescription: Read a ledger entry.\n---\n# Ledger read\n",
        )?;
        fs::write(
            skill_dir.join("run.yaml"),
            format!(
                "version: \"0.1.0\"\nminimum_runx: 0.12.0\nconnectors:\n  ledger:\n    manifest_digest: {}\nmethods:\n  inspect:\n    steps:\n      - id: read\n        operation: ledger/entry.read\n        inputs: {{}}\n",
                connector.manifest_digest
            ),
        )?;
        let broken = source_project.join("added-connectors/broken/sha-bad");
        fs::create_dir_all(&broken)?;
        fs::write(broken.join("connector.yaml"), "not: [a connector")?;
        let source_env = BTreeMap::from([
            (
                "RUNX_CWD".to_owned(),
                workspace.path().to_string_lossy().into_owned(),
            ),
            (
                "RUNX_PROJECT_DIR".to_owned(),
                source_project.to_string_lossy().into_owned(),
            ),
        ]);
        let case_env = BTreeMap::from([
            (
                "RUNX_CWD".to_owned(),
                workspace.path().to_string_lossy().into_owned(),
            ),
            (
                "RUNX_PROJECT_DIR".to_owned(),
                case_project.to_string_lossy().into_owned(),
            ),
        ]);

        let tests = skill_dir.join("tests");
        fs::create_dir_all(&tests)?;
        fs::write(
            tests.join("binds-installed.yaml"),
            "name: binds-installed\nmethod: inspect\nsetup:\n  credentials: [ledger]\nenv:\n  ACME_SERVICE_TOKEN: fixture-token\nexpect:\n  status: policy_denied\n",
        )?;
        let report = super::super::package::run_package_test_with_effects(
            &skill_dir,
            None,
            &source_env,
            &crate::RuntimeEffectRegistry::default(),
        )?;
        assert_eq!(report.status, "passed", "{report:?}");
        assert_eq!(report.case_names, ["binds-installed"]);

        stage_installed_connectors_for_case(&source_env, &case_env, &skill_dir)?;

        let scope = crate::ConnectorScope::in_project(
            workspace.path(),
            &crate::paths::RunxPaths::resolve(&case_env, workspace.path()),
        )?;
        let installed = scope.named(&format!("ledger@{version}"))?;
        assert_eq!(installed.manifest_digest, connector.manifest_digest);
        assert!(
            case_project
                .join("added-connectors/ledger")
                .join(version)
                .join("connector.yaml")
                .is_file()
        );
        assert!(!case_project.join("added-connectors/broken").exists());
        Ok(())
    }

    #[test]
    fn overlay_test_env_preserves_operator_env_and_allows_fixture_override()
    -> Result<(), crate::RuntimeError> {
        let mut options = TestOptions::new(BTreeMap::from([
            ("OPERATOR_ENV".to_owned(), "preserved".to_owned()),
            ("FIXTURE_OVERRIDE".to_owned(), "operator".to_owned()),
        ]))?;
        let fixture_env = BTreeMap::from([
            ("FIXTURE_OVERRIDE".to_owned(), "fixture".to_owned()),
            ("FIXTURE_ONLY".to_owned(), "fixture".to_owned()),
        ]);

        overlay_test_env(&mut options, &fixture_env);

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

        resolve_fixture_path(&mut env, Path::new("/workspace/skills/github"))?;

        let paths = std::env::split_paths(env.get("PATH").ok_or("PATH was not retained")?)
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                Path::new("/workspace/skills/github/fixtures/bin").to_path_buf(),
                Path::new("/usr/bin").to_path_buf(),
            ]
        );
        assert_eq!(
            env.get("RUNX_CWD"),
            Some(&"/workspace/skills/github/../..".to_owned())
        );
        Ok(())
    }
}
