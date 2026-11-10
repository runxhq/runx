//! performance.md's journey 4, long runs, counted: a run read with nothing
//! new since its cursor, a resume and its recovery, and a cancel, each in runs
//! of 10, 100 and 1,000 calls and children, and a receipt append to stores of
//! 16, 128 and 1,000 receipts.
//!
//! Each run is made as a caller makes one, in a workspace of its own: a
//! script's tool calls, or its skill calls, each a child run, and then a gate
//! the run holds on for a person; or a host-driven turn whose first call
//! waits on a person and whose other calls the host makes through its turn
//! session. With the run held, a read from the cursor that reached its end finds nothing new,
//! and a resume answers what it waits on and takes it to where it holds
//! again. A resume binds the run from its record, which is its recovery, so
//! the reads of each stream's events it makes are counted too.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use runx_contracts::{JsonNumber, JsonObject, JsonValue, ResolutionRequest};
use runx_runtime::work_counts::{WorkCounts, snapshot};
use runx_runtime::{
    CallCancel, ConnectorScope, HOSTED_API_BASE_URL_ENV, HOSTED_API_TOKEN_ENV, HttpMethod,
    Instances, KernelStore, LoadedConnector, LocalOrchestrator, ManagedAgentPolicy,
    OperationEffect, ResumeRequest, RuntimeEffectRegistry, RuntimeHttpError, RuntimeHttpRequest,
    RuntimeHttpResponse, RuntimeHttpTransport, RunxPaths, SkillRunRequest, TurnRequest,
};
use tempfile::TempDir;

const SIZES: [(&str, usize); 3] = [("10", 10), ("100", 100), ("1000", 1000)];
const RECEIPT_STORES: [(&str, usize); 3] = [("16", 16), ("128", 128), ("1000", 1000)];

#[allow(clippy::expect_used)]
pub(super) fn register() {
    let (journey, turn_calls) =
        long_runs().expect("the long-runs journey must read, resume and append at each size");
    crate::record_journey("4", journey).expect("the long-runs journey must be recorded");
    // Settling a call at a turn waiting on a person is journey 3's budget.
    crate::record_journey_measure("3", "turn_call_rows_read", turn_calls)
        .expect("the rows a turn's last call read must be recorded");
}

/// The shapes of a long run.
#[derive(Clone, Copy)]
enum Shape {
    /// A script's tool calls, then a gate it holds on.
    Calls,
    /// A script's skill calls, each a child run, then a gate it holds on.
    Children,
    /// A turn whose first call waits on a person, with the host's calls
    /// after it.
    WaitingTurn,
}

impl Shape {
    const ALL: [Self; 3] = [Self::Calls, Self::Children, Self::WaitingTurn];

    fn name(self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::Children => "children",
            Self::WaitingTurn => "waiting_turn",
        }
    }
}

/// Journey 4's measurements, and the rows the last call at a turn of each
/// size read.
fn long_runs() -> Result<(JsonValue, JsonValue), Box<dyn Error>> {
    let mut read_rows = JsonObject::new();
    let mut resume_rows = JsonObject::new();
    let mut most_reads_of_one_stream = 0_u64;
    let mut turn_call_rows = JsonObject::new();
    let mut runs = JsonObject::new();
    for shape in Shape::ALL {
        let mut shape_read_rows = JsonObject::new();
        let mut shape_resume_rows = JsonObject::new();
        let mut shape_runs = JsonObject::new();
        for (label, size) in SIZES {
            let measured = LongRun::measure(shape, size)?;
            shape_read_rows.insert(label.to_owned(), count(measured.read.store_rows_read));
            shape_resume_rows.insert(label.to_owned(), count(measured.resume.store_rows_read));
            let most = measured.stream_reads.values().copied().max().unwrap_or(0);
            most_reads_of_one_stream = most_reads_of_one_stream.max(most);
            if let Some(call) = measured.last_turn_call {
                turn_call_rows.insert(label.to_owned(), count(call.store_rows_read));
            }
            shape_runs.insert(label.to_owned(), measured.json(most));
        }
        read_rows.insert(shape.name().to_owned(), JsonValue::Object(shape_read_rows));
        resume_rows.insert(
            shape.name().to_owned(),
            JsonValue::Object(shape_resume_rows),
        );
        runs.insert(shape.name().to_owned(), JsonValue::Object(shape_runs));
    }
    let journey = JsonValue::Object(JsonObject::from([
        (
            "run_read_rows_with_nothing_new".to_owned(),
            JsonValue::Object(read_rows),
        ),
        (
            "resume_rows_read".to_owned(),
            JsonValue::Object(resume_rows),
        ),
        (
            "recovery_reads_per_stream".to_owned(),
            count(most_reads_of_one_stream),
        ),
        ("cancel_rows_read".to_owned(), cancels()?),
        ("receipt_append".to_owned(), receipt_appends()?),
        ("runs".to_owned(), JsonValue::Object(runs)),
    ]));
    Ok((journey, JsonValue::Object(turn_call_rows)))
}

/// What one run of a shape and size counted.
struct LongRun {
    /// The read from the cursor that reached the run's end.
    read: Counted,
    /// The resume, from where the run held to where it holds again.
    resume: Counted,
    /// The most times the resume read any one event of each stream.
    stream_reads: BTreeMap<String, u64>,
    /// At a turn, the last of its calls, made after all the others.
    last_turn_call: Option<Counted>,
}

impl LongRun {
    fn measure(shape: Shape, size: usize) -> Result<Self, Box<dyn Error>> {
        let workspace = Workspace::new()?;
        let (run_id, last_turn_call) = workspace.start(shape, size)?;
        let (cursor, waiting) = workspace.read_to_the_end(&run_id)?;
        let (read, counted_read) = counted(|| {
            runx_runtime::run_record::read_run(
                &workspace.env,
                workspace.root(),
                &run_id,
                Some(&cursor),
            )
        });
        let read = read?;
        if !read.events.is_empty() {
            return Err(format!(
                "a read of run {run_id} from the cursor at its end found {} events",
                read.events.len()
            )
            .into());
        }
        let ((resumed, stream_reads), counted_resume) = counted(|| {
            runx_runtime::probe_fixtures::stream_reads(|| workspace.resume(&run_id, &waiting))
        });
        let resumed = resumed?;
        let field = |request: &JsonValue, name: &str| {
            request
                .as_object()
                .and_then(|request| request.get(name))
                .and_then(JsonValue::as_str)
                .map(str::to_owned)
        };
        let holds = resumed.pending_requests.iter().any(|request| match shape {
            Shape::Calls | Shape::Children => {
                field(request, "id").as_deref() == Some("long-run.again")
            }
            Shape::WaitingTurn => field(request, "kind").as_deref() == Some("turn"),
        });
        if !holds {
            return Err(format!(
                "run {run_id} resumed to {:?} without holding again: {:?}",
                resumed.status, resumed.pending_requests
            )
            .into());
        }
        Ok(Self {
            read: counted_read,
            resume: counted_resume,
            stream_reads,
            last_turn_call,
        })
    }

    fn json(&self, most_reads_of_one_stream: u64) -> JsonValue {
        JsonValue::Object(JsonObject::from([
            ("read".to_owned(), self.read.json()),
            ("resume".to_owned(), self.resume.json()),
            (
                "last_turn_call".to_owned(),
                self.last_turn_call.map_or(JsonValue::Null, Counted::json),
            ),
            (
                "resume_streams_read".to_owned(),
                count(self.stream_reads.len() as u64),
            ),
            (
                "resume_most_reads_of_one_stream".to_owned(),
                count(most_reads_of_one_stream),
            ),
        ]))
    }
}

/// What a run waits on for a person: each request's address and digest.
type Waiting = Vec<(String, String)>;

/// A workspace holding the long-run package and the skills it calls, with a
/// home of its own.
struct Workspace {
    directory: TempDir,
    env: BTreeMap<String, String>,
}

impl Workspace {
    fn new() -> Result<Self, Box<dyn Error>> {
        let directory = TempDir::new()?;
        for (path, text) in PACKAGE {
            let path = directory.path().join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, text)?;
        }
        fs::create_dir_all(directory.path().join("home"))?;
        let mut env = std::env::vars().collect::<BTreeMap<_, _>>();
        env.insert(
            "RUNX_CWD".to_owned(),
            directory.path().to_string_lossy().into_owned(),
        );
        env.insert(
            "RUNX_HOME".to_owned(),
            directory.path().join("home").to_string_lossy().into_owned(),
        );
        Ok(Self { directory, env })
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn orchestrator() -> Result<LocalOrchestrator, Box<dyn Error>> {
        Ok(LocalOrchestrator::with_effects_and_environment(
            runx_runtime::RuntimeEffectRegistry::local()?,
            BTreeMap::new(),
        ))
    }

    /// A run of `shape` and `size`, held where it waits on a person, and at
    /// a turn what its last call counted.
    fn start(
        &self,
        shape: Shape,
        size: usize,
    ) -> Result<(String, Option<Counted>), Box<dyn Error>> {
        let run_id = format!("run_long_{}_{size}", shape.name());
        let (method, inputs) = match shape {
            Shape::Calls => ("calls", serde_json::json!({"calls": size})),
            Shape::Children => ("children", serde_json::json!({"children": size})),
            Shape::WaitingTurn => ("turn", serde_json::json!({})),
        };
        let started = Self::orchestrator()?.run_skill_with_binding(
            &SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: None,
                act: None,
                skill_path: self.root().join("skills/long-run"),
                receipt_dir: Some(self.root().join("receipts")),
                run_id: Some(run_id.clone()),
                answers: None,
                inputs: serde_json::from_value(inputs)?,
                env: self.env.clone(),
                cwd: self.root().to_path_buf(),
                managed_agent: ManagedAgentPolicy::HostDriven,
            },
            Some(method),
            None,
            None,
        )?;
        if started.pending_requests.is_empty() {
            return Err(format!("run {run_id} holds on nothing: {:?}", started.status).into());
        }
        let last_turn_call = match shape {
            Shape::WaitingTurn => Some(self.make_turn_calls(&run_id, size)?),
            Shape::Calls | Shape::Children => None,
        };
        Ok((run_id, last_turn_call))
    }

    /// At the run's turn, a skill call that waits on its child's gate, then
    /// `calls` digests, each settled, and what the last of them counted.
    fn make_turn_calls(&self, run_id: &str, calls: usize) -> Result<Counted, Box<dyn Error>> {
        let session = Self::orchestrator()?.open_turns(&TurnRequest {
            run_id: run_id.to_owned(),
            receipt_dir: Some(self.root().join("receipts")),
            env: self.env.clone(),
            cwd: self.root().to_path_buf(),
        })?;
        let turn = session
            .waiting()?
            .into_iter()
            .find_map(|request| match request {
                ResolutionRequest::Turn { id, .. } => Some(id),
                _ => None,
            })
            .ok_or_else(|| format!("run {run_id} has no open turn"))?;
        let waits = session.call(
            &turn,
            "skill__asker",
            None,
            serde_json::from_value(serde_json::json!({"act": {"purpose": "hold the turn"}}))?,
        )?;
        if !matches!(
            waits.outcome,
            runx_contracts::TurnCallOutcome::Pending { .. }
        ) {
            return Err(format!("the turn's first call didn't wait: {waits:?}").into());
        }
        let mut last = None;
        for index in 0..calls {
            let input = serde_json::from_value(
                serde_json::json!({"value": format!("call {index}"), "encoding": "utf8_text"}),
            )?;
            let (made, counted_call) = counted(|| session.call(&turn, "data_digest", None, input));
            let made = made?;
            last = Some(counted_call);
            if !matches!(
                made.outcome,
                runx_contracts::TurnCallOutcome::Completed { .. }
            ) {
                return Err(format!("call {index} of the turn didn't settle: {made:?}").into());
            }
        }
        Ok(last.ok_or("a turn of no calls has no last call")?)
    }

    /// The cursor that reaches the run's end, and what the run waits on there.
    fn read_to_the_end(&self, run_id: &str) -> Result<(String, Waiting), Box<dyn Error>> {
        let mut cursor: Option<String> = None;
        loop {
            let read = runx_runtime::run_record::read_run(
                &self.env,
                self.root(),
                run_id,
                cursor.as_deref(),
            )?;
            if !read.more {
                let waiting = read
                    .waiting
                    .iter()
                    .filter(|waited| !matches!(waited.request, ResolutionRequest::Turn { .. }))
                    .map(|waited| {
                        (
                            waited.address.as_str().to_owned(),
                            waited.request_digest.as_str().to_owned(),
                        )
                    })
                    .collect();
                return Ok((read.cursor.as_str().to_owned(), waiting));
            }
            cursor = Some(read.cursor.as_str().to_owned());
        }
    }

    /// Resume run `run_id`, approving each request in `waiting` by its
    /// address and digest.
    fn resume(
        &self,
        run_id: &str,
        waiting: &[(String, String)],
    ) -> Result<runx_runtime::RunResult, Box<dyn Error>> {
        let approvals = waiting
            .iter()
            .map(|(address, digest)| {
                (
                    address.clone(),
                    serde_json::json!({"request_digest": digest, "approved": true}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        Ok(Self::orchestrator()?.resume(&ResumeRequest {
            run_id: run_id.to_owned(),
            answers: serde_json::from_value(serde_json::json!({ "approvals": approvals }))?,
            receipt_dir: Some(self.root().join("receipts")),
            env: self.env.clone(),
            cwd: self.root().to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
            package_digest: None,
            execution_closure_digest: None,
        })?)
    }
}

/// The grant the cancelled read is made under.
const CANCEL_GRANT: &str = "grant_long_runs_cancel";

/// The rows a cancelled call read, at a turn that made 10, 100 and 1,000
/// digests: a read that reaches the world, held in a transport the bench
/// controls until its host has cancelled it from another thread.
fn cancels() -> Result<JsonValue, Box<dyn Error>> {
    let mut rows = JsonObject::new();
    for (label, calls) in SIZES {
        let counted = Workspace::new()?.cancel_after(calls)?;
        rows.insert(label.to_owned(), count(counted.store_rows_read));
    }
    Ok(JsonValue::Object(rows))
}

impl Workspace {
    /// At a turn that made `calls` digests, a read cancelled while it
    /// executes, and what it counted from its call to its settlement.
    fn cancel_after(&self, calls: usize) -> Result<Counted, Box<dyn Error>> {
        let chat = self.fixture_chat()?;
        let reads = GatedReads::for_grant(&chat)?;
        let mut env = self.env.clone();
        env.insert(
            HOSTED_API_BASE_URL_ENV.to_owned(),
            "https://api.runx.benchmark".to_owned(),
        );
        env.insert(HOSTED_API_TOKEN_ENV.to_owned(), "rxk_benchmark".to_owned());
        runx_runtime::kernel::setup::confirm_grant_over(
            &Instances::new(KernelStore::for_project(&RunxPaths::resolve(
                &env,
                self.root(),
            ))?),
            &chat,
            Arc::new(reads.clone()),
            CANCEL_GRANT,
            &env,
            self.root(),
        )?;
        let orchestrator = LocalOrchestrator::with_effects_and_environment(
            RuntimeEffectRegistry::with_effect(OperationEffect::with_http_transport(
                reads.clone(),
            ))?,
            BTreeMap::new(),
        );
        let run_id = format!("run_long_cancel_{calls}");
        orchestrator.run_skill_with_binding(
            &SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: None,
                act: None,
                skill_path: self.root().join("skills/canceller"),
                receipt_dir: Some(self.root().join("receipts")),
                run_id: Some(run_id.clone()),
                answers: None,
                inputs: BTreeMap::new(),
                env: env.clone(),
                cwd: self.root().to_path_buf(),
                managed_agent: ManagedAgentPolicy::HostDriven,
            },
            Some("answer"),
            None,
            None,
        )?;
        let session = orchestrator.open_turns(&TurnRequest {
            run_id: run_id.clone(),
            receipt_dir: Some(self.root().join("receipts")),
            env,
            cwd: self.root().to_path_buf(),
        })?;
        let turn = session
            .waiting()?
            .into_iter()
            .find_map(|request| match request {
                ResolutionRequest::Turn { id, .. } => Some(id),
                _ => None,
            })
            .ok_or_else(|| format!("run {run_id} has no open turn"))?;
        let read = session
            .status(&turn)?
            .tools
            .into_iter()
            .map(|tool| tool.name.as_str().to_owned())
            .find(|name| name.ends_with("thread_read"))
            .ok_or_else(|| format!("run {run_id}'s turn offers no thread read"))?;
        for index in 0..calls {
            let digest = serde_json::from_value(
                serde_json::json!({"value": format!("call {index}"), "encoding": "utf8_text"}),
            )?;
            session.call(&turn, "data_digest", None, digest)?;
        }
        let address = format!("{turn}/{calls}");
        let input = serde_json::from_value(
            serde_json::json!({"input": {"thread_locator": "chat://T1/C1/1.0"}}),
        )?;
        let (read, cancelled, counted) = std::thread::scope(|scope| {
            let before = snapshot();
            let reading = scope.spawn(|| {
                let read = session.call(&turn, &read, None, input);
                reads.ended();
                read
            });
            // The read is cancelled once it is in flight, at the transport,
            // and the bench lets its request return only then.
            let cancelled = reads
                .reached()
                .then(|| session.cancel(&address).ok())
                .flatten();
            reads.open();
            let read = reading.join();
            (read, cancelled, Counted::between(before, snapshot()))
        });
        let read = read.map_err(|_| "the cancelled read panicked")??;
        if cancelled != Some(CallCancel::Cancelled)
            || !matches!(read.outcome, runx_contracts::TurnCallOutcome::Cancelled)
        {
            return Err(format!(
                "the read at {address} wasn't cancelled: {cancelled:?}, {:?}",
                read.outcome
            )
            .into());
        }
        Ok(counted)
    }

    /// The kernel fixture plugin's chat connector, copied into the workspace
    /// as a project that installs it reaches it.
    fn fixture_chat(&self) -> Result<Arc<LoadedConnector>, Box<dyn Error>> {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/plugins/kernel/connectors/fixture/chat");
        let target = self.root().join("connectors/fixture/chat");
        fs::create_dir_all(&target)?;
        for entry in fs::read_dir(&source)? {
            let entry = entry?;
            if entry.path().is_file() {
                fs::copy(entry.path(), target.join(entry.file_name()))?;
            }
        }
        Ok(ConnectorScope::at(self.root())?.connector("fixture/chat")?)
    }
}

/// The hosted API a cancelled read reaches: it answers who the operator is
/// and the grant, and holds each operation until the bench opens it.
#[derive(Clone)]
struct GatedReads {
    grants: String,
    gate: Arc<(Mutex<Gate>, Condvar)>,
}

/// Where the read stands at the transport.
#[derive(Default)]
struct Gate {
    /// The read reached the transport.
    arrived: bool,
    /// The call making it returned.
    ended: bool,
    /// The bench lets it return.
    open: bool,
}

impl GatedReads {
    fn for_grant(connector: &LoadedConnector) -> Result<Self, Box<dyn Error>> {
        let grants = serde_json::to_string(&serde_json::json!({
            "status": "success",
            "grants": [{
                "grant_id": CANCEL_GRANT,
                "provider": connector.manifest.name.as_str(),
                "connector_binding": connector.binding(),
                "status": "active",
                "credential_material_bound": true,
                "scopes": ["thread.read"],
            }],
        }))?;
        Ok(Self {
            grants,
            gate: Arc::default(),
        })
    }

    fn update(&self, change: impl FnOnce(&mut Gate)) {
        let (gate, changed) = &*self.gate;
        change(&mut gate.lock().unwrap_or_else(PoisonError::into_inner));
        changed.notify_all();
    }

    /// Let every held operation return.
    fn open(&self) {
        self.update(|gate| gate.open = true);
    }

    /// The call making the read returned.
    fn ended(&self) {
        self.update(|gate| gate.ended = true);
    }

    /// Whether the read reached the transport, once it has or the call making
    /// it has returned.
    fn reached(&self) -> bool {
        let (gate, changed) = &*self.gate;
        let mut gate = gate.lock().unwrap_or_else(PoisonError::into_inner);
        while !(gate.arrived || gate.ended) {
            gate = changed.wait(gate).unwrap_or_else(PoisonError::into_inner);
        }
        gate.arrived
    }
}

impl RuntimeHttpTransport for GatedReads {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        if request.method == HttpMethod::Get && request.url.ends_with("/v1/me") {
            return Ok(RuntimeHttpResponse::new(
                200,
                r#"{"status":"success","principal":{"principal_id":"operator:benchmark"}}"#,
            ));
        }
        if request.method == HttpMethod::Get && request.url.ends_with("/v1/grants") {
            return Ok(RuntimeHttpResponse::new(200, self.grants.clone()));
        }
        if request.method != HttpMethod::Post || !request.url.ends_with("/v1/provider-operations") {
            return Err(RuntimeHttpError::Transport {
                message: format!("unexpected hosted API request {}", request.url),
            });
        }
        let (gate, changed) = &*self.gate;
        let mut gate = gate.lock().unwrap_or_else(PoisonError::into_inner);
        gate.arrived = true;
        changed.notify_all();
        while !gate.open {
            gate = changed.wait(gate).unwrap_or_else(PoisonError::into_inner);
        }
        drop(gate);
        Ok(RuntimeHttpResponse::new(
            200,
            serde_json::json!({
                "status": "success",
                "provider": "fixture/chat",
                "operation": "thread.read",
                "target": "chat://T1/C1/1.0",
                "access": "read",
                "operation_id": "benchmark-read",
                "result": {
                    "provider": "fixture/chat",
                    "external_tenant_ref": "T1",
                    "connected_subject_ref": "U1",
                    "thread_locator": "chat://T1/C1/1.0",
                    "messages": [],
                    "next_cursor": "",
                },
            })
            .to_string(),
        ))
    }
}

/// A receipt appended to stores of 16, 128 and 1,000 receipts, and the
/// receipt directory entries it listed and the index entries it read and
/// wrote.
fn receipt_appends() -> Result<JsonValue, Box<dyn Error>> {
    let signature = crate::fixture_signature();
    let mut appends = JsonObject::new();
    for (label, size) in RECEIPT_STORES {
        let mut receipts = crate::synthetic_receipts(&signature, size + 1);
        let pending = receipts
            .pop()
            .ok_or("a receipt store probe needs a receipt to append")?;
        let directory = TempDir::new()?;
        let store = runx_runtime::LocalReceiptStore::new(directory.path().join("receipts"));
        store.write_receipts(&receipts, signature.signature_policy())?;
        let (appended, counted) =
            counted(|| store.write_receipt(&pending, signature.signature_policy()));
        appended?;
        appends.insert(
            label.to_owned(),
            JsonValue::Object(JsonObject::from([
                (
                    "directory_entries_read".to_owned(),
                    count(counted.receipt_directory_entries_read),
                ),
                (
                    "index_entries_read".to_owned(),
                    count(counted.receipt_index_entries_read),
                ),
                (
                    "index_entries_written".to_owned(),
                    count(counted.receipt_index_entries_written),
                ),
            ])),
        );
    }
    Ok(JsonValue::Object(appends))
}

/// What `measure` returned, and what it counted.
fn counted<T>(measure: impl FnOnce() -> T) -> (T, Counted) {
    let before = snapshot();
    let measured = measure();
    (measured, Counted::between(before, snapshot()))
}

/// The work one measured action counted.
#[derive(Clone, Copy)]
struct Counted {
    store_rows_read: u64,
    store_writes: u64,
    run_materializations: u64,
    packages_read: u64,
    process_spawns: u64,
    receipt_directory_entries_read: u64,
    receipt_index_entries_read: u64,
    receipt_index_entries_written: u64,
}

impl Counted {
    fn between(before: WorkCounts, after: WorkCounts) -> Self {
        Self {
            store_rows_read: after.store_rows_read - before.store_rows_read,
            store_writes: after.store_writes - before.store_writes,
            run_materializations: after.run_materializations - before.run_materializations,
            packages_read: after.packages_read - before.packages_read,
            process_spawns: after.process_spawns - before.process_spawns,
            receipt_directory_entries_read: after.receipt_directory_entries_read
                - before.receipt_directory_entries_read,
            receipt_index_entries_read: after.receipt_index_entries_read
                - before.receipt_index_entries_read,
            receipt_index_entries_written: after.receipt_index_entries_written
                - before.receipt_index_entries_written,
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
            ("packages_read".to_owned(), count(self.packages_read)),
            ("process_spawns".to_owned(), count(self.process_spawns)),
        ]))
    }
}

fn count(value: u64) -> JsonValue {
    JsonValue::Number(JsonNumber::U64(value))
}

/// The long-run package and the two skills it calls: `child`, which digests
/// once, and `asker`, which holds on a person's gate; and `canceller`, whose
/// turn reads a chat thread, which only the cancel's workspace can reach.
const PACKAGE: [(&str, &str); 9] = [
    (
        "skills/long-run/SKILL.md",
        "---\nname: long-run\ndescription: Makes many calls or child runs, then holds for a person.\n---\n# Long run\n\nMake the calls, then hold.\n\n## Agent task: answer\n\nMake the calls, then answer.\n",
    ),
    ("skills/long-run/run.yaml", LONG_RUN),
    ("skills/long-run/work.mjs", WORK),
    (
        "skills/child/SKILL.md",
        "---\nname: child\ndescription: Digests one value.\n---\n# Child\n\nDigest the value.\n",
    ),
    (
        "skills/child/run.yaml",
        "minimum_runx: 0.12.0\nmethods:\n  digest:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs: {value: $input.value, encoding: utf8_text}\n",
    ),
    (
        "skills/canceller/SKILL.md",
        "---\nname: canceller\ndescription: Digests and reads a chat thread at its turn.\n---\n# Canceller\n\n## Agent task: answer\n\nDigest, read the thread, then answer.\n",
    ),
    (
        "skills/canceller/run.yaml",
        "minimum_runx: 0.12.0\nmethods:\n  answer:\n    steps:\n      - id: answer\n        agent: answer\n        calls:\n          tools: [data.digest]\n          operations: [fixture/chat/thread.read]\n        inputs: $input\n        outputs:\n          answer: {type: string}\n",
    ),
    (
        "skills/asker/SKILL.md",
        "---\nname: asker\ndescription: Waits for a person.\n---\n# Asker\n\nWait for a person.\n",
    ),
    (
        "skills/asker/run.yaml",
        "minimum_runx: 0.12.0\nmethods:\n  ask:\n    steps:\n      - id: hold\n        ask: {gate: asker.hold, reason: The probe's turn waits here.}\n",
    ),
];

/// A run of `calls` tool calls or `children` skill calls, holding after them
/// at `hold` and then at `again`; or a turn the host works.
const LONG_RUN: &str = r#"minimum_runx: 0.12.0
methods:
  calls:
    inputs:
      calls: {type: integer}
    steps:
      - id: work
        script: {module: work.mjs, export: calls}
        inputs: {calls: $input.calls}
        calls:
          tools: [data.digest]
        max_calls: 1000
        outputs:
          made: {type: integer}
      - id: hold
        ask: {gate: long-run.hold, reason: The probe holds the run here.}
      - id: again
        ask: {gate: long-run.again, reason: The probe holds the run here again.}
  children:
    inputs:
      children: {type: integer}
    steps:
      - id: work
        script: {module: work.mjs, export: children}
        inputs: {children: $input.children}
        calls:
          skills: [../child]
        max_calls: 1000
        outputs:
          made: {type: integer}
      - id: hold
        ask: {gate: long-run.hold, reason: The probe holds the run here.}
      - id: again
        ask: {gate: long-run.again, reason: The probe holds the run here again.}
  turn:
    steps:
      - id: answer
        agent: answer
        calls:
          tools: [data.digest]
          skills: [../asker]
        inputs: $input
        outputs:
          answer: {type: string}
"#;

/// `calls` digests, or `children` child runs.
const WORK: &str = r#"export async function calls(ctx, { calls }) {
  for (let index = 0; index < calls; index += 1) {
    await ctx.tool("data.digest", { value: `call ${index}`, encoding: "utf8_text" });
  }
  return { made: calls };
}

export async function children(ctx, { children }) {
  for (let index = 0; index < children; index += 1) {
    await ctx.skill("../child", { value: `child ${index}` });
  }
  return { made: children };
}
"#;
