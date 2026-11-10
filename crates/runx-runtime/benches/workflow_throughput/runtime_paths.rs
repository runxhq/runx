use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use criterion::{BatchSize, Criterion};
use runx_contracts::{
    ExecutionEvent, JsonNumber, JsonObject, JsonValue, ProofKind, ResolutionRequest,
    ResolutionResponse, ResolutionResponseActor, RunOutcome,
};
use runx_parser::{
    PROFILE_FILE, WorkerTool, Workflow, parse_method_manifest_yaml, validate_method_manifest,
};
use runx_runtime::adapters::javascript::JavaScriptAdapter;
use runx_runtime::{
    ConnectorScope, CredentialDelivery, Executable, HOSTED_API_BASE_URL_ENV, HOSTED_API_TOKEN_ENV,
    Host, HttpMethod, Instances, KernelStore, LoadedConnector, NoopHost, OperationEffect,
    RUNX_HOME_ENV, RUNX_RECEIPT_DIR_ENV, RunLog, Runtime, RuntimeEffectRegistry, RuntimeHttpError,
    RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport, RuntimeOptions, RunxPaths,
    SkillAdapter, SkillInvocation,
};
use tempfile::TempDir;

use super::{record_resource_metrics, session_metric};

#[path = "runtime_paths/composition.rs"]
mod composition;

const CREATED_AT: &str = "2026-07-20T00:00:00Z";
const BENCHMARK_GRANT: &str = "grant_runtime_perf";
const LARGE_INPUT_BYTES: usize = 4 * 1024 * 1024 - 64;

#[allow(clippy::expect_used)]
pub(super) fn register(c: &mut Criterion) {
    let fixtures = RuntimePathFixtures::new().expect("runtime-path benchmark fixtures must load");
    let mut resources = fixtures
        .smoke()
        .expect("runtime-path benchmark fixtures must execute successfully");
    // performance.md's journeys 2 and 3, by count, on the runtimes the smoke
    // warmed.
    resources.insert(
        "journeys".to_owned(),
        JsonValue::Object(JsonObject::from([
            (
                "2".to_owned(),
                fixtures
                    .warm_execution()
                    .expect("the warm-execution journey must run each of its calls"),
            ),
            (
                "3".to_owned(),
                fixtures
                    .composition()
                    .expect("the composition journey must settle each of its calls"),
            ),
        ])),
    );
    record_resource_metrics(&resources)
        .expect("runtime-path benchmark resource metrics must be recorded");

    c.bench_function("native_capability_dispatch", |b| {
        b.iter_batched(
            || fixtures.native_workflow.clone(),
            |workflow| {
                run_workflow(
                    &fixtures.next_run(&fixtures.context_runtime),
                    fixtures.root(),
                    workflow,
                )
            },
            BatchSize::SmallInput,
        )
    });

    c.bench_function("workflow_context_to_module", |b| {
        b.iter_batched(
            || fixtures.context_workflow.clone(),
            |workflow| {
                run_workflow(
                    &fixtures.next_run(&fixtures.context_runtime),
                    fixtures.root(),
                    workflow,
                )
            },
            BatchSize::SmallInput,
        )
    });

    c.bench_function("pure_module_cold_start", |b| {
        b.iter_batched(
            || {
                (
                    JavaScriptAdapter::new_session(),
                    fixtures.module_invocation.clone(),
                )
            },
            |(adapter, invocation)| adapter.invoke(black_box(invocation)),
            BatchSize::SmallInput,
        )
    });

    c.bench_function("pure_module_session_reuse", |b| {
        b.iter_batched(
            || fixtures.module_invocation.clone(),
            |invocation| fixtures.module_adapter.invoke(black_box(invocation)),
            BatchSize::SmallInput,
        )
    });

    c.bench_function("pure_module_large_input", |b| {
        b.iter_batched(
            || fixtures.large_module_invocation.clone(),
            |invocation| fixtures.module_adapter.invoke(black_box(invocation)),
            BatchSize::PerIteration,
        )
    });

    c.bench_function("provider_effect_finality", |b| {
        b.iter_batched(
            || fixtures.provider_workflow.clone(),
            |workflow| {
                run_provider_workflow(
                    &fixtures.next_run(&fixtures.provider_runtime),
                    &fixtures.provider_transport,
                    fixtures.root(),
                    workflow,
                )
                .expect("production provider-effect benchmark sample must reach sealed finality")
            },
            BatchSize::SmallInput,
        )
    });
}

struct RuntimePathFixtures {
    directory: TempDir,
    store: Arc<KernelStore>,
    runs: AtomicU64,
    native_workflow: Workflow,
    context_runtime: Runtime<JavaScriptAdapter>,
    context_workflow: Workflow,
    module_invocation: SkillInvocation,
    large_module_invocation: SkillInvocation,
    module_adapter: JavaScriptAdapter,
    provider_runtime: Runtime<JavaScriptAdapter>,
    /// What the provider runtime runs under, for a run that states its own
    /// authority.
    provider_options: RuntimeOptions,
    provider_workflow: Workflow,
    provider_transport: ProviderBenchmarkTransport,
}

/// Makes `root` a plugin holding a copy of the kernel fixture plugin's chat
/// connector, as a project that installs it would reach it.
fn plugin_with_fixture_chat(root: &Path) -> Result<(), Box<dyn Error>> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/plugins/kernel/connectors/fixture/chat");
    let target = root.join("connectors/fixture/chat");
    fs::create_dir_all(&target)?;
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        if entry.path().is_file() {
            fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
    }
    Ok(())
}

impl RuntimePathFixtures {
    fn new() -> Result<Self, Box<dyn Error>> {
        let directory = TempDir::new()?;
        write_modules(directory.path())?;
        plugin_with_fixture_chat(directory.path())?;
        let store = KernelStore::open(&directory.path().join(".runx"))?;
        let evaluation = |name: &str| {
            RunLog::evaluation(
                Arc::clone(&store),
                name,
                "workflow-throughput",
                name,
                CREATED_AT,
            )
        };

        let native_workflow = method_workflow("runtime-native-dispatch", NATIVE_WORKFLOW)?;
        let context_runtime = Runtime::new(
            JavaScriptAdapter::default(),
            runtime_options(evaluation("run_context_smoke")?),
        );
        let context_workflow = method_workflow("runtime-context-to-module", CONTEXT_WORKFLOW)?;
        let module_invocation = javascript_invocation(
            directory.path(),
            JsonObject::from([("payload".to_owned(), JsonValue::String("runx".to_owned()))]),
        );
        let large_module_invocation = javascript_invocation(
            directory.path(),
            JsonObject::from([(
                "payload".to_owned(),
                JsonValue::String("x".repeat(LARGE_INPUT_BYTES)),
            )]),
        );
        let module_adapter = JavaScriptAdapter::new_session();

        let chat = ConnectorScope::at(directory.path())?.connector("fixture/chat")?;
        let provider_transport = ProviderBenchmarkTransport::for_grant(&chat)?;
        let effects = RuntimeEffectRegistry::with_effect(OperationEffect::with_http_transport(
            provider_transport.clone(),
        ))?;
        let mut provider_options = runtime_options(evaluation("run_provider_smoke")?);
        provider_options.effects = effects.clone();
        provider_options.env.insert(
            HOSTED_API_BASE_URL_ENV.to_owned(),
            "https://api.runx.benchmark".to_owned(),
        );
        provider_options
            .env
            .insert(HOSTED_API_TOKEN_ENV.to_owned(), "rxk_benchmark".to_owned());
        // The receipts a provider call writes sit in the project's `.runx`,
        // as a project's do, and never in the package each call reads: there
        // each call would grow the package the next one reads, until it
        // passed the package size limit.
        provider_options.env.insert(
            RUNX_RECEIPT_DIR_ENV.to_owned(),
            directory
                .path()
                .join(".runx/provider-receipts")
                .to_string_lossy()
                .into_owned(),
        );
        provider_options.env.insert(
            RUNX_HOME_ENV.to_owned(),
            directory.path().join("home").to_string_lossy().into_owned(),
        );
        runx_runtime::kernel::setup::confirm_grant_over(
            &Instances::new(KernelStore::for_project(&RunxPaths::resolve(
                &provider_options.env,
                directory.path(),
            ))?),
            &chat,
            Arc::new(provider_transport.clone()),
            BENCHMARK_GRANT,
            &provider_options.env,
            directory.path(),
        )?;
        let provider_runtime = Runtime::new(JavaScriptAdapter::default(), provider_options.clone());
        let provider_workflow = method_workflow("provider-effect", PROVIDER_EFFECT_SKILL)?;

        Ok(Self {
            directory,
            store,
            runs: AtomicU64::new(0),
            native_workflow,
            context_runtime,
            context_workflow,
            module_invocation,
            large_module_invocation,
            module_adapter,
            provider_runtime,
            provider_options,
            provider_workflow,
            provider_transport,
        })
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    /// Each sample is a run of its own on the shared warm runtime services.
    #[allow(clippy::expect_used)]
    fn next_run(&self, runtime: &Runtime<JavaScriptAdapter>) -> Runtime<JavaScriptAdapter> {
        let run_id = format!("run_sample_{}", self.runs.fetch_add(1, Ordering::Relaxed));
        runtime.for_run(
            RunLog::evaluation(
                Arc::clone(&self.store),
                &run_id,
                "workflow-throughput",
                &run_id,
                CREATED_AT,
            )
            .expect("benchmark sample run must start"),
        )
    }

    fn smoke(&self) -> Result<JsonObject, Box<dyn Error>> {
        run_workflow(
            &self.next_run(&self.context_runtime),
            self.root(),
            self.native_workflow.clone(),
        )?;
        let cold = JavaScriptAdapter::new_session();
        require_success(cold.invoke(self.module_invocation.clone())?)?;
        require_success(self.module_adapter.invoke(self.module_invocation.clone())?)?;
        require_success(
            self.module_adapter
                .invoke(self.large_module_invocation.clone())?,
        )?;
        run_workflow(
            &self.context_runtime,
            self.root(),
            self.context_workflow.clone(),
        )?;
        run_provider_workflow(
            &self.provider_runtime,
            &self.provider_transport,
            self.root(),
            self.provider_workflow.clone(),
        )?;
        Ok(JsonObject::from([
            (
                "workflow_context_to_module".to_owned(),
                session_metric(self.context_runtime.javascript_session_stats()),
            ),
            (
                "pure_module_cold_start".to_owned(),
                session_metric(cold.session_stats()),
            ),
            (
                "pure_module_session_reuse".to_owned(),
                session_metric(self.module_adapter.session_stats()),
            ),
            (
                "pure_module_large_input".to_owned(),
                session_metric(self.module_adapter.session_stats()),
            ),
            (
                "provider_effect_finality".to_owned(),
                session_metric(self.provider_runtime.javascript_session_stats()),
            ),
        ]))
    }

    /// Journey 2, warm execution, counted: a run of each size's provider
    /// calls on the warm provider runtime, each call one operation over the
    /// local transport, and what each call past the first cost. A call's
    /// store writes are the run's past the one-call run's, over the calls
    /// past the first, so a constant cost reads the same at every size. A
    /// run is taken whole when its record is replayed or every call is sealed
    /// into its receipt; a call that did either would show here as more than
    /// none per call.
    fn warm_execution(&self) -> Result<JsonValue, Box<dyn Error>> {
        let mut measured = Vec::with_capacity(WARM_EXECUTION_CALLS.len());
        for calls in WARM_EXECUTION_CALLS {
            let workflow = method_workflow("warm-execution", &provider_calls_workflow(calls))?;
            let attempts = self.provider_transport.operation_attempts();
            let before = runx_runtime::work_counts::snapshot();
            let mut host = ProviderBenchmarkHost;
            let run = self
                .next_run(&self.provider_runtime)
                .run_workflow_with_host(self.root(), workflow, &mut host)?;
            let after = runx_runtime::work_counts::snapshot();
            let sent = self.provider_transport.operation_attempts() - attempts;
            let dispatched = after.provider_calls - before.provider_calls;
            let expected = u64::try_from(calls)?;
            if run.steps.len() != calls || sent != expected || dispatched != expected {
                return Err(std::io::Error::other(format!(
                    "a warm-execution run of {calls} calls recorded {} steps, sent {sent} operations and dispatched {dispatched}",
                    run.steps.len()
                ))
                .into());
            }
            measured.push((calls, CallCounts::between(before, after)));
        }
        let (_, base) = measured[0];
        let past_base = &measured[1..];
        let per_call = |calls: usize, total: u64, base: u64| {
            let added = total.saturating_sub(base);
            let past = u64::try_from(calls - 1).unwrap_or(u64::MAX);
            if added.is_multiple_of(past) {
                JsonNumber::U64(added / past)
            } else {
                JsonNumber::F64(added as f64 / past as f64)
            }
        };
        let store_writes_per_call = past_base
            .iter()
            .map(|(calls, counts)| {
                (
                    calls.to_string(),
                    JsonValue::Number(per_call(*calls, counts.store_writes, base.store_writes)),
                )
            })
            .collect::<JsonObject>();
        let context_reserializations_per_call = past_base
            .iter()
            .map(|(calls, counts)| {
                per_call(
                    *calls,
                    counts.run_materializations,
                    base.run_materializations,
                )
            })
            .max_by(|left, right| number_of(left).total_cmp(&number_of(right)))
            .unwrap_or(JsonNumber::U64(0));
        let transactions_spanning_provider_calls = measured
            .iter()
            .map(|(_, counts)| counts.transactions_spanning_provider_calls)
            .sum::<u64>();
        let runs = measured
            .iter()
            .map(|(calls, counts)| (calls.to_string(), counts.json()))
            .collect::<JsonObject>();
        Ok(JsonValue::Object(JsonObject::from([
            (
                "store_writes_per_call".to_owned(),
                JsonValue::Object(store_writes_per_call),
            ),
            (
                "context_reserializations_per_call".to_owned(),
                JsonValue::Number(context_reserializations_per_call),
            ),
            (
                "transactions_spanning_provider_calls".to_owned(),
                JsonValue::Number(JsonNumber::U64(transactions_spanning_provider_calls)),
            ),
            ("runs".to_owned(), JsonValue::Object(runs)),
        ])))
    }
}

/// Journey 2's sizes: a run of one call, the base each larger run is read
/// against, then 10, 100 and 1,000.
const WARM_EXECUTION_CALLS: [usize; 4] = [1, 10, 100, 1000];

/// What one warm-execution run counted.
#[derive(Clone, Copy)]
struct CallCounts {
    store_writes: u64,
    packages_read: u64,
    run_materializations: u64,
    provider_calls: u64,
    transactions_spanning_provider_calls: u64,
}

impl CallCounts {
    fn between(
        before: runx_runtime::work_counts::WorkCounts,
        after: runx_runtime::work_counts::WorkCounts,
    ) -> Self {
        Self {
            store_writes: after.store_writes - before.store_writes,
            packages_read: after.packages_read - before.packages_read,
            run_materializations: after.run_materializations - before.run_materializations,
            provider_calls: after.provider_calls - before.provider_calls,
            transactions_spanning_provider_calls: after.transactions_spanning_provider_calls
                - before.transactions_spanning_provider_calls,
        }
    }

    fn json(self) -> JsonValue {
        let count = |value| JsonValue::Number(JsonNumber::U64(value));
        JsonValue::Object(JsonObject::from([
            ("store_writes".to_owned(), count(self.store_writes)),
            ("packages_read".to_owned(), count(self.packages_read)),
            (
                "run_materializations".to_owned(),
                count(self.run_materializations),
            ),
            ("provider_calls".to_owned(), count(self.provider_calls)),
            (
                "transactions_spanning_provider_calls".to_owned(),
                count(self.transactions_spanning_provider_calls),
            ),
        ]))
    }
}

fn number_of(number: &JsonNumber) -> f64 {
    match number {
        JsonNumber::I64(value) => *value as f64,
        JsonNumber::U64(value) => *value as f64,
        JsonNumber::F64(value) => *value,
    }
}

/// A run of `calls` provider calls, each its own `thread.reply` with its own
/// idempotency key.
fn provider_calls_workflow(calls: usize) -> String {
    let steps = (0..calls)
        .map(|call| {
            format!(
                "      - id: call-{call}\n        operation: fixture/chat/thread.reply\n        idempotency_key: warm-execution-{calls}-{call}\n        result:\n          required: [state]\n          equals:\n            state: delivered\n        inputs:\n          thread_locator: chat://T1/C1/1.0\n          text: warm-execution\n"
            )
        })
        .collect::<String>();
    format!("minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n{steps}")
}

fn runtime_options(run: RunLog) -> RuntimeOptions {
    let mut options =
        RuntimeOptions::new(std::env::vars().collect(), crate::fixture_signature(), run);
    options.clock = runx_runtime::RunClock::Fixed(CREATED_AT.to_owned());
    options
}

/// The workflow of the one method `run_yaml` declares, read as the
/// `run.yaml` of the package `skill`.
fn method_workflow(skill: &str, run_yaml: &str) -> Result<Workflow, Box<dyn Error>> {
    let manifest = validate_method_manifest(skill, parse_method_manifest_yaml(run_yaml)?)?;
    Ok(manifest
        .default_method()
        .ok_or_else(|| format!("{skill}'s run.yaml declares one method"))?
        .workflow
        .clone())
}

fn run_workflow<A: SkillAdapter>(
    runtime: &Runtime<A>,
    workflow_dir: &Path,
    workflow: Workflow,
) -> Result<usize, runx_runtime::RuntimeError> {
    let mut host = NoopHost;
    runtime
        .run_workflow_with_host(workflow_dir, workflow, &mut host)
        .map(|run| black_box(run.steps.len()))
}

fn run_provider_workflow<A: SkillAdapter>(
    runtime: &Runtime<A>,
    transport: &ProviderBenchmarkTransport,
    workflow_dir: &Path,
    workflow: Workflow,
) -> Result<usize, Box<dyn Error>> {
    let attempts_before = transport.operation_attempts();
    let mut host = ProviderBenchmarkHost;
    let run = runtime.run_workflow_with_host(workflow_dir, workflow, &mut host)?;
    if transport.operation_attempts() != attempts_before.saturating_add(1) {
        return Err(std::io::Error::other(
            "provider benchmark sample did not attempt exactly one provider operation",
        )
        .into());
    }
    let step = run.steps.as_slice().first().ok_or_else(|| {
        std::io::Error::other("provider benchmark sample emitted no step receipt")
    })?;
    let operation = step
        .contract
        .get("provider_operation")
        .and_then(JsonValue::as_object)
        .and_then(|packet| packet.get("data"))
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            std::io::Error::other("provider benchmark sample emitted no operation packet")
        })?;
    if operation.get("finality").and_then(JsonValue::as_str) != Some("confirmed")
        || operation
            .get("readback_uri")
            .and_then(JsonValue::as_str)
            .is_none()
    {
        return Err(std::io::Error::other(
            "provider benchmark sample did not reach identity-bound readback finality",
        )
        .into());
    }
    let finality_refs = step
        .receipt
        .acts
        .iter()
        .flat_map(|act| &act.criterion_bindings)
        .flat_map(|binding| &binding.verification_refs)
        .filter(|reference| reference.proof_kind == Some(ProofKind::EffectFinality))
        .count();
    if step.receipt.seal.outcome != RunOutcome::Completed || finality_refs != 1 {
        return Err(std::io::Error::other(
            "provider benchmark sample did not seal exactly one independent finality proof",
        )
        .into());
    }
    Ok(black_box(run.steps.len()))
}

#[derive(Clone)]
struct ProviderBenchmarkTransport {
    state: Arc<ProviderBenchmarkState>,
}

struct ProviderBenchmarkState {
    /// The `GET /v1/grants` answer: the benchmark's grant for `thread.reply`.
    grants: String,
    operation_attempts: AtomicU64,
    logical_mutations: AtomicU64,
    mutations: Mutex<BTreeMap<String, JsonObject>>,
    /// Journey 3's slow read: an operation whose text is [`GATED_TEXT`]
    /// waits until the probe opens the gate.
    gate: Gate,
    /// Journey 3's crowd: an operation whose text is [`CROWDED_TEXT`] waits
    /// until [`CROWD`] of them are in flight at once, then all go on.
    crowd: std::sync::Barrier,
    /// The crowd's operations in flight now, and the most there were at once.
    crowded: AtomicU64,
    most_crowded: AtomicU64,
}

/// What an operation that waits on journey 3's gate says.
const GATED_TEXT: &str = "gated";
/// What an operation of journey 3's crowd says.
const CROWDED_TEXT: &str = "crowded";
/// How many of the crowd's operations go on together: the run's
/// `max_concurrency`, as performance.md's conditions set it.
const CROWD: usize = 8;

/// A gate an operation waits on until it opens.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    opened: std::sync::Condvar,
}

impl Gate {
    fn wait(&self) {
        let open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _open = self
            .opened
            .wait_while(open, |open| !*open)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }

    fn set(&self, open: bool) {
        *self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = open;
        self.opened.notify_all();
    }
}

/// One of the crowd's operations in flight, until it drops.
struct Crowded<'a>(&'a AtomicU64);

impl Drop for Crowded<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl ProviderBenchmarkTransport {
    fn for_grant(connector: &LoadedConnector) -> Result<Self, Box<dyn Error>> {
        let grants = serde_json::to_string(&serde_json::json!({
            "status": "success",
            "grants": [{
                "grant_id": BENCHMARK_GRANT,
                "provider": connector.manifest.name.as_str(),
                "connector_binding": connector.binding(),
                "status": "active",
                "credential_material_bound": true,
                "scopes": ["thread.reply"],
            }],
        }))?;
        Ok(Self {
            state: Arc::new(ProviderBenchmarkState {
                grants,
                operation_attempts: AtomicU64::default(),
                logical_mutations: AtomicU64::default(),
                mutations: Mutex::default(),
                gate: Gate::default(),
                crowd: std::sync::Barrier::new(CROWD),
                crowded: AtomicU64::default(),
                most_crowded: AtomicU64::default(),
            }),
        })
    }

    fn operation_attempts(&self) -> u64 {
        self.state.operation_attempts.load(Ordering::Relaxed)
    }
}

impl RuntimeHttpTransport for ProviderBenchmarkTransport {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        if request.method == HttpMethod::Get && request.url.ends_with("/v1/me") {
            return Ok(RuntimeHttpResponse::new(
                200,
                r#"{"status":"success","principal":{"principal_id":"operator:benchmark"}}"#,
            ));
        }
        if request.method == HttpMethod::Get && request.url.ends_with("/v1/grants") {
            return Ok(RuntimeHttpResponse::new(200, self.state.grants.clone()));
        }
        if request.method != HttpMethod::Post || !request.url.ends_with("/v1/provider-operations") {
            return Err(benchmark_transport_error("unexpected hosted API request"));
        }
        self.state
            .operation_attempts
            .fetch_add(1, Ordering::Relaxed);
        let body = request
            .body
            .as_deref()
            .ok_or_else(|| benchmark_transport_error("provider request body is missing"))?;
        let request: JsonObject = serde_json::from_str(body)
            .map_err(|error| benchmark_transport_error(format!("invalid request JSON: {error}")))?;
        let operation = benchmark_string(&request, "operation")?;
        let target = benchmark_string(&request, "target")?;
        let access = benchmark_string(&request, "access")?;
        let input = request
            .get("input")
            .and_then(JsonValue::as_object)
            .ok_or_else(|| benchmark_transport_error("provider input is missing"))?;
        let idempotency_key = benchmark_string(input, "idempotency_key")?;
        let _crowded = match input.get("text").and_then(JsonValue::as_str) {
            Some(GATED_TEXT) => {
                self.state.gate.wait();
                None
            }
            Some(CROWDED_TEXT) => {
                let now = self.state.crowded.fetch_add(1, Ordering::Relaxed) + 1;
                self.state.most_crowded.fetch_max(now, Ordering::Relaxed);
                let crowded = Crowded(&self.state.crowded);
                self.state.crowd.wait();
                Some(crowded)
            }
            _ => None,
        };
        let mut mutations = self
            .state
            .mutations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = mutations
            .entry(idempotency_key.to_owned())
            .or_insert_with(|| {
                let logical = self
                    .state
                    .logical_mutations
                    .fetch_add(1, Ordering::Relaxed)
                    .saturating_add(1);
                JsonObject::from([
                    (
                        "operation_id".to_owned(),
                        JsonValue::String(format!("benchmark-operation-{logical}")),
                    ),
                    (
                        "readback_uri".to_owned(),
                        JsonValue::String(format!("runx:provider_readback:{logical}")),
                    ),
                ])
            })
            .clone();
        let response = serde_json::json!({
            "status": "success",
            "provider": "fixture/chat",
            "operation": operation,
            "target": target,
            "access": access,
            "operation_id": benchmark_string(&result, "operation_id")?,
            "idempotency_key": idempotency_key,
            "readback_uri": benchmark_string(&result, "readback_uri")?,
            "result": {"state": "delivered"}
        });
        Ok(RuntimeHttpResponse::new(200, response.to_string()))
    }
}

struct ProviderBenchmarkHost;

impl Host for ProviderBenchmarkHost {
    fn log(&mut self, _message: String) -> Result<(), runx_runtime::RuntimeError> {
        Ok(())
    }

    fn report(&mut self, _event: ExecutionEvent) -> Result<(), runx_runtime::RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        _request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, runx_runtime::RuntimeError> {
        Ok(Some(ResolutionResponse {
            actor: ResolutionResponseActor::Human,
            payload: JsonValue::Bool(true),
        }))
    }
}

fn benchmark_string<'a>(object: &'a JsonObject, field: &str) -> Result<&'a str, RuntimeHttpError> {
    object
        .get(field)
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| benchmark_transport_error(format!("provider {field} is missing")))
}

fn benchmark_transport_error(message: impl Into<String>) -> RuntimeHttpError {
    RuntimeHttpError::Transport {
        message: message.into(),
    }
}

fn require_success(output: runx_runtime::InvocationOutput) -> Result<(), Box<dyn Error>> {
    if output.succeeded() {
        return Ok(());
    }
    Err(std::io::Error::other(
        output
            .failure_message()
            .unwrap_or_else(|| "runtime invocation failed".to_owned()),
    )
    .into())
}

fn write_modules(root: &Path) -> Result<(), std::io::Error> {
    fs::write(
        root.join("SKILL.md"),
        "---\nname: runtime-throughput\ndescription: Exercise production runtime paths for release performance budgets.\n---\n# Runtime throughput\n\nThis synthetic package binds the modules used by the release performance test.\n",
    )?;
    fs::write(
        root.join(PROFILE_FILE),
        "minimum_runx: 0.12.0\nmethods:\n  main:\n    inputs:\n      payload:\n        type: string\n    steps:\n      - id: measure\n        tool: {module: domain.mjs}\n        inputs:\n          payload: $input.payload\n        outputs:\n          result: {type: object}\n",
    )?;
    fs::write(
        root.join("domain.mjs"),
        "export default ({ payload }) => ({ result: { bytes: payload.length } });\n\
         export const project = ({ marker, digest_result }) => ({ projection: { marker, digest: digest_result.digest } });\n",
    )
}

fn javascript_invocation(root: &Path, inputs: JsonObject) -> SkillInvocation {
    SkillInvocation {
        reach: runx_contracts::Reach::Inputs,
        historical_context: Vec::new(),
        skill_name: "runtime-perf-module".to_owned(),
        step_id: None,
        executable: Executable::Worker(WorkerTool {
            requires: Vec::new(),
            module: "domain.mjs".to_owned(),
            export: None,
            cpu_seconds: None,
            paged_input: None,
            environment: Default::default(),
        }),
        outputs: None,
        environment: Default::default(),
        artifacts: None,
        inputs,
        resolved_inputs: JsonObject::new(),
        current_context: Vec::new(),
        provenance: Vec::new(),
        skill_directory: root.to_path_buf(),
        env: std::env::vars().collect(),
        credential_delivery: CredentialDelivery::none(),
    }
}

/// A native capability as every caller reaches it: one tool step, admitted,
/// dispatched and sealed.
const NATIVE_WORKFLOW: &str = r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value:
            subject: native-dispatch
"#;

const CONTEXT_WORKFLOW: &str = r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value:
            subject: workflow-context
      - id: project
        tool: {module: domain.mjs, export: project}
        inputs:
          marker: baseline
          digest_result: $steps.digest.digest_result
        outputs:
          projection: {type: object}
"#;

const PROVIDER_EFFECT_SKILL: &str = r#"
minimum_runx: 0.12.0
methods:
  run:
    result_from: [provider-effect]
    steps:
      - id: provider-effect
        operation: fixture/chat/thread.reply
        idempotency_key: benchmark-request
        result:
          required: [state]
          equals:
            state: delivered
        inputs:
          thread_locator: chat://T1/C1/1.0
          text: provider-effect-transition
"#;
