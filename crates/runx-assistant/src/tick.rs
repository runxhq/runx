//! The operator application is the local host for one finite assistant turn. Skills retain
//! source, queue, storage, model judgment, and notification ownership.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, SecondsFormat, TimeZone, Timelike, Utc};
use ring::rand::{SecureRandom, SystemRandom};
use runx_contracts::{
    ClosureDisposition, DecisionObservation, DecisionOrigin, ResolutionRequest, sha256_prefixed,
};
use runx_runtime::kernel::connectors::messages::Messages;
use runx_runtime::kernel::store::{
    Append, Expected as StoreExpected, HostRecords, HostSnapshot, StreamEvent,
};
use runx_runtime::{
    CONFIDENTIAL_TERMS_ENV, INJECTED_GRANT_ID_ENV, INJECTED_GRANTED_SCOPES_ENV,
    INJECTED_PRINCIPAL_REF_ENV, ManagedAgentPolicy, REQUIRE_MUTATION_APPROVAL_ENV,
    STANDING_APPROVAL_ID_ENV, STANDING_DELEGATE_DIGEST_ENV, SkillRunRequest, WorkspaceEnv,
    encode_injected_scopes, now_iso8601, outbound_text_is_safe as private_text_is_safe,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    GoalSchedule, LoadedProfile, LocalProviderProfile, QuietHours, SourceProfile, WorkRoute,
    model_environment,
};

const CONTROL_SCHEMA: &str = "runx.assistant.control.v1";
const MAX_OBSERVATIONS: usize = 20;
const MAX_DEFERRED_TURNS: usize = 16;
const MAX_WORK_ARTIFACT_BYTES: usize = 32 * 1024;

/// The assistant's state as one value: its control record and the records
/// it indexes, each kept in its own stream.
#[derive(Clone, Debug)]
struct Control {
    core: ControlCore,
    confirmed_memory: Vec<ConfirmedMemory>,
    work: Vec<WorkAssignment>,
    goal_occurrence_heads: BTreeMap<String, GoalOccurrenceHead>,
    /// Staged in the same commit as the disposition; never a second ledger.
    pending_handled_signals: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlCore {
    schema: String,
    instance_id: String,
    profile_revision: String,
    paused: bool,
    next_due_unix_seconds: u64,
    next_delivery_due_unix_seconds: u64,
    active_run_id: Option<String>,
    pending_turn: Option<PendingTurn>,
    pending_intent: Option<PendingIntent>,
    deferred_turns: Vec<DeferredTurn>,
    last_handled_receipt: Option<String>,
    last_blocker: Option<String>,
    scan_retry_from_first_page: bool,
    last_fresh_scan_at: BTreeMap<String, u64>,
    work_ids: Vec<String>,
    memory_ids: Vec<String>,
    occurrence_ids: Vec<String>,
    last_review_ref: Option<Value>,
    last_review_at: Option<String>,
    last_review_coverage_incomplete: bool,
}

impl Deref for Control {
    type Target = ControlCore;

    fn deref(&self) -> &Self::Target {
        &self.core
    }
}

impl DerefMut for Control {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.core
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkAssignment {
    id: String,
    goal_id: String,
    completion_condition: String,
    source_ref: String,
    source_digest: String,
    thread_locator: String,
    source_binding: Option<SourceBinding>,
    turn: WorkTurn,
    continuations: Vec<WorkTurn>,
    status: String,
    followup_blocker: Option<String>,
    completed_at: Option<String>,
    receipt: Option<String>,
    result_ref: Option<Value>,
    native_run: Option<BoundWorkRun>,
    failed_runs: Vec<PriorWorkRun>,
    decision: Option<DecisionProjection>,
    private_updates: Vec<WorkUpdate>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoalOccurrenceHead {
    schedule_digest: String,
    due_at_unix_seconds: u64,
}

/// Exact per-turn reporting custody inside the obligation. A newer native
/// result cannot replace an earlier result awaiting private delivery.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkUpdate {
    turn_id: String,
    receipt: String,
    result_ref: Value,
    status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionProjection {
    request_id: String,
    request_digest: String,
    nonce: String,
    adapter_skill: String,
    adapter_package_digest: String,
    provider: String,
    authorized_actor_ref: String,
    local_provider: Option<LocalProviderProfile>,
    presentation_ref: Option<String>,
    observation_receipt: Option<String>,
    evidence_ref: Option<String>,
    answer_ref: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceBinding {
    profile: super::SourceProfile,
    package_digest: String,
}

/// One admitted business turn, not a second execution lifecycle. The native
/// run owns suspension, approvals, attempts and recovery.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkTurn {
    id: String,
    route: WorkRoute,
    package_digest: String,
    target_ref: String,
    inputs: Value,
    due_at_unix_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundWorkRun {
    skill: String,
    run_id: String,
    package_digest: String,
    input_ref: Value,
    local_provider: Option<LocalProviderProfile>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PriorWorkRun {
    bound: BoundWorkRun,
    receipt: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmedMemory {
    memory_id: String,
    text: String,
    confirmed_at: String,
    confirmed_by: String,
    revision: String,
    supersedes: Option<String>,
    goal_ids: Vec<String>,
    source_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingIntent {
    artifact_ref: Value,
    material_digest: String,
    logical_run_id: String,
    selected_digests: Vec<String>,
    delivery_package_digest: String,
    delivery_skill: String,
    delivery_provider: String,
    delivery_instance: Option<String>,
    work_updates: BTreeMap<String, String>,
    decision_work_id: Option<String>,
    decision_request_digest: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingTurn {
    pages: Vec<PinnedPage>,
    review_ref: Option<Value>,
    deferred: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeferredTurn {
    pages: Vec<PinnedPage>,
    occurrence_digests: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedPage {
    source_id: String,
    artifact_ref: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PagePacket {
    source_id: String,
    query_digest: String,
    scan_id: String,
    page_index: u64,
    scan_lane: ScanLane,
    fetched_at_unix_seconds: u64,
    next_cursor: Option<String>,
    messages: Vec<Value>,
    observations: Vec<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScanLane {
    Backlog,
    Fresh,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewPacket {
    packet: Value,
    receipt: Option<String>,
}

#[derive(Clone)]
struct ControlRecord {
    version: u64,
    state: Control,
    baseline: Control,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum ControlEvent {
    Advanced { state: ControlCore, reason: String },
}

impl StreamEvent for ControlEvent {
    const KIND: &'static str = "control";

    fn event_type(&self) -> &'static str {
        "advanced"
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum WorkEvent {
    Put { value: Box<WorkAssignment> },
    Removed,
}

impl StreamEvent for WorkEvent {
    const KIND: &'static str = "obligation";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Put { .. } => "put",
            Self::Removed => "removed",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum MemoryEvent {
    Put { value: ConfirmedMemory },
    Removed,
}

impl StreamEvent for MemoryEvent {
    const KIND: &'static str = "memory";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Put { .. } => "put",
            Self::Removed => "removed",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum OccurrenceEvent {
    Set { head: GoalOccurrenceHead },
    Removed,
}

/// A source occurrence is admitted before its scan cursor is acknowledged.
/// The private page remains digest-bound on disk; this stream keeps only its
/// stable identity and provenance so later turns cannot erase the ingress.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum SignalEvent {
    Seen {
        source_id: String,
        source_ref_digest: String,
        source_digest: String,
        page_artifact_ref: String,
    },
    Handled {
        source_digest: String,
    },
}

impl StreamEvent for SignalEvent {
    const KIND: &'static str = "signal";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Seen { .. } => "seen",
            Self::Handled { .. } => "handled",
        }
    }
}

impl StreamEvent for OccurrenceEvent {
    const KIND: &'static str = "occurrence";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Set { .. } => "set",
            Self::Removed => "removed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkerLane {
    Delivery,
    Assignment,
}

fn due_worker_lane(
    delivery_due: Option<u64>,
    assignment_due: Option<u64>,
    now: u64,
) -> Option<WorkerLane> {
    if delivery_due.is_some_and(|due| now >= due) {
        Some(WorkerLane::Delivery)
    } else if assignment_due.is_some_and(|due| now >= due) {
        Some(WorkerLane::Assignment)
    } else {
        None
    }
}

fn next_work_due(work: &[WorkAssignment]) -> Option<u64> {
    work.iter()
        .filter(|item| work_is_open(item))
        .map(|item| item.turn.due_at_unix_seconds)
        .min()
}

fn schedule_digest(schedule: &GoalSchedule) -> Result<String, String> {
    serde_json::to_vec(schedule)
        .map(|bytes| sha256_prefixed(&bytes))
        .map_err(|error| format!("encoding goal schedule: {error}"))
}

fn goal_occurrence_prefix(goal_id: &str, revision: &str) -> String {
    format!("runx://assistant/goal/{goal_id}/schedule/{revision}/occurrence/")
}

fn due_goal_occurrence(
    schedule: &GoalSchedule,
    head: Option<&GoalOccurrenceHead>,
    now: u64,
) -> Result<Option<u64>, String> {
    if now < schedule.starts_at_unix_seconds {
        return Ok(None);
    }
    let digest = schedule_digest(schedule)?;
    let elapsed = now - schedule.starts_at_unix_seconds;
    let latest = schedule
        .starts_at_unix_seconds
        .saturating_add((elapsed / schedule.interval_seconds) * schedule.interval_seconds);
    Ok((head.is_none_or(|previous| {
        previous.schedule_digest != digest || latest > previous.due_at_unix_seconds
    }))
    .then_some(latest))
}

fn next_goal_due(loaded: &LoadedProfile, state: &Control) -> Option<u64> {
    loaded
        .profile
        .goals
        .iter()
        .filter(|goal| goal.enabled)
        .filter_map(|goal| {
            let schedule = goal.schedule.as_ref()?;
            if unresolved_goal_occurrence(&state.work, &goal.goal_id) {
                return None;
            }
            let head = state.goal_occurrence_heads.get(&goal.goal_id);
            Some(
                head.filter(|head| {
                    schedule_digest(schedule).ok().as_deref() == Some(head.schedule_digest.as_str())
                })
                .map_or(schedule.starts_at_unix_seconds, |head| {
                    head.due_at_unix_seconds
                        .saturating_add(schedule.interval_seconds)
                }),
            )
        })
        .min()
}

fn unresolved_goal_occurrence(work: &[WorkAssignment], goal_id: &str) -> bool {
    work.iter().any(|item| {
        item.goal_id == goal_id
            && item.status != "completed"
            && item.source_ref.starts_with("runx://assistant/goal/")
    })
}

fn due_work_index(work: &[WorkAssignment], now: u64) -> Option<usize> {
    work.iter()
        .position(|item| work_is_open(item) && item.turn.due_at_unix_seconds <= now)
}

fn work_is_open(item: &WorkAssignment) -> bool {
    item.status == "pending"
}

fn evictable_work_index(work: &[WorkAssignment], protected_id: Option<&str>) -> Option<usize> {
    work.iter().position(|item| {
        item.status == "completed"
            && !needs_work_update(item)
            && protected_id != Some(item.id.as_str())
    })
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn quiet_at(hours: Option<&QuietHours>, unix_seconds: u64) -> bool {
    let Some(hours) = hours else { return false };
    let Ok(seconds) = i64::try_from(unix_seconds) else {
        return true;
    };
    let Some(local) = Local.timestamp_opt(seconds, 0).single() else {
        return true;
    };
    let hour = local.hour() as u8;
    quiet_hour(hours, hour)
}

fn quiet_hour(hours: &QuietHours, hour: u8) -> bool {
    if hours.start_hour < hours.end_hour {
        hour >= hours.start_hour && hour < hours.end_hour
    } else {
        hour >= hours.start_hour || hour < hours.end_hour
    }
}

fn next_allowed_at(loaded: &LoadedProfile, first_due: u64) -> u64 {
    let mut due = first_due;
    for _ in 0..=432 {
        if !quiet_at(loaded.profile.quiet_hours.as_ref(), due) {
            return due;
        }
        due = due.saturating_add(300);
    }
    due
}

fn scheduled_due(minutes: u64) -> u64 {
    now_seconds().saturating_add(minutes.saturating_mul(60))
}

fn new_run_id(instance: &str) -> Result<String, String> {
    let mut random = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| "cannot obtain random assistant run identity")?;
    Ok(format!(
        "assistant-{instance}-{}",
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn notification_uuid(run_id: &str) -> Result<String, String> {
    let nonce = run_id
        .rsplit('-')
        .next()
        .ok_or("assistant run identity lacks its nonce")?;
    if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("assistant run identity lacks a 128-bit nonce".to_owned());
    }
    Ok(format!(
        "{}-{}-4{}-8{}-{}",
        &nonce[..8],
        &nonce[8..12],
        &nonce[13..16],
        &nonce[17..20],
        &nonce[20..32]
    ))
}

fn control_default(loaded: &LoadedProfile) -> Control {
    Control {
        core: ControlCore {
            schema: CONTROL_SCHEMA.to_owned(),
            instance_id: loaded.profile.instance_id.clone(),
            profile_revision: loaded.revision.clone(),
            paused: true,
            next_due_unix_seconds: 0,
            next_delivery_due_unix_seconds: 0,
            active_run_id: None,
            pending_turn: None,
            pending_intent: None,
            deferred_turns: Vec::new(),
            last_handled_receipt: None,
            last_blocker: None,
            scan_retry_from_first_page: false,
            last_fresh_scan_at: BTreeMap::new(),
            work_ids: Vec::new(),
            memory_ids: Vec::new(),
            occurrence_ids: Vec::new(),
            last_review_ref: None,
            last_review_at: None,
            last_review_coverage_incomplete: false,
        },
        confirmed_memory: Vec::new(),
        work: Vec::new(),
        goal_occurrence_heads: BTreeMap::new(),
        pending_handled_signals: Vec::new(),
    }
}

fn run_skill(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    name: &str,
    runner: &str,
    inputs: Value,
    model: bool,
    delivery: bool,
) -> Result<(Value, String), String> {
    run_skill_with_id(
        loaded, workspace, name, runner, inputs, model, delivery, None, None, None,
    )
    .map_err(String::from)
}

fn run_source_skill(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    source: &SourceProfile,
    runner: &str,
    inputs: Value,
) -> Result<(Value, String), String> {
    run_skill_with_resolution(
        loaded,
        workspace,
        &source.skill,
        runner,
        inputs,
        false,
        false,
        None,
        None,
        None,
        None,
        source.local_provider.as_ref(),
    )
    .map_err(String::from)
}

#[derive(Debug)]
struct SkillRunFailure {
    message: String,
    terminal_receipt: Option<String>,
}

#[derive(Debug)]
enum SkillRunStop {
    Failure(SkillRunFailure),
    Suspended {
        skill: String,
        run_id: String,
        requests: Vec<Value>,
    },
}

impl SkillRunStop {
    fn sealed_validation(skill: &str, receipt: &str, message: impl Into<String>) -> Self {
        Self::Failure(SkillRunFailure::sealed_validation(skill, receipt, message))
    }
}

impl From<SkillRunFailure> for SkillRunStop {
    fn from(error: SkillRunFailure) -> Self {
        Self::Failure(error)
    }
}

impl From<String> for SkillRunStop {
    fn from(message: String) -> Self {
        Self::Failure(message.into())
    }
}

impl From<&str> for SkillRunStop {
    fn from(message: &str) -> Self {
        Self::Failure(message.into())
    }
}

impl From<SkillRunStop> for String {
    fn from(stop: SkillRunStop) -> Self {
        match stop {
            SkillRunStop::Failure(error) => error.message,
            SkillRunStop::Suspended { run_id, .. } => {
                format!("native run {run_id} awaits a resolution")
            }
        }
    }
}

impl From<String> for SkillRunFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            terminal_receipt: None,
        }
    }
}

impl From<&str> for SkillRunFailure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl From<SkillRunFailure> for String {
    fn from(error: SkillRunFailure) -> Self {
        error.message
    }
}

impl SkillRunFailure {
    fn sealed_validation(skill: &str, receipt: &str, message: impl Into<String>) -> Self {
        let message: String = message.into();
        Self {
            message: format!("{skill}: {message}"),
            terminal_receipt: Some(receipt.to_owned()),
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one typed boundary for the finite skill turn"
)]
fn run_skill_with_id(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    name: &str,
    runner: &str,
    inputs: Value,
    model: bool,
    delivery: bool,
    run_id: Option<&str>,
    pinned_package_digest: Option<&str>,
    agent_context: Option<(
        &[runx_contracts::ContextEntry],
        &[runx_contracts::ContextEntry],
    )>,
) -> Result<(Value, String), SkillRunStop> {
    run_skill_with_resolution(
        loaded,
        workspace,
        name,
        runner,
        inputs,
        model,
        delivery,
        run_id,
        pinned_package_digest,
        agent_context,
        None,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "one native skill continuation boundary"
)]
fn run_skill_with_resolution(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    name: &str,
    runner: &str,
    inputs: Value,
    model: bool,
    delivery: bool,
    run_id: Option<&str>,
    pinned_package_digest: Option<&str>,
    agent_context: Option<(
        &[runx_contracts::ContextEntry],
        &[runx_contracts::ContextEntry],
    )>,
    answers_path: Option<&Path>,
    local_provider: Option<&LocalProviderProfile>,
) -> Result<(Value, String), SkillRunStop> {
    let source_path = loaded.profile.skill_path(name);
    let path = if let Some(digest) = pinned_package_digest {
        pinned_skill_path_by_digest(workspace, name, digest)?
    } else if run_id.is_some() {
        let expected = loaded
            .skill_bindings
            .get(name)
            .ok_or_else(|| format!("assistant has no inspected {name} skill binding"))?;
        pinned_skill_path(workspace, &source_path, name, expected)?
    } else {
        source_path.clone()
    };
    let mut env = if model {
        model_environment(&loaded.profile, workspace)?
    } else {
        workspace.env().clone()
    };
    if let Some(binding) = local_provider {
        // The run resolves the connector's selected instance; it must still
        // be the one this work was admitted against.
        super::instance_digest(workspace, &binding.instance_id)?;
    }
    env.insert(
        REQUIRE_MUTATION_APPROVAL_ENV.to_owned(),
        "required".to_owned(),
    );
    env.insert(
        CONFIDENTIAL_TERMS_ENV.to_owned(),
        serde_json::to_string(&loaded.profile.confidential_terms)
            .map_err(|error| format!("encoding assistant egress policy: {error}"))?,
    );
    if delivery {
        let notify = loaded
            .profile
            .notification
            .as_ref()
            .ok_or("notification is not configured")?;
        match notify.instance_id.as_deref() {
            Some(instance_id) => {
                super::instance_digest(workspace, instance_id)?;
            }
            None => {
                env.insert(
                    INJECTED_GRANT_ID_ENV.to_owned(),
                    notify.provider_grant_id.clone(),
                );
                env.insert(
                    INJECTED_PRINCIPAL_REF_ENV.to_owned(),
                    notify.principal_ref.clone(),
                );
                env.insert(
                    INJECTED_GRANTED_SCOPES_ENV.to_owned(),
                    encode_injected_scopes(&[
                        "channel.post".to_owned(),
                        "channel.post.read".to_owned(),
                        "thread.read".to_owned(),
                    ])
                    .ok_or("the notification scopes are invalid")?,
                );
            }
        }
        env.insert(
            STANDING_APPROVAL_ID_ENV.to_owned(),
            notify.authority_id.clone(),
        );
        env.insert(
            STANDING_DELEGATE_DIGEST_ENV.to_owned(),
            loaded.source_set_digest.clone(),
        );
    }
    let input_map: BTreeMap<String, runx_contracts::JsonValue> = serde_json::from_value(inputs)
        .map_err(|error| format!("assistant skill inputs are invalid: {error}"))?;
    let request = SkillRunRequest {
        current_context: agent_context.map_or_else(Vec::new, |context| context.0.to_vec()),
        historical_context: agent_context.map_or_else(Vec::new, |context| context.1.to_vec()),
        run_authority: Some(loaded.profile.authority.clone()),
        act: None,
        skill_path: path,
        receipt_dir: None,
        run_id: run_id.map(str::to_owned),
        answers_path: answers_path.map(Path::to_path_buf),
        inputs: input_map,
        env: env.clone(),
        cwd: workspace.cwd().to_path_buf(),
        managed_agent: if model {
            ManagedAgentPolicy::inline(loaded.profile.model.max_rounds)
                .map_err(|error| error.to_string())?
        } else {
            ManagedAgentPolicy::HostDriven
        },
    };
    let orchestrator = runx_runtime::RuntimeEffectRegistry::local()
        .map(|effects| {
            runx_runtime::LocalOrchestrator::with_effects_and_environment(effects, env.clone())
        })
        .map_err(|error| error.to_string())?;
    let result = orchestrator
        .run_skill_with_runner(&request, runner)
        .map_err(|error| format!("{name}#{runner}: {error}"))?;
    if result.status == runx_runtime::RunStatus::NeedsAgent {
        let run_id = run_id.ok_or("unbound skill run suspended without a durable identity")?;
        if result.pending_requests.is_empty() {
            return Err("native run suspended without a pending request".into());
        }
        return Err(SkillRunStop::Suspended {
            skill: name.to_owned(),
            run_id: run_id.to_owned(),
            requests: result
                .pending_requests
                .into_iter()
                .map(|request| serde_json::to_value(request).map_err(|error| error.to_string()))
                .collect::<Result<_, _>>()?,
        });
    }
    if !result.succeeded() {
        let receipt = result.receipt_refs.first().cloned();
        return Err(SkillRunStop::Failure(SkillRunFailure {
            message: format!(
                "{name}#{runner} did not close: {:?}; receipt: {}",
                result.disposition,
                receipt.as_deref().unwrap_or("unavailable")
            ),
            terminal_receipt: if result.status == runx_runtime::RunStatus::Sealed {
                receipt
            } else {
                None
            },
        }));
    }
    let receipt = result
        .receipt_refs
        .first()
        .cloned()
        .ok_or("skill closed without a receipt reference")?;
    Ok((
        serde_json::to_value(result.output)
            .map_err(|error| format!("encoding skill output: {error}"))?,
        receipt,
    ))
}

fn skill_retention_root(workspace: &WorkspaceEnv) -> PathBuf {
    workspace.paths().assistant().join("skill-packages")
}

fn retained_skill_name(name: &str) -> String {
    if name.contains('/') {
        let digest = sha256_prefixed(name.as_bytes());
        format!("v-{}", &digest[7..69])
    } else {
        name.to_owned()
    }
}

fn pinned_skill_path_by_digest(
    workspace: &WorkspaceEnv,
    name: &str,
    digest: &str,
) -> Result<PathBuf, String> {
    runx_runtime::resolve_retained_skill_binding(
        &skill_retention_root(workspace),
        &retained_skill_name(name),
        digest,
    )
    .map_err(|error| error.to_string())
}

fn pinned_skill_path(
    workspace: &WorkspaceEnv,
    source: &Path,
    name: &str,
    expected_digest: &str,
) -> Result<PathBuf, String> {
    runx_runtime::retain_skill_binding(
        &skill_retention_root(workspace),
        source,
        &retained_skill_name(name),
        expected_digest,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
fn package_digest(path: &Path) -> Result<String, String> {
    let inspected =
        runx_runtime::inspect_skill_package(path, None, None).map_err(|error| error.to_string())?;
    inspected
        .as_object()
        .and_then(|object| object.get("package_digest"))
        .and_then(runx_contracts::JsonValue::as_str)
        .map(str::to_owned)
        .ok_or("skill inspection has no package digest".to_owned())
}

#[cfg(test)]
fn copy_skill_files(source: &Path, target: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            fs::create_dir(&destination).map_err(|error| error.to_string())?;
            copy_skill_files(&entry.path(), &destination)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), destination).map_err(|error| error.to_string())?;
        } else {
            return Err("skill test fixture contains a special file".to_owned());
        }
    }
    Ok(())
}

fn work_run_id(
    loaded: &LoadedProfile,
    prefix: &str,
    item: &WorkAssignment,
    skill: &str,
) -> Result<String, String> {
    let _ = loaded;
    if item.turn.route.skill != skill {
        return Err("work skill differs from its admitted turn".to_owned());
    }
    let binding = sha256_prefixed(
        &serde_json::to_vec(&(&item.turn.id, &item.turn.package_digest))
            .map_err(|error| error.to_string())?,
    );
    Ok(format!("{prefix}{}", binding.trim_start_matches("sha256:")))
}

/// Work runs under the connector's selected instance; refuse it when that
/// is no longer the instance the work names.
fn pin_local_provider(
    workspace: &WorkspaceEnv,
    profile: &LocalProviderProfile,
) -> Result<LocalProviderProfile, String> {
    super::instance_digest(workspace, &profile.instance_id)?;
    Ok(profile.clone())
}

fn bound_work_inputs(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    index: usize,
    skill: &str,
    prefix: &str,
    build: impl FnOnce() -> Result<(Value, Vec<String>), String>,
) -> Result<(BoundWorkRun, Value, Vec<String>), String> {
    if let Some(bound) = record.state.work[index].native_run.as_ref() {
        if bound.skill != skill {
            return Err("native run differs from its admitted skill".to_owned());
        }
        pinned_skill_path_by_digest(workspace, skill, &bound.package_digest)?;
        let packet = read_bound_work_inputs(workspace, bound)?;
        return Ok((
            bound.clone(),
            packet["inputs"].clone(),
            serde_json::from_value(packet["evidence_receipts"].clone())
                .map_err(|error| format!("decoding bound work evidence: {error}"))?,
        ));
    }
    let expected = record.state.work[index].clone();
    let (inputs, evidence_receipts) = build()?;
    let _control_lock = lock(loaded, workspace)?;
    let mut current = read_control(loaded, workspace)?;
    let index = current
        .state
        .work
        .iter()
        .position(|item| item.id == expected.id)
        .ok_or("admitted obligation lost its durable head")?;
    if current.state.work[index].turn != expected.turn
        || current.state.work[index].native_run.is_some()
    {
        return Err("obligation changed before native input admission".to_owned());
    }
    let (current_context, historical_context) = operator_context(
        loaded,
        workspace,
        &current,
        Some(&current.state.work[index]),
    )?;
    let packet = json!({"inputs":inputs,"evidence_receipts":evidence_receipts,
        "current_context":current_context,"historical_context":historical_context});
    if serde_json::to_vec(&packet)
        .map_err(|error| error.to_string())?
        .len()
        > 64 * 1024
    {
        return Err("assistant work inputs exceed 64 KiB".to_owned());
    }
    let digest = &current.state.work[index].turn.package_digest;
    pinned_skill_path_by_digest(workspace, skill, digest)?;
    let run_id = work_run_id(loaded, prefix, &current.state.work[index], skill)?;
    let value = serde_json::from_value(packet.clone())
        .map_err(|error| format!("normalizing bound work inputs: {error}"))?;
    let reference = runx_runtime::persist_json_artifact(
        &workspace.paths().artifacts(),
        &run_id,
        "assistant-work-inputs",
        "inputs",
        &value,
    )?;
    let local_provider = current.state.work[index]
        .turn
        .route
        .local_provider
        .as_ref()
        .map(|profile| pin_local_provider(workspace, profile))
        .transpose()?;
    let bound = BoundWorkRun {
        skill: skill.to_owned(),
        run_id,
        package_digest: digest.clone(),
        input_ref: serde_json::to_value(reference)
            .map_err(|error| format!("encoding bound work input reference: {error}"))?,
        local_provider,
    };
    current.state.work[index].native_run = Some(bound.clone());
    write_control(loaded, workspace, &mut current, "work_run_bound")?;
    *record = current;
    Ok((bound, packet["inputs"].clone(), evidence_receipts))
}

fn read_bound_work_inputs(workspace: &WorkspaceEnv, bound: &BoundWorkRun) -> Result<Value, String> {
    let reference = serde_json::from_value(bound.input_ref.clone())
        .map_err(|error| format!("decoding bound work input reference: {error}"))?;
    let value = runx_runtime::read_json_artifact(&workspace.paths().artifacts(), &reference)?;
    let packet = serde_json::to_value(value)
        .map_err(|error| format!("decoding bound work inputs: {error}"))?;
    if !packet["inputs"].is_object() || !packet["evidence_receipts"].is_array() {
        return Err("bound work inputs are malformed".to_owned());
    }
    Ok(packet)
}

fn result_data<'a>(output: &'a Value, field: &str) -> Result<&'a Value, String> {
    output
        .pointer(&format!("/result/{field}/data"))
        .ok_or_else(|| format!("skill result lacks {field}.data"))
}

fn source_packet<'a>(output: &'a Value, field: &str) -> Result<&'a Value, String> {
    let value = result_data(output, field)?;
    value
        .is_object()
        .then_some(value)
        .ok_or_else(|| format!("source skill result {field}.data is not an object"))
}

fn read_control(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<ControlRecord, String> {
    let records = assistant_records(workspace)?;
    records
        .snapshot(|snapshot| read_control_snapshot(loaded, snapshot))
        .map_err(|error| error.to_string())?
}

fn read_control_snapshot(
    loaded: &LoadedProfile,
    snapshot: &HostSnapshot<'_>,
) -> Result<ControlRecord, String> {
    let latest = snapshot
        .latest::<ControlEvent>(&loaded.profile.instance_id)
        .map_err(|error| error.to_string())?;
    let Some(last) = latest else {
        let state = control_default(loaded);
        return Ok(ControlRecord {
            version: 0,
            baseline: state.clone(),
            state,
        });
    };
    let version = last.version;
    let ControlEvent::Advanced { state: core, .. } = &last.event;
    if !unique_ids(&core.work_ids)
        || !unique_ids(&core.memory_ids)
        || !unique_ids(&core.occurrence_ids)
    {
        return Err("assistant record index has duplicate identities".to_owned());
    }
    let mut state = Control {
        core: core.clone(),
        confirmed_memory: Vec::new(),
        work: Vec::new(),
        goal_occurrence_heads: BTreeMap::new(),
        pending_handled_signals: Vec::new(),
    };
    for id in &core.work_ids {
        let key = assistant_record_key(&loaded.profile.instance_id, id);
        let latest = snapshot
            .latest::<WorkEvent>(&key)
            .map_err(|error| error.to_string())?;
        match latest.as_ref().map(|event| &event.event) {
            Some(WorkEvent::Put { value }) if value.id == *id => state.work.push((**value).clone()),
            _ => return Err(format!("assistant obligation {id} is absent or mismatched")),
        }
    }
    for id in &core.memory_ids {
        let key = assistant_record_key(&loaded.profile.instance_id, id);
        let latest = snapshot
            .latest::<MemoryEvent>(&key)
            .map_err(|error| error.to_string())?;
        match latest.as_ref().map(|event| &event.event) {
            Some(MemoryEvent::Put { value }) if value.memory_id == *id => {
                state.confirmed_memory.push(value.clone());
            }
            _ => return Err(format!("assistant memory {id} is absent or mismatched")),
        }
    }
    for id in &core.occurrence_ids {
        let key = assistant_record_key(&loaded.profile.instance_id, id);
        let latest = snapshot
            .latest::<OccurrenceEvent>(&key)
            .map_err(|error| error.to_string())?;
        match latest.as_ref().map(|event| &event.event) {
            Some(OccurrenceEvent::Set { head }) => {
                state.goal_occurrence_heads.insert(id.clone(), head.clone());
            }
            _ => return Err(format!("assistant occurrence {id} is absent")),
        }
    }
    validate_control(loaded, &state)?;
    Ok(ControlRecord {
        version,
        baseline: state.clone(),
        state,
    })
}

fn validate_control(loaded: &LoadedProfile, state: &Control) -> Result<(), String> {
    if state.schema != CONTROL_SCHEMA || state.instance_id != loaded.profile.instance_id {
        return Err("assistant control identity or schema differs from profile".to_owned());
    }
    let mut memory_ids = BTreeSet::new();
    if state.confirmed_memory.len() > 20
        || state.confirmed_memory.iter().any(|item| {
            !super::valid_identifier(&item.memory_id)
                || !memory_ids.insert(&item.memory_id)
                || item.text.trim().is_empty()
                || item.text.len() > 300
                || DateTime::parse_from_rfc3339(&item.confirmed_at).is_err()
        })
    {
        return Err("assistant confirmed memory state is invalid".to_owned());
    }
    if state.goal_occurrence_heads.len() > 64
        || state.goal_occurrence_heads.iter().any(|(goal, head)| {
            !super::valid_identifier(goal)
                || !valid_sha256_digest(&head.schedule_digest)
                || head.due_at_unix_seconds == 0
        })
    {
        return Err("assistant scheduled occurrence state is invalid".to_owned());
    }
    if state.work.len() > 40
        || state.work.iter().any(|item| {
            !matches!(item.status.as_str(), "pending" | "completed" | "held")
                || item.id.len() > 80
                || item.source_ref.len() > 300
                || item.turn.target_ref.len() > 300
                || item.continuations.len() > 12
                || item.turn.route.goal_id != item.goal_id
                || item.native_run.as_ref().is_some_and(|run| {
                    run.skill != item.turn.route.skill
                        || run.package_digest != item.turn.package_digest
                })
                || item.private_updates.len() > 12
                || item.failed_runs.len() > 8
                || item.private_updates.iter().any(|update| {
                    update.turn_id.is_empty()
                        || update.receipt.is_empty()
                        || update
                            .status
                            .as_deref()
                            .is_some_and(|status| !matches!(status, "local_only" | "delivered"))
                })
        })
    {
        return Err("assistant obligation state is invalid".to_owned());
    }
    if let Some(intent) = state.pending_intent.as_ref()
        && (intent.work_updates.len() > 3
            || intent
                .work_updates
                .iter()
                .any(|(id, digest)| id.is_empty() || !intent.selected_digests.contains(digest)))
    {
        return Err("assistant notification provenance is invalid".to_owned());
    }
    Ok(())
}

fn persist_work_result(
    workspace: &WorkspaceEnv,
    item: &WorkAssignment,
    result: &Value,
) -> Result<Value, String> {
    let value = serde_json::from_value(result.clone())
        .map_err(|error| format!("encoding assistant work result: {error}"))?;
    let reference = runx_runtime::persist_json_artifact(
        &workspace.paths().artifacts(),
        &format!(
            "run_assistant_work_{}",
            item.id.trim_start_matches("sha256:")
        ),
        "assistant-work",
        "result",
        &value,
    )?;
    serde_json::to_value(reference)
        .map_err(|error| format!("encoding assistant work result reference: {error}"))
}

fn read_result_ref(
    workspace: &WorkspaceEnv,
    reference: Option<&Value>,
) -> Result<Option<Value>, String> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    let reference = serde_json::from_value(reference.clone())
        .map_err(|error| format!("decoding assistant work result reference: {error}"))?;
    let value = runx_runtime::read_json_artifact(&workspace.paths().artifacts(), &reference)?;
    serde_json::to_value(value)
        .map(Some)
        .map_err(|error| format!("decoding assistant work result: {error}"))
}

fn read_work_result(
    workspace: &WorkspaceEnv,
    item: &WorkAssignment,
) -> Result<Option<Value>, String> {
    read_result_ref(workspace, item.result_ref.as_ref())
}

fn pending_work_update(item: &WorkAssignment) -> Option<&WorkUpdate> {
    item.private_updates
        .iter()
        .find(|update| update.status.is_none())
}

fn work_result_digest(item: &WorkAssignment, update: &WorkUpdate) -> String {
    sha256_prefixed(
        format!(
            "assistant-work-result:{}:{}:{}",
            item.id, update.turn_id, update.receipt
        )
        .as_bytes(),
    )
}

fn needs_work_update(item: &WorkAssignment) -> bool {
    pending_work_update(item).is_some()
}

fn pending_work_update_indices(work: &[WorkAssignment]) -> Vec<usize> {
    work.iter()
        .enumerate()
        .filter(|(_, item)| needs_work_update(item))
        .map(|(index, _)| index)
        .collect()
}

fn work_update_item(workspace: &WorkspaceEnv, item: &WorkAssignment) -> Result<Value, String> {
    let update = pending_work_update(item).ok_or("work has no pending private update")?;
    let result = read_result_ref(workspace, Some(&update.result_ref))?
        .ok_or("completed work has no result")?;
    let summary = match item.followup_blocker.as_deref() {
        Some(blocker) => format!("{} Held: {blocker}", work_summary(&result)?),
        None => work_summary(&result)?,
    };
    let digest = work_result_digest(item, update);
    Ok(json!({
        "source_ref":format!("runx://assistant/work/{}", item.id),
        "source_digest":digest,
        "source_kind":"work_result",
        "priority":"medium",
        "summary":summary.chars().take(1000).collect::<String>(),
        "receipt":update.receipt,
        "work_id":item.id,
    }))
}

fn work_summary(result: &Value) -> Result<String, String> {
    result["summary"]
        .as_str()
        .filter(|summary| !summary.trim().is_empty() && summary.len() <= 1000)
        .map(str::to_owned)
        .ok_or("work result lacks a bounded summary".to_owned())
}

fn assistant_records(workspace: &WorkspaceEnv) -> Result<HostRecords, String> {
    runx_runtime::KernelStore::for_project(workspace.paths())
        .and_then(|store| store.host("assistant"))
        .map_err(|error| error.to_string())
}

fn assistant_record_key(instance_id: &str, id: &str) -> String {
    format!("{instance_id}:{id}")
}

/// A signal is handled once per mandate revision and confirmed-memory set,
/// so remembering or forgetting a fact revisits current observations.
fn signal_key(
    loaded: &LoadedProfile,
    state: &Control,
    source_digest: &str,
) -> Result<String, String> {
    let memory = state
        .confirmed_memory
        .iter()
        .map(|item| (item.memory_id.as_str(), item.revision.as_str()))
        .collect::<BTreeMap<_, _>>();
    let identity = sha256_prefixed(
        &serde_json::to_vec(&(&loaded.revision, memory, source_digest))
            .map_err(|error| error.to_string())?,
    );
    Ok(assistant_record_key(&loaded.profile.instance_id, &identity))
}

fn unique_ids(ids: &[String]) -> bool {
    ids.iter().collect::<BTreeSet<_>>().len() == ids.len()
}

fn expected_version(version: u64) -> StoreExpected {
    if version == 0 {
        StoreExpected::Absent
    } else {
        StoreExpected::Version(version)
    }
}

fn append_changes<'a, V: PartialEq + 'a, E: StreamEvent>(
    records: &HostRecords,
    instance_id: &str,
    before: impl IntoIterator<Item = (&'a str, &'a V)>,
    after: impl IntoIterator<Item = (&'a str, &'a V)>,
    event: impl Fn(Option<&V>) -> E,
    batch: &mut Vec<Append>,
) -> Result<(), String> {
    let before = before.into_iter().collect::<BTreeMap<_, _>>();
    let after = after.into_iter().collect::<BTreeMap<_, _>>();
    let ids = before
        .keys()
        .chain(after.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    for id in ids {
        let old = before.get(id).copied();
        let new = after.get(id).copied();
        if old == new {
            continue;
        }
        let key = assistant_record_key(instance_id, id);
        let version = records
            .version::<E>(&key)
            .map_err(|error| error.to_string())?;
        batch.push(
            records
                .append_of(key, expected_version(version), &event(new))
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(())
}

fn append_new_signals(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    records: &HostRecords,
    before: &Control,
    after: &Control,
    batch: &mut Vec<Append>,
) -> Result<(), String> {
    let old_pages = before
        .pending_turn
        .as_ref()
        .map(|turn| turn.pages.as_slice())
        .unwrap_or(&[]);
    let new_pages = after
        .pending_turn
        .as_ref()
        .map(|turn| turn.pages.as_slice())
        .unwrap_or(&[]);
    let mut inserted = BTreeMap::new();
    for pinned in new_pages {
        if old_pages
            .iter()
            .any(|old| old.source_id == pinned.source_id && old.artifact_ref == pinned.artifact_ref)
        {
            continue;
        }
        let source = loaded
            .profile
            .sources
            .iter()
            .find(|source| source.id() == pinned.source_id)
            .ok_or("pinned signal names an unknown source")?;
        let page = read_pinned_page(workspace, source, pinned)?;
        for observation in &page.observations {
            let source_ref = observation["source_ref"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("pinned signal has no source reference")?;
            let source_digest = observation["source_digest"]
                .as_str()
                .filter(|value| valid_sha256_digest(value))
                .ok_or("pinned signal has no valid digest")?;
            let key = signal_key(loaded, after, source_digest)?;
            if let Some(prior_ref) = inserted.insert(key.clone(), source_ref.to_owned()) {
                if prior_ref != source_ref {
                    return Err("pinned signals conflict within one source page".to_owned());
                }
                continue;
            }
            let event = SignalEvent::Seen {
                source_id: pinned.source_id.clone(),
                source_ref_digest: sha256_prefixed(source_ref.as_bytes()),
                source_digest: source_digest.to_owned(),
                page_artifact_ref: pinned.artifact_ref["ref"]
                    .as_str()
                    .filter(|value| value.starts_with("runx:project-artifact:sha256:"))
                    .ok_or("pinned signal has no opaque page reference")?
                    .to_owned(),
            };
            if let Some(existing) = records
                .latest::<SignalEvent>(&key)
                .map_err(|error| error.to_string())?
            {
                match existing.event {
                    SignalEvent::Seen {
                        source_id,
                        source_ref_digest,
                        source_digest: prior_digest,
                        ..
                    } if source_id == pinned.source_id
                        && source_ref_digest == sha256_prefixed(source_ref.as_bytes())
                        && prior_digest == source_digest => {}
                    SignalEvent::Handled {
                        source_digest: prior_digest,
                    } if prior_digest == source_digest => {}
                    _ => return Err("pinned signal conflicts with its durable identity".to_owned()),
                }
                continue;
            }
            batch.push(
                records
                    .append_of(key, StoreExpected::Absent, &event)
                    .map_err(|error| error.to_string())?,
            );
        }
    }
    Ok(())
}

fn signal_is_handled(
    records: &HostRecords,
    loaded: &LoadedProfile,
    state: &Control,
    source_digest: &str,
) -> Result<bool, String> {
    let key = signal_key(loaded, state, source_digest)?;
    Ok(matches!(
        records
            .latest::<SignalEvent>(&key)
            .map_err(|error| error.to_string())?
            .map(|record| record.event),
        Some(SignalEvent::Handled { source_digest: previous }) if previous == source_digest
    ))
}

fn handled_signal_digests(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    state: &Control,
    observations: &[Value],
) -> Result<Vec<String>, String> {
    let records = assistant_records(workspace)?;
    let mut handled = Vec::new();
    for observation in observations {
        if let Some(digest) = observation["source_digest"].as_str()
            && signal_is_handled(&records, loaded, state, digest)?
        {
            handled.push(digest.to_owned());
        }
    }
    Ok(handled)
}

fn append_handled_signals(
    loaded: &LoadedProfile,
    records: &HostRecords,
    before: &Control,
    state: &Control,
    batch: &mut Vec<Append>,
) -> Result<(), String> {
    let mut handled = BTreeSet::new();
    for digest in &state.pending_handled_signals {
        if !handled.insert(digest) {
            continue;
        }
        let key = signal_key(loaded, state, digest)?;
        let prior = records
            .latest::<SignalEvent>(&key)
            .map_err(|error| error.to_string())?;
        let expected = match prior {
            Some(record) => match record.event {
                SignalEvent::Seen { source_digest, .. } if source_digest == *digest => {
                    StoreExpected::Version(record.version)
                }
                SignalEvent::Handled { source_digest } if source_digest == *digest => continue,
                _ => return Err("handled signal differs from its source identity".to_owned()),
            },
            None if before
                .pending_intent
                .as_ref()
                .is_some_and(|intent| intent.selected_digests.contains(digest))
                && state.pending_intent.is_none()
                && state.last_handled_receipt != before.last_handled_receipt
                && state.last_handled_receipt.is_some() =>
            {
                StoreExpected::Absent
            }
            None => return Err("handled signal was never durably observed".to_owned()),
        };
        batch.push(
            records
                .append_of(
                    key,
                    expected,
                    &SignalEvent::Handled {
                        source_digest: digest.clone(),
                    },
                )
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(())
}

fn write_control(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    reason: &str,
) -> Result<(), String> {
    let records = assistant_records(workspace)?;
    let instance_id = &loaded.profile.instance_id;
    record.state.core.work_ids = record
        .state
        .work
        .iter()
        .map(|item| item.id.clone())
        .collect();
    record.state.core.memory_ids = record
        .state
        .confirmed_memory
        .iter()
        .map(|item| item.memory_id.clone())
        .collect();
    record.state.core.occurrence_ids = record.state.goal_occurrence_heads.keys().cloned().collect();
    validate_control(loaded, &record.state)?;
    if !unique_ids(&record.state.core.work_ids)
        || !unique_ids(&record.state.core.memory_ids)
        || !unique_ids(&record.state.core.occurrence_ids)
    {
        return Err("assistant record identities are not unique".to_owned());
    }
    let mut batch = vec![
        records
            .append_of(
                instance_id,
                expected_version(record.version),
                &ControlEvent::Advanced {
                    state: record.state.core.clone(),
                    reason: reason.to_owned(),
                },
            )
            .map_err(|error| error.to_string())?,
    ];

    append_changes(
        &records,
        instance_id,
        record
            .baseline
            .work
            .iter()
            .map(|item| (item.id.as_str(), item)),
        record
            .state
            .work
            .iter()
            .map(|item| (item.id.as_str(), item)),
        |item| {
            item.map_or(WorkEvent::Removed, |value| WorkEvent::Put {
                value: Box::new(value.clone()),
            })
        },
        &mut batch,
    )?;
    append_changes(
        &records,
        instance_id,
        record
            .baseline
            .confirmed_memory
            .iter()
            .map(|item| (item.memory_id.as_str(), item)),
        record
            .state
            .confirmed_memory
            .iter()
            .map(|item| (item.memory_id.as_str(), item)),
        |item| {
            item.map_or(MemoryEvent::Removed, |value| MemoryEvent::Put {
                value: value.clone(),
            })
        },
        &mut batch,
    )?;
    append_changes(
        &records,
        instance_id,
        record
            .baseline
            .goal_occurrence_heads
            .iter()
            .map(|(id, head)| (id.as_str(), head)),
        record
            .state
            .goal_occurrence_heads
            .iter()
            .map(|(id, head)| (id.as_str(), head)),
        |head| {
            head.map_or(OccurrenceEvent::Removed, |head| OccurrenceEvent::Set {
                head: head.clone(),
            })
        },
        &mut batch,
    )?;
    append_new_signals(
        loaded,
        workspace,
        &records,
        &record.baseline,
        &record.state,
        &mut batch,
    )?;
    append_handled_signals(
        loaded,
        &records,
        &record.baseline,
        &record.state,
        &mut batch,
    )?;
    records.commit(batch).map_err(|error| error.to_string())?;
    record.version += 1;
    record.state.pending_handled_signals.clear();
    record.baseline = record.state.clone();
    Ok(())
}

fn lock(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<File, String> {
    let dir = workspace.paths().assistant();
    fs::create_dir_all(&dir)
        .map_err(|error| format!("creating assistant lock directory: {error}"))?;
    let path = dir.join(format!("{}.lock", loaded.profile.instance_id));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("opening assistant lock: {error}"))?;
    file.try_lock()
        .map_err(|error| format!("assistant tick already running or lock failed: {error}"))?;
    Ok(file)
}

fn worker_lock(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<File, String> {
    let dir = workspace.paths().assistant();
    fs::create_dir_all(&dir)
        .map_err(|error| format!("creating assistant lock directory: {error}"))?;
    let path = dir.join(format!("{}.worker.lock", loaded.profile.instance_id));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("opening assistant worker lock: {error}"))?;
    file.try_lock()
        .map_err(|error| format!("assistant worker already running or lock failed: {error}"))?;
    Ok(file)
}

pub(super) fn status(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<Value, String> {
    let record = read_control(loaded, workspace)?;
    let authority = loaded
        .profile
        .notification
        .as_ref()
        .map(|notify| {
            super::standing_approvals(workspace)?
                .status(&notify.authority_id)
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    Ok(json!({
        "schema": CONTROL_SCHEMA,
        "instance_id": record.state.instance_id,
        "profile_revision": record.state.profile_revision,
        "current_profile_revision": loaded.revision,
        "paused": record.state.paused,
        "next_due_unix_seconds": record.state.next_due_unix_seconds,
        "next_delivery_due_unix_seconds": record.state.next_delivery_due_unix_seconds,
        "next_worker_due_unix_seconds": next_work_due(&record.state.work).unwrap_or(0),
        "next_goal_due_unix_seconds": next_goal_due(loaded, &record.state).unwrap_or(0),
        "goal_occurrence_heads": record.state.goal_occurrence_heads,
        "active_run_id": record.state.active_run_id,
        "pending_page_count": record.state.pending_turn.as_ref().map_or(0, |turn| turn.pages.len()),
        "pending_review": record.state.pending_turn.as_ref().is_some_and(|turn| turn.review_ref.is_some()),
        "pending_notification": record.state.pending_intent.is_some(),
        "deferred_attention_turns":record.state.deferred_turns.len(),
        "last_handled_receipt": record.state.last_handled_receipt,
        "last_blocker": record.state.last_blocker,
        "scan_retry_from_first_page": record.state.scan_retry_from_first_page
            || (record.state.last_blocker.is_some() && record.state.pending_intent.is_none()),
        "last_fresh_scan_at_unix_seconds":record.state.last_fresh_scan_at,
        "confirmed_memory_count":record.state.confirmed_memory.len(),
        "pending_work_count":record.state.work.iter().filter(|item| item.status == "pending").count(),
        "awaiting_resolution_count":record.state.work.iter().filter(|item| work_is_open(item)).filter_map(|item| item.native_run.as_ref())
            .map(|bound| runx_runtime::run_record::paused_requests(workspace.env(), workspace.cwd(), &bound.run_id))
            .collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?.iter().filter(|waiting| !waiting.is_empty()).count(),
        "pending_private_work_updates":record.state.work.iter()
            .map(|item| item.private_updates.iter().filter(|update| update.status.is_none()).count()).sum::<usize>(),
        "report_available":record.state.last_review_ref.is_some(),
        "charter_configured":!loaded.profile.charter.is_empty(),
        "notification_authority":authority,
        "timer_installed":super::timer::timer_installed(loaded, workspace)?,
        "version": record.version
    }))
}

pub(super) fn report(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<Value, String> {
    let record = read_control(loaded, workspace)?;
    let Some(reference) = record.state.last_review_ref.as_ref() else {
        return Ok(
            json!({"schema":"runx.assistant.report.v1","status":"not_available","instance_id":loaded.profile.instance_id}),
        );
    };
    let review = read_review(workspace, reference)?;
    if review.receipt.is_some() && review.packet["validation"]["status"] != "pass" {
        return Err("stored assistant report lacks a passing review".to_owned());
    }
    Ok(json!({
        "schema":"runx.assistant.report.v1",
        "status":"available",
        "instance_id":loaded.profile.instance_id,
        "reviewed_at":record.state.last_review_at,
        "coverage_incomplete":record.state.last_review_coverage_incomplete,
        "profile_revision":record.state.profile_revision,
        "current_profile_revision":loaded.revision,
        "attention_packet":review.packet,
        "receipt":review.receipt,
        "recent_work":record.state.work.iter().rev().filter(|item| item.status == "completed").take(5)
            .map(|item| json!({"id":item.id,"goal_id":item.goal_id,"status":item.status,
                "completed_at":item.completed_at,"receipt":item.receipt})).collect::<Vec<_>>()
    }))
}

pub(super) fn discard_notification(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
) -> Result<Value, String> {
    let _lock = lock(loaded, workspace)?;
    let mut record = read_control(loaded, workspace)?;
    if !record.state.paused || record.state.profile_revision != loaded.revision {
        return Err("pause the unchanged assistant profile before discarding an intent".to_owned());
    }
    let pending = record
        .state
        .pending_intent
        .as_ref()
        .ok_or("assistant has no pending notification")?;
    let notification = loaded
        .profile
        .notification
        .as_ref()
        .ok_or("assistant notification binding is missing")?;
    let reference = serde_json::from_value(pending.artifact_ref.clone())
        .map_err(|error| format!("decoding notification reference: {error}"))?;
    let exact = serde_json::to_value(runx_runtime::read_json_artifact(
        &workspace.paths().artifacts(),
        &reference,
    )?)
    .map_err(|error| format!("decoding exact notification: {error}"))?;
    if exact["channel_locator"] != notification.destination_locator {
        return Err("pending notification destination differs from profile".to_owned());
    }
    let key = exact["idempotency_key"]
        .as_str()
        .ok_or("pending notification lacks idempotency key")?;
    if super::standing_approvals(workspace)?
        .attempted(&notification.authority_id, key)
        .map_err(|error| error.to_string())?
    {
        return Err(
            "notification was attempted; reconcile its provider outcome before discarding"
                .to_owned(),
        );
    }
    record.state.pending_intent = None;
    record.state.next_delivery_due_unix_seconds = 0;
    record.state.active_run_id = None;
    record.state.pending_turn = None;
    record.state.scan_retry_from_first_page = true;
    record.state.last_blocker = None;
    record.state.next_due_unix_seconds = 0;
    write_control(
        loaded,
        workspace,
        &mut record,
        "discard_unreserved_notification",
    )?;
    Ok(json!({"status":"discarded_unreserved","instance_id":loaded.profile.instance_id}))
}

pub(super) fn memories(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<Value, String> {
    let record = read_control(loaded, workspace)?;
    Ok(
        json!({"schema":"runx.assistant.memories.v1","instance_id":loaded.profile.instance_id,"confirmed_memory":record.state.confirmed_memory}),
    )
}

pub(super) fn work(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    cursor: Option<&str>,
) -> Result<Value, String> {
    let assignments = read_control(loaded, workspace)?
        .state
        .work
        .iter()
        .map(|item| {
            let mut projection = serde_json::to_value(item)
                .map_err(|error| format!("encoding assistant work item: {error}"))?;
            let fields = projection
                .as_object_mut()
                .ok_or("assistant work item is not an object")?;
            fields.remove("result_ref");
            fields.remove("native_run");
            fields.remove("failed_runs");
            fields.remove("private_updates");
            projection["pending_private_updates"] = json!(
                item.private_updates
                    .iter()
                    .filter(|update| update.status.is_none())
                    .count()
            );
            if let Some(pending) = item.native_run.as_ref() {
                let ids = runx_runtime::run_record::paused_requests(
                    workspace.env(),
                    workspace.cwd(),
                    &pending.run_id,
                )
                .map_err(|error| format!("inspecting native pending request: {error}"))?
                .into_iter()
                .map(|request| request.id)
                .collect::<Vec<_>>();
                projection["pending_request_ids"] = json!((!ids.is_empty()).then_some(ids));
            }
            projection["result"] = json!(read_work_result(workspace, item)?);
            Ok::<Value, String>(projection)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut inputs = json!({"data_source_ref":loaded.profile.inbox_data_source_ref,"limit":20});
    if let Some(cursor) = cursor {
        inputs["cursor"] = json!(cursor);
    }
    let (output, receipt) = run_skill(
        loaded,
        workspace,
        "operator-inbox",
        "list_actions",
        inputs,
        false,
        false,
    )?;
    let data = result_data(&output, "data_operation_result")?;
    let rows = data["rows"]
        .as_array()
        .ok_or("assistant work read lacks bounded rows")?;
    if rows.len() > 20 {
        return Err("assistant work read exceeds page limit".to_owned());
    }
    Ok(json!({
        "schema":"runx.assistant.work.v1",
        "instance_id":loaded.profile.instance_id,
        "rows":rows,
        "assignments":assignments,
        "next_cursor":data["next_cursor"],
        "receipt":receipt
    }))
}

/// Re-admit a terminal held turn only after its skill or trusted route inputs
/// change and its exact receipt tree proves no external authority was used.
/// Effects and uncertain attempts require reconciliation instead.
pub(super) fn retry_work(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    work_id: &str,
) -> Result<Value, String> {
    let _lock = lock(loaded, workspace)?;
    let mut record = read_control(loaded, workspace)?;
    if record.state.paused || record.state.profile_revision != loaded.revision {
        return Err(
            "assistant must be resumed on its current profile before retrying work".to_owned(),
        );
    }
    let item = record
        .state
        .work
        .iter_mut()
        .find(|item| item.id == work_id)
        .ok_or("work obligation does not exist")?;
    if item.status != "held" || item.decision.is_some() || item.failed_runs.len() >= 8 {
        return Err("only a bounded terminal held turn can be retried".to_owned());
    }
    let bound = item
        .native_run
        .as_ref()
        .ok_or("held work lacks its exact native run")?
        .clone();
    let receipt = item
        .receipt
        .as_deref()
        .ok_or("held work lacks a terminal receipt")?
        .to_owned();
    let package_digest = loaded
        .skill_bindings
        .get(&item.turn.route.skill)
        .ok_or("retry skill is not installed")?
        .clone();
    let current_route = loaded
        .profile
        .work_routes
        .iter()
        .find(|route| route.route_id == item.turn.route.route_id)
        .ok_or("retry route is no longer configured")?;
    let mut expected_route = item.turn.route.clone();
    expected_route.inputs = current_route.inputs.clone();
    expected_route.local_provider = current_route.local_provider.clone();
    if *current_route != expected_route {
        return Err("retry cannot change the held work's route or judgment policy".to_owned());
    }
    if let Some(source) = item.source_binding.as_ref()
        && !loaded.profile.sources.iter().any(|candidate| {
            candidate.source_id == source.profile.source_id
                && candidate.provider == source.profile.provider
                && candidate.locator_prefix == source.profile.locator_prefix
        })
    {
        return Err("retry source instance is no longer configured".to_owned());
    }
    receipt_tree_has_no_external_authority(workspace, &receipt)?;
    pinned_skill_path(
        workspace,
        &loaded.profile.skill_path(&item.turn.route.skill),
        &item.turn.route.skill,
        &package_digest,
    )?;
    item.failed_runs.push(PriorWorkRun {
        bound,
        receipt: receipt.clone(),
    });
    item.turn.id = sha256_prefixed(
        format!(
            "assistant-work-retry:{}:{receipt}:{package_digest}:{}",
            item.id, loaded.revision
        )
        .as_bytes(),
    );
    if let Some(inputs) = item.turn.inputs.as_object_mut() {
        for key in item.turn.route.inputs.keys() {
            inputs.remove(key);
        }
        for (key, value) in &current_route.inputs {
            inputs.insert(key.clone(), value.clone());
        }
    } else {
        return Err("admitted work inputs are not an object".to_owned());
    }
    item.turn.route = current_route.clone();
    item.turn.package_digest = package_digest;
    item.turn.due_at_unix_seconds = 0;
    item.status = "pending".to_owned();
    item.completed_at = None;
    item.receipt = None;
    item.result_ref = None;
    item.native_run = None;
    item.followup_blocker = None;
    record.state.last_blocker = None;
    write_control(loaded, workspace, &mut record, "work_retry_admitted")?;
    Ok(json!({"status":"work_retry_admitted","work_id":work_id,"previous_receipt":receipt}))
}

fn receipt_tree_has_no_external_authority(
    workspace: &WorkspaceEnv,
    receipt_id: &str,
) -> Result<(), String> {
    let store = runx_runtime::LocalReceiptStore::new(super::receipt_root(workspace));
    let verifier = runx_runtime::receipt_verifier(
        workspace.env(),
        workspace.paths(),
        runx_runtime::ReceiptTrust::HomeAndConfigured,
    )
    .map_err(|error| format!("reading failed run proof: {error}"))?;
    let mut pending = vec![receipt_id.to_owned()];
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) || seen.len() > 64 {
            return Err("failed run receipt tree is cyclic or exceeds the bound".to_owned());
        }
        let receipt = store
            .read_exact(
                &id,
                runx_runtime::RuntimeReceiptSignaturePolicy::verifying(&verifier),
            )
            .map_err(|error| format!("reading failed run proof: {error}"))?;
        if id == receipt_id
            && !matches!(
                receipt.seal.disposition,
                runx_contracts::ClosureDisposition::Closed
                    | runx_contracts::ClosureDisposition::Failed
                    | runx_contracts::ClosureDisposition::Blocked
            )
        {
            return Err("retry receipt does not match the held native run".to_owned());
        }
        if !receipt.authority.scope_refs.is_empty() || !receipt.authority.grant_refs.is_empty() {
            return Err(
                "failed run exercised external authority; reconcile its effects before retry"
                    .to_owned(),
            );
        }
        for child in receipt.lineage.children {
            let id = child
                .uri
                .as_str()
                .strip_prefix("runx:receipt:")
                .ok_or("failed run has a non-receipt child proof")?;
            pending.push(id.to_owned());
        }
    }
    Ok(())
}

pub(super) fn remember(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    memory_id: &str,
    text_path: &Path,
) -> Result<Value, String> {
    if !super::valid_identifier(memory_id) {
        return Err("memory ID must use 1-64 lowercase letters, digits, - or _".to_owned());
    }
    let path = text_path
        .canonicalize()
        .map_err(|error| format!("resolving private memory file: {error}"))?;
    let file =
        File::open(&path).map_err(|error| format!("opening private memory file: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("reading private memory file metadata: {error}"))?;
    if !metadata.is_file() || metadata.len() > 300 {
        return Err("memory text must be a regular file of at most 300 bytes".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("memory text file must be private (chmod 600)".to_owned());
        }
    }
    let mut text = String::new();
    file.take(301)
        .read_to_string(&mut text)
        .map_err(|error| format!("reading private memory file: {error}"))?;
    if text.len() > 300 {
        return Err("memory text exceeds 300 bytes".to_owned());
    }
    let text = text.trim().to_owned();
    if text.is_empty() || text.len() > 300 {
        return Err("memory text must contain 1-300 nonblank bytes".to_owned());
    }
    let _lock = lock(loaded, workspace)?;
    let mut record = read_control(loaded, workspace)?;
    memory_edit_allowed(loaded, &record)?;
    let confirmed_at = now_iso8601();
    let revision = sha256_prefixed(
        &serde_json::to_vec(&json!([memory_id, text, confirmed_at]))
            .map_err(|error| error.to_string())?,
    );
    // A re-confirmed fact becomes the newest, so context selection, which
    // takes the newest first, prefers it.
    if let Some(index) = record
        .state
        .confirmed_memory
        .iter()
        .position(|item| item.memory_id == memory_id)
    {
        let mut existing = record.state.confirmed_memory.remove(index);
        existing.supersedes = Some(existing.revision.clone());
        existing.text = text;
        existing.confirmed_at = confirmed_at;
        existing.confirmed_by = "local_operator".to_owned();
        existing.revision = revision;
        record.state.confirmed_memory.push(existing);
    } else {
        if record.state.confirmed_memory.len() >= 20 {
            return Err("assistant confirmed memory limit is 20 entries".to_owned());
        }
        record.state.confirmed_memory.push(ConfirmedMemory {
            memory_id: memory_id.to_owned(),
            text,
            confirmed_at,
            confirmed_by: "local_operator".to_owned(),
            revision,
            supersedes: None,
            goal_ids: Vec::new(),
            source_refs: Vec::new(),
        });
    }
    reset_for_context_change(&mut record.state);
    write_control(loaded, workspace, &mut record, "operator_confirmed_memory")?;
    Ok(
        json!({"status":"remembered","memory_id":memory_id,"confirmed_memory_count":record.state.confirmed_memory.len()}),
    )
}

pub(super) fn forget(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    memory_id: &str,
) -> Result<Value, String> {
    if !super::valid_identifier(memory_id) {
        return Err("memory ID must use 1-64 lowercase letters, digits, - or _".to_owned());
    }
    let _lock = lock(loaded, workspace)?;
    let mut record = read_control(loaded, workspace)?;
    memory_edit_allowed(loaded, &record)?;
    let before = record.state.confirmed_memory.len();
    record
        .state
        .confirmed_memory
        .retain(|item| item.memory_id != memory_id);
    if record.state.confirmed_memory.len() == before {
        return Err("assistant memory ID does not exist".to_owned());
    }
    reset_for_context_change(&mut record.state);
    write_control(loaded, workspace, &mut record, "operator_forgot_memory")?;
    Ok(
        json!({"status":"forgotten","memory_id":memory_id,"confirmed_memory_count":record.state.confirmed_memory.len()}),
    )
}

fn memory_edit_allowed(loaded: &LoadedProfile, record: &ControlRecord) -> Result<(), String> {
    if record.state.profile_revision != loaded.revision
        || record.state.active_run_id.is_some()
        || record.state.pending_turn.is_some()
        || record.state.pending_intent.is_some()
        || record.state.work.iter().any(work_is_open)
    {
        return Err(
            "assistant memory cannot change during pending work or profile drift".to_owned(),
        );
    }
    Ok(())
}

fn reset_for_context_change(state: &mut Control) {
    state.next_due_unix_seconds = 0;
    state.last_handled_receipt = None;
    state.scan_retry_from_first_page = true;
}

pub(super) fn set_paused(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    paused: bool,
) -> Result<Value, String> {
    let _lock = lock(loaded, workspace)?;
    let mut record = read_control(loaded, workspace)?;
    if !paused && record.state.profile_revision != loaded.revision {
        if record.state.pending_turn.is_some() {
            return Err(
                "cannot resume a pinned assistant source turn with changed bindings".to_owned(),
            );
        }
        if let Some(pending) = record.state.pending_intent.as_ref() {
            let notify = loaded
                .profile
                .notification
                .as_ref()
                .ok_or("cannot remove the binding of a pending private delivery")?;
            let exact = read_notification_intent(workspace, pending)?;
            if pending.delivery_skill != notify.skill
                || pending.delivery_provider != notify.provider
                || pending.delivery_instance != notify.instance_id
                || exact["channel_locator"] != notify.destination_locator
                || exact["notification_plan"]["principal"] != notify.principal_ref
            {
                return Err(
                    "pending private delivery requires its original destination and adapter"
                        .to_owned(),
                );
            }
        }
        // Non-model native runs own their exact skill, inputs, provider
        // instance and pending decision. They can survive an operator route
        // update. A suspended model turn must retain its configured model.
        if record
            .state
            .work
            .iter()
            .any(|item| work_is_open(item) && item.native_run.is_some() && item.turn.route.model)
        {
            return Err(
                "cannot change the configured model during a suspended model work run".to_owned(),
            );
        }
    }
    record.state.paused = paused;
    if !paused {
        if record.state.profile_revision != loaded.revision {
            record.state.active_run_id = None;
            record.state.scan_retry_from_first_page = true;
            record.state.last_fresh_scan_at.clear();
            record.state.goal_occurrence_heads.retain(|goal_id, _| {
                loaded
                    .profile
                    .goals
                    .iter()
                    .any(|goal| &goal.goal_id == goal_id)
            });
        }
        record.state.profile_revision = loaded.revision.clone();
        record.state.next_due_unix_seconds = 0;
        record.state.next_delivery_due_unix_seconds = 0;
        for item in &mut record.state.work {
            if work_is_open(item) {
                item.turn.due_at_unix_seconds = 0;
            }
        }
    }
    write_control(
        loaded,
        workspace,
        &mut record,
        if paused { "pause" } else { "resume" },
    )?;
    status(loaded, workspace)
}

pub(super) fn tick(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<Value, String> {
    let _lock = lock(loaded, workspace)?;
    let mut record = read_control(loaded, workspace)?;
    if record.state.paused {
        return Ok(json!({"status":"paused","instance_id":loaded.profile.instance_id}));
    }
    if record.state.profile_revision != loaded.revision {
        return Err("assistant profile changed; inspect and resume explicitly".to_owned());
    }
    admit_scheduled_work(loaded, workspace, &mut record)?;
    if now_seconds() < record.state.next_due_unix_seconds {
        return Ok(json!({
            "status":"not_due",
            "next_due_unix_seconds":record.state.next_due_unix_seconds
        }));
    }
    if record.state.pending_intent.is_some() {
        return finish_or_hold(
            loaded,
            workspace,
            &mut record,
            intake_while_delivery_pending,
        );
    }
    if record.state.pending_turn.is_none() && !record.state.deferred_turns.is_empty() {
        let deferred = record.state.deferred_turns.remove(0);
        record.state.active_run_id = Some(new_run_id(&loaded.profile.instance_id)?);
        record.state.pending_turn = Some(PendingTurn {
            pages: deferred.pages,
            review_ref: None,
            deferred: true,
        });
        write_control(loaded, workspace, &mut record, "replay_deferred_attention")?;
    }
    if record.state.active_run_id.is_some() && record.state.pending_turn.is_none() {
        record.state.active_run_id = None;
        record.state.scan_retry_from_first_page = true;
        write_control(loaded, workspace, &mut record, "restart_unpinned_turn")?;
    }
    if record.state.active_run_id.is_none() {
        record.state.active_run_id = Some(new_run_id(&loaded.profile.instance_id)?);
        record.state.pending_turn = Some(PendingTurn {
            pages: Vec::new(),
            review_ref: None,
            deferred: false,
        });
        write_control(loaded, workspace, &mut record, "begin")?;
    }
    finish_or_hold(loaded, workspace, &mut record, run_turn)
}

fn admit_scheduled_work(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
) -> Result<(), String> {
    let now = now_seconds();
    for goal in loaded.profile.goals.iter().filter(|goal| goal.enabled) {
        let Some(schedule) = goal.schedule.as_ref() else {
            continue;
        };
        let Some(due) = due_goal_occurrence(
            schedule,
            record.state.goal_occurrence_heads.get(&goal.goal_id),
            now,
        )?
        else {
            continue;
        };
        if unresolved_goal_occurrence(&record.state.work, &goal.goal_id) {
            continue;
        }
        let revision = schedule_digest(schedule)?;
        let source_ref = format!("{}{due}", goal_occurrence_prefix(&goal.goal_id, &revision));
        let observation = json!({"source_ref":source_ref,
            "source_digest":sha256_prefixed(source_ref.as_bytes()),
            "source_kind":"scheduled_goal", "thread_locator":schedule.target_ref});
        let proposal = runx_contracts::WorkProposal {
            route_id: schedule.route_id.clone(),
            target_ref: schedule.target_ref.clone(),
            inputs: Default::default(),
            due_at_unix_seconds: 0,
            due_after_seconds: None,
        };
        let item = admitted_assignment(loaded, workspace, &observation, None, &proposal, true)?;
        make_work_room(&mut record.state.work, None)?;
        record.state.work.push(item);
        record.state.goal_occurrence_heads.insert(
            goal.goal_id.clone(),
            GoalOccurrenceHead {
                schedule_digest: revision,
                due_at_unix_seconds: due,
            },
        );
        write_control(loaded, workspace, record, "goal_occurrences_admitted")?;
    }
    Ok(())
}

/// The source scan continues while an exact delivery is waiting for quiet
/// hours, provider recovery, or a human decision. Its pages are still pinned
/// before the operator-inbox cursor advances; the delivery intent keeps its
/// own run identity and is never replaced by this scan.
fn intake_while_delivery_pending(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
) -> Result<Value, String> {
    if record.state.pending_turn.is_none() {
        if record.state.deferred_turns.len() >= MAX_DEFERRED_TURNS {
            record.state.next_due_unix_seconds = scheduled_due(loaded.profile.min_check_minutes);
            write_control(loaded, workspace, record, "attention_backlog_full")?;
            return Ok(json!({
                "status":"attention_backlog_full",
                "deferred_attention_turns":record.state.deferred_turns.len(),
                "pending_notification":true
            }));
        }
        record.state.active_run_id = Some(new_run_id(&loaded.profile.instance_id)?);
        record.state.pending_turn = Some(PendingTurn {
            pages: Vec::new(),
            review_ref: None,
            deferred: false,
        });
        write_control(loaded, workspace, record, "begin_held_intake")?;
    }
    let (observations, coverage_incomplete) = collect_observations(loaded, workspace, record)?;
    let handled = handled_signal_digests(loaded, workspace, &record.state, &observations)?;
    let new_digests = unreviewed_digests(
        &handled,
        record
            .state
            .pending_intent
            .as_ref()
            .map(|intent| intent.selected_digests.as_slice())
            .unwrap_or(&[]),
        &record.state.deferred_turns,
        &observations,
    );
    if !new_digests.is_empty() {
        if record.state.deferred_turns.len() >= MAX_DEFERRED_TURNS {
            return Err(
                "assistant attention backlog is full; source cursor was not advanced".to_owned(),
            );
        }
        let pages = record
            .state
            .pending_turn
            .as_ref()
            .ok_or("assistant held intake lacks pinned pages")?
            .pages
            .clone();
        record.state.deferred_turns.push(DeferredTurn {
            pages,
            occurrence_digests: new_digests,
        });
        // Persist the exact source references before advancing a scan cursor.
        write_control(loaded, workspace, record, "attention_deferred")?;
    }
    commit_pending_pages(loaded, workspace, record)?;
    record.state.pending_turn = None;
    record.state.active_run_id = None;
    record.state.scan_retry_from_first_page = false;
    record.state.next_due_unix_seconds = scheduled_due(loaded.profile.min_check_minutes);
    write_control(loaded, workspace, record, "held_intake_committed")?;
    Ok(json!({
        "status":"intake_committed",
        "observation_count":observations.len(),
        "coverage_incomplete":coverage_incomplete,
        "pending_notification":true,
        "deferred_attention_turns":record.state.deferred_turns.len()
    }))
}

fn unreviewed_digests(
    handled: &[String],
    already_pending: &[String],
    deferred: &[DeferredTurn],
    observations: &[Value],
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    observations
        .iter()
        .filter_map(|item| item["source_digest"].as_str())
        .filter(|digest| {
            !handled.iter().any(|item| item == digest)
                && !already_pending.iter().any(|item| item == digest)
                && !deferred
                    .iter()
                    .any(|turn| turn.occurrence_digests.iter().any(|item| item == digest))
                && seen.insert(*digest)
        })
        .map(str::to_owned)
        .collect()
}

/// Work and delivery use native pinned run identities. A separate local
/// worker lock prevents duplicate concurrent attempts, while the common
/// control CAS protects state if an intake tick commits during an effect.
pub(super) fn execute(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<Value, String> {
    let _worker_lock = worker_lock(loaded, workspace)?;
    let mut record = read_control(loaded, workspace)?;
    if record.state.paused {
        return Ok(json!({"status":"paused","instance_id":loaded.profile.instance_id}));
    }
    if record.state.profile_revision != loaded.revision {
        return Err("assistant profile changed; inspect and resume explicitly".to_owned());
    }
    let now = now_seconds();
    let delivery_due = record
        .state
        .pending_intent
        .as_ref()
        .map(|_| record.state.next_delivery_due_unix_seconds);
    let assignment_due = next_work_due(&record.state.work);
    let lane = due_worker_lane(delivery_due, assignment_due, now);
    if lane.is_none() {
        return Ok(json!({
            "status":if delivery_due.is_some() || assignment_due.is_some() { "worker_not_due" } else { "worker_idle" },
            "next_delivery_due_unix_seconds":record.state.next_delivery_due_unix_seconds,
            "next_worker_due_unix_seconds":assignment_due.unwrap_or(0)
        }));
    }
    let attempted_work_id = if lane == Some(WorkerLane::Assignment) {
        due_work_index(&record.state.work, now).map(|index| record.state.work[index].id.clone())
    } else {
        None
    };
    let result: Result<Value, SkillRunStop> = match lane {
        Some(WorkerLane::Delivery) => {
            deliver_pending(loaded, workspace, &mut record).map_err(SkillRunStop::from)
        }
        Some(WorkerLane::Assignment) => dispatch_pending_work(loaded, workspace, &mut record),
        None => unreachable!(),
    };
    match result {
        Ok(output) => Ok(output),
        Err(SkillRunStop::Suspended {
            skill,
            run_id,
            requests,
        }) => {
            let work_id = attempted_work_id
                .as_deref()
                .ok_or("unassigned native run suspended")?;
            record_suspended_work(loaded, workspace, work_id, &skill, &run_id, &requests)
        }
        Err(SkillRunStop::Failure(error)) => {
            // Intake may have advanced the control version. Retry only the
            // original bound run after a bounded delay.
            let _control_lock = lock(loaded, workspace)?;
            let mut current = read_control(loaded, workspace)?;
            current.state.last_blocker = Some(error.message.chars().take(500).collect());
            if lane == Some(WorkerLane::Delivery) {
                current.state.next_delivery_due_unix_seconds =
                    scheduled_due(loaded.profile.min_check_minutes);
            } else if let Some(work_id) = attempted_work_id.as_deref()
                && let Some(item) = current
                    .state
                    .work
                    .iter_mut()
                    .find(|item| item.id == work_id && work_is_open(item))
            {
                apply_assignment_failure(item, &error, loaded.profile.min_check_minutes);
                if let Some(receipt) = error.terminal_receipt.as_ref() {
                    let summary = if private_text_is_safe(
                        &error.message,
                        &loaded.profile.confidential_terms,
                        workspace.env(),
                    ) {
                        format!("Work held: {}", error.message)
                    } else {
                        "Work held; inspect its local receipt for the reason.".to_owned()
                    };
                    let result = json!({
                        "disposition":"held",
                        "summary":summary.chars().take(1000).collect::<String>(),
                        "completion_evidence":[],"followups":[]
                    });
                    let reference = persist_work_result(workspace, item, &result)?;
                    item.result_ref = Some(reference.clone());
                    item.completed_at = Some(now_iso8601());
                    if item.private_updates.len() < 12 {
                        item.private_updates.push(WorkUpdate {
                            turn_id: item.turn.id.clone(),
                            receipt: receipt.clone(),
                            result_ref: reference,
                            status: None,
                        });
                    }
                }
            }
            write_control(loaded, workspace, &mut current, "worker_held")?;
            Err(error.message)
        }
    }
}

fn record_suspended_work(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    work_id: &str,
    skill: &str,
    run_id: &str,
    requests: &[Value],
) -> Result<Value, String> {
    let native =
        match runx_runtime::run_record::paused_requests(workspace.env(), workspace.cwd(), run_id)
            .map_err(|error| format!("inspecting native pending request: {error}"))?
            .as_slice()
        {
            [] => return Err("native run has no paused request".to_owned()),
            [native] => native.clone(),
            _ => return Err("assistant work waits on one native request at a time".to_owned()),
        };
    if requests.len() != 1 || requests[0]["id"] != native.id {
        return Err("native suspended request differs from its run checkpoint".to_owned());
    }
    let digest = sha256_prefixed(
        &serde_json::to_vec(&native.value)
            .map_err(|error| format!("encoding native pending request: {error}"))?,
    );
    let native_request = serde_json::from_value::<ResolutionRequest>(
        serde_json::to_value(&native.value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("decoding native pending request: {error}"))?;
    let approval = matches!(&native_request, ResolutionRequest::Approval { .. });
    let retry_after_seconds = match native_request {
        ResolutionRequest::Retry {
            retry_after_seconds,
            ..
        } if (1..=3600).contains(&retry_after_seconds) => Some(retry_after_seconds),
        ResolutionRequest::Retry { .. } => {
            return Err("native retry delay exceeds operator policy".to_owned());
        }
        _ => None,
    };
    let _control_lock = lock(loaded, workspace)?;
    let mut current = read_control(loaded, workspace)?;
    let item = current
        .state
        .work
        .iter_mut()
        .find(|item| item.id == work_id && work_is_open(item))
        .ok_or("suspended work is no longer open")?;
    if item
        .native_run
        .as_ref()
        .map(|run| (run.skill.as_str(), run.run_id.as_str()))
        != Some((skill, run_id))
    {
        return Err("native suspension differs from the admitted run".to_owned());
    }
    if retry_after_seconds.is_some() {
        item.decision = None;
    }
    if approval
        && let Some(profile) = loaded.profile.decision.as_ref()
        && item.decision.as_ref().is_none_or(|decision| {
            decision.request_id != native.id || decision.request_digest != digest
        })
    {
        let package_digest = loaded
            .skill_bindings
            .get(&profile.skill)
            .ok_or("assistant has no inspected decision skill binding")?;
        let nonce_digest = sha256_prefixed(format!("{run_id}:{digest}").as_bytes());
        item.decision = Some(DecisionProjection {
            request_id: native.id.clone(),
            request_digest: digest.clone(),
            nonce: nonce_digest[7..23].to_owned(),
            adapter_skill: profile.skill.clone(),
            adapter_package_digest: package_digest.clone(),
            provider: profile.provider.clone(),
            authorized_actor_ref: profile.authorized_actor_ref.clone(),
            local_provider: profile
                .local_provider
                .as_ref()
                .map(|binding| pin_local_provider(workspace, binding))
                .transpose()?,
            presentation_ref: None,
            observation_receipt: None,
            evidence_ref: None,
            answer_ref: None,
        });
    }
    item.turn.due_at_unix_seconds = retry_after_seconds
        .map(|seconds| now_seconds().saturating_add(seconds))
        .unwrap_or_else(|| {
            if item.decision.is_some() {
                0
            } else {
                scheduled_due(loaded.profile.min_check_minutes)
            }
        });
    current.state.last_blocker = None;
    write_control(loaded, workspace, &mut current, "work_awaiting_resolution")?;
    Ok(json!({
        "status":if approval { "work_awaiting_approval" } else if retry_after_seconds.is_some() {
            "work_awaiting_readback" } else { "work_awaiting_resolution" },
        "work_id":work_id,
        "run_id":run_id,
        "request_ids":[native.id]
    }))
}

fn apply_assignment_failure(item: &mut WorkAssignment, error: &SkillRunFailure, minutes: u64) {
    if let Some(receipt) = &error.terminal_receipt {
        item.status = "held".to_owned();
        item.receipt = Some(receipt.clone());
    } else {
        item.turn.due_at_unix_seconds = scheduled_due(minutes);
    }
}

fn finish_or_hold(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    turn: impl FnOnce(&LoadedProfile, &WorkspaceEnv, &mut ControlRecord) -> Result<Value, String>,
) -> Result<Value, String> {
    match turn(loaded, workspace, record) {
        Ok(value) => Ok(value),
        Err(error) => {
            record.state.pending_handled_signals.clear();
            record.state.last_blocker = Some(error.chars().take(500).collect());
            if record.state.pending_turn.is_none() && record.state.pending_intent.is_none() {
                record.state.scan_retry_from_first_page = true;
                record.state.active_run_id = None;
            }
            record.state.next_due_unix_seconds = scheduled_due(loaded.profile.min_check_minutes);
            write_control(loaded, workspace, record, "held")?;
            Err(error)
        }
    }
}

fn run_turn(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
) -> Result<Value, String> {
    if record
        .state
        .pending_turn
        .as_ref()
        .is_some_and(|turn| turn.pages.is_empty() && turn.review_ref.is_none())
    {
        let due = pending_work_update_indices(&record.state.work)
            .into_iter()
            .take(3)
            .collect::<Vec<_>>();
        if !due.is_empty() {
            let items = due
                .iter()
                .map(|index| work_update_item(workspace, &record.state.work[*index]))
                .collect::<Result<Vec<_>, _>>()?;
            let material_digest =
                sha256_prefixed(&serde_json::to_vec(&items).map_err(|error| error.to_string())?);
            if loaded.profile.notification.is_some() {
                let packet =
                    json!({"brief":"Verified assistant work is ready for review.","items":items});
                return prepare_notification(
                    loaded,
                    workspace,
                    record,
                    &packet,
                    &material_digest,
                    false,
                );
            }
            let receipts = due
                .iter()
                .filter_map(|index| {
                    pending_work_update(&record.state.work[*index])
                        .map(|update| update.receipt.clone())
                })
                .collect::<Vec<_>>();
            for index in due {
                if let Some(update) = record.state.work[index]
                    .private_updates
                    .iter_mut()
                    .find(|update| update.status.is_none())
                {
                    update.status = Some("local_only".to_owned());
                }
            }
            record.state.last_blocker = None;
            record.state.active_run_id = None;
            record.state.pending_turn = None;
            record.state.next_due_unix_seconds =
                if pending_work_update_indices(&record.state.work).is_empty() {
                    scheduled_due(loaded.profile.min_check_minutes)
                } else {
                    0
                };
            write_control(loaded, workspace, record, "work_available_locally")?;
            return Ok(
                json!({"status":"work_available_undelivered","work_count":receipts.len(),"receipts":receipts}),
            );
        }
    }
    let (observations, coverage_incomplete) = collect_observations(loaded, workspace, record)?;
    let handled = handled_signal_digests(loaded, workspace, &record.state, &observations)?;
    let observations = observations
        .into_iter()
        .filter(|item| {
            item["source_digest"]
                .as_str()
                .is_some_and(|digest| !handled.iter().any(|prior| prior == digest))
        })
        .collect::<Vec<_>>();
    if observations.is_empty() {
        commit_pending_pages(loaded, workspace, record)?;
        record.state.last_blocker = None;
        record.state.scan_retry_from_first_page = false;
        record.state.active_run_id = None;
        record.state.pending_turn = None;
        record.state.next_due_unix_seconds = scheduled_due(if coverage_incomplete {
            loaded.profile.min_check_minutes
        } else {
            loaded.profile.max_check_minutes
        });
        write_control(loaded, workspace, record, "unchanged")?;
        return Ok(json!({
            "status":"unchanged",
            "observation_count":0,
            "coverage_incomplete":coverage_incomplete
        }));
    }
    let material_digest =
        sha256_prefixed(&serde_json::to_vec(&observations).map_err(|error| error.to_string())?);
    let review_ref = record
        .state
        .pending_turn
        .as_ref()
        .ok_or("assistant turn lacks pinned work")?
        .review_ref
        .clone();
    let review = if let Some(reference) = review_ref {
        read_review(workspace, &reference)?
    } else {
        let review = review_observations(loaded, workspace, record, &observations)?;
        validate_review(loaded, &review)?;
        let logical = record
            .state
            .active_run_id
            .as_deref()
            .ok_or("assistant run has no identity")?;
        let artifact_value: runx_contracts::JsonValue = serde_json::from_value(
            serde_json::to_value(&review)
                .map_err(|error| format!("encoding assistant review: {error}"))?,
        )
        .map_err(|error| format!("binding assistant review: {error}"))?;
        let reference = runx_runtime::persist_json_artifact(
            &workspace.paths().artifacts(),
            logical,
            "assistant-reviews",
            "review",
            &artifact_value,
        )?;
        record
            .state
            .pending_turn
            .as_mut()
            .ok_or("assistant turn lacks pinned work")?
            .review_ref = Some(
            serde_json::to_value(reference)
                .map_err(|error| format!("encoding assistant review reference: {error}"))?,
        );
        write_control(loaded, workspace, record, "review_pinned")?;
        review
    };
    validate_review(loaded, &review)?;
    commit_pending_pages(loaded, workspace, record)?;
    admit_work(loaded, workspace, record, &observations, &review.packet)?;
    record.state.last_review_ref = record
        .state
        .pending_turn
        .as_ref()
        .and_then(|turn| turn.review_ref.clone());
    record.state.last_review_at = Some(now_iso8601());
    record.state.last_review_coverage_incomplete = coverage_incomplete;
    let packet = &review.packet;
    let decision = packet["decision"]
        .as_str()
        .ok_or("attention packet has no decision")?;
    let minutes = packet["next_check_minutes"]
        .as_u64()
        .ok_or("attention packet has no next-check minutes")?;
    if decision == "ready"
        && packet["recommended_action_ids"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == "private_update"))
    {
        return prepare_notification(
            loaded,
            workspace,
            record,
            packet,
            &material_digest,
            coverage_incomplete,
        );
    }
    if decision == "ready" {
        for item in &observations {
            if let Some(digest) = item["source_digest"].as_str() {
                mark_handled(&mut record.state, digest);
            }
        }
        record.state.last_handled_receipt = review.receipt.clone();
        record.state.last_blocker = None;
        record.state.scan_retry_from_first_page = false;
        record.state.active_run_id = None;
        record.state.pending_turn = None;
        record.state.next_due_unix_seconds =
            if pending_work_update_indices(&record.state.work).is_empty() {
                scheduled_due(loaded.profile.min_check_minutes)
            } else {
                0
            };
        write_control(loaded, workspace, record, "ready_undelivered")?;
        return Ok(
            json!({"status":"ready_undelivered","receipt":review.receipt,"observation_count":observations.len(),"coverage_incomplete":coverage_incomplete}),
        );
    }
    for item in &observations {
        if let Some(digest) = item["source_digest"].as_str() {
            mark_handled(&mut record.state, digest);
        }
    }
    record.state.last_blocker = None;
    record.state.scan_retry_from_first_page = false;
    record.state.last_handled_receipt = review.receipt;
    record.state.active_run_id = None;
    record.state.pending_turn = None;
    record.state.next_due_unix_seconds =
        if pending_work_update_indices(&record.state.work).is_empty() {
            scheduled_due(if coverage_incomplete {
                loaded.profile.min_check_minutes
            } else {
                minutes
            })
        } else {
            0
        };
    write_control(loaded, workspace, record, decision)?;
    Ok(
        json!({"status":decision,"observation_count":observations.len(),"next_check_minutes":minutes,"coverage_incomplete":coverage_incomplete}),
    )
}

fn context_entry(
    loaded: &LoadedProfile,
    purpose: runx_contracts::ContextPurpose,
    trust: runx_contracts::ContextTrust,
    item: Option<&WorkAssignment>,
    created_at: &str,
    data: Value,
    receipt: Option<&str>,
) -> Result<runx_contracts::ContextEntry, String> {
    let data: runx_contracts::JsonObject =
        serde_json::from_value(data).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&data).map_err(|error| error.to_string())?;
    let digest = sha256_prefixed(&bytes);
    Ok(runx_contracts::ContextEntry {
        entry_type: Some("runx.operator.context".into()),
        version: runx_contracts::ContextEntryVersion::V1,
        scope: runx_contracts::ContextScope {
            purpose,
            trust,
            sharing: loaded.profile.model.context_sharing,
            goal_id: item.map(|item| item.goal_id.clone().into()),
            source_ref: item.map(|item| item.source_ref.clone().into()),
        },
        data,
        meta: runx_contracts::ContextArtifactMeta {
            artifact_id: digest.clone().into(),
            run_id: format!("assistant:{}", loaded.profile.instance_id).into(),
            step_id: None,
            producer: runx_contracts::ContextArtifactProducer {
                skill: "runx-assistant".into(),
                runner: "assistant".into(),
            },
            created_at: created_at.to_owned().into(),
            hash: digest.into(),
            size_bytes: bytes.len() as u64,
            parent_artifact_id: None,
            receipt_id: receipt.map(|receipt| receipt.to_owned().into()),
            redacted: false,
        },
    })
}

fn operator_context(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &ControlRecord,
    item: Option<&WorkAssignment>,
) -> Result<
    (
        Vec<runx_contracts::ContextEntry>,
        Vec<runx_contracts::ContextEntry>,
    ),
    String,
> {
    use runx_contracts::{ContextPurpose as Purpose, ContextTrust as Trust};
    let now = now_iso8601();
    let goals = loaded
        .profile
        .goals
        .iter()
        .filter(|goal| goal.enabled && item.is_none_or(|item| item.goal_id == goal.goal_id))
        .collect::<Vec<_>>();
    let mut current = vec![context_entry(
        loaded,
        Purpose::Mandate,
        Trust::OperatorConfirmed,
        item,
        &now,
        json!({"profile_revision":loaded.revision,"charter":loaded.profile.charter,"goals":goals,
            "memory_selection":"newest matching confirmed facts within the configured context budget"}),
        None,
    )?];
    if let Some(item) = item {
        current.push(context_entry(loaded, Purpose::Observation, Trust::UntrustedEvidence, Some(item), &now,
            json!({"source_ref":item.source_ref,"source_digest":item.source_digest,"thread_locator":item.thread_locator,
                "input":item.turn.route.source_context_input}), None)?);
        current.push(context_entry(loaded, Purpose::WorkContinuity, Trust::UntrustedEvidence, Some(item), &now,
            json!({"work_id":item.id,"goal_id":item.goal_id,"completion_condition":item.completion_condition,
                "source_ref":item.source_ref,"source_digest":item.source_digest,"target_ref":item.turn.target_ref,
            "turn_id":item.turn.id}), None)?);
    }
    let entry_bytes = |entry: &runx_contracts::ContextEntry| {
        serde_json::to_vec(entry)
            .map(|encoded| encoded.len())
            .map_err(|error| error.to_string())
    };
    let mut used = current
        .iter()
        .map(&entry_bytes)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .sum::<usize>();
    if used > loaded.profile.context_budget_bytes {
        return Err("assistant mandate and active work exceed the context budget".to_owned());
    }
    for memory in record
        .state
        .confirmed_memory
        .iter()
        .rev()
        .filter(|memory| {
            memory.goal_ids.is_empty()
                || item.is_some_and(|item| memory.goal_ids.contains(&item.goal_id))
        })
        .filter(|memory| {
            memory.source_refs.is_empty()
                || item.is_some_and(|item| memory.source_refs.contains(&item.source_ref))
        })
    {
        let entry = context_entry(
            loaded,
            Purpose::ConfirmedMemory,
            Trust::OperatorConfirmed,
            item,
            &memory.confirmed_at,
            serde_json::to_value(memory).map_err(|error| error.to_string())?,
            None,
        )?;
        let bytes = entry_bytes(&entry)?;
        if used.saturating_add(bytes) <= loaded.profile.context_budget_bytes {
            current.push(entry);
            used += bytes;
        }
    }
    let mut historical = Vec::new();
    if let Some(item) = item {
        for prior in record
            .state
            .work
            .iter()
            .filter(|prior| {
                prior.goal_id == item.goal_id
                    && prior.source_ref == item.source_ref
                    && prior.receipt.is_some()
                    && prior.result_ref.is_some()
            })
            .rev()
            .take(3)
        {
            let result =
                read_work_result(workspace, prior)?.ok_or("retrieved work lost its result")?;
            let entry = context_entry(
                loaded,
                Purpose::RetrievedHistory,
                Trust::UntrustedEvidence,
                Some(item),
                prior.completed_at.as_deref().unwrap_or(&now),
                json!({"work_id":prior.id,"result":result,
                    "status":prior.status,"source_digest":prior.source_digest}),
                prior.receipt.as_deref(),
            )?;
            let bytes = entry_bytes(&entry)?;
            if used.saturating_add(bytes) <= loaded.profile.context_budget_bytes {
                historical.push(entry);
                used += bytes;
            }
        }
    }
    Ok((current, historical))
}

fn review_observations(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &ControlRecord,
    observations: &[Value],
) -> Result<ReviewPacket, String> {
    let dispositions = current_dispositions(loaded, workspace, observations)?;
    let model_evidence = observations
        .iter()
        .map(|item| {
            json!({
                "source_ref":item["source_ref"],
                "source_digest":item["source_digest"],
                "source_kind":item["source_kind"],
                "thread_locator":item["thread_locator"],
                "observed_at":item["observed_at"],
                "summary":item["summary"]
            })
        })
        .collect::<Vec<_>>();
    let work_results = record
        .state
        .work
        .iter()
        .filter(|work| {
            work.status == "completed"
                && observations
                    .iter()
                    .any(|item| item["source_digest"] == work.source_digest)
        })
        .take(20)
        .map(|work| -> Result<Option<Value>, String> {
            let Some(result) = read_work_result(workspace, work)? else {
                return Ok(None);
            };
            let Some(receipt) = work.receipt.as_ref() else {
                return Ok(None);
            };
            let status = result["disposition"]
                .as_str()
                .ok_or("work result lacks disposition")?;
            let kind = "skill_work";
            let checked_at = work
                .completed_at
                .as_deref()
                .ok_or("completed work lacks completion time")?;
            Ok(Some(json!({
                "source_ref":work.source_ref,
                "source_digest":work.source_digest,
                "route_id":work.turn.route.route_id,
                "target_ref":work.turn.target_ref,
                "kind":kind,
                "status":status,
                "summary":work_summary(&result)?.chars().take(1000).collect::<String>(),
                "source_complete":result["source_complete"].as_bool().unwrap_or(true),
                "checked_at":checked_at,
                "receipt":receipt,
            })))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let routes = work_routes(loaded)?;
    let context = operator_context(loaded, workspace, record, None)?;
    let (review, receipt) = run_skill_with_id(
        loaded,
        workspace,
        "attention-review",
        "review",
        json!({
            "objective":"Review bounded observations from the configured sources. For each selected actionable source, propose a suitable configured work route with its exact target; without a proposal, only a brief is produced. Use verified results to avoid stale claims and preserve partial scan coverage.",
            "as_of":now_iso8601(),
            "evidence":model_evidence,
            "current_actions":dispositions,
            "allowed_action_ids":loaded.profile.allowed_action_ids,
            "work_routes":routes,
            "work_results":work_results,
            "min_check_minutes":loaded.profile.min_check_minutes,
            "max_check_minutes":loaded.profile.max_check_minutes
        }),
        true,
        false,
        None,
        None,
        Some((&context.0, &context.1)),
    )?;
    let packet = result_data(&review, "attention_packet")?.clone();
    if packet["decision"] == "held" {
        return Err(format!(
            "attention judgment was held: {}",
            packet["validation"]["findings"]
        ));
    }
    Ok(ReviewPacket {
        packet,
        receipt: Some(receipt),
    })
}

fn read_review(workspace: &WorkspaceEnv, reference: &Value) -> Result<ReviewPacket, String> {
    let reference = serde_json::from_value(reference.clone())
        .map_err(|error| format!("decoding assistant review reference: {error}"))?;
    let value = runx_runtime::read_json_artifact(&workspace.paths().artifacts(), &reference)?;
    serde_json::from_value(
        serde_json::to_value(value)
            .map_err(|error| format!("decoding assistant review: {error}"))?,
    )
    .map_err(|error| format!("assistant review artifact is invalid: {error}"))
}

fn validate_review(loaded: &LoadedProfile, review: &ReviewPacket) -> Result<(), String> {
    let packet = &review.packet;
    let decision = packet["decision"]
        .as_str()
        .ok_or("attention packet has no decision")?;
    if decision != "ready" && decision != "idle" {
        return Err("attention judgment was not admitted".to_owned());
    }
    let minutes = packet["next_check_minutes"]
        .as_u64()
        .ok_or("attention packet has no next-check minutes")?;
    if !(loaded.profile.min_check_minutes..=loaded.profile.max_check_minutes).contains(&minutes) {
        return Err("attention packet next-check delay is outside policy".to_owned());
    }
    if review.receipt.is_some() && packet["validation"]["status"] != "pass" {
        return Err("attention packet lacks a passing skill validation".to_owned());
    }
    let actions = packet["recommended_action_ids"].as_array();
    if review.receipt.is_some() && actions.is_none() {
        return Err("attention packet lacks an action roster".to_owned());
    }
    if actions.is_some_and(|ids| {
        ids.iter().any(|id| {
            id.as_str().is_none_or(|id| {
                !loaded
                    .profile
                    .allowed_action_ids
                    .iter()
                    .any(|allowed| allowed == id)
            })
        })
    }) {
        return Err("attention packet recommends an unconfigured action".to_owned());
    }
    if review.receipt.is_some() && packet["work_proposals"].as_array().is_none() {
        return Err("attention packet lacks work proposals".to_owned());
    }
    Ok(())
}

fn commit_pending_pages(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
) -> Result<(), String> {
    let pending = record
        .state
        .pending_turn
        .as_ref()
        .ok_or("assistant turn lacks pinned work")?;
    if pending.deferred {
        return Ok(());
    }
    if pending.pages.len() != loaded.profile.sources.len() {
        return Err("assistant turn lacks a complete pinned source set".to_owned());
    }
    let pages = pending.pages.clone();
    for source in &loaded.profile.sources {
        let pinned = pages
            .iter()
            .find(|page| page.source_id == source.id())
            .ok_or("assistant turn lacks a configured source page")?;
        let page = read_pinned_page(workspace, source, pinned)?;
        record_scan(loaded, workspace, source, &page)?;
        if page.page_index == 1 {
            record
                .state
                .last_fresh_scan_at
                .insert(source.id().to_owned(), page.fetched_at_unix_seconds);
        }
    }
    Ok(())
}

fn current_dispositions(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    observations: &[Value],
) -> Result<Vec<Value>, String> {
    let mut answer = Vec::new();
    for observation in observations {
        let Some(thread) = observation["thread_locator"].as_str() else {
            continue;
        };
        if let Some(action) = read_action_state(loaded, workspace, thread)? {
            let status = action["status"]
                .as_str()
                .ok_or("operator-inbox action lacks status")?;
            answer.push(json!({"source_ref":observation["source_ref"],"disposition":status}));
        }
    }
    Ok(answer)
}

fn read_action_state(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    thread: &str,
) -> Result<Option<Value>, String> {
    let action_id = format!(
        "action-{}",
        sha256_prefixed(thread.as_bytes()).trim_start_matches("sha256:")
    );
    let (output, _) = run_skill(
        loaded,
        workspace,
        "operator-inbox",
        "read_action",
        json!({"data_source_ref":loaded.profile.inbox_data_source_ref,"action_id":action_id}),
        false,
        false,
    )?;
    let rows = result_data(&output, "data_operation_result")?["rows"]
        .as_array()
        .filter(|rows| rows.len() <= 1)
        .ok_or("operator-inbox action read lacks bounded rows")?;
    Ok(rows
        .first()
        .map(|row| row["event"]["payload"]["action"].clone()))
}

fn work_routes(loaded: &LoadedProfile) -> Result<Vec<Value>, String> {
    let mut routes = Vec::new();
    for route in &loaded.profile.work_routes {
        if route.successor_only {
            continue;
        }
        let Some(goal) = loaded
            .profile
            .goals
            .iter()
            .find(|goal| goal.goal_id == route.goal_id && goal.enabled)
        else {
            continue;
        };
        let inspection = runx_runtime::inspect_skill_package(
            &loaded.profile.skill_path(&route.skill),
            Some(&route.runner),
            None,
        )
        .map_err(|error| error.to_string())?;
        let inspection = serde_json::to_value(inspection).map_err(|error| error.to_string())?;
        routes.push(json!({
            "route_id":route.route_id,"goal_id":goal.goal_id,"purpose":goal.purpose,
            "completion_condition":goal.completion_condition,"description":route.description,
            "skill":route.skill,"runner":route.runner,"fixed_inputs":route.inputs,
            "source_kinds":route.source_kinds,"source_context_input":route.source_context_input,
            "turn_id_input":route.turn_id_input,"origin_ref_input":route.origin_ref_input,
            "input_schema":inspection["runner"]["input_schema"],
        }));
    }
    Ok(routes)
}

fn admit_work(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    observations: &[Value],
    packet: &Value,
) -> Result<(), String> {
    let proposals = packet["work_proposals"]
        .as_array()
        .filter(|items| items.len() <= 3)
        .ok_or("attention packet lacks bounded work proposals")?;
    let dispositions = current_dispositions(loaded, workspace, observations)?;
    let mut admitted = record.state.work.clone();
    for proposal in proposals {
        let source_ref = proposal["source_ref"]
            .as_str()
            .ok_or("proposal lacks source")?;
        if !dispositions
            .iter()
            .any(|item| item["source_ref"] == source_ref && item["disposition"] == "open")
        {
            return Err("work proposal source is not open".to_owned());
        }
        let observation = observations
            .iter()
            .find(|item| item["source_ref"] == source_ref)
            .ok_or("work proposal lost its source")?;
        let route = loaded
            .profile
            .work_routes
            .iter()
            .find(|route| proposal["route_id"] == route.route_id)
            .ok_or("proposal route is not configured")?;
        if observation["source_digest"] != proposal["source_digest"]
            || observation["thread_locator"] != proposal["target_ref"]
            || (!route.source_kinds.is_empty()
                && !route
                    .source_kinds
                    .iter()
                    .any(|kind| observation["source_kind"] == *kind))
        {
            return Err(
                "proposal is outside the exact source occurrence or route policy".to_owned(),
            );
        }
        let proposal: runx_contracts::WorkProposal = serde_json::from_value(json!({
            "route_id":proposal["route_id"], "target_ref":proposal["target_ref"],
            "inputs":proposal["inputs"], "due_at_unix_seconds":proposal["due_at_unix_seconds"],
        }))
        .map_err(|error| format!("invalid work proposal: {error}"))?;
        let item = admitted_assignment(loaded, workspace, observation, None, &proposal, false)?;
        if let Some(existing) = admitted.iter().find(|entry| entry.id == item.id) {
            if existing.source_digest != item.source_digest
                || existing.turn.inputs != item.turn.inputs
            {
                return Err("existing obligation requires explicit revision; content cannot replace its occurrence".to_owned());
            }
            continue;
        }
        make_work_room(&mut admitted, None)?;
        admitted.push(item);
    }
    if !proposals.is_empty() {
        record.state.work = admitted;
        write_control(loaded, workspace, record, "work_admitted")?;
    }
    Ok(())
}

fn admitted_assignment(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    observation: &Value,
    parent: Option<&WorkAssignment>,
    proposal: &runx_contracts::WorkProposal,
    scheduled_occurrence: bool,
) -> Result<WorkAssignment, String> {
    let route = loaded
        .profile
        .work_routes
        .iter()
        .find(|route| route.route_id == proposal.route_id)
        .ok_or("proposal route is not configured")?;
    let goal = loaded
        .profile
        .goals
        .iter()
        .find(|goal| goal.goal_id == route.goal_id && goal.enabled)
        .ok_or("proposal goal is not enabled")?;
    if scheduled_occurrence {
        let schedule = goal
            .schedule
            .as_ref()
            .ok_or("scheduled work has no goal schedule")?;
        let expected = goal_occurrence_prefix(&goal.goal_id, &schedule_digest(schedule)?);
        let due = observation["source_ref"]
            .as_str()
            .and_then(|reference| reference.strip_prefix(&expected))
            .and_then(|due| due.parse::<u64>().ok());
        if parent.is_some()
            || schedule.route_id != route.route_id
            || schedule.target_ref != proposal.target_ref
            || due.is_none_or(|due| {
                due_goal_occurrence(schedule, None, due).ok().flatten() != Some(due)
            })
            || observation["source_digest"]
                != sha256_prefixed(observation["source_ref"].as_str().unwrap_or("").as_bytes())
            || observation["thread_locator"] != schedule.target_ref
            || !proposal.inputs.is_empty()
        {
            return Err("scheduled work differs from its declared occurrence".to_owned());
        }
    }
    if parent.is_none() && route.successor_only {
        return Err("successor-only work cannot start from an observation".to_owned());
    }
    // A work route selects its own exact adapter instance. It may intentionally
    // act through a different provider from the source; a same-provider reply
    // is still bound by the native effect's exact target and instance checks.
    if parent.is_some_and(|parent| {
        parent.goal_id != goal.goal_id || proposal.target_ref != parent.thread_locator
    }) || proposal.target_ref.trim().is_empty()
        || proposal.target_ref.len() > 300
        || proposal.due_after_seconds.is_some_and(|delay| {
            delay == 0 || delay > 24 * 60 * 60 || proposal.due_at_unix_seconds != 0
        })
        || proposal.due_at_unix_seconds > now_seconds().saturating_add(30 * 24 * 60 * 60)
    {
        return Err("work proposal exceeds its goal, target or scheduling policy".to_owned());
    }
    let mut inputs: serde_json::Map<String, Value> = serde_json::from_value(
        serde_json::to_value(&proposal.inputs).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    for (key, fixed) in &route.inputs {
        if inputs.get(key).is_some_and(|proposed| proposed != fixed) {
            return Err(format!("proposal cannot replace fixed route input {key}"));
        }
        inputs.insert(key.clone(), fixed.clone());
    }
    if route
        .source_context_input
        .as_ref()
        .is_some_and(|key| inputs.contains_key(key))
        || route
            .turn_id_input
            .as_ref()
            .is_some_and(|key| inputs.contains_key(key))
        || route
            .origin_ref_input
            .as_ref()
            .is_some_and(|key| inputs.contains_key(key))
    {
        return Err("proposal cannot fabricate operator-bound work evidence".to_owned());
    }
    let source_ref = observation["source_ref"]
        .as_str()
        .ok_or("work origin lacks identity")?;
    // Identity is the occurrence and target, never a content hash or the mutable skill version.
    let id = sha256_prefixed(
        &serde_json::to_vec(&json!([
            loaded.profile.instance_id,
            parent
                .map(|parent| parent.id.as_str())
                .unwrap_or(source_ref),
            route.route_id,
            proposal.target_ref,
        ]))
        .map_err(|error| error.to_string())?,
    );
    let package_digest = loaded
        .skill_bindings
        .get(&route.skill)
        .ok_or("work has no inspected skill binding")?
        .clone();
    pinned_skill_path(
        workspace,
        &loaded.profile.skill_path(&route.skill),
        &route.skill,
        &package_digest,
    )?;
    let turn_id = sha256_prefixed(
        &serde_json::to_vec(&json!([
            parent.map(|parent| parent.turn.id.as_str()).unwrap_or(&id),
            route.route_id,
            proposal.target_ref,
        ]))
        .map_err(|error| error.to_string())?,
    );
    let source_binding = if let Some(parent) = parent {
        parent.source_binding.clone()
    } else if scheduled_occurrence {
        None
    } else {
        let sources = loaded
            .profile
            .sources
            .iter()
            .filter(|source| source_ref.starts_with(&source.locator_prefix))
            .collect::<Vec<_>>();
        let [source] = sources.as_slice() else {
            return Err("work origin must bind one exact configured source instance".to_owned());
        };
        let digest = loaded
            .skill_bindings
            .get(&source.skill)
            .ok_or("source lacks an inspected package")?
            .clone();
        pinned_skill_path(
            workspace,
            &loaded.profile.skill_path(&source.skill),
            &source.skill,
            &digest,
        )?;
        let mut pinned = (*source).clone();
        pinned.local_provider = source
            .local_provider
            .as_ref()
            .map(|profile| pin_local_provider(workspace, profile))
            .transpose()?;
        Some(SourceBinding {
            profile: pinned,
            package_digest: digest,
        })
    };
    Ok(WorkAssignment {
        id,
        goal_id: goal.goal_id.clone(),
        completion_condition: goal.completion_condition.clone(),
        source_ref: source_ref.to_owned(),
        source_digest: observation["source_digest"]
            .as_str()
            .ok_or("work origin lacks revision")?
            .to_owned(),
        thread_locator: observation["thread_locator"]
            .as_str()
            .ok_or("work origin lacks locator")?
            .to_owned(),
        source_binding,
        turn: WorkTurn {
            id: turn_id,
            route: route.clone(),
            package_digest,
            target_ref: proposal.target_ref.clone(),
            inputs: Value::Object(inputs),
            due_at_unix_seconds: proposal
                .due_after_seconds
                .map_or(proposal.due_at_unix_seconds, |delay| {
                    now_seconds().saturating_add(delay)
                }),
        },
        continuations: Vec::new(),
        status: "pending".to_owned(),
        followup_blocker: None,
        completed_at: None,
        receipt: None,
        result_ref: None,
        native_run: None,
        failed_runs: Vec::new(),
        decision: None,
        private_updates: Vec::new(),
    })
}

fn make_work_room(work: &mut Vec<WorkAssignment>, parent: Option<&str>) -> Result<(), String> {
    while work.len() >= 40 {
        let index = evictable_work_index(work, parent)
            .ok_or("work ledger is full of unresolved obligations")?;
        work.remove(index);
    }
    Ok(())
}

fn dispatch_pending_work(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
) -> Result<Value, SkillRunStop> {
    let index = due_work_index(&record.state.work, now_seconds()).ok_or("no due assistant work")?;
    let item = record.state.work[index].clone();
    let mut retry_answer_path = None;
    if let Some(bound) = item.native_run.as_ref() {
        let native =
            runx_runtime::run_record::paused_run(workspace.env(), workspace.cwd(), &bound.run_id)
                .map_err(|error| format!("inspecting native run: {error}"))?;
        if let Some(native) = native {
            let path = pinned_skill_path_by_digest(workspace, &bound.skill, &bound.package_digest)?;
            if Some(native.skill_ref.as_str()) != path.to_str() {
                return Err("native run differs from its admitted binding".into());
            }
            let [request] = native.waiting.as_slice() else {
                return Err("assistant work waits on one native request at a time".into());
            };
            let native_request: ResolutionRequest = serde_json::from_value(
                serde_json::to_value(&request.value).map_err(|error| error.to_string())?,
            )
            .map_err(|error| format!("decoding native pending request: {error}"))?;
            if let ResolutionRequest::Retry {
                retry_after_seconds,
                ..
            } = native_request
            {
                if retry_after_seconds == 0 || retry_after_seconds > 3600 || item.decision.is_some()
                {
                    return Err("native retry differs from the bound readback continuation".into());
                }
                let answer =
                    serde_json::from_value(json!({"answers":{request.id.clone():{"retry":true}}}))
                        .map_err(|error| format!("encoding native retry answer: {error}"))?;
                let reference = runx_runtime::persist_json_artifact(
                    &workspace.paths().artifacts(),
                    &bound.run_id,
                    "assistant-retries",
                    &request.id,
                    &answer,
                )?;
                runx_runtime::read_json_artifact(&workspace.paths().artifacts(), &reference)?;
                let path =
                    serde_json::to_value(&reference).map_err(|error| error.to_string())?["path"]
                        .as_str()
                        .map(PathBuf::from)
                        .ok_or("native retry artifact lacks a path")?;
                retry_answer_path = Some(path);
            } else if let Some(decision) = item.decision.as_ref() {
                let current_digest = sha256_prefixed(
                    &serde_json::to_vec(&request.value).map_err(|error| error.to_string())?,
                );
                if decision.request_id != request.id || decision.request_digest != current_digest {
                    return Err(
                        "decision projection differs from the native approval checkpoint".into(),
                    );
                }
                if decision.answer_ref.is_none() {
                    return handle_pending_decision(
                        loaded,
                        workspace,
                        record,
                        &item,
                        decision,
                        &request.value,
                    )
                    .map_err(SkillRunStop::from);
                }
            } else {
                return record_suspended_work(
                    loaded,
                    workspace,
                    &item.id,
                    &bound.skill,
                    &bound.run_id,
                    &[json!({"id":request.id})],
                )
                .map_err(SkillRunStop::from);
            }
        }
    }
    let route = &item.turn.route;
    // Source-dependent fresh work requires an open source. Recovery reads may
    // proceed after closure without losing custody of an accepted operation.
    if item.native_run.is_none() && route.source_context_input.is_some() {
        let dispositions = current_dispositions(
            loaded,
            workspace,
            &[json!({"source_ref":item.source_ref,"thread_locator":item.thread_locator})],
        )?;
        if !dispositions
            .iter()
            .any(|state| state["disposition"] == "open")
        {
            record.state.work[index].status = "held".to_owned();
            write_control(loaded, workspace, record, "work_source_closed")?;
            return Ok(
                json!({"status":"work_held","work_id":item.id,"reason":"source is no longer open"}),
            );
        }
    }
    let (result, receipt) = run_assigned_skill(
        loaded,
        workspace,
        record,
        index,
        &item,
        route,
        retry_answer_path.as_deref(),
    )?;
    bounded_work_artifact(&result)?;
    let outcome: runx_contracts::WorkOutcome =
        serde_json::from_value(result.clone()).map_err(|error| {
            SkillRunStop::sealed_validation(
                &route.skill,
                &receipt,
                format!("invalid work result: {error}"),
            )
        })?;
    if outcome.summary.trim().is_empty()
        || outcome.summary.len() > 1000
        || outcome.followups.len() > 3
        || (outcome.disposition == runx_contracts::WorkDisposition::Satisfied
            && outcome.completion_evidence.is_empty())
        || (outcome.disposition == runx_contracts::WorkDisposition::FollowUp
            && outcome.followups.is_empty())
        || (outcome.disposition != runx_contracts::WorkDisposition::FollowUp
            && !outcome.followups.is_empty())
    {
        return Err(SkillRunStop::sealed_validation(
            &route.skill,
            &receipt,
            "work result lacks bounded completion or successor evidence",
        ));
    }
    let origin = json!({"source_ref":item.source_ref, "source_digest":item.source_digest, "thread_locator":item.thread_locator});
    let children = outcome
        .followups
        .iter()
        .map(|proposal| {
            admitted_assignment(loaded, workspace, &origin, Some(&item), proposal, false)
        })
        .collect::<Result<Vec<_>, _>>();
    let (children, followup_blocker) = match children {
        Ok(children) => (children, None),
        Err(error) => (
            Vec::new(),
            Some(format!("successor was not admitted: {error}")),
        ),
    };
    let reference = persist_work_result(workspace, &item, &result)?;
    let next_turns = children
        .into_iter()
        .map(|child| child.turn)
        .collect::<Vec<_>>();
    let expected = record
        .state
        .work
        .iter()
        .find(|work| work.id == item.id)
        .ok_or("executed obligation lost its head")?
        .clone();
    let mut admitted_outcome = outcome.clone();
    if followup_blocker.is_some() {
        admitted_outcome.disposition = runx_contracts::WorkDisposition::Held;
        admitted_outcome.followups.clear();
    }
    let mut updated = transition_assignment(
        &expected,
        &admitted_outcome,
        next_turns,
        &receipt,
        reference,
    )?;
    updated.followup_blocker = followup_blocker;
    let held_reason = updated.followup_blocker.clone();
    let terminal_status = match updated.status.as_str() {
        "held" => "work_held",
        "pending" => "work_advanced",
        _ => "work_satisfied",
    };
    // Model/provider execution holds no intake lock or SQLite transaction.
    let _control_lock = lock(loaded, workspace)?;
    let mut current = read_control(loaded, workspace)?;
    let index = current
        .state
        .work
        .iter()
        .position(|work| work.id == expected.id)
        .ok_or("executed obligation lost its durable head")?;
    let head = &current.state.work[index];
    if head.turn != expected.turn || head.native_run != expected.native_run {
        return Err(
            "obligation changed while its bound run was executing; sealed evidence is retained"
                .into(),
        );
    }
    // Result acknowledgement and all successors are one CAS against the fresh
    // head. Concurrent source intake and notification progress are preserved.
    current.state.work[index] = updated;
    current.state.next_due_unix_seconds = 0;
    current.state.last_blocker = None;
    write_control(loaded, workspace, &mut current, "work_completed")?;
    *record = current;
    Ok(json!({"status":terminal_status,
        "work_id":item.id,"receipt":receipt,"followup_blocker":held_reason,"result":result}))
}

fn handle_pending_decision(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    item: &WorkAssignment,
    decision: &DecisionProjection,
    native_request: &runx_contracts::JsonValue,
) -> Result<Value, String> {
    let bound = item
        .native_run
        .as_ref()
        .ok_or("decision lost its native run")?;
    let request: ResolutionRequest = serde_json::from_value(
        serde_json::to_value(native_request).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("invalid native decision request: {error}"))?;
    let ResolutionRequest::Approval { gate, .. } = request else {
        return Err("decision projection does not name a native approval".to_owned());
    };
    if decision.presentation_ref.is_none() {
        if record.state.pending_intent.is_some() {
            let index = record
                .state
                .work
                .iter()
                .position(|work| work.id == item.id)
                .ok_or("decision obligation disappeared")?;
            record.state.work[index].turn.due_at_unix_seconds =
                scheduled_due(loaded.profile.min_check_minutes);
            write_control(
                loaded,
                workspace,
                record,
                "decision_waiting_private_delivery",
            )?;
            return Ok(json!({"status":"decision_waiting_private_delivery","work_id":item.id}));
        }
        let exact_request = serde_json::to_string(&gate)
            .map_err(|error| format!("encoding exact approval gate: {error}"))?;
        let mention = loaded
            .profile
            .notification
            .as_ref()
            .and_then(|notification| notification.allowed_mention.as_deref())
            .unwrap_or("");
        let text = format!(
            "{mention} Approval needed for a paused Runx action.\nRequest: {}\nDigest: {}\nExact gate: {}\nReply in this thread with exactly `approve {}` or `deny {}`.",
            decision.nonce, decision.request_digest, exact_request, decision.nonce, decision.nonce
        );
        let digest =
            sha256_prefixed(format!("{}:{}", bound.run_id, decision.request_digest).as_bytes());
        let logical = format!("run_assistant_decision-{}", &digest[7..39]);
        return stage_notification(
            loaded,
            workspace,
            record,
            &text,
            &decision.request_digest,
            Vec::new(),
            BTreeMap::new(),
            &logical,
            Some((&item.id, &decision.request_digest)),
        );
    }
    let presentation_ref = decision
        .presentation_ref
        .as_deref()
        .ok_or("decision presentation is missing")?;
    pinned_skill_path_by_digest(
        workspace,
        &decision.adapter_skill,
        &decision.adapter_package_digest,
    )?;
    let (output, receipt) = run_skill_with_resolution(
        loaded,
        workspace,
        &decision.adapter_skill,
        "assistant_decide",
        json!({"presentation_ref":presentation_ref,"request_digest":decision.request_digest,
            "nonce":decision.nonce,"authorized_actor_ref":decision.authorized_actor_ref}),
        false,
        false,
        None,
        Some(&decision.adapter_package_digest),
        None,
        None,
        decision.local_provider.as_ref(),
    )
    .map_err(String::from)?;
    let observation: DecisionObservation =
        serde_json::from_value(result_data(&output, "decision_observation")?.clone()).map_err(
            |error| format!("decision adapter violated the shared observation contract: {error}"),
        )?;
    let mut answer_ref = None;
    let mut evidence_ref = None;
    let status = match observation {
        DecisionObservation::Pending {
            request_digest,
            nonce,
            coverage_complete,
            next_cursor,
        } => {
            if request_digest != decision.request_digest
                || nonce != decision.nonce
                || (coverage_complete && next_cursor.is_some())
            {
                return Err(
                    "decision adapter returned a different or inconsistent pending request"
                        .to_owned(),
                );
            }
            if coverage_complete {
                "decision_pending"
            } else {
                "decision_coverage_incomplete"
            }
        }
        DecisionObservation::Decided {
            request_digest,
            nonce,
            presentation_ref: observed_ref,
            actor_ref,
            origin,
            evidence_ref: observed_evidence,
            occurred_at,
            approved,
        } => {
            let channel = presentation_ref
                .rsplit_once('/')
                .map(|part| part.0)
                .ok_or("decision presentation locator is malformed")?;
            let observed_at = DateTime::parse_from_rfc3339(&occurred_at)
                .map_err(|_| "decision response lacks a valid time")?;
            if request_digest != decision.request_digest
                || nonce != decision.nonce
                || observed_ref != presentation_ref
                || actor_ref != decision.authorized_actor_ref
                || origin != DecisionOrigin::HumanAccount
                || !observed_evidence.starts_with(&format!("{channel}/"))
                || observed_evidence == presentation_ref
                || observed_at > Utc::now() + chrono::Duration::minutes(5)
            {
                return Err(
                    "decision adapter response differs from the authorized human request"
                        .to_owned(),
                );
            }
            let answer: runx_contracts::JsonValue = serde_json::from_value(json!({
                "approvals":{decision.request_id.clone():approved}
            }))
            .map_err(|error| format!("encoding native approval answer: {error}"))?;
            answer_ref = Some(
                serde_json::to_value(runx_runtime::persist_json_artifact(
                    &workspace.paths().artifacts(),
                    &bound.run_id,
                    "assistant-decisions",
                    &decision.nonce,
                    &answer,
                )?)
                .map_err(|error| error.to_string())?,
            );
            evidence_ref = Some(observed_evidence);
            "decision_recorded"
        }
    };
    let _control_lock = lock(loaded, workspace)?;
    let mut current = read_control(loaded, workspace)?;
    let current_item = current
        .state
        .work
        .iter_mut()
        .find(|work| work.id == item.id && work_is_open(work))
        .ok_or("decision obligation changed while its adapter ran")?;
    if current_item.decision.as_ref() != Some(decision) {
        return Err("decision changed while its adapter ran".to_owned());
    }
    let current_decision = current_item
        .decision
        .as_mut()
        .ok_or("decision disappeared")?;
    current_decision.observation_receipt = Some(receipt.clone());
    current_decision.evidence_ref = evidence_ref;
    current_decision.answer_ref = answer_ref;
    current_item.turn.due_at_unix_seconds = if current_decision.answer_ref.is_some() {
        0
    } else {
        scheduled_due(loaded.profile.min_check_minutes)
    };
    current.state.last_blocker = if status == "decision_coverage_incomplete" {
        Some(
            "approval thread exceeds the bounded decision read; human review is required"
                .to_owned(),
        )
    } else {
        None
    };
    write_control(loaded, workspace, &mut current, status)?;
    *record = current;
    Ok(json!({"status":status,"work_id":item.id,"receipt":receipt}))
}

fn transition_assignment(
    item: &WorkAssignment,
    outcome: &runx_contracts::WorkOutcome,
    mut next: Vec<WorkTurn>,
    receipt: &str,
    result_ref: Value,
) -> Result<WorkAssignment, String> {
    let mut updated = item.clone();
    if next.len() + updated.continuations.len() > 12 {
        return Err("obligation exceeds 12 admitted continuations".to_owned());
    }
    let mut ids = BTreeSet::new();
    if next
        .iter()
        .chain(&updated.continuations)
        .any(|turn| !ids.insert(&turn.id))
    {
        return Err("obligation repeats an admitted turn occurrence".to_owned());
    }
    next.append(&mut updated.continuations);
    updated.receipt = Some(receipt.to_owned());
    updated.result_ref = Some(result_ref);
    updated.completed_at = Some(now_iso8601());
    updated
        .private_updates
        .retain(|update| update.status.is_none());
    if updated.private_updates.len() >= 12 {
        return Err("obligation has 12 undelivered private updates; deliver or resolve them before another turn".to_owned());
    }
    updated.private_updates.push(WorkUpdate {
        turn_id: item.turn.id.clone(),
        receipt: receipt.to_owned(),
        result_ref: updated
            .result_ref
            .clone()
            .ok_or("work result reference is missing")?,
        status: None,
    });
    updated.decision = None;
    updated.followup_blocker = None;
    if outcome.disposition == runx_contracts::WorkDisposition::Held {
        updated.status = "held".to_owned();
        updated.continuations = next;
    } else if !next.is_empty() {
        updated.turn = next.remove(0);
        updated.continuations = next;
        updated.status = "pending".to_owned();
        updated.native_run = None;
    } else {
        updated.status = "completed".to_owned();
    }
    Ok(updated)
}

fn bounded_work_artifact(value: &Value) -> Result<(), String> {
    if serde_json::to_vec(value)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_WORK_ARTIFACT_BYTES
    {
        return Err("assistant work artifact exceeds 32 KiB".to_owned());
    }
    Ok(())
}

fn run_assigned_skill(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    index: usize,
    item: &WorkAssignment,
    route: &WorkRoute,
    retry_answer_path: Option<&Path>,
) -> Result<(Value, String), SkillRunStop> {
    let (bound, inputs, _) = bound_work_inputs(
        loaded,
        workspace,
        record,
        index,
        &route.skill,
        "run_assistant_work_",
        || {
            let mut inputs = item
                .turn
                .inputs
                .as_object()
                .cloned()
                .ok_or("admitted work lacks inputs")?;
            let mut receipts = Vec::new();
            if let Some(key) = &route.turn_id_input {
                inputs.insert(key.clone(), json!(item.turn.id));
            }
            if let Some(key) = &route.origin_ref_input {
                inputs.insert(key.clone(), json!(item.source_ref));
            }
            if let Some(key) = &route.source_context_input {
                let source = hydrate_source(loaded, workspace, item)?;
                inputs.insert(key.clone(), source.packet);
                receipts = source.receipts;
            }
            let native = serde_json::from_value(Value::Object(inputs))
                .map_err(|error| format!("invalid admitted inputs: {error}"))?;
            let inputs = runx_runtime::validate_skill_runner_inputs(
                &pinned_skill_path_by_digest(workspace, &route.skill, &item.turn.package_digest)?,
                &route.runner,
                &native,
            )
            .map_err(|error| error.to_string())?;
            Ok((
                serde_json::to_value(inputs).map_err(|error| error.to_string())?,
                receipts,
            ))
        },
    )?;
    let packet = read_bound_work_inputs(workspace, &bound)?;
    let current_context: Vec<runx_contracts::ContextEntry> =
        serde_json::from_value(packet["current_context"].clone())
            .map_err(|error| error.to_string())?;
    let historical_context: Vec<runx_contracts::ContextEntry> =
        serde_json::from_value(packet["historical_context"].clone())
            .map_err(|error| error.to_string())?;
    let answer_path = item
        .decision
        .as_ref()
        .and_then(|decision| decision.answer_ref.as_ref())
        .map(|reference| {
            let native_ref = serde_json::from_value(reference.clone())
                .map_err(|error| format!("invalid decision answer reference: {error}"))?;
            runx_runtime::read_json_artifact(&workspace.paths().artifacts(), &native_ref)?;
            reference["path"]
                .as_str()
                .map(PathBuf::from)
                .ok_or("decision answer reference lacks a path".to_owned())
        })
        .transpose()?;
    let (output, receipt) = run_skill_with_resolution(
        loaded,
        workspace,
        &route.skill,
        &route.runner,
        inputs,
        route.model,
        false,
        Some(&bound.run_id),
        Some(&bound.package_digest),
        Some((&current_context, &historical_context)),
        retry_answer_path.or(answer_path.as_deref()),
        bound.local_provider.as_ref(),
    )?;
    let result = result_data(&output, "work_result")?.clone();
    Ok((result, receipt))
}

struct HydratedSource {
    packet: Value,
    receipts: Vec<String>,
}

fn hydrate_source(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    item: &WorkAssignment,
) -> Result<HydratedSource, String> {
    let binding = item
        .source_binding
        .as_ref()
        .ok_or("source-dependent work has no admitted source binding")?;
    let source = &binding.profile;
    if !item.source_ref.starts_with(&source.locator_prefix)
        || !item.thread_locator.starts_with(&source.locator_prefix)
    {
        return Err("source evidence differs from its admitted adapter instance".to_owned());
    }
    pinned_skill_path_by_digest(workspace, &source.skill, &binding.package_digest)?;
    let action = read_action_state(loaded, workspace, &item.thread_locator)?
        .ok_or("source intake has no canonical open action")?;
    if action["status"] != "open" || action["thread_locator"] != item.thread_locator {
        return Err("source intake differs from its canonical open action".to_owned());
    }
    let latest = &action["latest_message"];
    if latest["message_locator"] != item.source_ref {
        return Err("source intake no longer names the canonical latest message".to_owned());
    }
    let mut cursor: Option<String> = None;
    let mut receipts = Vec::new();
    let mut messages = Vec::new();
    let mut source_packet: Option<Value> = None;
    let mut source_context: Option<Value> = None;
    let mut title = String::new();
    let mut body = String::new();
    let mut complete = true;
    let mut source_seen = false;
    let mut reply_context_digest: Option<String> = None;
    let mut requires_reply_context = false;
    let mut conversation_id: Option<u64> = None;
    for _ in 0..3 {
        let (output, receipt) = run_skill_with_resolution(
            loaded,
            workspace,
            &source.skill,
            "assistant_hydrate",
            json!({"config":source.config,"source_ref":item.source_ref,"source_digest":item.source_digest,"thread_locator":item.thread_locator,"action":action,"cursor":cursor}),
            false,
            false,
            None,
            Some(&binding.package_digest),
            None,
            None,
            source.local_provider.as_ref(),
        )?;
        receipts.push(receipt);
        let packet: runx_contracts::SourcePacket = serde_json::from_value(
            result_data(&output, "assistant_context")?.clone(),
        )
        .map_err(|error| format!("source adapter violates the shared source contract: {error}"))?;
        if packet.source_ref.uri.as_str() != item.source_ref
            || packet
                .source_ref
                .provider
                .as_ref()
                .map(|provider| provider.as_str())
                != Some(source.provider.as_str())
        {
            return Err("source adapter returned a different instance or occurrence".to_owned());
        }
        let packet_value = serde_json::to_value(&packet).map_err(|error| error.to_string())?;
        if let Some(first) = source_packet.as_ref() {
            for field in [
                "schema",
                "packet_id",
                "source_ref",
                "signal_type",
                "observed_at",
                "redaction_status",
                "authenticity",
                "related_refs",
                "extensions",
            ] {
                if first.get(field) != packet_value.get(field) {
                    return Err(
                        "source packet identity or trust changed across hydration pages".to_owned(),
                    );
                }
            }
        } else {
            source_packet = Some(packet_value);
        }
        let page =
            serde_json::to_value(&packet.workflow_inputs).map_err(|error| error.to_string())?;
        if page["source_ref"] != item.source_ref
            || page["source_digest"] != item.source_digest
            || page["thread_locator"] != item.thread_locator
        {
            return Err("source skill hydration differs from the admitted observation".to_owned());
        }
        if let Some(current) = page["source_message"].as_object() {
            let current = Value::Object(current.clone());
            if source_seen
                || current["provider"] != source.provider
                || current["message_locator"] != item.source_ref
                || current["thread_locator"] != item.thread_locator
                || current["external_tenant_ref"] != action["external_tenant_ref"]
                || current["connected_subject_ref"] != action["connected_subject_ref"]
                || current["author"]["external_id"] != action["requester"]["external_id"]
                || current["author"]["external_id"] == current["connected_subject_ref"]
                || message_for_queue(&current)?["occurred_at"] != latest["occurred_at"]
            {
                return Err("fresh source occurrence differs from the admitted action".to_owned());
            }
            source_seen = true;
        }
        let page_messages = page["context"]["messages"]
            .as_array()
            .ok_or("source context lacks bounded messages")?;
        if page_messages.len() > 25 || messages.len() + page_messages.len() > 30 {
            return Err("source context exceeds the bounded thread window".to_owned());
        }
        messages.extend(page_messages.iter().cloned());
        let mut metadata = page["context"].clone();
        let metadata_fields = metadata
            .as_object_mut()
            .ok_or("source context is not an object")?;
        metadata_fields.remove("messages");
        metadata_fields.remove("coverage_incomplete");
        if let Some(first) = source_context.as_ref() {
            if first != &metadata {
                return Err("source context metadata changed across hydration pages".to_owned());
            }
        } else {
            source_context = Some(metadata);
        }
        if let Some(id) = page["context"]["conversation_id"].as_u64() {
            if id == 0 || conversation_id.is_some_and(|current| current != id) {
                return Err(
                    "source conversation identity changed across hydration pages".to_owned(),
                );
            }
            conversation_id = Some(id);
        } else if conversation_id.is_some() {
            return Err(
                "source conversation identity disappeared across hydration pages".to_owned(),
            );
        }
        let page_requires_reply_context = page["context"]["requires_reply_context"] == true;
        if page_requires_reply_context {
            let digest = page["context"]["reply_context_digest"]
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= 128)
                .ok_or("source reply context lacks a bounded digest")?;
            if reply_context_digest
                .as_deref()
                .is_some_and(|current| current != digest)
            {
                return Err("source reply context changed across hydration pages".to_owned());
            }
            reply_context_digest = Some(digest.to_owned());
            requires_reply_context = true;
        } else if requires_reply_context {
            return Err("source reply context disappeared across hydration pages".to_owned());
        }
        if title.is_empty() {
            title = page["title"].as_str().unwrap_or("").to_owned();
        }
        if body.is_empty() {
            body = page["body"].as_str().unwrap_or("").to_owned();
        }
        complete &= page["complete"] == true;
        cursor = page["next_cursor"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if cursor.as_ref().is_some_and(|value| value.len() > 500) {
            return Err("source context cursor exceeds the bound".to_owned());
        }
        if cursor.is_none() {
            break;
        }
    }
    complete &= cursor.is_none();
    if !source_seen
        || title.is_empty()
        || body.is_empty()
        || title.chars().nth(200).is_some()
        || body.chars().nth(4000).is_some()
    {
        return Err("source context lacks a bounded title or body".to_owned());
    }
    let mut context = source_context.ok_or("source context is missing")?;
    context["source_ref"] = json!(item.source_ref);
    context["source_digest"] = json!(item.source_digest);
    context["thread_locator"] = json!(item.thread_locator);
    context["messages"] = json!(messages);
    context["coverage_incomplete"] = json!(!complete);
    let mut packet = source_packet.ok_or("source packet is missing")?;
    packet["title"] = json!(title);
    let workflow = packet["workflow_inputs"]
        .as_object_mut()
        .ok_or("source packet has no workflow inputs")?;
    workflow.remove("source_message");
    workflow.remove("next_cursor");
    workflow.remove("context");
    workflow.remove("complete");
    workflow.insert("thread".to_owned(), context);
    workflow.insert("source_complete".to_owned(), json!(complete));
    let _: runx_contracts::SourcePacket = serde_json::from_value(packet.clone())
        .map_err(|error| format!("composed source packet is invalid: {error}"))?;
    Ok(HydratedSource { packet, receipts })
}

fn private_notification_text(
    packet: &Value,
    max_bytes: usize,
    coverage_incomplete: bool,
    messages: &Messages,
) -> Result<String, String> {
    let _ = packet["brief"].as_str().ok_or("ready packet lacks brief")?;
    let items = packet["items"]
        .as_array()
        .ok_or("ready packet lacks selected items")?;
    let caveat = if coverage_incomplete {
        "\n\nSource scan is partial; older items remain unscanned."
    } else {
        ""
    };
    for summary_bytes in [160, 128, 96, 64, 48, 32, 16] {
        let mut compact = format!("Assistant update ({} items):", items.len());
        for item in items {
            let priority = item["priority"]
                .as_str()
                .ok_or("selected item lacks priority")?;
            let summary = messages.inert(
                item["summary"]
                    .as_str()
                    .ok_or("selected item lacks summary")?,
            );
            let source_digest = item["source_digest"]
                .as_str()
                .ok_or("selected item lacks source digest")?;
            if !valid_sha256_digest(source_digest) {
                return Err("selected item has invalid source digest".to_owned());
            }
            let (label, reference_kind, reference) = if item["source_kind"] == "work_result" {
                let receipt = item["receipt"]
                    .as_str()
                    .ok_or("completed work update lacks receipt")?;
                if !valid_sha256_digest(receipt) {
                    return Err("completed work update has invalid receipt".to_owned());
                }
                ("WORK", "receipt", receipt)
            } else {
                (priority, "source", source_digest)
            };
            let mut end = 0;
            for (index, character) in summary.char_indices() {
                if index + character.len_utf8() > summary_bytes {
                    break;
                }
                end = index + character.len_utf8();
            }
            let suffix = if end < summary.len() { "…" } else { "" };
            compact.push_str(&format!(
                "\n{}: {}{} ({reference_kind} {reference})",
                label.to_uppercase(),
                &summary[..end],
                suffix
            ));
        }
        let location = if items
            .iter()
            .all(|item| item["source_kind"] == "work_result")
        {
            "runx assistant work"
        } else {
            "runx assistant report"
        };
        compact.push_str(&format!("\n\nExact source links: {location}."));
        compact.push_str(caveat);
        if compact.len() <= max_bytes && messages.is_inert(&compact) {
            return Ok(compact);
        }
    }
    Err("brief and exact source references exceed configured notification limit".to_owned())
}

fn valid_sha256_digest(value: &str) -> bool {
    value.starts_with("sha256:")
        && value.len() == 71
        && value[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn admit_private_text(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    text: &str,
) -> Result<(), String> {
    if private_text_is_safe(text, &loaded.profile.confidential_terms, workspace.env()) {
        Ok(())
    } else {
        Err("assistant delivery contains protected local or credential material".to_owned())
    }
}

fn prepare_notification(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    packet: &Value,
    material_digest: &str,
    coverage_incomplete: bool,
) -> Result<Value, String> {
    let notify = loaded
        .profile
        .notification
        .as_ref()
        .ok_or("private_update has no notification binding")?;
    let text = private_notification_text(
        packet,
        notify.max_text_bytes as usize,
        coverage_incomplete,
        &Messages::for_skill(&loaded.profile.skill_path(&notify.skill), &notify.provider)?,
    )?;
    admit_private_text(loaded, workspace, &text)?;
    let selected_digests = packet["items"]
        .as_array()
        .ok_or("ready packet lacks selected items")?
        .iter()
        .map(|item| {
            item["source_digest"]
                .as_str()
                .map(str::to_owned)
                .ok_or("selected item lacks digest")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let work_updates = packet["items"]
        .as_array()
        .ok_or("ready packet lacks selected items")?
        .iter()
        .filter(|item| item["source_kind"] == "work_result")
        .map(|item| {
            let id = item["work_id"]
                .as_str()
                .ok_or("work update lacks work id")?;
            let assignment = record
                .state
                .work
                .iter()
                .find(|work| work.id == id && needs_work_update(work))
                .ok_or("work update is not a pending completed assignment")?;
            let update =
                pending_work_update(assignment).ok_or("work update has no pending result")?;
            if item["source_digest"].as_str()
                != Some(work_result_digest(assignment, update).as_str())
                || item["receipt"].as_str() != Some(update.receipt.as_str())
            {
                return Err("work update differs from its sealed receipt".to_owned());
            }
            Ok((id.to_owned(), work_result_digest(assignment, update)))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    if work_updates.len() > 3 {
        return Err("work update selection is invalid".to_owned());
    }
    let logical = record
        .state
        .active_run_id
        .clone()
        .ok_or("assistant run has no identity")?;
    stage_notification(
        loaded,
        workspace,
        record,
        &text,
        material_digest,
        selected_digests,
        work_updates,
        &logical,
        None,
    )
}

#[expect(clippy::too_many_arguments, reason = "exact private delivery custody")]
fn stage_notification(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    text: &str,
    material_digest: &str,
    selected_digests: Vec<String>,
    work_updates: BTreeMap<String, String>,
    logical: &str,
    decision: Option<(&str, &str)>,
) -> Result<Value, String> {
    if record.state.pending_intent.is_some() {
        return Err("another exact private notification is already pending".to_owned());
    }
    let notify = loaded
        .profile
        .notification
        .as_ref()
        .ok_or("private delivery has no notification binding")?;
    let authority = super::standing_approvals(workspace)?
        .status(&notify.authority_id)
        .map_err(|error| error.to_string())?
        .ok_or("private notification authority is not installed")?;
    if authority.approval != super::notification_approval(notify, &loaded.source_set_digest)?
        || authority.revoked_at_unix_seconds.is_some()
        || authority.approval.expires_at_unix_seconds <= now_seconds()
    {
        return Err(
            "private notification authority differs from current sources; regrant before staging"
                .to_owned(),
        );
    }
    if text.is_empty() || text.len() > notify.max_text_bytes as usize {
        return Err("exact private delivery exceeds its configured text limit".to_owned());
    }
    admit_private_text(loaded, workspace, text)?;
    let delivery_package_digest = loaded
        .skill_bindings
        .get(&notify.skill)
        .ok_or("assistant has no inspected notification skill binding")?;
    pinned_skill_path(
        workspace,
        &loaded.profile.skill_path(&notify.skill),
        &notify.skill,
        delivery_package_digest,
    )?;
    let (planned, _) = run_skill_with_resolution(
        loaded,
        workspace,
        &notify.skill,
        "plan_exact",
        json!({
            "channel":notify.destination_locator,
            "content":{"message":text},
            "principal":notify.principal_ref
        }),
        false,
        false,
        None,
        Some(delivery_package_digest),
        None,
        None,
        None,
    )
    .map_err(String::from)?;
    let plan = result_data(&planned, "notify_plan")?;
    if plan["decision"] != "ready_for_provider" || plan["provider"] != notify.provider {
        return Err("notification skill did not admit the exact private brief".to_owned());
    }
    let idempotency_key = notification_uuid(logical)?;
    let exact = json!({
        "notification_plan":plan,
        "channel_locator":notify.destination_locator,
        "text":text,
        "idempotency_key":idempotency_key
    });
    let exact_json: runx_contracts::JsonValue = serde_json::from_value(exact)
        .map_err(|error| format!("encoding exact notification intent: {error}"))?;
    let artifact = runx_runtime::persist_json_artifact(
        &workspace.paths().artifacts(),
        logical,
        "assistant",
        "notification-intent",
        &exact_json,
    )?;
    record.state.pending_intent = Some(PendingIntent {
        artifact_ref: serde_json::to_value(artifact)
            .map_err(|error| format!("encoding notification artifact reference: {error}"))?,
        material_digest: material_digest.to_owned(),
        logical_run_id: logical.to_owned(),
        selected_digests,
        delivery_package_digest: delivery_package_digest.clone(),
        delivery_skill: notify.skill.clone(),
        delivery_provider: notify.provider.clone(),
        delivery_instance: notify.instance_id.clone(),
        work_updates,
        decision_work_id: decision.map(|binding| binding.0.to_owned()),
        decision_request_digest: decision.map(|binding| binding.1.to_owned()),
    });
    if decision.is_none() {
        record.state.pending_turn = None;
    }
    record.state.last_blocker = None;
    record.state.next_due_unix_seconds = scheduled_due(loaded.profile.min_check_minutes);
    record.state.next_delivery_due_unix_seconds = 0;
    write_control(loaded, workspace, record, "notification_intent")?;
    Ok(json!({"status":"notification_pending","material_digest":material_digest}))
}

fn deliver_pending(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
) -> Result<Value, String> {
    if quiet_at(loaded.profile.quiet_hours.as_ref(), now_seconds()) {
        record.state.next_delivery_due_unix_seconds = next_allowed_at(loaded, now_seconds());
        write_control(loaded, workspace, record, "quiet_delivery")?;
        return Ok(json!({
            "status":"quiet_delivery",
            "next_delivery_due_unix_seconds":record.state.next_delivery_due_unix_seconds
        }));
    }
    let pending = record
        .state
        .pending_intent
        .clone()
        .ok_or("no pending intent")?;
    let exact = read_notification_intent(workspace, &pending)?;
    let expected_digest = exact["notification_plan"]["content_digest"]
        .as_str()
        .ok_or("notification intent lacks content digest")?
        .to_owned();
    let expected_channel = exact["channel_locator"]
        .as_str()
        .ok_or("notification intent lacks channel locator")?
        .to_owned();
    let exact_text = exact["text"]
        .as_str()
        .ok_or("notification intent lacks exact text")?;
    if loaded
        .profile
        .notification
        .as_ref()
        .and_then(|profile| profile.instance_id.as_ref())
        != pending.delivery_instance.as_ref()
    {
        return Err(
            "private delivery adapter instance differs from the admitted intent".to_owned(),
        );
    }
    admit_private_text(loaded, workspace, exact_text)?;
    let delivery_run_id = format!(
        "run_assistant_deliver_{}",
        sha256_prefixed(pending.logical_run_id.as_bytes()).trim_start_matches("sha256:")
    );
    let (delivered, receipt) = run_skill_with_resolution(
        loaded,
        workspace,
        &pending.delivery_skill,
        "deliver",
        exact,
        false,
        true,
        Some(&delivery_run_id),
        (!pending.delivery_package_digest.is_empty())
            .then_some(pending.delivery_package_digest.as_str()),
        None,
        None,
        None,
    )?;
    let readback = result_data(&delivered, "provider_operation")
        .or_else(|_| result_data(&delivered, "notify_delivery"))?;
    if !verified_notification_readback(
        readback,
        &expected_digest,
        &expected_channel,
        &pending.delivery_provider,
    ) {
        return Err("notification delivery lacks verified provider readback".to_owned());
    }
    acknowledge_notification(loaded, workspace, record, &pending, readback, &receipt)
}

fn read_notification_intent(
    workspace: &WorkspaceEnv,
    pending: &PendingIntent,
) -> Result<Value, String> {
    let reference = serde_json::from_value(pending.artifact_ref.clone())
        .map_err(|error| format!("decoding notification artifact reference: {error}"))?;
    serde_json::to_value(runx_runtime::read_json_artifact(
        &workspace.paths().artifacts(),
        &reference,
    )?)
    .map_err(|error| format!("decoding notification intent: {error}"))
}

pub(super) fn reconcile_notification(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    message_locator: &str,
) -> Result<Value, String> {
    let _worker_lock = worker_lock(loaded, workspace)?;
    let mut record = read_control(loaded, workspace)?;
    if record.state.profile_revision != loaded.revision {
        return Err("assistant profile changed; inspect and resume explicitly".to_owned());
    }
    let pending = record
        .state
        .pending_intent
        .clone()
        .ok_or("assistant has no pending notification")?;
    let notify = loaded
        .profile
        .notification
        .as_ref()
        .ok_or("notification binding is missing")?;
    if notify.skill != pending.delivery_skill
        || notify.provider != pending.delivery_provider
        || notify.instance_id != pending.delivery_instance
    {
        return Err("notification binding differs from its pending intent".to_owned());
    }
    let exact = read_notification_intent(workspace, &pending)?;
    let channel = exact["channel_locator"]
        .as_str()
        .ok_or("pending notification lacks channel")?;
    if channel != notify.destination_locator || !message_locator.starts_with(&format!("{channel}/"))
    {
        return Err(
            "reconciliation target differs from the admitted private destination".to_owned(),
        );
    }
    let delivery_run_id = format!(
        "run_assistant_deliver_{}",
        sha256_prefixed(pending.logical_run_id.as_bytes()).trim_start_matches("sha256:")
    );
    let sealed =
        runx_runtime::run_record::sealed_run(workspace.env(), workspace.cwd(), &delivery_run_id)
            .map_err(|error| error.to_string())?
            .ok_or("native delivery run has no sealed outcome")?;
    let retained = pinned_skill_path_by_digest(
        workspace,
        &pending.delivery_skill,
        &pending.delivery_package_digest,
    )?;
    let package = serde_json::to_value(
        runx_runtime::inspect_skill_package(&retained, Some("deliver"), Some(workspace.env()))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("encoding retained notification skill: {error}"))?;
    if sealed.runner != "deliver"
        || package["package_digest"] != sealed.package_digest
        || serde_json::to_value(&sealed.inputs)
            .map_err(|error| format!("encoding sealed notification inputs: {error}"))?
            != exact
        || sealed.receipt.seal.disposition != ClosureDisposition::Failed
    {
        return Err("native delivery outcome cannot be reconciled as a failed post".to_owned());
    }
    let expected_digest = exact["notification_plan"]["content_digest"]
        .as_str()
        .ok_or("pending notification lacks content digest")?;
    let text = exact["text"]
        .as_str()
        .ok_or("pending notification lacks exact text")?;
    admit_private_text(loaded, workspace, text)?;
    let (reconciled, receipt) = run_skill_with_resolution(
        loaded, workspace, &pending.delivery_skill, "reconcile_exact",
        json!({"notification_plan":exact["notification_plan"],"channel_locator":channel,
            "text":text,"message_locator":message_locator,"failed_at":sealed.receipt.created_at.to_string()}), false, true, None, None, None, None, None,
    ).map_err(String::from)?;
    let readback = result_data(&reconciled, "provider_operation")?;
    let messages = Messages::for_skill(
        &loaded.profile.skill_path(&pending.delivery_skill),
        &pending.delivery_provider,
    )?;
    if readback["operation"] != "channel.post.reconcile"
        || readback["result"]["message_locator"] != message_locator
        || readback["result"]["normalization"].as_str() != messages.readback_normalization()
        || !verified_notification_readback(
            readback,
            expected_digest,
            channel,
            &pending.delivery_provider,
        )
    {
        return Err("reconciliation lacks exact provider readback".to_owned());
    }
    acknowledge_notification(loaded, workspace, &mut record, &pending, readback, &receipt)
}

fn acknowledge_notification(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    pending: &PendingIntent,
    readback: &Value,
    receipt: &str,
) -> Result<Value, String> {
    let _control_lock = lock(loaded, workspace)?;
    let current = read_control(loaded, workspace)?;
    if current.state.pending_intent.as_ref() != Some(pending) {
        return Err(
            "notification head changed while the native delivery ran; receipt is retained"
                .to_owned(),
        );
    }
    *record = current;
    if let Some(work_id) = &pending.decision_work_id {
        let expected_request = pending
            .decision_request_digest
            .as_deref()
            .ok_or("decision notification lacks its native request digest")?;
        let presentation_ref = readback["result"]["message_locator"]
            .as_str()
            .ok_or("decision notification lacks a stable message locator")?;
        let assignment = record
            .state
            .work
            .iter_mut()
            .find(|work| &work.id == work_id && work_is_open(work))
            .ok_or("decision notification lost its open obligation")?;
        let decision = assignment
            .decision
            .as_mut()
            .filter(|decision| {
                decision.request_digest == expected_request && decision.presentation_ref.is_none()
            })
            .ok_or("decision notification differs from its native request")?;
        decision.presentation_ref = Some(presentation_ref.to_owned());
        assignment.turn.due_at_unix_seconds = 0;
        record.state.pending_intent = None;
        record.state.next_delivery_due_unix_seconds = 0;
        record.state.last_blocker = None;
        write_control(loaded, workspace, record, "decision_presented")?;
        return Ok(json!({"status":"decision_presented","work_id":work_id,"receipt":receipt}));
    }
    record.state.last_handled_receipt = Some(receipt.to_owned());
    record.state.last_blocker = None;
    record.state.scan_retry_from_first_page = false;
    let work_digests = pending.work_updates.values().collect::<BTreeSet<_>>();
    for digest in &pending.selected_digests {
        if !work_digests.contains(digest) {
            mark_handled(&mut record.state, digest);
        }
    }
    for (id, digest) in &pending.work_updates {
        if let Some(assignment) = record.state.work.iter_mut().find(|work| work.id == *id) {
            let work_id = assignment.id.clone();
            if let Some(update) = assignment.private_updates.iter_mut().find(|update| {
                update.status.is_none()
                    && sha256_prefixed(
                        format!(
                            "assistant-work-result:{work_id}:{}:{}",
                            update.turn_id, update.receipt
                        )
                        .as_bytes(),
                    ) == *digest
            }) {
                update.status = Some("delivered".to_owned());
            }
        }
        // A later turn may already have a new result. Delivering an older exact
        // report cannot acknowledge that newer result or erase its next report.
    }
    record.state.pending_intent = None;
    record.state.next_delivery_due_unix_seconds = 0;
    record.state.active_run_id = None;
    if !record.state.deferred_turns.is_empty()
        || !pending_work_update_indices(&record.state.work).is_empty()
    {
        record.state.next_due_unix_seconds = 0;
    }
    write_control(loaded, workspace, record, "delivered")?;
    Ok(json!({"status":"delivered","receipt":receipt}))
}

fn verified_notification_readback(
    readback: &Value,
    digest: &str,
    channel: &str,
    provider: &str,
) -> bool {
    readback["status"] == "success"
        && readback["provider"] == provider
        && (readback["operation"] == "channel.post.read"
            || readback["operation"] == "channel.post.reconcile")
        && readback["finality"] == "confirmed"
        && readback["result"]["content_digest"] == digest
        && readback["result"]["conversation_locator"] == channel
        && readback["result"]["message_locator"]
            .as_str()
            .is_some_and(|locator| !locator.is_empty())
        && readback["readback_ref"]
            .as_str()
            .is_some_and(|reference| reference.starts_with("runx:provider-readback:sha256:"))
}

fn mark_handled(state: &mut Control, digest: &str) {
    if !state
        .pending_handled_signals
        .iter()
        .any(|item| item == digest)
    {
        state.pending_handled_signals.push(digest.to_owned());
    }
}

fn collect_observations(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
) -> Result<(Vec<Value>, bool), String> {
    let mut observations = Vec::new();
    let mut coverage_incomplete = false;
    for source in &loaded.profile.sources {
        let pinned = record
            .state
            .pending_turn
            .as_ref()
            .ok_or("assistant turn lacks a pending page set")?
            .pages
            .iter()
            .find(|page| page.source_id == source.id())
            .cloned();
        let page = if let Some(pinned) = pinned {
            read_pinned_page(workspace, source, &pinned)?
        } else {
            let page = fetch_page(loaded, workspace, source, record)?;
            let logical = record
                .state
                .active_run_id
                .as_deref()
                .ok_or("assistant run has no identity")?;
            let artifact_value: runx_contracts::JsonValue = serde_json::from_value(
                serde_json::to_value(&page)
                    .map_err(|error| format!("encoding assistant source page: {error}"))?,
            )
            .map_err(|error| format!("binding assistant source page: {error}"))?;
            let artifact_ref = runx_runtime::persist_json_artifact(
                &workspace.paths().artifacts(),
                logical,
                "assistant-pages",
                source.id(),
                &artifact_value,
            )?;
            record
                .state
                .pending_turn
                .as_mut()
                .ok_or("assistant turn lacks a pending page set")?
                .pages
                .push(PinnedPage {
                    source_id: source.id().to_owned(),
                    artifact_ref: serde_json::to_value(artifact_ref)
                        .map_err(|error| format!("encoding source page reference: {error}"))?,
                });
            write_control(loaded, workspace, record, "source_page_pinned")?;
            page
        };
        for message in &page.messages {
            if message["author"]["external_id"] != message["connected_subject_ref"] {
                record_observation(loaded, workspace, source, message)?;
            }
        }
        coverage_incomplete |= page.next_cursor.is_some();
        if observations.len() + page.observations.len() > MAX_OBSERVATIONS {
            return Err("pinned source pages exceed the assistant observation budget".to_owned());
        }
        observations.extend(page.observations);
    }
    Ok((observations, coverage_incomplete))
}

fn read_pinned_page(
    workspace: &WorkspaceEnv,
    source: &SourceProfile,
    pinned: &PinnedPage,
) -> Result<PagePacket, String> {
    let reference = serde_json::from_value(pinned.artifact_ref.clone())
        .map_err(|error| format!("decoding source page reference: {error}"))?;
    let value = runx_runtime::read_json_artifact(&workspace.paths().artifacts(), &reference)?;
    let page: PagePacket = serde_json::from_value(
        serde_json::to_value(value).map_err(|error| format!("decoding source page: {error}"))?,
    )
    .map_err(|error| format!("source page artifact is invalid: {error}"))?;
    let query_digest = source_scan_digest(source, page.scan_lane)?;
    if page.source_id != source.id()
        || page.query_digest != query_digest
        || (page.scan_lane == ScanLane::Fresh && page.page_index != 1)
        || page.scan_id.is_empty()
        || !(1..=10_000).contains(&page.page_index)
        || page.messages.len() > MAX_OBSERVATIONS
        || page.observations.len() > page.messages.len()
        || page
            .next_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.len() > 500)
    {
        return Err("pinned source page differs from the configured source or bounds".to_owned());
    }
    Ok(page)
}

fn source_scan_digest(source: &SourceProfile, lane: ScanLane) -> Result<String, String> {
    let bytes = match lane {
        ScanLane::Backlog => serde_json::to_vec(source),
        ScanLane::Fresh => serde_json::to_vec(&("fresh", source)),
    }
    .map_err(|error| format!("encoding assistant source scan: {error}"))?;
    Ok(sha256_prefixed(&bytes))
}

fn fresh_scan_due(last_fresh_at: Option<u64>, now: u64, min_check_minutes: u64) -> bool {
    last_fresh_at
        .is_none_or(|last| now.saturating_sub(last) >= min_check_minutes.saturating_mul(120))
}

fn fetch_page(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    source: &SourceProfile,
    record: &ControlRecord,
) -> Result<PagePacket, String> {
    let backlog_continuation = if record.state.scan_retry_from_first_page
        || (record.state.last_blocker.is_some() && record.state.pending_intent.is_none())
    {
        None
    } else {
        scan_continuation(loaded, workspace, source)?
    };
    // The backlog cursor remains in operator-inbox. A page-one refresh uses a
    // second scan head in that same owner, so it cannot overwrite continuation.
    let scan_lane = if backlog_continuation.is_some()
        && fresh_scan_due(
            record.state.last_fresh_scan_at.get(source.id()).copied(),
            now_seconds(),
            loaded.profile.min_check_minutes,
        ) {
        ScanLane::Fresh
    } else {
        ScanLane::Backlog
    };
    let continuation = (scan_lane == ScanLane::Backlog)
        .then_some(backlog_continuation)
        .flatten();
    let (output, _) = run_source_skill(
        loaded,
        workspace,
        source,
        "assistant_scan",
        json!({"config":source.config,"cursor":continuation.as_ref().map(|next| &next.cursor),"page_limit":source.page_limit}),
    )?;
    let page = source_packet(&output, "assistant_page")?;
    let _: runx_contracts::ConversationPage =
        serde_json::from_value(page.clone()).map_err(|error| {
            format!("conversation adapter violates the shared page contract: {error}")
        })?;
    if page["next_cursor"]
        .as_str()
        .is_some_and(|cursor| cursor.len() > 500)
    {
        return Err("conversation adapter cursor exceeds the bound".to_owned());
    }
    let items = page["items"]
        .as_array()
        .ok_or("source skill returned no bounded item page")?;
    if items.len() > source.page_limit as usize || items.len() > MAX_OBSERVATIONS {
        return Err("source page exceeds assistant's bounded observation limit".to_owned());
    }
    let mut page_messages = Vec::new();
    let mut observations = Vec::new();
    for item in items {
        let (observed, _) = run_source_skill(
            loaded,
            workspace,
            source,
            "assistant_observe",
            json!({"config":source.config,"item":item}),
        )?;
        let projection = source_packet(&observed, "assistant_observation")?;
        let _: runx_contracts::ConversationObservation = serde_json::from_value(projection.clone())
            .map_err(|error| {
                format!("conversation adapter violates the shared observation contract: {error}")
            })?;
        let message = projection["message"].clone();
        if message["provider"] != source.provider {
            return Err("source skill returned another provider".to_owned());
        }
        let locator = message["message_locator"]
            .as_str()
            .ok_or("source message lacks locator")?;
        let thread = message["thread_locator"]
            .as_str()
            .ok_or("source message lacks thread")?;
        if !locator.starts_with(&source.locator_prefix)
            || !thread.starts_with(&source.locator_prefix)
        {
            return Err("source skill returned a locator outside its configured prefix".to_owned());
        }
        let (evidence, message) = observation(
            projection["source_kind"]
                .as_str()
                .ok_or("source observation lacks kind")?,
            thread.to_owned(),
            locator.to_owned(),
            message["occurred_at"]
                .as_str()
                .ok_or("source message lacks time")?
                .to_owned(),
            projection["summary"]
                .as_str()
                .ok_or("source observation lacks summary")?
                .to_owned(),
            message,
        )
        .ok_or("source page contains an observation that cannot be safely normalized")?;
        if message["author"]["external_id"] == message["connected_subject_ref"] {
            page_messages.push(message);
            continue;
        }
        page_messages.push(message);
        observations.push(evidence);
    }
    let query_digest = source_scan_digest(source, scan_lane)?;
    let logical = record
        .state
        .active_run_id
        .as_deref()
        .ok_or("assistant run has no identity")?;
    let page_digest =
        sha256_prefixed(&serde_json::to_vec(&page_messages).map_err(|error| error.to_string())?);
    let scan_id = continuation
        .as_ref()
        .map(|next| next.scan_id.clone())
        .unwrap_or_else(|| {
            format!(
                "{logical}-{}-{}-{}",
                source.id(),
                if scan_lane == ScanLane::Fresh {
                    "fresh"
                } else {
                    "backlog"
                },
                &page_digest.trim_start_matches("sha256:")[..12],
            )
        });
    Ok(PagePacket {
        source_id: source.id().to_owned(),
        query_digest,
        scan_id,
        page_index: continuation.as_ref().map_or(1, |next| next.page_index),
        scan_lane,
        fetched_at_unix_seconds: now_seconds(),
        next_cursor: page["next_cursor"].as_str().map(str::to_owned),
        messages: page_messages,
        observations,
    })
}

struct ScanContinuation {
    scan_id: String,
    page_index: u64,
    cursor: String,
}

fn scan_continuation(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    source: &SourceProfile,
) -> Result<Option<ScanContinuation>, String> {
    let query_digest = source_scan_digest(source, ScanLane::Backlog)?;
    let (read, _) = run_skill(
        loaded,
        workspace,
        "operator-inbox",
        "read_scan",
        json!({"data_source_ref":loaded.profile.inbox_data_source_ref,"query_digest":query_digest}),
        false,
        false,
    )?;
    let rows = result_data(&read, "data_operation_result")?["rows"]
        .as_array()
        .ok_or("operator-inbox scan read lacks rows")?;
    let Some(scan) = rows.first().map(|row| &row["event"]["payload"]["scan"]) else {
        return Ok(None);
    };
    Ok(scan_continuation_from_value(scan))
}

fn scan_continuation_from_value(scan: &Value) -> Option<ScanContinuation> {
    if scan["status"] != "truncated" {
        return None;
    }
    let cursor = scan["next_cursor"]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 500)?;
    let page_index = scan["page_index"]
        .as_u64()
        .filter(|index| *index < 10_000)?;
    let scan_id = scan["scan_id"]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 200)?;
    Some(ScanContinuation {
        scan_id: scan_id.to_owned(),
        page_index: page_index + 1,
        cursor: cursor.to_owned(),
    })
}

fn observation(
    kind: &str,
    thread: String,
    locator: String,
    occurred: String,
    summary: String,
    message: Value,
) -> Option<(Value, Value)> {
    if summary.trim().is_empty() || summary.len() > 1000 || !super::valid_identifier(kind) {
        return None;
    }
    let digest =
        sha256_prefixed(&serde_json::to_vec(&json!([&locator, &occurred, &summary])).ok()?);
    Some((
        json!({
            "source_ref":locator,
            "source_digest":digest,
            "source_kind":kind,
            "observed_at":occurred,
            "summary":summary,
            "thread_locator":thread
        }),
        message,
    ))
}

fn record_observation(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    source: &SourceProfile,
    message: &Value,
) -> Result<(), String> {
    let message = message_for_queue(message)?;
    let thread = message["thread_locator"]
        .as_str()
        .ok_or("source message lacks thread")?;
    let locator = message["message_locator"]
        .as_str()
        .ok_or("source message lacks locator")?;
    let action_id = format!(
        "action-{}",
        sha256_prefixed(thread.as_bytes()).trim_start_matches("sha256:")
    );
    let (read, _) = run_skill(
        loaded,
        workspace,
        "operator-inbox",
        "read_action",
        json!({"data_source_ref":loaded.profile.inbox_data_source_ref,"action_id":action_id}),
        false,
        false,
    )?;
    let rows = result_data(&read, "data_operation_result")?["rows"]
        .as_array()
        .ok_or("operator-inbox read lacks rows")?;
    let current = rows.first().map(|row| &row["event"]["payload"]["action"]);
    if current.is_some_and(|action| {
        action["message_locators"]
            .as_array()
            .is_some_and(|locators| locators.iter().any(|value| value == locator))
    }) {
        return Ok(());
    }
    let version = rows
        .first()
        .map_or(0, |row| row["version"].as_u64().unwrap_or(0));
    run_skill(
        loaded,
        workspace,
        "operator-inbox",
        "record_action_observation",
        json!({
            "data_source_ref":loaded.profile.inbox_data_source_ref,
            "expected_version":version,
            "current_action":current,
            "observed_at":now_iso8601(),
            "message":message,
            "triage":{"kind":source.triage_kind,"reason":"Observed in a configured personal source."}
        }),
        false,
        false,
    )?;
    Ok(())
}

fn record_scan(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    source: &SourceProfile,
    page: &PagePacket,
) -> Result<(), String> {
    let query_digest = source_scan_digest(source, page.scan_lane)?;
    if page.source_id != source.id() || page.query_digest != query_digest {
        return Err("committed scan page differs from its pinned source".to_owned());
    }
    let (read, _) = run_skill(
        loaded,
        workspace,
        "operator-inbox",
        "read_scan",
        json!({"data_source_ref":loaded.profile.inbox_data_source_ref,"query_digest":query_digest}),
        false,
        false,
    )?;
    let rows = result_data(&read, "data_operation_result")?["rows"]
        .as_array()
        .ok_or("operator-inbox scan read lacks rows")?;
    let version = rows
        .first()
        .map_or(0, |row| row["version"].as_u64().unwrap_or(0));
    if rows.first().is_some_and(|row| {
        row["event"]["payload"]["scan"]["scan_id"].as_str() == Some(page.scan_id.as_str())
            && row["event"]["payload"]["scan"]["page_index"].as_u64() == Some(page.page_index)
    }) {
        return Ok(());
    }
    let messages = page
        .messages
        .iter()
        .map(message_for_queue)
        .collect::<Result<Vec<_>, _>>()?;
    run_skill(
        loaded,
        workspace,
        "operator-inbox",
        "record_scan_page",
        json!({
            "data_source_ref":loaded.profile.inbox_data_source_ref,
            "expected_version":version,
            "observed_at":now_iso8601(),
            "scan":{
                "scan_id":page.scan_id,
                "provider":source.provider,
                "query_digest":query_digest,
                "page_index":page.page_index,
                "status":if page.next_cursor.is_some() {"truncated"} else {"complete"},
                "next_cursor":page.next_cursor
            },
            "messages":messages
        }),
        false,
        false,
    )?;
    Ok(())
}

fn message_for_queue(message: &Value) -> Result<Value, String> {
    let occurred = message["occurred_at"]
        .as_str()
        .ok_or("source message lacks an occurrence time")?;
    let occurred = DateTime::parse_from_rfc3339(occurred)
        .map_err(|_| "source message has an invalid occurrence time")?
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut normalized = message.clone();
    normalized["occurred_at"] = json!(occurred);
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{
        DeferredTurn, PagePacket, PendingTurn, ScanLane, SkillRunFailure, WorkAssignment,
        WorkerLane, apply_assignment_failure, due_work_index, due_worker_lane,
        evictable_work_index, fresh_scan_due, next_work_due, notification_uuid, package_digest,
        persist_work_result, pinned_skill_path, private_notification_text, private_text_is_safe,
        quiet_hour, read_work_result, scan_continuation_from_value, source_scan_digest,
        unreviewed_digests, verified_notification_readback,
    };
    use crate::{GoalSchedule, QuietHours, SourceProfile};

    fn official_slack() -> Result<super::Messages, String> {
        let slack = runx_runtime::ConnectorScope::official_only()
            .and_then(|scope| scope.connector("slack"))
            .map_err(|error| error.to_string())?;
        super::Messages::of(&slack)
    }

    #[test]
    fn scheduled_goal_occurrences_have_stable_identity_and_coalesce_sleep() -> Result<(), String> {
        let schedule = GoalSchedule {
            starts_at_unix_seconds: 1_000,
            interval_seconds: 300,
            route_id: "daily_review".to_owned(),
            target_ref: "runx://project/review".to_owned(),
        };
        assert_eq!(super::due_goal_occurrence(&schedule, None, 999)?, None);
        assert_eq!(
            super::due_goal_occurrence(&schedule, None, 1_000)?,
            Some(1_000)
        );
        let head = super::GoalOccurrenceHead {
            schedule_digest: super::schedule_digest(&schedule)?,
            due_at_unix_seconds: 1_000,
        };
        assert_eq!(
            super::due_goal_occurrence(&schedule, Some(&head), 1_299)?,
            None
        );
        assert_eq!(
            super::due_goal_occurrence(&schedule, Some(&head), 1_300)?,
            Some(1_300)
        );
        assert_eq!(
            super::due_goal_occurrence(&schedule, Some(&head), 2_501)?,
            Some(2_500)
        );
        let revised = GoalSchedule {
            interval_seconds: 600,
            ..schedule.clone()
        };
        assert_eq!(
            super::due_goal_occurrence(&revised, Some(&head), 1_601)?,
            Some(1_600)
        );
        assert_ne!(
            super::goal_occurrence_prefix("review", &super::schedule_digest(&schedule)?),
            super::goal_occurrence_prefix("review", &super::schedule_digest(&revised)?)
        );
        Ok(())
    }

    #[test]
    fn scheduled_goal_admission_persists_work_and_head_together() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!(
            "runx-goal-occurrence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let workspace = runx_runtime::WorkspaceEnv::load_process(root.clone())
            .map_err(|error| error.to_string())?;
        let skills_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../skills")
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let profile: crate::AssistantProfile = serde_json::from_value(json!({
            "schema":"runx.assistant.profile.v1", "instance_id":"scheduled-test",
            "skills_root":skills_root,
            "inbox_data_source_ref":"local://runx-data-store/scheduled-test-inbox",
            "authority":{"actor_ref":{"type":"principal","uri":"runx:principal:assistant:scheduled-test"},"delegations":[]},
            "sources":[], "allowed_action_ids":[],
            "goals":[{"goal_id":"review", "purpose":"Review current work",
                "completion_condition":"A verified review result exists", "enabled":true,
                "schedule":{"starts_at_unix_seconds":1000,"interval_seconds":300,
                    "route_id":"review","target_ref":"runx://project/review"}}],
            "work_routes":[{"route_id":"review","goal_id":"review","skill":"data-store",
                "runner":"read_events","description":"Read bounded current work",
                "source_kinds":["scheduled_goal"],"model":false,"inputs":{},
                "source_context_input":null,"local_provider":null}],
            "charter":"", "model":{"model":"unused","endpoint_url":"http://127.0.0.1:1/v1/chat/completions",
                "context_sharing":"local_only"}, "notification":null
        })).map_err(|error| error.to_string())?;
        let digest = runx_runtime::inspect_local_skill_binding(
            &skills_root.join("data-store"),
            Some(workspace.env()),
        )
        .map_err(|error| error.to_string())?;
        let loaded = crate::LoadedProfile {
            path: root.join("profile.json"),
            profile,
            revision: "test-revision".to_owned(),
            source_set_digest: "test-sources".to_owned(),
            skill_set_digest: "test-skills".to_owned(),
            skill_bindings: std::collections::BTreeMap::from([("data-store".to_owned(), digest)]),
        };
        let mut record = super::ControlRecord {
            version: 0,
            state: super::control_default(&loaded),
            baseline: super::control_default(&loaded),
        };
        super::admit_scheduled_work(&loaded, &workspace, &mut record)?;
        let first = super::read_control(&loaded, &workspace)?;
        assert_eq!(first.version, 1);
        assert_eq!(first.state.work.len(), 1);
        assert_eq!(first.state.goal_occurrence_heads.len(), 1);
        let (route_context, _) = super::operator_context(&loaded, &workspace, &first, None)?;
        assert_eq!(route_context.len(), 1);
        let mut budget_loaded = loaded.clone();
        budget_loaded.profile.context_budget_bytes = 8 * 1024;
        let mut many_memories = first.clone();
        for index in 0..20 {
            many_memories
                .state
                .confirmed_memory
                .push(super::ConfirmedMemory {
                    memory_id: format!("memory-{index}"),
                    text: "m".repeat(300),
                    confirmed_at: super::now_iso8601(),
                    confirmed_by: "operator".to_owned(),
                    revision: "reviewed".to_owned(),
                    supersedes: None,
                    goal_ids: Vec::new(),
                    source_refs: Vec::new(),
                });
        }
        let (bounded, history) =
            super::operator_context(&budget_loaded, &workspace, &many_memories, None)?;
        let used = bounded
            .iter()
            .chain(&history)
            .map(|entry| serde_json::to_vec(entry).map(|bytes| bytes.len()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
            .into_iter()
            .sum::<usize>();
        assert!(bounded.len() > 1 && bounded.len() < 21);
        assert!(used <= budget_loaded.profile.context_budget_bytes);
        assert!(first.state.work[0].source_binding.is_none());
        assert_eq!(
            first.state.work[0].source_ref,
            format!(
                "runx://assistant/goal/review/schedule/{}/occurrence/{}",
                first.state.goal_occurrence_heads["review"].schedule_digest,
                first.state.goal_occurrence_heads["review"].due_at_unix_seconds
            )
        );
        super::admit_scheduled_work(&loaded, &workspace, &mut record)?;
        assert_eq!(super::read_control(&loaded, &workspace)?.version, 1);

        let mut stale = super::read_control(&loaded, &workspace)?;
        let mut winner = super::read_control(&loaded, &workspace)?;
        winner.state.paused = false;
        super::write_control(&loaded, &workspace, &mut winner, "unpaused")?;
        let mut orphan = stale.state.work[0].clone();
        orphan.id = "conflict-child".to_owned();
        stale.state.work.push(orphan);
        stale.state.goal_occurrence_heads.insert(
            "conflict".to_owned(),
            super::GoalOccurrenceHead {
                schedule_digest: super::schedule_digest(
                    &loaded.profile.goals[0]
                        .schedule
                        .clone()
                        .ok_or("missing schedule")?,
                )?,
                due_at_unix_seconds: 1000,
            },
        );
        assert!(super::write_control(&loaded, &workspace, &mut stale, "stale").is_err());
        let after = super::read_control(&loaded, &workspace)?;
        assert_eq!(after.version, 2);
        assert_eq!(after.state.work.len(), 1);
        assert!(!after.state.goal_occurrence_heads.contains_key("conflict"));
        assert_eq!(
            super::assistant_records(&workspace)?
                .version::<super::WorkEvent>(&super::assistant_record_key(
                    "scheduled-test",
                    "conflict-child"
                ))
                .map_err(|error| error.to_string())?,
            0
        );
        let mut signal_loaded = loaded.clone();
        let source = SourceProfile {
            source_id: "mentions".to_owned(),
            skill: "slack".to_owned(),
            provider: "slack".to_owned(),
            triage_kind: "direct_mention".to_owned(),
            locator_prefix: "slack://".to_owned(),
            config: json!({"query":{"mentions_connected_subject":true,"limit":1}}),
            page_limit: 1,
            local_provider: None,
        };
        signal_loaded.profile.sources.push(source.clone());
        let digest = runx_contracts::sha256_prefixed(b"one observed message");
        let page = PagePacket {
            source_id: source.source_id.clone(),
            query_digest: source_scan_digest(&source, ScanLane::Backlog)?,
            scan_id: "scan-1".to_owned(),
            page_index: 1,
            scan_lane: ScanLane::Backlog,
            fetched_at_unix_seconds: 1,
            next_cursor: None,
            messages: vec![json!({})],
            observations: vec![json!({"source_ref":"slack://message/one","source_digest":digest})],
        };
        let artifact = runx_runtime::persist_json_artifact(
            &workspace.paths().artifacts(),
            "signal-test",
            "assistant-pages",
            "mentions",
            &serde_json::from_value(serde_json::to_value(page).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?,
        )?;
        let mut admitted = super::read_control(&signal_loaded, &workspace)?;
        admitted.state.pending_turn = Some(PendingTurn {
            pages: vec![super::PinnedPage {
                source_id: source.source_id.clone(),
                artifact_ref: serde_json::to_value(artifact).map_err(|error| error.to_string())?,
            }],
            review_ref: None,
            deferred: false,
        });
        super::write_control(
            &signal_loaded,
            &workspace,
            &mut admitted,
            "source_page_pinned",
        )?;
        let signal = super::assistant_records(&workspace)?
            .latest::<super::SignalEvent>(&super::signal_key(
                &signal_loaded,
                &admitted.state,
                &digest,
            )?)
            .map_err(|error| error.to_string())?;
        assert!(
            matches!(signal.map(|record| record.event), Some(super::SignalEvent::Seen { source_digest, .. }) if source_digest == digest)
        );
        let mut disposition = super::read_control(&signal_loaded, &workspace)?;
        super::mark_handled(&mut disposition.state, &digest);
        super::write_control(&signal_loaded, &workspace, &mut disposition, "handled")?;
        let handled = super::read_control(&signal_loaded, &workspace)?.state;
        assert!(super::signal_is_handled(
            &super::assistant_records(&workspace)?,
            &signal_loaded,
            &handled,
            &digest,
        )?);
        let mut revised_profile = signal_loaded.clone();
        revised_profile.revision = "revised-profile".to_owned();
        assert!(!super::signal_is_handled(
            &super::assistant_records(&workspace)?,
            &revised_profile,
            &handled,
            &digest,
        )?);
        let mut remembered = handled.clone();
        remembered.confirmed_memory.push(super::ConfirmedMemory {
            memory_id: "tone".to_owned(),
            text: "Keep replies short.".to_owned(),
            confirmed_at: "2026-10-07T00:00:00Z".to_owned(),
            confirmed_by: "local_operator".to_owned(),
            revision: "sha256:memory".to_owned(),
            supersedes: None,
            goal_ids: Vec::new(),
            source_refs: Vec::new(),
        });
        assert!(
            !super::signal_is_handled(
                &super::assistant_records(&workspace)?,
                &signal_loaded,
                &remembered,
                &digest,
            )?,
            "a memory change revisits current observations"
        );
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn fresh_page_keeps_the_backlog_scan_head_and_old_page_binding() -> Result<(), String> {
        let source = SourceProfile {
            source_id: "mentions".to_owned(),
            skill: "slack".to_owned(),
            provider: "slack".to_owned(),
            triage_kind: "direct_mention".to_owned(),
            locator_prefix: "slack://".to_owned(),
            config: json!({"query":{"mentions_connected_subject":true,"limit":3}}),
            page_limit: 3,
            local_provider: None,
        };
        let backlog = source_scan_digest(&source, ScanLane::Backlog)?;
        let fresh = source_scan_digest(&source, ScanLane::Fresh)?;
        assert_eq!(
            backlog,
            runx_contracts::sha256_prefixed(
                &serde_json::to_vec(&source).map_err(|error| error.to_string())?
            )
        );
        assert_ne!(backlog, fresh);
        assert!(!fresh_scan_due(Some(100), 100 + 29 * 60, 15));
        assert!(fresh_scan_due(Some(100), 100 + 30 * 60, 15));
        assert!(fresh_scan_due(None, 100, 15));
        Ok(())
    }

    fn assignment(id: &str) -> WorkAssignment {
        WorkAssignment {
            id: id.to_owned(),
            goal_id: "correspondence".to_owned(),
            completion_condition: "answer the request".to_owned(),
            source_ref: "slack://message".to_owned(),
            source_digest: "sha256:source".to_owned(),
            thread_locator: "slack://thread".to_owned(),
            source_binding: None,
            turn: super::WorkTurn {
                id: format!("{id}:turn"),
                route: crate::WorkRoute {
                    successor_only: false,
                    route_id: "review".to_owned(),
                    goal_id: "correspondence".to_owned(),
                    skill: "conversation-review".to_owned(),
                    runner: "operate".to_owned(),
                    description: "Review the conversation".to_owned(),
                    source_kinds: vec!["slack".to_owned()],
                    model: true,
                    inputs: Default::default(),
                    source_context_input: Some("source_context".to_owned()),
                    turn_id_input: None,
                    origin_ref_input: None,
                    local_provider: None,
                },
                package_digest: "sha256:old".to_owned(),
                target_ref: "slack://thread".to_owned(),
                inputs: json!({}),
                due_at_unix_seconds: 0,
            },
            continuations: Vec::new(),
            status: "pending".to_owned(),
            followup_blocker: None,
            completed_at: None,
            receipt: None,
            result_ref: None,
            native_run: None,
            failed_runs: Vec::new(),
            decision: None,
            private_updates: Vec::new(),
        }
    }

    #[test]
    fn sealed_failure_preserves_native_custody_and_transient_failure_keeps_identity() {
        let mut work = assignment("work-1");
        work.native_run = Some(super::BoundWorkRun {
            skill: "conversation-review".to_owned(),
            run_id: "run_original".to_owned(),
            package_digest: "sha256:old".to_owned(),
            input_ref: json!({"artifact":"original-inputs"}),
            local_provider: None,
        });
        apply_assignment_failure(
            &mut work,
            &SkillRunFailure {
                message: "temporary transport error".to_owned(),
                terminal_receipt: None,
            },
            15,
        );
        assert_eq!(work.status, "pending");
        assert!(work.turn.due_at_unix_seconds > 0);
        assert_eq!(work.native_run.as_ref().unwrap().run_id, "run_original");
        apply_assignment_failure(
            &mut work,
            &SkillRunFailure {
                message: "model failed".to_owned(),
                terminal_receipt: Some("sha256:failed".to_owned()),
            },
            15,
        );
        assert_eq!(work.status, "held");
        assert_eq!(work.receipt.as_deref(), Some("sha256:failed"));
        assert_eq!(
            work.native_run.as_ref().unwrap().package_digest,
            "sha256:old"
        );
        assert_eq!(due_work_index(&[work.clone()], u64::MAX), None);
        assert_eq!(evictable_work_index(&[work], None), None);
    }

    #[test]
    fn repeated_followups_keep_one_obligation_and_survive_serialization() -> Result<(), String> {
        use runx_contracts::{WorkDisposition, WorkOutcome};
        let mut item = assignment("work-1");
        let root = item.id.clone();
        for ordinal in 0..100 {
            let mut next = item.turn.clone();
            next.id = format!("next-{ordinal}");
            next.due_at_unix_seconds = ordinal;
            let outcome = WorkOutcome {
                disposition: WorkDisposition::FollowUp,
                summary: "Waiting for delivery proof".to_owned(),
                completion_evidence: Vec::new(),
                followups: Vec::new(),
            };
            item = super::transition_assignment(
                &item,
                &outcome,
                vec![next],
                "sha256:turn-receipt",
                json!({"digest":"result"}),
            )?;
            item = serde_json::from_slice(&serde_json::to_vec(&item).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            assert_eq!(item.id, root);
            assert_eq!(item.status, "pending");
            assert!(item.continuations.is_empty());
            assert!(item.native_run.is_none());
            assert_eq!(item.turn.id, format!("next-{ordinal}"));
            item.private_updates.last_mut().unwrap().status = Some("delivered".to_owned());
        }
        let satisfied = WorkOutcome {
            disposition: WorkDisposition::Satisfied,
            summary: "Delivered and independently read back".to_owned(),
            completion_evidence: vec!["provider:delivered".to_owned()],
            followups: Vec::new(),
        };
        item = super::transition_assignment(
            &item,
            &satisfied,
            Vec::new(),
            "sha256:final-receipt",
            json!({"digest":"final"}),
        )?;
        assert_eq!(item.status, "completed");
        assert!(super::needs_work_update(&item));
        Ok(())
    }

    #[test]
    fn pinned_skill_survives_a_patch_without_rebinding_started_code() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!(
            "runx-assistant-skill-pin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos()
        ));
        let source = root.join("source");
        std::fs::create_dir_all(&source).map_err(|error| error.to_string())?;
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: example\ndescription: test\n---\nExample skill.\n",
        )
        .map_err(|error| error.to_string())?;
        let workspace = runx_runtime::WorkspaceEnv::load_process(root.clone())
            .map_err(|error| error.to_string())?;
        let source_digest = package_digest(&source)?;
        let first = pinned_skill_path(&workspace, &source, "example", &source_digest)?;
        let first_digest = package_digest(&first)?;
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: example\ndescription: patched\n---\nPatched skill.\n",
        )
        .map_err(|error| error.to_string())?;
        assert!(pinned_skill_path(&workspace, &source, "example", &source_digest).is_err());
        let patched_digest = package_digest(&source)?;
        let second = pinned_skill_path(&workspace, &source, "example", &patched_digest)?;
        assert_ne!(first, second);
        assert_eq!(package_digest(&first)?, first_digest);
        assert_eq!(package_digest(&second)?, package_digest(&source)?);
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn local_provider_work_refuses_once_its_instance_is_no_longer_selected() -> Result<(), String> {
        use runx_contracts::connector::{InstanceId, Placement};
        use runx_contracts::connector_instance::ConnectorInstanceSchema;
        use runx_contracts::{ConnectorInstance, InstanceAuthority};

        let root = std::env::temp_dir().join(format!(
            "runx-assistant-instance-pin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos(),
        ));
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let workspace = runx_runtime::WorkspaceEnv::load_process(root.clone())
            .map_err(|error| error.to_string())?;
        let slack = runx_runtime::ConnectorScope::official_only()
            .and_then(|scope| scope.connector("slack"))
            .map_err(|error| error.to_string())?;
        let instances = runx_runtime::Instances::new(
            runx_runtime::KernelStore::for_project(workspace.paths())
                .map_err(|error| error.to_string())?,
        );
        let instance = |id: &str| ConnectorInstance {
            schema: ConnectorInstanceSchema::V1,
            instance_id: InstanceId::new(id).unwrap_or_else(|| unreachable!()),
            connector: slack.manifest.id.clone(),
            plugin_root: None,
            placement: Placement::Hosted,
            principal_ref: "runx:principal:operator".into(),
            account_ref: None,
            authority: InstanceAuthority::HostedGrant {
                grant_id: "grant_slack".into(),
            },
            parameters: Default::default(),
            credential_declaration_digest: None,
            operation_digests: slack.operation_digests.clone(),
            confirmed_at: "2026-10-06T00:00:00Z".into(),
        };
        instances
            .confirm(instance("slack-acme"), true)
            .map_err(|error| error.to_string())?;
        let profile = super::LocalProviderProfile {
            instance_id: "slack-acme".to_owned(),
        };
        super::pin_local_provider(&workspace, &profile)?;
        instances
            .confirm(instance("slack-other"), true)
            .map_err(|error| error.to_string())?;
        assert!(super::pin_local_provider(&workspace, &profile).is_err());
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn pinned_composed_skill_keeps_its_sibling_after_a_patch() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!(
            "runx-assistant-closure-pin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos()
        ));
        let source = root.join("skills");
        std::fs::create_dir_all(&source).map_err(|error| error.to_string())?;
        std::fs::write(root.join("pnpm-workspace.yaml"), b"packages: []\n")
            .map_err(|error| error.to_string())?;
        let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let packets = root.join("dist/packets");
        std::fs::create_dir_all(&packets).map_err(|error| error.to_string())?;
        super::copy_skill_files(&repository.join("dist/packets"), &packets)?;
        let official = repository.join("skills");
        for name in ["send-as", "slack", "slack-notify"] {
            let destination = source.join(name);
            std::fs::create_dir(&destination).map_err(|error| error.to_string())?;
            super::copy_skill_files(&official.join(name), &destination)?;
        }
        let workspace = runx_runtime::WorkspaceEnv::load_process(root.clone())
            .map_err(|error| error.to_string())?;
        let skill = source.join("slack-notify");
        let binding = |path: &std::path::Path| -> Result<String, String> {
            runx_runtime::inspect_local_skill_binding(path, None).map_err(|error| error.to_string())
        };
        let first_digest = binding(&skill)?;
        let first = pinned_skill_path(&workspace, &skill, "slack-notify", &first_digest)?;
        assert_eq!(
            first.file_name().and_then(|name| name.to_str()),
            Some("slack-notify")
        );
        assert_eq!(
            super::pinned_skill_path_by_digest(&workspace, "slack-notify", &first_digest)?,
            first
        );
        let dependency = source.join("send-as/send-as.mjs");
        let original = std::fs::read(&dependency).map_err(|error| error.to_string())?;
        let mut patched = original;
        patched.extend_from_slice(b"\n// patched local dependency\n");
        std::fs::write(&dependency, patched).map_err(|error| error.to_string())?;
        let second_digest = binding(&skill)?;
        assert_ne!(first_digest, second_digest);
        let second = pinned_skill_path(&workspace, &skill, "slack-notify", &second_digest)?;
        assert_ne!(first, second);
        assert_eq!(
            super::pinned_skill_path_by_digest(&workspace, "slack-notify", &first_digest)?,
            first
        );
        let pinned_dependency = first
            .parent()
            .ok_or("pinned bundle has no parent")?
            .join("send-as/send-as.mjs");
        std::fs::write(pinned_dependency, b"tampered").map_err(|error| error.to_string())?;
        assert!(
            super::pinned_skill_path_by_digest(&workspace, "slack-notify", &first_digest).is_err()
        );
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn work_result_is_exact_private_artifact_and_tampering_blocks_read() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!(
            "runx-assistant-work-result-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos()
        ));
        std::fs::create_dir(&root).map_err(|error| error.to_string())?;
        let workspace = runx_runtime::WorkspaceEnv::load_process(root.clone())
            .map_err(|error| error.to_string())?;
        let mut item = assignment("work-1");
        item.status = "completed".to_owned();
        item.receipt = Some(format!("sha256:{}", "a".repeat(64)));
        let result = json!({"kind":"coding_intake","summary":"Check the bounded request","change_set":{"summary":"private context"}});
        item.result_ref = Some(persist_work_result(&workspace, &item, &result)?);
        item.private_updates.push(super::WorkUpdate {
            turn_id: item.turn.id.clone(),
            receipt: item.receipt.clone().unwrap(),
            result_ref: item.result_ref.clone().unwrap(),
            status: None,
        });
        assert_eq!(read_work_result(&workspace, &item)?, Some(result));
        assert_eq!(super::pending_work_update_indices(&[item.clone()]), vec![0]);
        assert_eq!(evictable_work_index(&[item.clone()], None), None);
        let update = super::work_update_item(&workspace, &item)?;
        assert_eq!(update["work_id"], item.id);
        assert_eq!(update["receipt"].as_str(), item.receipt.as_deref());
        let text = private_notification_text(
            &json!({"brief":"Verified work","items":[update]}),
            2000,
            false,
            &official_slack()?,
        )?;
        assert!(text.contains("WORK: Check the bounded request"));
        assert!(text.contains(&format!("receipt sha256:{}", "a".repeat(64))));
        assert!(!text.contains(&format!(
            "source {}",
            update["source_digest"].as_str().unwrap_or_default()
        )));
        assert!(text.contains("runx assistant work"));
        item.private_updates[0].status = Some("local_only".to_owned());
        assert!(super::pending_work_update_indices(&[item.clone()]).is_empty());
        assert_eq!(evictable_work_index(&[item.clone()], None), Some(0));
        item.private_updates[0].status = Some("delivered".to_owned());
        assert!(super::pending_work_update_indices(&[item.clone()]).is_empty());
        let path = item
            .result_ref
            .as_ref()
            .and_then(|reference| reference["path"].as_str())
            .ok_or("work result reference lacks path")?;
        std::fs::write(path, b"{}").map_err(|error| error.to_string())?;
        assert!(read_work_result(&workspace, &item).is_err());
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn later_turn_cannot_replace_an_undelivered_private_update() -> Result<(), String> {
        use runx_contracts::{WorkDisposition, WorkOutcome};
        let root = std::env::temp_dir().join(format!("runx-work-update-{}", std::process::id()));
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let workspace = runx_runtime::WorkspaceEnv::load_process(root.clone())
            .map_err(|error| error.to_string())?;
        let first = assignment("work-1");
        let mut second_turn = first.turn.clone();
        second_turn.id = "second-turn".to_owned();
        let follow_up = WorkOutcome {
            disposition: WorkDisposition::FollowUp,
            summary: "Verify later".to_owned(),
            completion_evidence: Vec::new(),
            followups: Vec::new(),
        };
        let first_ref =
            persist_work_result(&workspace, &first, &json!({"summary":"First result"}))?;
        let second = super::transition_assignment(
            &first,
            &follow_up,
            vec![second_turn],
            "sha256:first",
            first_ref,
        )?;
        let first_update = super::work_update_item(&workspace, &second)?;
        let second_ref =
            persist_work_result(&workspace, &second, &json!({"summary":"Second result"}))?;
        let satisfied = WorkOutcome {
            disposition: WorkDisposition::Satisfied,
            summary: "Verified".to_owned(),
            completion_evidence: vec!["provider:confirmed".to_owned()],
            followups: Vec::new(),
        };
        let mut completed = super::transition_assignment(
            &second,
            &satisfied,
            Vec::new(),
            "sha256:second",
            second_ref,
        )?;
        assert_eq!(completed.private_updates.len(), 2);
        assert_eq!(
            super::work_update_item(&workspace, &completed)?,
            first_update
        );
        completed.private_updates[0].status = Some("delivered".to_owned());
        let later = super::work_update_item(&workspace, &completed)?;
        assert_eq!(later["receipt"], "sha256:second");
        assert_ne!(later["source_digest"], first_update["source_digest"]);
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn held_delivery_does_not_starve_due_assignment() {
        assert_eq!(
            due_worker_lane(Some(120), Some(0), 60),
            Some(WorkerLane::Assignment)
        );
        assert_eq!(
            due_worker_lane(Some(0), Some(0), 60),
            Some(WorkerLane::Delivery)
        );
        assert_eq!(due_worker_lane(Some(120), Some(120), 60), None);
        assert_eq!(due_worker_lane(None, None, 60), None);
    }

    #[test]
    fn failed_assignment_waits_without_blocking_another_due_item() -> Result<(), String> {
        let old = assignment("first");
        assert_eq!(old.turn.due_at_unix_seconds, 0);
        let mut work = vec![
            old.clone(),
            WorkAssignment {
                id: "second".to_owned(),
                ..old
            },
        ];
        work[0].turn.due_at_unix_seconds = 120;
        assert_eq!(next_work_due(&work), Some(0));
        assert_eq!(due_work_index(&work, 60), Some(1));
        work[1].status = "completed".to_owned();
        // Suspension remains in the native checkpoint; the obligation stays pending.
        work[0].native_run = Some(super::BoundWorkRun {
            skill: "conversation-review".to_owned(),
            run_id: "run_original".to_owned(),
            package_digest: "sha256:old".to_owned(),
            input_ref: json!({}),
            local_provider: None,
        });
        let second_turn_id = work[1].turn.id.clone();
        work[1].private_updates.push(super::WorkUpdate {
            turn_id: second_turn_id,
            receipt: "sha256:second".to_owned(),
            result_ref: json!({}),
            status: Some("local_only".to_owned()),
        });
        assert_eq!(next_work_due(&work), Some(120));
        assert_eq!(due_work_index(&work, 60), None);
        assert_eq!(due_work_index(&work, 120), Some(0));
        assert_eq!(evictable_work_index(&work, None), Some(1));
        Ok(())
    }

    #[test]
    fn pending_delivery_preserves_only_new_attention() {
        let deferred = [DeferredTurn {
            pages: Vec::new(),
            occurrence_digests: vec!["second".to_owned()],
        }];
        let observations = [
            json!({"source_digest":"handled"}),
            json!({"source_digest":"pending"}),
            json!({"source_digest":"second"}),
            json!({"source_digest":"new"}),
            json!({"source_digest":"new"}),
        ];
        assert_eq!(
            unreviewed_digests(
                &["handled".to_owned()],
                &["pending".to_owned()],
                &deferred,
                &observations,
            ),
            vec!["new"]
        );
    }

    #[test]
    fn quiet_hours_and_scan_continuation_cross_expected_boundaries() -> Result<(), &'static str> {
        let hours = QuietHours {
            start_hour: 22,
            end_hour: 8,
        };
        assert!(quiet_hour(&hours, 23));
        assert!(quiet_hour(&hours, 7));
        assert!(!quiet_hour(&hours, 8));
        assert!(
            scan_continuation_from_value(&json!({
                "status":"truncated","next_cursor":null,"page_index":2,"scan_id":"prior"
            }))
            .is_none()
        );
        let next = scan_continuation_from_value(&json!({
            "status":"truncated","next_cursor":"page:3","page_index":2,"scan_id":"prior"
        }))
        .ok_or("valid bounded continuation")?;
        assert_eq!(next.cursor, "page:3");
        assert_eq!(next.page_index, 3);
        Ok(())
    }

    #[test]
    fn private_notification_preserves_sources_and_partial_coverage_within_quota()
    -> Result<(), String> {
        let items = (0..5)
            .map(|index| {
                json!({
                    "priority":"high",
                    "summary":"<@U123|Kam> @channel <!here> <https://example.com/a|the link> A direct request with important context ".repeat(25),
                    "source_ref":format!("slack://workspace/channel/{index}"),
                    "source_digest":format!("sha256:{index:064x}")
                })
            })
            .collect::<Vec<_>>();
        let packet = json!({"brief":"too long ".repeat(500),"items":items});
        let slack = official_slack()?;
        let text = private_notification_text(&packet, 2000, true, &slack)?;
        assert!(text.len() <= 2000);
        assert!(text.contains("Source scan is partial"));
        assert!(slack.is_inert(&text));
        assert!(text.contains("Kam"));
        assert!(text.contains("the link"));
        for index in 0..5 {
            assert!(text.contains(&format!("sha256:{index:064x}")));
            assert!(!text.contains(&format!("slack://workspace/channel/{index}")));
        }
        assert!(private_notification_text(&packet, 100, true, &slack).is_err());
        Ok(())
    }

    #[test]
    fn notification_retry_identity_is_a_stable_provider_uuid() -> Result<(), String> {
        let run = "assistant-private-0123456789abcdef0123456789abcdef";
        let key = notification_uuid(run)?;
        assert_eq!(key.len(), 36);
        assert_eq!(&key[..8], &run["assistant-private-".len()..][..8]);
        assert_eq!(key.as_bytes()[14], b'4');
        assert_eq!(notification_uuid(run)?, key);
        assert!(notification_uuid("assistant-private-invalid").is_err());
        Ok(())
    }

    #[test]
    fn notification_completion_requires_exact_independent_provider_readback() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let channel = "slack://T123/C456";
        let readback = json!({
            "status":"success",
            "provider":"slack",
            "operation":"channel.post.read",
            "finality":"confirmed",
            "readback_ref":format!("runx:provider-readback:sha256:{}", "b".repeat(64)),
            "result":{
                "content_digest":digest,
                "conversation_locator":channel,
                "message_locator":"slack://T123/C456/1791095930.389929"
            }
        });
        assert!(verified_notification_readback(
            &readback, &digest, channel, "slack"
        ));
        let mut wrong = readback.clone();
        wrong["result"]["content_digest"] = json!(format!("sha256:{}", "c".repeat(64)));
        assert!(!verified_notification_readback(
            &wrong, &digest, channel, "slack"
        ));
        wrong = readback;
        wrong["operation"] = json!("channel.post");
        assert!(!verified_notification_readback(
            &wrong, &digest, channel, "slack"
        ));
    }

    #[test]
    fn private_delivery_refuses_paths_variables_and_encoded_credentials() {
        let env = BTreeMap::from([(
            "SERVICE_API_KEY".to_owned(),
            "specific-secret-value".to_owned(),
        )]);
        let terms = vec!["internal.example.test".to_owned()];
        for text in [
            "Build failed under /Users/someone/dev/project",
            "See /etc/hosts for the answer",
            "Path=/mnt/work/report",
            "deploy log at '/opt/app/logs'",
            "see \"/srv/data/report\"",
            "see\r\n/opt/data",
            "The file is D:\\dev\\repo\\secret",
            "Open the file at %252fprivate%252ftmp%252freport",
            "Use SERVICE_API_KEY for the next step",
            "Use $HOME for the working directory",
            "Token specific-secret-value was rotated",
            "See internal.example.test for details",
            "Bearer opaque-private-token",
        ] {
            assert!(!private_text_is_safe(text, &terms, &env), "accepted {text}");
        }
        assert!(private_text_is_safe(
            "HIGH: The customer asked for a release decision (source sha256:abcd).",
            &terms,
            &env,
        ));
    }
}
