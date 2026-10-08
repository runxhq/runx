//! The local assistant: standing work run for the operator on the native
//! runtime. Execution, effects and receipts stay runtime-owned.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{Reference, ReferenceType, sha256_prefixed};
use runx_runtime::{
    InstanceRecord, Instances, KernelStore, ReceiptPathInputs, RunAuthority, RuntimeReceiptConfig,
    StandingApproval, StandingApprovals, WorkspaceEnv, resolve_receipt_path,
};
use serde::{Deserialize, Serialize};

mod tick;
mod timer;

const PROFILE_SCHEMA: &str = "runx.assistant.profile.v1";
const MAX_SOURCES: usize = 8;
const MAX_ACTIONS: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssistantAction {
    Check,
    Tick,
    Status,
    Pause,
    Resume,
    Grant,
    Revoke,
    InstallTimer,
    RemoveTimer,
    Remember,
    Forget,
    Memories,
    Work,
    RetryWork,
    Execute,
    Report,
    DiscardNotification,
    ReconcileNotification,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssistantRequest {
    pub action: AssistantAction,
    pub profile_path: PathBuf,
    pub memory_id: Option<String>,
    pub text_file: Option<PathBuf>,
    pub cursor: Option<String>,
    pub work_id: Option<String>,
    pub message_locator: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssistantProfile {
    pub(crate) schema: String,
    pub(crate) instance_id: String,
    pub(crate) skills_root: PathBuf,
    #[serde(default)]
    pub(crate) skill_paths: BTreeMap<String, PathBuf>,
    pub(crate) inbox_data_source_ref: String,
    pub(crate) sources: Vec<SourceProfile>,
    pub(crate) allowed_action_ids: Vec<String>,
    #[serde(default)]
    pub(crate) work_routes: Vec<WorkRoute>,
    pub(crate) goals: Vec<StandingGoal>,
    /// The operator's delegated authority for assistant-run skills. The
    /// runtime mints its terms from the bound instance's live capability.
    pub(crate) authority: RunAuthority,
    #[serde(default)]
    pub(crate) charter: String,
    #[serde(default)]
    pub(crate) confidential_terms: Vec<String>,
    #[serde(default = "default_context_budget_bytes")]
    pub(crate) context_budget_bytes: usize,
    pub(crate) model: ModelProfile,
    pub(crate) notification: Option<NotificationProfile>,
    #[serde(default)]
    pub(crate) decision: Option<DecisionProfile>,
    #[serde(default = "default_heartbeat_seconds")]
    pub(crate) heartbeat_seconds: u64,
    #[serde(default = "default_min_check_minutes")]
    pub(crate) min_check_minutes: u64,
    #[serde(default = "default_max_check_minutes")]
    pub(crate) max_check_minutes: u64,
    #[serde(default)]
    pub(crate) quiet_hours: Option<QuietHours>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QuietHours {
    pub(crate) start_hour: u8,
    pub(crate) end_hour: u8,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceProfile {
    pub(crate) source_id: String,
    pub(crate) skill: String,
    pub(crate) provider: String,
    pub(crate) triage_kind: String,
    pub(crate) locator_prefix: String,
    pub(crate) config: serde_json::Value,
    pub(crate) page_limit: u64,
    pub(crate) local_provider: Option<LocalProviderProfile>,
}

impl SourceProfile {
    fn id(&self) -> &str {
        &self.source_id
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelProfile {
    pub(crate) model: String,
    pub(crate) endpoint_url: String,
    pub(crate) context_sharing: runx_contracts::ContextSharing,
    #[serde(default)]
    pub(crate) auth_mode: ModelAuthMode,
    #[serde(default = "default_model_rounds")]
    pub(crate) max_rounds: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ModelAuthMode {
    #[default]
    LocalNone,
    ApiKey,
}

impl ModelAuthMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::LocalNone => "local_none",
            Self::ApiKey => "api_key",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NotificationProfile {
    pub(crate) authority_id: String,
    pub(crate) skill: String,
    pub(crate) provider: String,
    pub(crate) provider_grant_id: String,
    pub(crate) principal_ref: String,
    pub(crate) destination_locator: String,
    pub(crate) expires_at_unix_seconds: u64,
    pub(crate) max_posts_total: u32,
    pub(crate) max_posts_per_day: u32,
    pub(crate) max_text_bytes: u32,
    pub(crate) allowed_mention: Option<String>,
    /// A local connector instance to deliver through instead of the hosted grant.
    pub(crate) instance_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DecisionProfile {
    pub(crate) skill: String,
    pub(crate) provider: String,
    pub(crate) authorized_actor_ref: String,
    pub(crate) local_provider: Option<LocalProviderProfile>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalProviderProfile {
    /// The connector instance this work runs under. It must stay the
    /// selected instance of its connector for the work to run.
    pub(crate) instance_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StandingGoal {
    pub(crate) goal_id: String,
    pub(crate) purpose: String,
    pub(crate) completion_condition: String,
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) schedule: Option<GoalSchedule>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GoalSchedule {
    /// The first possible occurrence. Subsequent occurrences are aligned to
    /// this instant, rather than to the last time the host happened to wake.
    pub(crate) starts_at_unix_seconds: u64,
    pub(crate) interval_seconds: u64,
    pub(crate) route_id: String,
    pub(crate) target_ref: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkRoute {
    pub(crate) route_id: String,
    #[serde(default)]
    pub(crate) successor_only: bool,
    pub(crate) goal_id: String,
    pub(crate) skill: String,
    pub(crate) runner: String,
    pub(crate) description: String,
    pub(crate) source_kinds: Vec<String>,
    pub(crate) model: bool,
    /// Trusted fixed inputs (scopes, limits, workspace bindings). A proposal
    /// cannot replace these values.
    pub(crate) inputs: serde_json::Map<String, serde_json::Value>,
    /// Optional input implementing the common normalized source-context contract.
    pub(crate) source_context_input: Option<String>,
    /// Optional runner input carrying the admitted turn occurrence, never model-authored.
    #[serde(default)]
    pub(crate) turn_id_input: Option<String>,
    /// Optional runner input carrying the exact admitted source occurrence.
    #[serde(default)]
    pub(crate) origin_ref_input: Option<String>,
    pub(crate) local_provider: Option<LocalProviderProfile>,
}

#[derive(Clone, Debug)]
pub(crate) struct LoadedProfile {
    pub(crate) path: PathBuf,
    pub(crate) profile: AssistantProfile,
    pub(crate) revision: String,
    pub(crate) source_set_digest: String,
    pub(crate) skill_set_digest: String,
    pub(crate) skill_bindings: BTreeMap<String, String>,
}

impl AssistantProfile {
    fn skill_path(&self, name: &str) -> PathBuf {
        self.skill_paths
            .get(name)
            .cloned()
            .unwrap_or_else(|| self.skills_root.join(name))
    }
}

fn default_heartbeat_seconds() -> u64 {
    300
}

fn default_min_check_minutes() -> u64 {
    15
}

fn default_max_check_minutes() -> u64 {
    60
}

fn default_model_rounds() -> u32 {
    8
}

fn default_context_budget_bytes() -> usize {
    48 * 1024
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn valid_skill_ref(value: &str) -> bool {
    match value.split_once('/') {
        Some((owner, name)) => {
            valid_identifier(owner) && valid_identifier(name) && !name.contains('/')
        }
        None => valid_identifier(value),
    }
}

fn valid_local_provider(value: &LocalProviderProfile) -> bool {
    runx_contracts::connector::InstanceId::new(value.instance_id.as_str()).is_some()
}

/// A live, selected instance is required when work actually executes.
pub(crate) fn instance_digest(
    workspace: &WorkspaceEnv,
    instance_id: &str,
) -> Result<String, String> {
    let record = instance_record(workspace, instance_id)?;
    if record.revoked || !record.selected {
        return Err(format!(
            "connector instance {instance_id} is not the selected instance of its connector"
        ));
    }
    instance_revision(&record)
}

/// Control-plane reads keep the last confirmed binding revision after its
/// instance is revoked, so pause and status do not depend on provider access.
fn recorded_instance_digest(workspace: &WorkspaceEnv, instance_id: &str) -> Result<String, String> {
    instance_revision(&instance_record(workspace, instance_id)?)
}

fn instance_revision(record: &InstanceRecord) -> Result<String, String> {
    record
        .instance
        .binding_digest()
        .map(|digest| digest.as_str().to_owned())
        .map_err(|error| error.to_string())
}

fn instance_record(workspace: &WorkspaceEnv, instance_id: &str) -> Result<InstanceRecord, String> {
    let store = KernelStore::for_workspace(workspace.env(), workspace.cwd())
        .map_err(|error| error.to_string())?;
    Instances::new(store)
        .get(instance_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("connector instance {instance_id} does not exist"))
}

pub(crate) fn standing_approvals(workspace: &WorkspaceEnv) -> Result<StandingApprovals, String> {
    KernelStore::for_workspace(workspace.env(), workspace.cwd())
        .map(StandingApprovals::new)
        .map_err(|error| error.to_string())
}

/// The standing approval the notification profile asks the operator to grant:
/// bounded posts of `channel.post` to one destination, under the instance the
/// delivery runs as, usable only by this source set.
pub(crate) fn notification_approval(
    notification: &NotificationProfile,
    source_set_digest: &str,
) -> Result<StandingApproval, String> {
    let instance = notification
        .instance_id
        .clone()
        .unwrap_or_else(|| format!("injected.{}", notification.provider));
    Ok(StandingApproval {
        approval_id: notification.authority_id.clone(),
        connector: runx_contracts::connector::ConnectorId::new(notification.provider.as_str())
            .ok_or("the notification provider is not a connector id")?,
        operation: runx_contracts::connector::OperationName::new("channel.post")
            .ok_or("channel.post is an operation name")?,
        instance_id: runx_contracts::connector::InstanceId::new(instance)
            .ok_or("the notification instance id is invalid")?,
        target: notification.destination_locator.clone(),
        delegate_digest: source_set_digest.to_owned(),
        expires_at_unix_seconds: notification.expires_at_unix_seconds,
        max_total: notification.max_posts_total,
        max_per_day: notification.max_posts_per_day,
        max_text_bytes: notification.max_text_bytes,
        allowed_mention: notification.allowed_mention.clone(),
    })
}

fn load_profile(path: &Path, workspace: &WorkspaceEnv) -> Result<LoadedProfile, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.cwd().join(path)
    };
    let path = path
        .canonicalize()
        .map_err(|error| format!("resolving assistant profile {}: {error}", path.display()))?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("reading assistant profile metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("assistant profile must be a regular file".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("assistant profile must be private (chmod 600)".to_owned());
        }
    }
    if metadata.len() > 128 * 1024 {
        return Err("assistant profile exceeds 128 KiB".to_owned());
    }
    let bytes = fs::read(&path)
        .map_err(|error| format!("reading assistant profile {}: {error}", path.display()))?;
    let mut profile: AssistantProfile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("assistant profile JSON is invalid: {error}"))?;
    validate_profile(&profile)?;
    for (name, path) in &profile.skill_paths {
        let inspected = runx_runtime::inspect_skill_package(path, None, Some(workspace.env()))
            .map_err(|error| format!("invalid assistant skill path {name}: {error}"))?;
        if inspected
            .as_object()
            .and_then(|fields| fields.get("name"))
            .and_then(runx_contracts::JsonValue::as_str)
            != Some(name.as_str())
        {
            return Err(format!(
                "assistant skill path {name} names a different package"
            ));
        }
    }
    runx_runtime::load_managed_agent_config(
        &model_environment(&profile, workspace)?,
        workspace.cwd(),
    )
    .map_err(|error| format!("assistant model configuration is invalid: {error}"))?
    .ok_or("assistant model configuration is incomplete")?;
    let source_bytes = serde_json::to_vec(&profile.sources)
        .map_err(|error| format!("encoding assistant source roster: {error}"))?;
    let source_bindings = profile
        .sources
        .iter()
        .filter_map(|source| {
            source
                .local_provider
                .as_ref()
                .map(|binding| (&source.source_id, &binding.instance_id))
        })
        .map(|(id, path)| Ok((id.clone(), recorded_instance_digest(workspace, path)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let source_set_digest = if source_bindings.is_empty() {
        sha256_prefixed(&source_bytes)
    } else {
        sha256_prefixed(
            &serde_json::to_vec(&(&profile.sources, &source_bindings))
                .map_err(|error| format!("encoding source binding revision: {error}"))?,
        )
    };
    let mut names = BTreeSet::from(["operator-inbox", "attention-review"]);
    if let Some(notification) = &profile.notification {
        names.insert(notification.skill.as_str());
    }
    if let Some(decision) = &profile.decision {
        names.insert(decision.skill.as_str());
    }
    for source in &profile.sources {
        names.insert(source.skill.as_str());
    }
    for route in &profile.work_routes {
        names.insert(route.skill.as_str());
    }
    let mut skill_bindings = BTreeMap::new();
    for name in names {
        let digest = runx_runtime::inspect_local_skill_binding(
            &profile.skill_path(name),
            Some(workspace.env()),
        )
        .map_err(|error| format!("inspecting assistant skill {name}: {error}"))?;
        skill_bindings.insert(name.to_owned(), digest);
    }
    let skill_set_digest = sha256_prefixed(
        &serde_json::to_vec(&skill_bindings)
            .map_err(|error| format!("encoding assistant skill bindings: {error}"))?,
    );
    // Operator configuration is the control-plane revision. Skill packages
    // are bound by each native run, so a patch must not stop unrelated work.
    let other_bindings = profile
        .work_routes
        .iter()
        .filter_map(|route| {
            route
                .local_provider
                .as_ref()
                .map(|binding| (&route.route_id, &binding.instance_id))
        })
        .chain(profile.decision.iter().filter_map(|decision| {
            decision
                .local_provider
                .as_ref()
                .map(|binding| (&decision.skill, &binding.instance_id))
        }))
        .chain(profile.notification.iter().filter_map(|notification| {
            notification
                .instance_id
                .as_ref()
                .map(|instance| (&notification.skill, instance))
        }))
        .map(|(id, path)| Ok((id.clone(), recorded_instance_digest(workspace, path)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let revision = if source_bindings.is_empty() && other_bindings.is_empty() {
        sha256_prefixed(&bytes)
    } else {
        sha256_prefixed(
            &serde_json::to_vec(&(&bytes, &source_bindings, &other_bindings))
                .map_err(|error| format!("encoding selected provider revisions: {error}"))?,
        )
    };
    profile.authority.mandate_ref = Some(Reference::runx(ReferenceType::Mandate, &revision));
    Ok(LoadedProfile {
        path,
        profile,
        revision,
        source_set_digest,
        skill_set_digest,
        skill_bindings,
    })
}

fn validate_profile(profile: &AssistantProfile) -> Result<(), String> {
    if profile.schema != PROFILE_SCHEMA {
        return Err("assistant profile schema is unsupported".to_owned());
    }
    if !valid_identifier(&profile.instance_id) {
        return Err(
            "assistant instance_id must use 1-64 lowercase letters, digits, - or _".to_owned(),
        );
    }
    let expected_actor = Reference::with_uri(
        ReferenceType::Principal,
        format!("runx:principal:assistant:{}", profile.instance_id),
    );
    if profile.authority.actor_ref != expected_actor {
        return Err(
            "assistant authority must name this assistant instance as its actor".to_owned(),
        );
    }
    if !profile.skills_root.is_absolute() || !profile.skills_root.is_dir() {
        return Err("assistant skills_root must be an existing absolute directory".to_owned());
    }
    for (name, path) in &profile.skill_paths {
        if !valid_skill_ref(name)
            || !path.is_absolute()
            || !path.join(runx_runtime::PROFILE_FILE).is_file()
        {
            return Err(
                "assistant skill_paths must bind names to absolute skill packages".to_owned(),
            );
        }
    }
    for name in ["operator-inbox", "attention-review"] {
        if !profile
            .skill_path(name)
            .join(runx_runtime::PROFILE_FILE)
            .is_file()
        {
            return Err(format!("assistant skills_root is missing {name}"));
        }
    }
    if !profile.inbox_data_source_ref.starts_with("local://") {
        return Err("assistant inbox data source must be a local:// ref".to_owned());
    }
    if profile.sources.len() > MAX_SOURCES
        || (profile.sources.is_empty()
            && !profile
                .goals
                .iter()
                .any(|goal| goal.enabled && goal.schedule.is_some()))
    {
        return Err(format!(
            "assistant requires a conversation source or an enabled scheduled goal (at most {MAX_SOURCES} sources)"
        ));
    }
    let mut source_ids = BTreeSet::new();
    let mut page_budget = 0_u64;
    for source in &profile.sources {
        if !valid_identifier(source.id()) || !source_ids.insert(source.id()) {
            return Err("assistant source IDs must be unique bounded identifiers".to_owned());
        }
        if !valid_skill_ref(&source.skill)
            || !valid_identifier(&source.provider)
            || !matches!(
                source.triage_kind.as_str(),
                "direct_mention" | "operator_selected_query" | "imported"
            )
            || !source.config.is_object()
            || !(1..=20).contains(&source.page_limit)
            || source.locator_prefix.is_empty()
            || source.locator_prefix.len() > 300
            || !source.locator_prefix.contains("://")
            || source.locator_prefix.starts_with("runx://assistant/")
        {
            return Err(
                "assistant source needs a bounded skill, locator prefix, config, and page limit"
                    .to_owned(),
            );
        }
        page_budget += source.page_limit;
        if profile.sources.iter().any(|other| {
            other.source_id != source.source_id
                && (source.locator_prefix.starts_with(&other.locator_prefix)
                    || other.locator_prefix.starts_with(&source.locator_prefix))
        }) {
            return Err("assistant source locator prefixes must not overlap".to_owned());
        }
        if !profile
            .skill_path(&source.skill)
            .join(runx_runtime::PROFILE_FILE)
            .is_file()
        {
            return Err(format!("assistant skills_root is missing {}", source.skill));
        }
        if source
            .local_provider
            .as_ref()
            .is_some_and(|binding| !valid_local_provider(binding))
        {
            return Err("assistant source local provider binding is incomplete".to_owned());
        }
    }
    if page_budget > 20 {
        return Err("assistant combined source-page budget exceeds 20 observations".to_owned());
    }
    if profile.allowed_action_ids.len() > MAX_ACTIONS {
        return Err(format!("assistant action roster exceeds {MAX_ACTIONS}"));
    }
    if profile.work_routes.len() > 16 || profile.goals.len() > 16 {
        return Err("assistant goal and work rosters are limited to 16 entries".to_owned());
    }
    let mut goal_ids = BTreeSet::new();
    for goal in &profile.goals {
        if !valid_identifier(&goal.goal_id)
            || !goal_ids.insert(&goal.goal_id)
            || goal.purpose.trim().is_empty()
            || goal.purpose.len() > 2000
            || goal.completion_condition.trim().is_empty()
            || goal.completion_condition.len() > 1000
        {
            return Err(
                "assistant goals require unique IDs, a purpose and a completion condition"
                    .to_owned(),
            );
        }
    }
    let mut route_ids = BTreeSet::new();
    for route in &profile.work_routes {
        if !valid_identifier(&route.route_id)
            || !route_ids.insert(&route.route_id)
            || !goal_ids.contains(&route.goal_id)
            || !valid_skill_ref(&route.skill)
            || !valid_identifier(&route.runner)
            || route.description.trim().is_empty()
            || route.description.len() > 1000
            || route.source_kinds.len() > 8
            || route
                .source_kinds
                .iter()
                .any(|kind| !valid_identifier(kind))
            || route
                .source_context_input
                .as_ref()
                .is_some_and(|key| !valid_identifier(key) || route.inputs.contains_key(key))
            || route.turn_id_input.as_ref().is_some_and(|key| {
                !valid_identifier(key)
                    || route.inputs.contains_key(key)
                    || route.source_context_input.as_ref() == Some(key)
            })
            || route.origin_ref_input.as_ref().is_some_and(|key| {
                !valid_identifier(key)
                    || route.inputs.contains_key(key)
                    || route.source_context_input.as_ref() == Some(key)
                    || route.turn_id_input.as_ref() == Some(key)
            })
        {
            return Err("assistant routes require a declared goal, exact skill/runner and bounded input policy".to_owned());
        }
        let inspected = runx_runtime::inspect_skill_package(
            &profile.skill_path(&route.skill),
            Some(&route.runner),
            None,
        )
        .map_err(|error| format!("invalid work binding {}: {error}", route.route_id))?;
        let result_contract = inspected
            .as_object()
            .and_then(|fields| fields.get("runner"))
            .and_then(runx_contracts::JsonValue::as_object)
            .and_then(|runner| runner.get("declared_result_packets"))
            .and_then(runx_contracts::JsonValue::as_object)
            .and_then(|packets| packets.get("work_result"))
            .and_then(runx_contracts::JsonValue::as_str);
        if result_contract != Some("runx.work_outcome.v1") {
            return Err(format!(
                "work route {} must declare the shared work_result packet",
                route.route_id
            ));
        }
        if route
            .local_provider
            .as_ref()
            .is_some_and(|binding| !valid_local_provider(binding))
        {
            return Err("assistant work local provider binding is incomplete".to_owned());
        }
    }
    for goal in &profile.goals {
        let Some(schedule) = goal.schedule.as_ref() else {
            continue;
        };
        let route = profile
            .work_routes
            .iter()
            .find(|route| {
                route.route_id == schedule.route_id
                    && route.goal_id == goal.goal_id
                    && !route.successor_only
            })
            .ok_or_else(|| format!("scheduled goal {} has no eligible route", goal.goal_id))?;
        if schedule.starts_at_unix_seconds == 0
            || !(300..=30 * 24 * 60 * 60).contains(&schedule.interval_seconds)
            || schedule.target_ref.trim().is_empty()
            || schedule.target_ref.len() > 300
            || route.source_context_input.is_some()
            || (!route.source_kinds.is_empty()
                && !route
                    .source_kinds
                    .iter()
                    .any(|kind| kind == "scheduled_goal"))
        {
            return Err(format!(
                "scheduled goal {} has an invalid occurrence policy",
                goal.goal_id
            ));
        }
    }
    if profile.charter.len() > 2000 {
        return Err("assistant charter exceeds 2000 bytes".to_owned());
    }
    if !(8 * 1024..=128 * 1024).contains(&profile.context_budget_bytes) {
        return Err("assistant context budget must be between 8 and 128 KiB".to_owned());
    }
    if profile.confidential_terms.len() > 16
        || profile
            .confidential_terms
            .iter()
            .any(|term| term.len() < 3 || term.len() > 200 || term.trim() != term)
    {
        return Err("assistant confidential terms must be 3-200 bytes, at most 16".to_owned());
    }
    if profile
        .allowed_action_ids
        .iter()
        .any(|action| action != "private_update")
    {
        return Err("assistant V1 only executes the configured private_update action".to_owned());
    }
    let mut actions = BTreeSet::new();
    for action in &profile.allowed_action_ids {
        if !valid_identifier(action) || !actions.insert(action) {
            return Err("assistant action IDs must be unique bounded identifiers".to_owned());
        }
    }
    if profile
        .allowed_action_ids
        .iter()
        .any(|action| action == "private_update")
        != profile.notification.is_some()
    {
        return Err(
            "private_update action and notification configuration must appear together".to_owned(),
        );
    }
    if profile.model.model.trim().is_empty() || !(1..=8).contains(&profile.model.max_rounds) {
        return Err("assistant model and max_rounds must be bounded".to_owned());
    }
    if !(60..=3600).contains(&profile.heartbeat_seconds)
        || profile.min_check_minutes == 0
        || profile.min_check_minutes > profile.max_check_minutes
        || profile.max_check_minutes > 1440
    {
        return Err("assistant heartbeat and next-check bounds are invalid".to_owned());
    }
    if profile.quiet_hours.as_ref().is_some_and(|hours| {
        hours.start_hour >= 24 || hours.end_hour >= 24 || hours.start_hour == hours.end_hour
    }) {
        return Err(
            "assistant quiet hours must have distinct local start/end hours in 0..24".to_owned(),
        );
    }
    if let Some(notification) = &profile.notification {
        if notification.authority_id.trim().is_empty()
            || !valid_skill_ref(&notification.skill)
            || notification.provider.is_empty()
            || notification.provider.len() > 80
            || !notification
                .provider
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || notification.provider_grant_id.trim().is_empty()
            || notification.principal_ref.trim().is_empty()
            || !notification
                .destination_locator
                .starts_with(&format!("{}://", notification.provider))
            || !profile
                .skill_path(&notification.skill)
                .join(runx_runtime::PROFILE_FILE)
                .is_file()
        {
            return Err("assistant notification binding is incomplete".to_owned());
        }
        if notification.max_posts_total == 0
            || notification.max_posts_per_day == 0
            || !(1..=4000).contains(&notification.max_text_bytes)
        {
            return Err("assistant notification quotas or text limit are invalid".to_owned());
        }
        if notification
            .allowed_mention
            .as_ref()
            .is_some_and(|mention| mention.len() > 100 || mention.chars().any(char::is_control))
        {
            return Err("assistant notification mention is invalid".to_owned());
        }
    }
    if let Some(decision) = &profile.decision {
        if profile
            .notification
            .as_ref()
            .is_none_or(|notification| notification.provider != decision.provider)
            || !valid_skill_ref(&decision.skill)
            || !valid_identifier(&decision.provider)
            || decision.authorized_actor_ref.trim().is_empty()
            || decision.authorized_actor_ref.len() > 200
        {
            return Err("assistant decision binding requires an exact notification provider and human actor".to_owned());
        }
        // The notification skill posts the decision request, so its
        // connector's rules say how the actor is mentioned.
        let notification_skill = profile
            .notification
            .as_ref()
            .map(|notification| profile.skill_path(&notification.skill))
            .ok_or("assistant decision binding requires a notification binding")?;
        let mention = runx_runtime::kernel::connectors::messages::Messages::for_skill(
            &notification_skill,
            &decision.provider,
        )?
        .mention(&decision.authorized_actor_ref);
        if mention.is_none()
            || profile
                .notification
                .as_ref()
                .and_then(|notification| notification.allowed_mention.as_ref())
                != mention.as_ref()
        {
            return Err(
                "decision requests must mention exactly the authorized human actor".to_owned(),
            );
        }
        runx_runtime::inspect_skill_package(
            &profile.skill_path(&decision.skill),
            Some("assistant_decide"),
            None,
        )
        .map_err(|error| format!("invalid decision binding: {error}"))?;
        if decision
            .local_provider
            .as_ref()
            .is_some_and(|binding| !valid_local_provider(binding))
        {
            return Err("assistant decision local provider binding is incomplete".to_owned());
        }
    }
    Ok(())
}

fn receipt_root(workspace: &WorkspaceEnv) -> PathBuf {
    resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: None,
        runtime_config: Some(&RuntimeReceiptConfig::default()),
        env: workspace.env(),
        cwd: workspace.cwd(),
    })
    .path
}

pub fn execute(
    plan: &AssistantRequest,
    workspace: &WorkspaceEnv,
) -> Result<serde_json::Value, String> {
    let loaded = load_profile(&plan.profile_path, workspace)?;
    let profile = &loaded.profile;
    match plan.action {
        AssistantAction::Check => Ok(serde_json::json!({
            "status": "profile_valid",
            "instance_id": profile.instance_id,
            "profile_revision": loaded.revision,
            "source_set_digest": loaded.source_set_digest,
            "skill_set_digest": loaded.skill_set_digest,
            "source_count": profile.sources.len(),
            "allowed_action_ids": profile.allowed_action_ids,
            "profile_path": loaded.path,
        })),
        AssistantAction::Grant => {
            let notification = profile
                .notification
                .as_ref()
                .ok_or("assistant notification is not configured")?;
            let approvals = standing_approvals(workspace)?;
            approvals
                .grant(notification_approval(
                    notification,
                    &loaded.source_set_digest,
                )?)
                .map_err(|error| error.to_string())?;
            let status = approvals
                .status(&notification.authority_id)
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({"status":"installed","authority":status}))
        }
        AssistantAction::Revoke => {
            let notification = profile
                .notification
                .as_ref()
                .ok_or("assistant notification is not configured")?;
            standing_approvals(workspace)?
                .revoke(&notification.authority_id)
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({"status":"revoked","authority_id":notification.authority_id}))
        }
        AssistantAction::Tick => tick::tick(&loaded, workspace),
        AssistantAction::Execute => tick::execute(&loaded, workspace),
        AssistantAction::Status => tick::status(&loaded, workspace),
        AssistantAction::Pause => tick::set_paused(&loaded, workspace, true),
        AssistantAction::Resume => tick::set_paused(&loaded, workspace, false),
        AssistantAction::InstallTimer => timer::install_timer(&loaded, workspace),
        AssistantAction::RemoveTimer => timer::remove_timer(&loaded, workspace),
        AssistantAction::Remember => {
            let memory_id = plan.memory_id.as_deref().ok_or("memory ID is required")?;
            let text_path = plan
                .text_file
                .as_deref()
                .ok_or("memory text file is required")?;
            tick::remember(&loaded, workspace, memory_id, text_path)
        }
        AssistantAction::Forget => {
            let memory_id = plan.memory_id.as_deref().ok_or("memory ID is required")?;
            tick::forget(&loaded, workspace, memory_id)
        }
        AssistantAction::Memories => tick::memories(&loaded, workspace),
        AssistantAction::Work => tick::work(&loaded, workspace, plan.cursor.as_deref()),
        AssistantAction::RetryWork => tick::retry_work(
            &loaded,
            workspace,
            plan.work_id.as_deref().ok_or("work ID is required")?,
        ),
        AssistantAction::Report => tick::report(&loaded, workspace),
        AssistantAction::DiscardNotification => tick::discard_notification(&loaded, workspace),
        AssistantAction::ReconcileNotification => tick::reconcile_notification(
            &loaded,
            workspace,
            plan.message_locator
                .as_deref()
                .ok_or("message locator is required")?,
        ),
    }
}
pub(crate) fn model_environment(
    profile: &AssistantProfile,
    workspace: &WorkspaceEnv,
) -> Result<BTreeMap<String, String>, String> {
    let mut env = workspace.env().clone();
    apply_model_profile(&mut env, &profile.model)?;
    Ok(env)
}

fn apply_model_profile(
    env: &mut BTreeMap<String, String>,
    model: &ModelProfile,
) -> Result<(), String> {
    if model.auth_mode == ModelAuthMode::ApiKey
        && env
            .get("RUNX_AGENT_API_KEY")
            .is_none_or(|key| key.trim().is_empty())
    {
        return Err("remote assistant model requires an explicit RUNX_AGENT_API_KEY".to_owned());
    }
    if model.auth_mode == ModelAuthMode::LocalNone {
        env.remove("RUNX_AGENT_API_KEY");
    }
    env.insert(
        "RUNX_PROFILE_SHARING".to_owned(),
        match model.context_sharing {
            runx_contracts::ContextSharing::LocalOnly => "local_only",
            runx_contracts::ContextSharing::ConfiguredModel => "configured_model",
        }
        .to_owned(),
    );
    // The profile's model is a chat completions endpoint the operator chose.
    // As a provider runx has no profile for, it reads only RUNX_AGENT_API_KEY,
    // never an ambient vendor key.
    env.insert("RUNX_AGENT_PROVIDER".to_owned(), "assistant".to_owned());
    env.insert(
        "RUNX_AGENT_DIALECT".to_owned(),
        runx_runtime::ModelDialect::ChatCompletions
            .as_str()
            .to_owned(),
    );
    env.insert(
        "RUNX_AGENT_AUTH_MODE".to_owned(),
        model.auth_mode.as_str().to_owned(),
    );
    env.insert("RUNX_AGENT_MODEL".to_owned(), model.model.clone());
    env.insert(
        "RUNX_AGENT_ENDPOINT_URL".to_owned(),
        model.endpoint_url.clone(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::{
        AssistantProfile, ModelAuthMode, ModelProfile, SourceProfile, apply_model_profile,
        validate_profile,
    };

    #[test]
    fn calendar_goal_route_uses_the_shared_work_result_contract() -> Result<(), String> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../skills")
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let profile: AssistantProfile = serde_json::from_value(json!({
            "schema":"runx.assistant.profile.v1","instance_id":"calendar-test",
            "skills_root":root,
            "inbox_data_source_ref":"local://runx-data-store/calendar-test-inbox",
            "authority":{"actor_ref":{"type":"principal","uri":"runx:principal:assistant:calendar-test"},"delegations":[]},
            "sources":[],
            "allowed_action_ids":[],
            "goals":[{"goal_id":"calendar","purpose":"Review upcoming commitments",
                "completion_condition":"A complete calendar window is reviewed","enabled":true,
                "schedule":{"starts_at_unix_seconds":1000,"interval_seconds":86400,
                    "route_id":"calendar_brief","target_ref":"runx://calendar/primary"}}],
            "work_routes":[{"route_id":"calendar_brief","goal_id":"calendar",
                "skill":"google-calendar","runner":"assistant-brief",
                "description":"Review the next calendar window","source_kinds":["scheduled_goal"],
                "model":false,"inputs":{"calendar_id":"primary","window_hours":24},
                "origin_ref_input":"occurrence_ref","source_context_input":null,
                "local_provider":null}],
            "model":{"model":"unused","endpoint_url":"http://127.0.0.1:1/v1/chat/completions",
                "context_sharing":"local_only"},"notification":null
        }))
        .map_err(|error| error.to_string())?;
        validate_profile(&profile)?;
        Ok(())
    }

    #[test]
    fn source_pages_cannot_exceed_queue_commit_capacity() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills");
        let mut profile = AssistantProfile {
            schema: "runx.assistant.profile.v1".to_owned(),
            instance_id: "test".to_owned(),
            skills_root: root,
            skill_paths: Default::default(),
            inbox_data_source_ref: "local://runx/operator-inbox/test".to_owned(),
            sources: vec![
                SourceProfile {
                    source_id: "slack".to_owned(),
                    skill: "slack".to_owned(),
                    provider: "slack".to_owned(),
                    triage_kind: "direct_mention".to_owned(),
                    locator_prefix: "slack://".to_owned(),
                    config: json!({"query":{"mentions_connected_subject":true,"limit":20}}),
                    page_limit: 20,
                    local_provider: None,
                },
                SourceProfile {
                    source_id: "other".to_owned(),
                    skill: "slack".to_owned(),
                    provider: "example".to_owned(),
                    triage_kind: "operator_selected_query".to_owned(),
                    locator_prefix: "example://tenant/".to_owned(),
                    config: json!({"query":{"keywords":"example","limit":10}}),
                    page_limit: 10,
                    local_provider: None,
                },
            ],
            allowed_action_ids: vec![],
            work_routes: vec![],
            goals: vec![],
            authority: runx_runtime::RunAuthority {
                actor_ref: runx_contracts::Reference::with_uri(
                    runx_contracts::ReferenceType::Principal,
                    "runx:principal:assistant:test",
                ),
                mandate_ref: None,
                delegations: Vec::new(),
            },
            charter: String::new(),
            confidential_terms: vec![],
            context_budget_bytes: super::default_context_budget_bytes(),
            model: ModelProfile {
                model: "local-model".to_owned(),
                endpoint_url: "http://127.0.0.1:18081/v1/chat/completions".to_owned(),
                context_sharing: runx_contracts::ContextSharing::LocalOnly,
                auth_mode: ModelAuthMode::LocalNone,
                max_rounds: 4,
            },
            notification: None,
            decision: None,
            heartbeat_seconds: 300,
            min_check_minutes: 15,
            max_check_minutes: 60,
            quiet_hours: None,
        };
        assert!(matches!(
            validate_profile(&profile),
            Err(message) if message.contains("combined source-page budget")
        ));
        profile.sources[0].page_limit = 10;
        profile.sources[0].config["query"]["limit"] = json!(10);
        profile.sources[1].locator_prefix = "slack://workspace/".to_owned();
        assert!(matches!(
            validate_profile(&profile),
            Err(message) if message.contains("must not overlap")
        ));
    }

    #[test]
    fn operator_pinned_model_uses_configured_auth_mode() -> Result<(), String> {
        let local: ModelProfile = serde_json::from_value(json!({
            "model":"chosen-local-model",
            "endpoint_url":"http://127.0.0.1:18082/v1/chat/completions",
            "context_sharing":"local_only", "max_rounds":4
        }))
        .map_err(|error| format!("local profile must decode: {error}"))?;
        assert_eq!(local.auth_mode, ModelAuthMode::LocalNone);
        let mut env = std::collections::BTreeMap::from([
            ("RUNX_AGENT_API_KEY".to_owned(), "remote-secret".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "other-secret".to_owned()),
        ]);
        let mut model = ModelProfile {
            model: "chosen-local-model".to_owned(),
            endpoint_url: "http://127.0.0.1:18082/v1/chat/completions".to_owned(),
            context_sharing: runx_contracts::ContextSharing::LocalOnly,
            auth_mode: ModelAuthMode::LocalNone,
            max_rounds: 4,
        };
        apply_model_profile(&mut env, &model)?;
        assert_eq!(
            env.get("RUNX_AGENT_MODEL").map(String::as_str),
            Some("chosen-local-model")
        );
        assert_eq!(
            env.get("RUNX_AGENT_AUTH_MODE").map(String::as_str),
            Some("local_none")
        );
        assert!(!env.contains_key("RUNX_AGENT_API_KEY"));
        let home = std::env::temp_dir().join(format!(
            "runx-assistant-model-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        env.insert("RUNX_HOME".to_owned(), home.display().to_string());
        let resolved = |env: &std::collections::BTreeMap<String, String>| {
            runx_runtime::load_managed_agent_config(env, &home)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "the profile resolves".to_owned())
        };
        let local_config = resolved(&env)?;
        assert_eq!(
            local_config.dialect,
            runx_runtime::ModelDialect::ChatCompletions
        );
        assert!(
            local_config.api_key.is_none(),
            "an ambient vendor key is never read"
        );

        model.model = "chosen-remote-model".to_owned();
        model.endpoint_url = "https://example.test/v1/chat/completions".to_owned();
        model.auth_mode = ModelAuthMode::ApiKey;
        assert!(
            apply_model_profile(&mut env, &model)
                .is_err_and(|error| error.contains("explicit RUNX_AGENT_API_KEY"))
        );
        env.insert("RUNX_AGENT_API_KEY".to_owned(), "remote-secret".to_owned());
        apply_model_profile(&mut env, &model)?;
        assert_eq!(
            env.get("RUNX_AGENT_MODEL").map(String::as_str),
            Some("chosen-remote-model")
        );
        assert_eq!(
            env.get("RUNX_AGENT_AUTH_MODE").map(String::as_str),
            Some("api_key")
        );
        assert_eq!(
            env.get("RUNX_AGENT_API_KEY").map(String::as_str),
            Some("remote-secret")
        );
        assert_eq!(
            resolved(&env)?.api_key,
            Some(runx_runtime::SecretString::new("remote-secret"))
        );
        assert_eq!(
            env.get("RUNX_AGENT_ENDPOINT_URL").map(String::as_str),
            Some("https://example.test/v1/chat/completions")
        );
        Ok(())
    }
}
