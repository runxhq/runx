//! performance.md's journey 3, composition, counted: what settling a child
//! call costs at 10, 100 and 1,000 children and at depth 1, 3 and 5, whether
//! a slow provider read delays another call recording its result, and the
//! most provider calls a run's nested scripts have in flight at once.

use std::error::Error;
use std::fs;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::atomic::Ordering;

use runx_contracts::{
    ExecutionEvent, JsonNumber, JsonObject, JsonValue, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor,
};
use runx_runtime::adapters::javascript::JavaScriptAdapter;
use runx_runtime::host::DetachedHost;
use runx_runtime::work_counts::{WorkCounts, snapshot};
use runx_runtime::{Host, NoopHost, RunAuthority, RunLog, Runtime};

use super::{
    CREATED_AT, CROWD, CROWDED_TEXT, GATED_TEXT, ProviderBenchmarkTransport, RuntimePathFixtures,
    method_workflow,
};

/// The children a run calls, one after another: one, the base each larger
/// run is read against, then 10, 100 and 1,000.
const CHILD_CALLS: [usize; 4] = [1, 10, 100, 1000];
/// The depths a leaf call settles at, each read from a run of one leaf call
/// and a run of ten at that depth.
const DEPTHS: [usize; 3] = [1, 3, 5];
const DEPTH_CALLS: [usize; 2] = [1, 10];
/// The slow read's siblings: native children, more than a script's slots,
/// so some start only as others settle.
const FAST_CHILDREN: usize = 15;
/// The crowd: child scripts the run calls at once, and the provider calls
/// each makes at once, its declared concurrency. Each child alone could fill
/// the run's [`CROWD`], and together they ask for more.
const CROWD_CHILDREN: usize = 2;
const CROWD_CALLS: usize = CROWD;

impl RuntimePathFixtures {
    /// Journey 3, composition, counted.
    ///
    /// A child call's cost is the run's past a one-call run's, over the calls
    /// past the first, so a settle that costs the same however many came
    /// before reads the same at every size; a deep leaf's is read the same
    /// way at its depth. The rows the kernel store reads count each read of a
    /// run's record, a replay included.
    ///
    /// The slow read's run calls one child whose provider operation waits on
    /// a gate, beside native children, and its host opens the gate only once
    /// every other child has sealed. The driver hands a call's reports to its
    /// host and records the call in one step, one call at a time, so the
    /// order the host is handed them is the order the calls are recorded in.
    /// Each sibling handed back after the slow child was delayed by it.
    ///
    /// The crowd's run, under a `max_concurrency` of [`CROWD`], calls
    /// [`CROWD_CHILDREN`] child scripts at once, each declaring a concurrency
    /// of [`CROWD`] and making that many provider calls at once, and each
    /// provider call waits until [`CROWD`] are in flight: the most in flight
    /// is the run's peak. It reaches [`CROWD`] only if nested scripts keep
    /// their concurrency, and passes it if they don't draw from the run's.
    pub(super) fn composition(&self) -> Result<JsonValue, Box<dyn Error>> {
        let packages = self.root().join("composition");
        write_packages(&packages)?;

        let mut wide = Vec::with_capacity(CHILD_CALLS.len());
        for calls in CHILD_CALLS {
            wide.push((calls, self.settled(&packages, 1, calls)?));
        }
        let mut deep = Vec::with_capacity(DEPTHS.len());
        for depth in DEPTHS {
            let [one, ten] = DEPTH_CALLS.map(|calls| self.settled(&packages, depth, calls));
            deep.push((depth, one?, ten?));
        }
        let (base_calls, base) = wide[0];
        let per_size = |count: fn(&Counts) -> u64| {
            wide[1..]
                .iter()
                .map(|(calls, counts)| {
                    (
                        calls.to_string(),
                        per_call(*calls - base_calls, count(counts), count(&base)),
                    )
                })
                .collect::<JsonObject>()
        };
        let per_depth = |count: fn(&Counts) -> u64| {
            deep.iter()
                .map(|(depth, one, ten)| {
                    (
                        depth.to_string(),
                        per_call(DEPTH_CALLS[1] - DEPTH_CALLS[0], count(ten), count(one)),
                    )
                })
                .collect::<JsonObject>()
        };
        let delayed = self.slow_read(&packages)?;
        let peak = self.crowd(&packages)?;
        let runs = wide
            .iter()
            .map(|(calls, counts)| (calls.to_string(), counts.json()))
            .collect::<JsonObject>();
        let depths = deep
            .iter()
            .map(|(depth, one, ten)| {
                (
                    depth.to_string(),
                    JsonValue::Object(JsonObject::from([
                        (DEPTH_CALLS[0].to_string(), one.json()),
                        (DEPTH_CALLS[1].to_string(), ten.json()),
                    ])),
                )
            })
            .collect::<JsonObject>();
        Ok(JsonValue::Object(JsonObject::from([
            (
                "settle_reads_per_call".to_owned(),
                JsonValue::Object(per_size(|counts| counts.store_rows_read)),
            ),
            (
                "settle_writes_per_call".to_owned(),
                JsonValue::Object(per_size(|counts| counts.store_writes)),
            ),
            (
                "settle_replays_per_call".to_owned(),
                JsonValue::Object(per_size(|counts| counts.run_materializations)),
            ),
            (
                "settle_reads_at_depth".to_owned(),
                JsonValue::Object(per_depth(|counts| counts.store_rows_read)),
            ),
            (
                "settle_writes_at_depth".to_owned(),
                JsonValue::Object(per_depth(|counts| counts.store_writes)),
            ),
            ("results_delayed_by_slow_read".to_owned(), count(delayed)),
            ("peak_concurrency".to_owned(), count(peak)),
            ("max_concurrency".to_owned(), count(CROWD as u64)),
            ("runs".to_owned(), JsonValue::Object(runs)),
            ("depths".to_owned(), JsonValue::Object(depths)),
        ])))
    }

    /// What a run of the script at `depth` making `calls` leaf calls, one
    /// after another, counted.
    fn settled(
        &self,
        packages: &Path,
        depth: usize,
        calls: usize,
    ) -> Result<Counts, Box<dyn Error>> {
        let package = packages.join(format!("depth-{depth}"));
        let workflow = method_workflow(
            &format!("depth-{depth}"),
            &script_method(
                &script_module(depth),
                &format!("calls: {calls}"),
                &script_calls(depth),
                calls.max(1),
                1,
            ),
        )?;
        let before = snapshot();
        let run = self
            .next_run(&self.context_runtime)
            .run_workflow_with_host(&package, workflow, &mut NoopHost)?;
        let after = snapshot();
        let result = run
            .steps
            .last()
            .and_then(|step| step.contract.get("calls"))
            .and_then(JsonValue::as_u64);
        if result != Some(u64::try_from(calls)?) {
            return Err(std::io::Error::other(format!(
                "a run of {calls} leaf calls at depth {depth} settled {result:?}"
            ))
            .into());
        }
        Ok(Counts::between(before, after))
    }

    /// How many of the slow read's siblings were recorded after it.
    fn slow_read(&self, packages: &Path) -> Result<u64, Box<dyn Error>> {
        let state = &self.provider_transport.state;
        state.gate.set(false);
        let workflow = method_workflow("slow", &slow_method())?;
        let mut host = CompositionHost::opening_after(&self.provider_transport, FAST_CHILDREN);
        self.next_run(&self.provider_runtime)
            .run_workflow_with_host(&packages.join("slow"), workflow, &mut host)?;
        let leaf = |message: &&String| message.starts_with("workflow leaf ");
        let sealed = &host.sealed;
        let leaves = sealed.iter().filter(leaf).count();
        let slow = sealed
            .iter()
            .position(|message| message.starts_with("workflow slow-reader "))
            .ok_or("the slow read's child never sealed")?;
        if leaves != FAST_CHILDREN {
            return Err(std::io::Error::other(format!(
                "the slow read's run sealed {leaves} of its {FAST_CHILDREN} native children: {sealed:?}"
            ))
            .into());
        }
        Ok(u64::try_from(sealed[slow..].iter().filter(leaf).count())?)
    }

    /// The most provider calls the crowd's nested scripts had in flight at
    /// once.
    fn crowd(&self, packages: &Path) -> Result<u64, Box<dyn Error>> {
        let state = &self.provider_transport.state;
        state.most_crowded.store(0, Ordering::Relaxed);
        let mut options = self.provider_options.clone();
        options.run = RunLog::evaluation(
            std::sync::Arc::clone(&self.store),
            "run_composition_crowd",
            "workflow-throughput",
            "run_composition_crowd",
            CREATED_AT,
        )?;
        options.run_authority = Some(RunAuthority {
            max_concurrency: NonZeroU32::new(u32::try_from(CROWD)?),
            ..RunAuthority::default()
        });
        let runtime = Runtime::new(JavaScriptAdapter::default(), options);
        let workflow = method_workflow("crowd", &crowd_method())?;
        let attempts = self.provider_transport.operation_attempts();
        let mut host = CompositionHost::default();
        runtime.run_workflow_with_host(&packages.join("crowd"), workflow, &mut host)?;
        let sent = self.provider_transport.operation_attempts() - attempts;
        let expected = u64::try_from(CROWD_CHILDREN * CROWD_CALLS)?;
        if sent != expected {
            return Err(std::io::Error::other(format!(
                "the crowd's run sent {sent} of its {expected} provider calls"
            ))
            .into());
        }
        Ok(state.most_crowded.load(Ordering::Relaxed))
    }
}

/// A run's host for journey 3: it serves each call apart from its driver,
/// approving what a call asks, and keeps each child run's seal as it's
/// handed it, opening the transport's gate once it has `open_after`.
#[derive(Default)]
struct CompositionHost {
    gate: Option<(ProviderBenchmarkTransport, usize)>,
    sealed: Vec<String>,
}

impl CompositionHost {
    fn opening_after(transport: &ProviderBenchmarkTransport, sealed: usize) -> Self {
        Self {
            gate: Some((transport.clone(), sealed)),
            sealed: Vec::new(),
        }
    }
}

fn approved() -> ResolutionResponse {
    ResolutionResponse {
        actor: ResolutionResponseActor::Human,
        payload: JsonValue::Bool(true),
    }
}

impl Host for CompositionHost {
    fn report(&mut self, event: ExecutionEvent) -> Result<(), runx_runtime::RuntimeError> {
        let ExecutionEvent::Completed { message, data } = event else {
            return Ok(());
        };
        let sealed_a_run = data
            .as_ref()
            .and_then(JsonValue::as_object)
            .and_then(|data| data.get("kind"))
            .and_then(JsonValue::as_str)
            == Some("run_sealed");
        if sealed_a_run {
            self.sealed.push(message);
            if let Some((transport, after)) = &self.gate
                && self.sealed.len() == *after
            {
                transport.state.gate.set(true);
            }
        }
        Ok(())
    }

    fn resolve(
        &mut self,
        _request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, runx_runtime::RuntimeError> {
        Ok(Some(approved()))
    }

    fn log(&mut self, _message: String) -> Result<(), runx_runtime::RuntimeError> {
        Ok(())
    }

    fn detached(&self) -> Option<DetachedHost> {
        Some(DetachedHost::new(Box::new(|_| Ok(Some(approved())))))
    }
}

/// What one of journey 3's runs counted.
#[derive(Clone, Copy)]
struct Counts {
    store_rows_read: u64,
    store_writes: u64,
    run_materializations: u64,
}

impl Counts {
    fn between(before: WorkCounts, after: WorkCounts) -> Self {
        Self {
            store_rows_read: after.store_rows_read - before.store_rows_read,
            store_writes: after.store_writes - before.store_writes,
            run_materializations: after.run_materializations - before.run_materializations,
        }
    }

    fn json(self) -> JsonValue {
        JsonValue::Object(JsonObject::from([
            ("store_rows_read".to_owned(), count(self.store_rows_read)),
            ("store_writes".to_owned(), count(self.store_writes)),
            (
                "run_materializations".to_owned(),
                count(self.run_materializations),
            ),
        ]))
    }
}

fn count(value: u64) -> JsonValue {
    JsonValue::Number(JsonNumber::U64(value))
}

/// What each of `calls` more calls cost, from a run's `total` and the
/// `base` run's.
fn per_call(calls: usize, total: u64, base: u64) -> JsonValue {
    let added = total.saturating_sub(base);
    let calls = u64::try_from(calls).unwrap_or(u64::MAX);
    JsonValue::Number(if added.is_multiple_of(calls) {
        JsonNumber::U64(added / calls)
    } else {
        JsonNumber::F64(added as f64 / calls as f64)
    })
}

/// A method of one script step, `module`, given `inputs` and allowed
/// `calls`, up to `max_calls` of them, `concurrency` at once.
fn script_method(
    module: &str,
    inputs: &str,
    calls: &str,
    max_calls: usize,
    concurrency: usize,
) -> String {
    format!(
        "minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n      - id: script\n        script: {{module: {module}}}\n        inputs: {{{inputs}}}\n        calls: {{{calls}}}\n        max_calls: {max_calls}\n        concurrency: {concurrency}\n        outputs:\n          calls: {{type: number}}\n"
    )
}

/// The slow read's run: one child whose read waits on the gate, beside
/// [`FAST_CHILDREN`] native ones, as many at once as the crowd.
fn slow_method() -> String {
    script_method(
        "slow.mjs",
        &format!("children: {FAST_CHILDREN}"),
        "skills: [../leaf, ../slow-reader]",
        FAST_CHILDREN + 1,
        CROWD,
    )
}

/// The crowd's run: [`CROWD_CHILDREN`] child scripts at once.
fn crowd_method() -> String {
    script_method(
        "crowd.mjs",
        &format!("children: {CROWD_CHILDREN}"),
        "skills: [../crowd-child]",
        CROWD_CHILDREN,
        CROWD_CHILDREN,
    )
}

/// The script at `depth`: the first calls the leaf, each deeper one the
/// script a level up.
fn script_module(depth: usize) -> String {
    if depth == 1 {
        "calls.mjs".to_owned()
    } else {
        "nest.mjs".to_owned()
    }
}

fn script_calls(depth: usize) -> String {
    if depth == 1 {
        "skills: [../leaf]".to_owned()
    } else {
        format!("skills: [../depth-{}]", depth - 1)
    }
}

fn write_package(
    root: &Path,
    name: &str,
    run_yaml: &str,
    modules: &[(&str, &str)],
) -> Result<(), Box<dyn Error>> {
    let package = root.join(name);
    fs::create_dir_all(&package)?;
    fs::write(
        package.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Journey 3's {name}.\n---\n# {name}\n"),
    )?;
    fs::write(package.join("run.yaml"), run_yaml)?;
    for (module, source) in modules {
        fs::write(package.join(module), source)?;
    }
    Ok(())
}

/// Journey 3's packages: a native leaf, the scripts that call it at each
/// depth, the slow read's child and run, and the crowd's.
fn write_packages(root: &Path) -> Result<(), Box<dyn Error>> {
    write_package(
        root,
        "leaf",
        "minimum_runx: 0.12.0\nmethods:\n  digest:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n",
        &[],
    )?;
    let calls = "export default async function (ctx, { calls }) {\n  for (let call = 0; call < calls; call += 1) {\n    await ctx.skill(\"../leaf\", { value: String(call) });\n  }\n  return { calls };\n}\n";
    for depth in 1..=*DEPTHS.iter().max().unwrap_or(&1) {
        let method = format!(
            "minimum_runx: 0.12.0\nmethods:\n  run:\n    inputs:\n      calls: {{type: integer}}\n    steps:\n      - id: script\n        script: {{module: {module}}}\n        inputs: {{calls: $input.calls}}\n        calls: {{{calls}}}\n        max_calls: {max_calls}\n        outputs:\n          calls: {{type: number}}\n",
            module = script_module(depth),
            calls = script_calls(depth),
            max_calls = CHILD_CALLS[CHILD_CALLS.len() - 1],
        );
        let nest = format!(
            "export default async function (ctx, {{ calls }}) {{\n  await ctx.skill(\"../depth-{}\", {{ calls }});\n  return {{ calls }};\n}}\n",
            depth.saturating_sub(1)
        );
        let module = if depth == 1 { calls } else { nest.as_str() };
        write_package(
            root,
            &format!("depth-{depth}"),
            &method,
            &[(script_module(depth).as_str(), module)],
        )?;
    }
    write_package(
        root,
        "slow-reader",
        &format!(
            "minimum_runx: 0.12.0\nmethods:\n  read:\n    steps:\n      - id: read\n        operation: fixture/chat/thread.reply\n        idempotency_key: composition-slow-read\n        inputs:\n          thread_locator: chat://T1/C1/1.0\n          text: {GATED_TEXT}\n"
        ),
        &[],
    )?;
    write_package(
        root,
        "slow",
        &slow_method(),
        &[(
            "slow.mjs",
            "export default async function (ctx, { children }) {\n  const calls = [ctx.skill(\"../slow-reader\", {})];\n  for (let child = 0; child < children; child += 1) {\n    calls.push(ctx.skill(\"../leaf\", { value: String(child) }));\n  }\n  await Promise.all(calls);\n  return { calls: calls.length };\n}\n",
        )],
    )?;
    write_package(
        root,
        "crowd-child",
        &format!(
            "minimum_runx: 0.12.0\nmethods:\n  run:\n    inputs:\n      child: {{type: integer}}\n    steps:\n      - id: script\n        script: {{module: child.mjs}}\n        inputs: {{child: $input.child}}\n        calls: {{operations: [fixture/chat/thread.reply]}}\n        max_calls: {CROWD_CALLS}\n        concurrency: {CROWD_CALLS}\n        outputs:\n          calls: {{type: number}}\n"
        ),
        &[(
            "child.mjs",
            &format!(
                "export default async function (ctx, {{ child }}) {{\n  const calls = [];\n  for (let call = 0; call < {CROWD_CALLS}; call += 1) {{\n    calls.push(ctx.operation(\"fixture/chat/thread.reply\", {{ thread_locator: \"chat://T1/C1/1.0\", text: \"{CROWDED_TEXT}\" }}, {{ idempotency_key: `composition-crowd-${{child}}-${{call}}` }}));\n  }}\n  await Promise.all(calls);\n  return {{ calls: calls.length }};\n}}\n"
            ),
        )],
    )?;
    write_package(
        root,
        "crowd",
        &crowd_method(),
        &[(
            "crowd.mjs",
            "export default async function (ctx, { children }) {\n  const calls = [];\n  for (let child = 0; child < children; child += 1) {\n    calls.push(ctx.skill(\"../crowd-child\", { child }));\n  }\n  await Promise.all(calls);\n  return { calls: calls.length };\n}\n",
        )],
    )?;
    Ok(())
}
