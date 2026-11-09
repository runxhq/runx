//! One pass of a script step. The module runs from the beginning, called as
//! `(ctx, inputs)`; every call it makes through `ctx` takes the next
//! position and a promise only the pass settles. The settlements the run
//! records are delivered one at a time, in the order they settled, with the
//! job queue drained after each, so `Promise.all` comes out the same after a
//! crash as it did live. When the record runs out, the pass ends and reports
//! every call it saw: the kernel admits, runs and records the new ones and
//! starts the next pass.

use std::collections::BTreeMap;
use std::rc::Rc;

use boa_engine::builtins::promise::{PromiseState, ResolvingFunctions};
use boa_engine::context::time::FixedClock;
use boa_engine::module::MapModuleLoader;
use boa_engine::object::builtins::JsPromise;
use boa_engine::object::{FunctionObjectBuilder, IntegrityLevel, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{
    Context, JsArgs, JsError, JsNativeError, JsResult, JsValue, NativeFunction, Source, js_string,
};
use boa_gc::{Finalize, Gc, GcRefCell, Trace};

use super::jobs::BoundedJobExecutor;
use super::validation::{validate_bundle, validate_input};
use super::{
    EngineError, WorkerHost, configure_context, engine_failure, parse_modules, settle_module,
};
use crate::protocol::{
    InvocationLimits, ScriptCall, ScriptCallOutcome, ScriptCallTarget, ScriptEnding,
    ScriptSettlement, WorkerFailureCode,
};

/// What a script's `ctx` sees of the clock: nothing but `ctx.now()`, which
/// the run records. `Date.now()`, `Date()` and `new Date()` read the clock,
/// so they throw; a `Date` built from a number is deterministic and stays.
const CLOCK_GUARD: &str = r#"
(() => {
  const Clocked = Date;
  const refuse = () => {
    throw new TypeError("a script reads the time with ctx.now(), never the clock");
  };
  function ScriptDate(...values) {
    if (new.target === undefined || values.length === 0) refuse();
    return Reflect.construct(Clocked, values, new.target);
  }
  Object.setPrototypeOf(ScriptDate, Clocked);
  Object.defineProperty(ScriptDate, "prototype", { value: Clocked.prototype });
  Object.defineProperty(ScriptDate, "now", { value: refuse });
  Object.freeze(ScriptDate);
  Object.defineProperty(globalThis, "Date", { value: ScriptDate, writable: false, configurable: false });
})();
"#;

/// The calls one pass made, and the promises only the pass settles.
#[derive(Trace, Finalize)]
struct Ledger {
    #[unsafe_ignore_trace]
    next: u32,
    #[unsafe_ignore_trace]
    calls: Vec<ScriptCall>,
    /// What each recorded `ctx.now()` read, by position.
    #[unsafe_ignore_trace]
    recorded_now: BTreeMap<u32, i64>,
    /// What a `ctx.now()` the record doesn't hold reads.
    #[unsafe_ignore_trace]
    now: i64,
    /// The promise of each call not yet settled, by position.
    pending: BTreeMap<u32, ResolvingFunctions>,
}

type Shared = Gc<GcRefCell<Ledger>>;

/// What a pass reports: the calls the script made since the last report, in
/// position order, and how it stands.
pub(crate) struct Passed {
    pub(crate) calls: Vec<ScriptCall>,
    pub(crate) ending: ScriptEnding,
}

pub(crate) struct ScriptPass<'a> {
    pub(crate) entry_module: &'a str,
    pub(crate) export_name: &'a str,
    pub(crate) modules: &'a BTreeMap<String, String>,
    pub(crate) inputs: serde_json::Value,
    pub(crate) limits: InvocationLimits,
    pub(crate) settled: &'a [ScriptSettlement],
    pub(crate) now: i64,
}

/// A script that waits on its calls, kept running between settlements, so a
/// settlement goes on from where the script stands rather than running it
/// from the beginning. It is indistinguishable from a pass replaying the same
/// settlements in the same order: each is delivered alone, and the job queue
/// drained after it.
pub(crate) struct LiveScript {
    context: Context,
    jobs: Rc<BoundedJobExecutor>,
    ledger: Shared,
    returned: JsValue,
    output_bytes: usize,
    _loader: Rc<MapModuleLoader>,
}

/// Run a pass from the beginning against `request.settled`. A script still
/// waiting on its calls comes back live, to take further settlements.
pub(crate) fn start(request: ScriptPass<'_>) -> Result<(Option<LiveScript>, Passed), EngineError> {
    let limits = request
        .limits
        .validate()
        .map_err(|error| EngineError::new(WorkerFailureCode::InvalidRequest, error.to_string()))?;
    validate_bundle(
        request.entry_module,
        request.export_name,
        request.modules,
        limits,
    )?;
    validate_input(&request.inputs, limits.input_bytes)?;

    let loader = Rc::new(MapModuleLoader::new());
    let jobs = Rc::new(BoundedJobExecutor::new(limits.queued_jobs));
    let mut context = Context::builder()
        .clock(Rc::new(FixedClock::from_millis(0)))
        .host_hooks(Rc::new(WorkerHost))
        .job_executor(jobs.clone())
        .module_loader(loader.clone())
        .build()
        .map_err(|error| engine_failure("creating JavaScript context", error))?;
    configure_context(&mut context, limits)?;
    context
        .eval(Source::from_bytes(CLOCK_GUARD))
        .map_err(|error| engine_failure("installing the script's clock guard", error))?;

    let parsed = parse_modules(request.modules, &loader, &mut context)?;
    let entry = parsed.get(request.entry_module).ok_or_else(|| {
        EngineError::new(
            WorkerFailureCode::ModuleRejected,
            format!(
                "entry module {:?} is absent from the validated bundle",
                request.entry_module
            ),
        )
    })?;
    settle_module(entry, &jobs, &mut context)?;
    let exported = entry
        .namespace(&mut context)
        .get(js_string!(request.export_name), &mut context)
        .map_err(|error| engine_failure("resolving JavaScript export", error))?;
    let callable = exported.as_callable().ok_or_else(|| {
        EngineError::new(
            WorkerFailureCode::ExecutionFailed,
            format!(
                "JavaScript module does not export callable {:?}",
                request.export_name
            ),
        )
    })?;

    let ledger: Shared = Gc::new(GcRefCell::new(Ledger {
        next: 0,
        calls: Vec::new(),
        recorded_now: request
            .settled
            .iter()
            .filter_map(|settled| match settled.outcome {
                ScriptCallOutcome::Now { millis } => Some((settled.position, millis)),
                _ => None,
            })
            .collect(),
        now: request.now,
        pending: BTreeMap::new(),
    }));
    let ctx = script_context(&ledger, &mut context)?;
    let input = JsValue::from_json(&request.inputs, &mut context)
        .map_err(|error| engine_failure("materializing JavaScript input", error))?;

    jobs.renew();
    let returned = match callable.call(&JsValue::undefined(), &[ctx, input], &mut context) {
        Ok(returned) => returned,
        Err(error) => {
            let message = thrown_message(&error.to_opaque(&mut context), &mut context);
            return Ok((
                None,
                Passed {
                    calls: take_calls(&ledger),
                    ending: ScriptEnding::Threw { message },
                },
            ));
        }
    };
    drain(&jobs, &mut context)?;
    let mut live = LiveScript {
        context,
        jobs,
        ledger,
        returned,
        output_bytes: limits.output_bytes,
        _loader: loader,
    };
    let passed = live.deliver(request.settled)?;
    let waiting = passed.ending == ScriptEnding::Waiting;
    Ok((waiting.then_some(live), passed))
}

/// Run a pass from the beginning against `request.settled`, and end it there.
#[cfg(test)]
pub(crate) fn pass(request: ScriptPass<'_>) -> Result<Passed, EngineError> {
    start(request).map(|(_, passed)| passed)
}

impl LiveScript {
    /// Deliver further settlements, in the order they settled; a `ctx.now()`
    /// the script reads from here reads `now`.
    pub(crate) fn settle(
        &mut self,
        settled: &[ScriptSettlement],
        now: i64,
    ) -> Result<Passed, EngineError> {
        self.ledger.borrow_mut().now = now;
        self.deliver(settled)
    }

    /// Deliver each settlement alone, draining the job queue after it, and
    /// report the calls made since the last report and how the script stands.
    fn deliver(&mut self, settled: &[ScriptSettlement]) -> Result<Passed, EngineError> {
        let context = &mut self.context;
        for settled in settled {
            let value = match &settled.outcome {
                ScriptCallOutcome::Now { .. } => continue,
                ScriptCallOutcome::Resolved { value } => Ok(JsValue::from_json(value, context)
                    .map_err(|error| engine_failure("materializing a call's result", error))?),
                ScriptCallOutcome::Rejected {
                    reason,
                    code,
                    message,
                } => Err(rejection(
                    serde_json::json!({ "reason": reason, "code": code, "message": message }),
                    context,
                )?),
            };
            let Some(resolvers) = self.ledger.borrow_mut().pending.remove(&settled.position) else {
                return Ok(Passed {
                    calls: take_calls(&self.ledger),
                    ending: ScriptEnding::Diverged {
                        position: settled.position,
                    },
                });
            };
            let (function, value) = match value {
                Ok(value) => (resolvers.resolve, value),
                Err(reason) => (resolvers.reject, reason),
            };
            // Each delivery drains under the job budget afresh: the budget
            // bounds what one settlement sets running, not a script's length.
            self.jobs.renew();
            function
                .call(&JsValue::undefined(), &[value], context)
                .map_err(|error| engine_failure("settling a script call", error))?;
            drain(&self.jobs, context)?;
        }
        let ending = ending(self.returned.clone(), self.output_bytes, context)?;
        Ok(Passed {
            calls: take_calls(&self.ledger),
            ending,
        })
    }
}

fn drain(jobs: &BoundedJobExecutor, context: &mut Context) -> Result<(), EngineError> {
    let ran = context.run_jobs();
    jobs.check()?;
    ran.map_err(|error| engine_failure("running the script's job queue", error))
}

/// How the function stands once the record ran out.
fn ending(
    returned: JsValue,
    output_bytes: usize,
    context: &mut Context,
) -> Result<ScriptEnding, EngineError> {
    let value = match returned
        .as_object()
        .and_then(|object| JsPromise::from_object(object.clone()).ok())
    {
        None => returned,
        Some(promise) => match promise.state() {
            PromiseState::Pending => return Ok(ScriptEnding::Waiting),
            PromiseState::Rejected(error) => {
                return Ok(ScriptEnding::Threw {
                    message: thrown_message(&error, context),
                });
            }
            PromiseState::Fulfilled(value) => value,
        },
    };
    let output = value
        .to_json(context)
        .map_err(|error| engine_failure("converting the script's result to JSON", error))?
        .ok_or_else(|| {
            EngineError::new(
                WorkerFailureCode::OutputRejected,
                "the script's result is not JSON-compatible",
            )
        })?;
    let bytes = serde_json::to_vec(&output)
        .map_err(|error| EngineError::new(WorkerFailureCode::OutputRejected, error.to_string()))?;
    if bytes.len() > output_bytes {
        return Err(EngineError::limit(format!(
            "the script's output is {} bytes; limit is {output_bytes} bytes",
            bytes.len()
        )));
    }
    Ok(ScriptEnding::Returned { output })
}

/// What a script threw: its `message` when it has one, as a rejected call's
/// `{ reason, code, message }` does, or the value as text.
fn thrown_message(thrown: &JsValue, context: &mut Context) -> String {
    let message = thrown
        .as_object()
        .and_then(|object| object.get(js_string!("message"), context).ok())
        .filter(|message| message.is_string());
    message
        .unwrap_or_else(|| thrown.clone())
        .to_string(context)
        .map_or_else(
            |_| "the script threw a value with no text".to_owned(),
            |text| text.to_std_string_escaped(),
        )
}

/// A rejected call's reason: `{ reason, code, message }`, frozen.
fn rejection(value: serde_json::Value, context: &mut Context) -> Result<JsValue, EngineError> {
    let reason = JsValue::from_json(&value, context)
        .map_err(|error| engine_failure("materializing a call's rejection", error))?;
    if let Some(object) = reason.as_object() {
        object
            .set_integrity_level(IntegrityLevel::Frozen, context)
            .map_err(|error| engine_failure("freezing a call's rejection", error))?;
    }
    Ok(reason)
}

fn take_calls(ledger: &Shared) -> Vec<ScriptCall> {
    std::mem::take(&mut ledger.borrow_mut().calls)
}

/// `ctx`: `skill`, `tool`, `operation` and `now`, frozen.
fn script_context(ledger: &Shared, context: &mut Context) -> Result<JsValue, EngineError> {
    let functions: [(&str, usize, CallFn); 4] = [
        ("skill", 2, skill),
        ("tool", 2, tool),
        ("operation", 3, operation),
        ("now", 0, now),
    ];
    let mut built = Vec::with_capacity(functions.len());
    for (name, length, function) in functions {
        let object = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_copy_closure_with_captures(function, ledger.clone()),
        )
        .name(js_string!(name))
        .length(length)
        .build();
        object
            .set_integrity_level(IntegrityLevel::Frozen, context)
            .map_err(|error| engine_failure("freezing the script's ctx", error))?;
        built.push((name, object));
    }
    let mut initializer = ObjectInitializer::new(context);
    for (name, object) in built {
        initializer.property(js_string!(name), object, Attribute::default());
    }
    let ctx = initializer.build();
    ctx.set_integrity_level(IntegrityLevel::Frozen, context)
        .map_err(|error| engine_failure("freezing the script's ctx", error))?;
    Ok(ctx.into())
}

type CallFn = fn(&JsValue, &[JsValue], &Shared, &mut Context) -> JsResult<JsValue>;

fn skill(
    _this: &JsValue,
    arguments: &[JsValue],
    ledger: &Shared,
    context: &mut Context,
) -> JsResult<JsValue> {
    let reference = text(arguments.get_or_undefined(0), "ctx.skill", "a skill#method")?;
    let inputs = inputs(arguments.get_or_undefined(1), "ctx.skill", context)?;
    call(
        ledger,
        ScriptCallTarget::Skill { reference, inputs },
        context,
    )
}

fn tool(
    _this: &JsValue,
    arguments: &[JsValue],
    ledger: &Shared,
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = text(arguments.get_or_undefined(0), "ctx.tool", "a tool's name")?;
    let inputs = inputs(arguments.get_or_undefined(1), "ctx.tool", context)?;
    call(ledger, ScriptCallTarget::Tool { name, inputs }, context)
}

fn operation(
    _this: &JsValue,
    arguments: &[JsValue],
    ledger: &Shared,
    context: &mut Context,
) -> JsResult<JsValue> {
    let operation = text(
        arguments.get_or_undefined(0),
        "ctx.operation",
        "<connector>/<operation>",
    )?;
    let inputs = inputs(arguments.get_or_undefined(1), "ctx.operation", context)?;
    let options = arguments.get_or_undefined(2);
    let idempotency_key = match options.as_object() {
        None if options.is_undefined() => None,
        None => {
            return Err(JsNativeError::typ()
                .with_message("ctx.operation's options are an object")
                .into());
        }
        Some(options) => {
            let key = options.get(js_string!("idempotency_key"), context)?;
            if key.is_undefined() {
                None
            } else {
                Some(text(&key, "ctx.operation", "an idempotency_key string")?)
            }
        }
    };
    call(
        ledger,
        ScriptCallTarget::Operation {
            operation,
            inputs,
            idempotency_key,
        },
        context,
    )
}

/// `ctx.now()`: what the record holds at this position, or the pass's time,
/// which the kernel then records there.
fn now(
    _this: &JsValue,
    _arguments: &[JsValue],
    ledger: &Shared,
    _context: &mut Context,
) -> JsResult<JsValue> {
    let mut ledger = ledger.borrow_mut();
    let position = ledger.next;
    ledger.next = position.saturating_add(1);
    let millis = ledger
        .recorded_now
        .get(&position)
        .copied()
        .unwrap_or(ledger.now);
    ledger.calls.push(ScriptCall {
        position,
        target: ScriptCallTarget::Now { millis },
    });
    // Milliseconds since the epoch stay well inside a double's exact range.
    #[allow(clippy::cast_precision_loss)]
    Ok(JsValue::from(millis as f64))
}

/// Take the next position for `target`, and hand the script a promise only
/// the pass settles.
fn call(ledger: &Shared, target: ScriptCallTarget, context: &mut Context) -> JsResult<JsValue> {
    let (promise, resolvers) = JsPromise::new_pending(context);
    let mut ledger = ledger.borrow_mut();
    let position = ledger.next;
    ledger.next = position.saturating_add(1);
    ledger.calls.push(ScriptCall { position, target });
    ledger.pending.insert(position, resolvers);
    Ok(promise.into())
}

fn text(value: &JsValue, call: &str, what: &str) -> JsResult<String> {
    value
        .as_string()
        .and_then(|value| value.to_std_string().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message(format!("{call} takes {what}"))
                .into()
        })
}

/// A call's inputs: an object, or nothing for none.
fn inputs(value: &JsValue, call: &str, context: &mut Context) -> JsResult<serde_json::Value> {
    if value.is_undefined() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    match value.to_json(context)? {
        Some(inputs @ serde_json::Value::Object(_)) => Ok(inputs),
        _ => Err(JsError::from(
            JsNativeError::typ().with_message(format!("{call}'s inputs are a JSON object")),
        )),
    }
}

#[cfg(test)]
mod tests;
