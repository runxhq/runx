use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use boa_engine::builtins::promise::PromiseState;
use boa_engine::context::HostHooks;
use boa_engine::context::time::FixedClock;
use boa_engine::module::MapModuleLoader;
use boa_engine::object::IntegrityLevel;
use boa_engine::{Context, JsError, JsValue, Module, Source, js_string};
use thiserror::Error;

use crate::protocol::{InvocationLimits, WorkerFailureCode};

mod globals;
mod jobs;
mod validation;

use jobs::BoundedJobExecutor;
use validation::{validate_bundle, validate_input};

const VIRTUAL_ROOT: &str = "/runx";
const LOOP_ITERATION_LIMIT: u64 = 10_000_000;
const RECURSION_LIMIT: usize = 1_024;
const BACKTRACE_LIMIT: usize = 32;
const ERROR_MESSAGE_BYTES: usize = 4_096;

/// The engine's host: every default, except that local time is UTC, so a
/// script reads nothing of the machine it runs on, its time zone included.
struct WorkerHost;

impl HostHooks for WorkerHost {
    fn local_timezone_offset_seconds(&self, _unix_time_seconds: i64) -> i32 {
        0
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
pub(crate) struct EngineError {
    pub(crate) code: WorkerFailureCode,
    pub(crate) message: String,
}

impl EngineError {
    pub(super) fn new(code: WorkerFailureCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: bounded_message(message.into()),
        }
    }

    /// A failure at one of the invocation's ceilings; the message names it.
    pub(super) fn limit(message: impl Into<String>) -> Self {
        Self::new(WorkerFailureCode::ResourceLimit, message)
    }
}

pub(crate) fn evaluate(
    entry_module: &str,
    export_name: &str,
    modules: &BTreeMap<String, String>,
    inputs: serde_json::Value,
    environment: BTreeMap<String, String>,
    limits: InvocationLimits,
) -> Result<serde_json::Value, EngineError> {
    let limits = limits
        .validate()
        .map_err(|error| EngineError::new(WorkerFailureCode::InvalidRequest, error.to_string()))?;
    validate_bundle(entry_module, export_name, modules, limits)?;
    validate_input(&inputs, limits.input_bytes)?;

    let loader = Rc::new(MapModuleLoader::new());
    let jobs = Rc::new(BoundedJobExecutor::new(limits.queued_jobs));
    let mut context = Context::builder()
        .clock(Rc::new(FixedClock::from_millis(0)))
        .host_hooks(Rc::new(WorkerHost))
        .job_executor(jobs.clone())
        .module_loader(loader.clone())
        .build()
        .map_err(|error| engine_failure("creating JavaScript context", error))?;
    configure_context(&mut context, limits.stack_bytes)?;

    let parsed = parse_modules(modules, &loader, &mut context)?;
    let entry = parsed.get(entry_module).ok_or_else(|| {
        EngineError::new(
            WorkerFailureCode::ModuleRejected,
            format!("entry module {entry_module:?} is absent from the validated bundle"),
        )
    })?;
    settle_module(entry, &jobs, &mut context)?;
    let exported = entry
        .namespace(&mut context)
        .get(js_string!(export_name), &mut context)
        .map_err(|error| engine_failure("resolving JavaScript export", error))?;
    let callable = exported.as_callable().ok_or_else(|| {
        EngineError::new(
            WorkerFailureCode::ExecutionFailed,
            format!("JavaScript module does not export callable {export_name:?}"),
        )
    })?;
    let input = JsValue::from_json(&inputs, &mut context)
        .map_err(|error| engine_failure("materializing JavaScript input", error))?;
    let execution_context = execution_context(environment, &mut context)?;
    let result = callable
        .call(
            &JsValue::undefined(),
            &[input, execution_context],
            &mut context,
        )
        .map_err(|error| engine_failure("calling JavaScript export", error))?;
    let result = settle_result(result, &jobs, &mut context)?;
    let output = result
        .to_json(&mut context)
        .map_err(|error| engine_failure("converting JavaScript result to JSON", error))?
        .ok_or_else(|| {
            EngineError::new(
                WorkerFailureCode::OutputRejected,
                "JavaScript result is not JSON-compatible",
            )
        })?;
    let output_bytes = serde_json::to_vec(&output)
        .map_err(|error| EngineError::new(WorkerFailureCode::OutputRejected, error.to_string()))?;
    if output_bytes.len() > limits.output_bytes {
        return Err(EngineError::limit(format!(
            "JavaScript output is {} bytes; limit is {} bytes",
            output_bytes.len(),
            limits.output_bytes
        )));
    }
    Ok(output)
}

fn execution_context(
    environment: BTreeMap<String, String>,
    context: &mut Context,
) -> Result<JsValue, EngineError> {
    let value = JsValue::from_json(&serde_json::json!({ "environment": environment }), context)
        .map_err(|error| engine_failure("materializing JavaScript execution context", error))?;
    let object = value.as_object().ok_or_else(|| {
        EngineError::new(
            WorkerFailureCode::InternalFailure,
            "JavaScript execution context is not an object",
        )
    })?;
    let environment = object
        .get(js_string!("environment"), context)
        .map_err(|error| engine_failure("resolving JavaScript environment", error))?;
    let environment = environment.as_object().ok_or_else(|| {
        EngineError::new(
            WorkerFailureCode::InternalFailure,
            "JavaScript environment is not an object",
        )
    })?;
    if !environment
        .set_integrity_level(IntegrityLevel::Frozen, context)
        .map_err(|error| engine_failure("freezing JavaScript environment", error))?
        || !object
            .set_integrity_level(IntegrityLevel::Frozen, context)
            .map_err(|error| engine_failure("freezing JavaScript execution context", error))?
    {
        return Err(EngineError::new(
            WorkerFailureCode::InternalFailure,
            "failed to freeze JavaScript execution context",
        ));
    }
    Ok(value)
}

fn configure_context(context: &mut Context, stack_bytes: usize) -> Result<(), EngineError> {
    let mut runtime_limits = context.runtime_limits();
    runtime_limits.set_loop_iteration_limit(LOOP_ITERATION_LIMIT);
    runtime_limits.set_stack_size_limit(stack_bytes / std::mem::size_of::<JsValue>());
    runtime_limits.set_recursion_limit(RECURSION_LIMIT);
    runtime_limits.set_backtrace_limit(BACKTRACE_LIMIT);
    context.set_runtime_limits(runtime_limits);
    globals::install(context)
        .map_err(|error| engine_failure("installing deterministic globals", error))
}

fn parse_modules(
    modules: &BTreeMap<String, String>,
    loader: &MapModuleLoader,
    context: &mut Context,
) -> Result<BTreeMap<String, Module>, EngineError> {
    let mut parsed = BTreeMap::new();
    for (path, source) in modules {
        let virtual_path = virtual_path(path);
        let module = Module::parse(
            Source::from_bytes(source.as_bytes()).with_path(&virtual_path),
            None,
            context,
        )
        .map_err(|error| engine_failure(format!("parsing JavaScript module {path:?}"), error))?;
        loader.insert(virtual_path.to_string_lossy(), module.clone());
        parsed.insert(path.clone(), module);
    }
    Ok(parsed)
}

fn settle_module(
    module: &Module,
    jobs: &BoundedJobExecutor,
    context: &mut Context,
) -> Result<(), EngineError> {
    let promise = module.load_link_evaluate(context);
    context
        .run_jobs()
        .map_err(|error| engine_failure("evaluating JavaScript module", error))?;
    jobs.check()?;
    match promise.state() {
        PromiseState::Fulfilled(_) => Ok(()),
        PromiseState::Rejected(error) => Err(engine_failure(
            "evaluating JavaScript module",
            JsError::from_opaque(error),
        )),
        PromiseState::Pending => Err(EngineError::new(
            WorkerFailureCode::ExecutionFailed,
            "JavaScript module evaluation left a pending promise",
        )),
    }
}

fn settle_result(
    result: JsValue,
    jobs: &BoundedJobExecutor,
    context: &mut Context,
) -> Result<JsValue, EngineError> {
    let Some(object) = result.as_object() else {
        return Ok(result);
    };
    let Ok(promise) = boa_engine::object::builtins::JsPromise::from_object(object.clone()) else {
        return Ok(result);
    };
    context
        .run_jobs()
        .map_err(|error| engine_failure("settling JavaScript result", error))?;
    jobs.check()?;
    match promise.state() {
        PromiseState::Fulfilled(value) => Ok(value),
        PromiseState::Rejected(error) => Err(engine_failure(
            "settling JavaScript result",
            JsError::from_opaque(error),
        )),
        PromiseState::Pending => Err(EngineError::new(
            WorkerFailureCode::ExecutionFailed,
            "JavaScript export returned a promise that did not settle in the immediate job queue",
        )),
    }
}

fn virtual_path(path: &str) -> PathBuf {
    Path::new(VIRTUAL_ROOT).join(path)
}

fn engine_failure(context: impl AsRef<str>, error: impl std::fmt::Display) -> EngineError {
    EngineError::new(
        WorkerFailureCode::ExecutionFailed,
        format!("{}: {error}", context.as_ref()),
    )
}

fn bounded_message(mut message: String) -> String {
    if message.len() <= ERROR_MESSAGE_BYTES {
        return message;
    }
    let mut end = ERROR_MESSAGE_BYTES;
    while !message.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    message.truncate(end);
    message.push('…');
    message
}

#[cfg(test)]
mod tests;
