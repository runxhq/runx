use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use criterion::{BatchSize, Criterion};
use runx_contracts::{
    ClosureDisposition, ExecutionEvent, JsonObject, JsonValue, ProofKind, ResolutionRequest,
    ResolutionResponse, ResolutionResponseActor,
};
use runx_parser::{ExecutionGraph, PROFILE_FILE, WorkerTool, parse_graph_yaml, validate_graph};
use runx_runtime::adapters::javascript::JavaScriptAdapter;
use runx_runtime::{
    CredentialDelivery, Executable, HOSTED_API_BASE_URL_ENV, HOSTED_API_TOKEN_ENV, Host,
    HttpMethod, INJECTED_GRANT_ID_ENV, INJECTED_GRANTED_SCOPES_ENV, INJECTED_PRINCIPAL_REF_ENV,
    KernelStore, NoopHost, OperationEffect, RUNX_RECEIPT_DIR_ENV, RunLog, Runtime,
    RuntimeEffectRegistry, RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse,
    RuntimeHttpTransport, RuntimeOptions, SkillAdapter, SkillInvocation, encode_injected_scopes,
};
use tempfile::TempDir;

use super::{record_resource_metrics, session_metric};

const CREATED_AT: &str = "2026-07-20T00:00:00Z";
const LARGE_INPUT_BYTES: usize = 4 * 1024 * 1024 - 64;

#[allow(clippy::expect_used)]
pub(super) fn register(c: &mut Criterion) {
    let fixtures = RuntimePathFixtures::new().expect("runtime-path benchmark fixtures must load");
    let resources = fixtures
        .smoke()
        .expect("runtime-path benchmark fixtures must execute successfully");
    record_resource_metrics(&resources)
        .expect("runtime-path benchmark resource metrics must be recorded");

    c.bench_function("native_capability_dispatch", |b| {
        b.iter_batched(
            || fixtures.native_graph.clone(),
            |graph| {
                run_graph(
                    &fixtures.next_run(&fixtures.context_runtime),
                    fixtures.root(),
                    graph,
                )
            },
            BatchSize::SmallInput,
        )
    });

    c.bench_function("graph_context_to_module", |b| {
        b.iter_batched(
            || fixtures.context_graph.clone(),
            |graph| {
                run_graph(
                    &fixtures.next_run(&fixtures.context_runtime),
                    fixtures.root(),
                    graph,
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
            || fixtures.provider_graph.clone(),
            |graph| {
                run_provider_graph(
                    &fixtures.next_run(&fixtures.provider_runtime),
                    &fixtures.provider_transport,
                    fixtures.root(),
                    graph,
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
    native_graph: ExecutionGraph,
    context_runtime: Runtime<JavaScriptAdapter>,
    context_graph: ExecutionGraph,
    module_invocation: SkillInvocation,
    large_module_invocation: SkillInvocation,
    module_adapter: JavaScriptAdapter,
    provider_runtime: Runtime<JavaScriptAdapter>,
    provider_graph: ExecutionGraph,
    provider_transport: ProviderBenchmarkTransport,
}

impl RuntimePathFixtures {
    fn new() -> Result<Self, Box<dyn Error>> {
        let directory = TempDir::new()?;
        write_modules(directory.path())?;
        let store = KernelStore::open(&directory.path().join(".runx"))?;
        let evaluation = |name: &str| {
            RunLog::evaluation(
                Arc::clone(&store),
                name,
                "graph-throughput",
                name,
                CREATED_AT,
            )
        };

        let native_graph = graph(NATIVE_GRAPH)?;
        let context_runtime = Runtime::new(
            JavaScriptAdapter::default(),
            runtime_options(evaluation("run_context_smoke")?),
        );
        let context_graph = graph(CONTEXT_GRAPH)?;
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

        let provider_transport = ProviderBenchmarkTransport::default();
        let effects = RuntimeEffectRegistry::with_effect(OperationEffect::with_http_transport(
            provider_transport.clone(),
        ))?;
        let mut provider_options = runtime_options(evaluation("run_provider_smoke")?);
        provider_options.effects = effects.clone();
        provider_options.env.insert(
            INJECTED_GRANT_ID_ENV.to_owned(),
            "grant_runtime_perf".to_owned(),
        );
        provider_options.env.insert(
            INJECTED_GRANTED_SCOPES_ENV.to_owned(),
            encode_injected_scopes(&["thread.reply".to_owned()]).ok_or("scopes")?,
        );
        provider_options.env.insert(
            INJECTED_PRINCIPAL_REF_ENV.to_owned(),
            "runx:principal:operator:benchmark".to_owned(),
        );
        provider_options.env.insert(
            HOSTED_API_BASE_URL_ENV.to_owned(),
            "https://api.runx.benchmark".to_owned(),
        );
        provider_options
            .env
            .insert(HOSTED_API_TOKEN_ENV.to_owned(), "rxk_benchmark".to_owned());
        provider_options.env.insert(
            RUNX_RECEIPT_DIR_ENV.to_owned(),
            directory
                .path()
                .join("provider-receipts")
                .to_string_lossy()
                .into_owned(),
        );
        let provider_runtime = Runtime::new(JavaScriptAdapter::default(), provider_options);
        let provider_graph = runx_parser::validate_runner_manifest(
            runx_parser::parse_runner_manifest_yaml(PROVIDER_EFFECT_SKILL)?,
        )?
        .runners
        .remove("run")
        .map(|runner| runner.graph)
        .ok_or("the provider benchmark skill has no graph")?;

        Ok(Self {
            directory,
            store,
            runs: AtomicU64::new(0),
            native_graph,
            context_runtime,
            context_graph,
            module_invocation,
            large_module_invocation,
            module_adapter,
            provider_runtime,
            provider_graph,
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
                "graph-throughput",
                &run_id,
                CREATED_AT,
            )
            .expect("benchmark sample run must start"),
        )
    }

    fn smoke(&self) -> Result<JsonObject, Box<dyn Error>> {
        run_graph(
            &self.next_run(&self.context_runtime),
            self.root(),
            self.native_graph.clone(),
        )?;
        let cold = JavaScriptAdapter::new_session();
        require_success(cold.invoke(self.module_invocation.clone())?)?;
        require_success(self.module_adapter.invoke(self.module_invocation.clone())?)?;
        require_success(
            self.module_adapter
                .invoke(self.large_module_invocation.clone())?,
        )?;
        run_graph(
            &self.context_runtime,
            self.root(),
            self.context_graph.clone(),
        )?;
        run_provider_graph(
            &self.provider_runtime,
            &self.provider_transport,
            self.root(),
            self.provider_graph.clone(),
        )?;
        Ok(JsonObject::from([
            (
                "graph_context_to_module".to_owned(),
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
}

fn runtime_options(run: RunLog) -> RuntimeOptions {
    let mut options = RuntimeOptions::local_development(std::env::vars().collect(), run);
    options.created_at = CREATED_AT.to_owned();
    options
}

fn graph(source: &str) -> Result<ExecutionGraph, Box<dyn Error>> {
    Ok(validate_graph(parse_graph_yaml(source)?)?)
}

fn run_graph<A: SkillAdapter>(
    runtime: &Runtime<A>,
    graph_dir: &Path,
    graph: ExecutionGraph,
) -> Result<usize, runx_runtime::RuntimeError> {
    let mut host = NoopHost;
    runtime
        .run_graph_with_host(graph_dir, graph, &mut host)
        .map(|run| black_box(run.steps.len()))
}

fn run_provider_graph<A: SkillAdapter>(
    runtime: &Runtime<A>,
    transport: &ProviderBenchmarkTransport,
    graph_dir: &Path,
    graph: ExecutionGraph,
) -> Result<usize, Box<dyn Error>> {
    let attempts_before = transport.operation_attempts();
    let mut host = ProviderBenchmarkHost;
    let run = runtime.run_graph_with_host(graph_dir, graph, &mut host)?;
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
            .get("readback_ref")
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
    if step.receipt.seal.disposition != ClosureDisposition::Closed || finality_refs != 1 {
        return Err(std::io::Error::other(
            "provider benchmark sample did not seal exactly one independent finality proof",
        )
        .into());
    }
    Ok(black_box(run.steps.len()))
}

#[derive(Clone, Default)]
struct ProviderBenchmarkTransport {
    state: Arc<ProviderBenchmarkState>,
}

#[derive(Default)]
struct ProviderBenchmarkState {
    operation_attempts: AtomicU64,
    logical_mutations: AtomicU64,
    mutations: Mutex<BTreeMap<String, JsonObject>>,
}

impl ProviderBenchmarkTransport {
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
                        "readback_ref".to_owned(),
                        JsonValue::String(format!("runx:provider_readback:{logical}")),
                    ),
                ])
            })
            .clone();
        let response = serde_json::json!({
            "status": "success",
            "provider": "slack",
            "operation": operation,
            "target": target,
            "access": access,
            "operation_id": benchmark_string(&result, "operation_id")?,
            "idempotency_key": idempotency_key,
            "readback_ref": benchmark_string(&result, "readback_ref")?,
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
        "---\nname: runtime-throughput\ndescription: Exercise production runtime paths for release performance budgets.\n---\n# Runtime throughput\n\nThis synthetic package binds the modules used by the release performance harness.\n",
    )?;
    fs::write(
        root.join(PROFILE_FILE),
        "skill: runtime-throughput\nrunners:\n  main:\n    default: true\n    inputs:\n      payload:\n        type: string\n        required: true\n    steps:\n      - id: measure\n        tool: { module: domain.mjs }\n        inputs:\n          payload: $input.payload\n        outputs:\n          result: object\n",
    )?;
    fs::write(
        root.join("domain.mjs"),
        "export default ({ payload }) => ({ result: { bytes: payload.length } });\n\
         export const project = ({ marker, digest_result }) => ({ projection: { marker, digest: digest_result.digest } });\n",
    )
}

fn javascript_invocation(root: &Path, inputs: JsonObject) -> SkillInvocation {
    SkillInvocation {
        historical_context: Vec::new(),
        skill_name: "runtime-perf-module".to_owned(),
        step_id: None,
        executable: Executable::Worker(WorkerTool {
            module: "domain.mjs".to_owned(),
            export: None,
            timeout_seconds: None,
            pages: None,
            environment: Default::default(),
        }),
        outputs: None,
        requirements: Default::default(),
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
const NATIVE_GRAPH: &str = r#"
name: runtime-native-dispatch
steps:
  - id: digest
    tool: data.digest
    inputs:
      value:
        subject: native-dispatch
"#;

const CONTEXT_GRAPH: &str = r#"
name: runtime-context-to-module
steps:
  - id: digest
    tool: data.digest
    inputs:
      value:
        subject: graph-context
  - id: project
    tool: { module: domain.mjs, export: project }
    inputs:
      marker: baseline
      digest_result: $steps.digest.digest_result
    outputs:
      projection: object
"#;

const PROVIDER_EFFECT_SKILL: &str = r#"
skill: runtime-provider-effect-transition
connectors:
  slack:
    operations: [thread.reply]
runners:
  run:
    result_from: [provider-effect]
    steps:
      - id: provider-effect
        operation: slack/thread.reply
        idempotency_key: benchmark-request
        expected_result:
          state: delivered
        result_fields: [state]
        input:
          thread_locator: slack://T1/C1/1.0
          text: provider-effect-transition
"#;
