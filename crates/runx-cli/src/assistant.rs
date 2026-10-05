//! One local, bounded personal-assistant turn. Source skills, data-store,
//! operator-inbox and provider effects retain their existing owners.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::sha256_prefixed;
use runx_runtime::{
    NotificationAuthorityGrant, NotificationAuthorityGrantSpec, ReceiptPathInputs,
    RuntimeReceiptConfig, WorkspaceEnv, install_notification_authority,
    notification_authority_status, resolve_receipt_path, revoke_notification_authority,
};
use serde::{Deserialize, Serialize};

use crate::cli_args::{flag_value, os_arg, split_flag};

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
    Execute,
    Report,
    DiscardNotification,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssistantPlan {
    pub action: AssistantAction,
    pub profile_path: PathBuf,
    pub json: bool,
    pub memory_id: Option<String>,
    pub text_file: Option<PathBuf>,
    pub cursor: Option<String>,
}

pub fn parse_assistant_plan(args: &[OsString]) -> Result<AssistantPlan, String> {
    if args.first().and_then(|part| part.to_str()) != Some("assistant") {
        return Err("internal error: assistant dispatcher received another command".to_owned());
    }
    let action = match os_arg(args, 1, "assistant")? {
        "check" => AssistantAction::Check,
        "tick" => AssistantAction::Tick,
        "status" => AssistantAction::Status,
        "pause" => AssistantAction::Pause,
        "resume" => AssistantAction::Resume,
        "grant" => AssistantAction::Grant,
        "revoke" => AssistantAction::Revoke,
        "install-timer" => AssistantAction::InstallTimer,
        "remove-timer" => AssistantAction::RemoveTimer,
        "remember" => AssistantAction::Remember,
        "forget" => AssistantAction::Forget,
        "memories" => AssistantAction::Memories,
        "work" => AssistantAction::Work,
        "execute" => AssistantAction::Execute,
        "report" => AssistantAction::Report,
        "discard-notification" => AssistantAction::DiscardNotification,
        _ => return Err("assistant action must be check, tick, execute, status, report, pause, resume, grant, revoke, install-timer, remove-timer, remember, forget, memories, work or discard-notification".to_owned()),
    };
    let mut profile_path = None;
    let mut json = false;
    let mut memory_id = None;
    let mut text_file = None;
    let mut cursor = None;
    let mut index = 2;
    while index < args.len() {
        let token = os_arg(args, index, "assistant")?;
        let (flag, inline) = split_flag(token);
        match flag {
            "--profile" => {
                let (value, next) = flag_value(args, index, flag, inline, "assistant")?;
                if profile_path.replace(PathBuf::from(value)).is_some() {
                    return Err("assistant --profile may be supplied only once".to_owned());
                }
                index = next;
            }
            "--json" | "-j" if inline.is_none() => {
                json = true;
                index += 1;
            }
            "--memory-id" => {
                let (value, next) = flag_value(args, index, flag, inline, "assistant")?;
                if memory_id.replace(value.to_owned()).is_some() {
                    return Err("assistant --memory-id may be supplied only once".to_owned());
                }
                index = next;
            }
            "--text-file" => {
                let (value, next) = flag_value(args, index, flag, inline, "assistant")?;
                if text_file.replace(PathBuf::from(value)).is_some() {
                    return Err("assistant --text-file may be supplied only once".to_owned());
                }
                index = next;
            }
            "--cursor" => {
                let (value, next) = flag_value(args, index, flag, inline, "assistant")?;
                if value.is_empty()
                    || value.len() > 500
                    || cursor.replace(value.to_owned()).is_some()
                {
                    return Err(
                        "assistant --cursor must be a single bounded nonempty value".to_owned()
                    );
                }
                index = next;
            }
            _ => return Err(format!("unknown assistant argument {token}")),
        }
    }
    match action {
        AssistantAction::Remember if memory_id.is_some() && text_file.is_some() => {}
        AssistantAction::Forget if memory_id.is_some() && text_file.is_none() => {}
        AssistantAction::Remember | AssistantAction::Forget => {
            return Err(
                "remember requires --memory-id and --text-file; forget requires --memory-id only"
                    .to_owned(),
            );
        }
        _ if memory_id.is_some() || text_file.is_some() => {
            return Err("assistant memory flags are only valid for remember or forget".to_owned());
        }
        _ => {}
    }
    if cursor.is_some() && action != AssistantAction::Work {
        return Err("assistant --cursor is only valid for work".to_owned());
    }
    Ok(AssistantPlan {
        action,
        profile_path: profile_path.ok_or("assistant requires --profile path")?,
        json,
        memory_id,
        text_file,
        cursor,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AssistantProfile {
    pub(super) schema: String,
    pub(super) instance_id: String,
    pub(super) skills_root: PathBuf,
    pub(super) data_source_ref: String,
    pub(super) inbox_data_source_ref: String,
    pub(super) sources: Vec<SourceProfile>,
    pub(super) allowed_action_ids: Vec<String>,
    #[serde(default)]
    pub(super) work_routes: Vec<WorkRoute>,
    #[serde(default)]
    pub(super) charter: String,
    #[serde(default)]
    pub(super) confidential_terms: Vec<String>,
    pub(super) model: ModelProfile,
    pub(super) notification: Option<NotificationProfile>,
    #[serde(default = "default_heartbeat_seconds")]
    pub(super) heartbeat_seconds: u64,
    #[serde(default = "default_min_check_minutes")]
    pub(super) min_check_minutes: u64,
    #[serde(default = "default_max_check_minutes")]
    pub(super) max_check_minutes: u64,
    #[serde(default)]
    pub(super) quiet_hours: Option<QuietHours>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QuietHours {
    pub(super) start_hour: u8,
    pub(super) end_hour: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum SourceProfile {
    SlackMentions {
        source_id: String,
        query: serde_json::Value,
    },
    NitrosendInbox {
        source_id: String,
        brand_sid: String,
        arguments: serde_json::Value,
        credential_profile: Option<String>,
    },
}

impl SourceProfile {
    fn id(&self) -> &str {
        match self {
            Self::SlackMentions { source_id, .. } | Self::NitrosendInbox { source_id, .. } => {
                source_id
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelProfile {
    pub(super) model: String,
    pub(super) endpoint_url: String,
    #[serde(default)]
    pub(super) auth_mode: ModelAuthMode,
    #[serde(default = "default_model_rounds")]
    pub(super) max_rounds: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ModelAuthMode {
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
pub(super) struct NotificationProfile {
    pub(super) authority_id: String,
    pub(super) provider_grant_id: String,
    pub(super) principal_ref: String,
    pub(super) channel_locator: String,
    pub(super) expires_at_unix_seconds: u64,
    pub(super) max_posts_total: u32,
    pub(super) max_posts_per_day: u32,
    pub(super) max_text_bytes: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkRoute {
    pub(super) route_id: String,
    pub(super) kind: String,
    #[serde(default)]
    pub(super) repositories: Vec<String>,
    #[serde(default)]
    pub(super) credential_profile: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct LoadedProfile {
    pub(super) path: PathBuf,
    pub(super) profile: AssistantProfile,
    pub(super) revision: String,
    pub(super) source_set_digest: String,
    pub(super) skill_set_digest: String,
    pub(super) skill_bindings: BTreeMap<String, String>,
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

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
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
    let profile: AssistantProfile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("assistant profile JSON is invalid: {error}"))?;
    validate_profile(&profile)?;
    runx_runtime::load_managed_agent_config(
        &model_environment(&profile, workspace)?,
        workspace.cwd(),
    )
    .map_err(|error| format!("assistant model configuration is invalid: {error}"))?
    .ok_or("assistant model configuration is incomplete")?;
    let source_bytes = serde_json::to_vec(&profile.sources)
        .map_err(|error| format!("encoding assistant source roster: {error}"))?;
    let source_set_digest = sha256_prefixed(&source_bytes);
    let mut names = BTreeSet::from([
        "data-store",
        "operator-inbox",
        "attention-review",
        "slack-notify",
    ]);
    for source in &profile.sources {
        names.insert(match source {
            SourceProfile::SlackMentions { .. } => "slack",
            SourceProfile::NitrosendInbox { .. } => "nitrosend",
        });
    }
    if !profile.work_routes.is_empty() {
        for route in &profile.work_routes {
            match route.kind.as_str() {
                "github_pr_status" => {
                    names.insert("github-sync");
                }
                "conversation_review" => {
                    names.insert("conversation-review");
                    names.insert("issue-intake");
                }
                "work_plan" => {
                    names.insert("work-plan");
                }
                _ => {}
            }
        }
    }
    let mut skill_bindings = BTreeMap::new();
    for name in names {
        let digest = runx_runtime::inspect_local_skill_binding(
            &profile.skills_root.join(name),
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
    let revision = sha256_prefixed(&bytes);
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
    if !profile.skills_root.is_absolute() || !profile.skills_root.is_dir() {
        return Err("assistant skills_root must be an existing absolute directory".to_owned());
    }
    for name in [
        "data-store",
        "operator-inbox",
        "attention-review",
        "slack-notify",
    ] {
        if !profile.skills_root.join(name).join("X.yaml").is_file() {
            return Err(format!("assistant skills_root is missing {name}"));
        }
    }
    if !profile.data_source_ref.starts_with("local://")
        || !profile.inbox_data_source_ref.starts_with("local://")
    {
        return Err("assistant control and inbox data sources must be local:// refs".to_owned());
    }
    if profile.sources.is_empty() || profile.sources.len() > MAX_SOURCES {
        return Err(format!("assistant requires 1-{MAX_SOURCES} sources"));
    }
    let mut source_ids = BTreeSet::new();
    let mut page_budget = 0_u64;
    for source in &profile.sources {
        if !valid_identifier(source.id()) || !source_ids.insert(source.id()) {
            return Err("assistant source IDs must be unique bounded identifiers".to_owned());
        }
        let package = match source {
            SourceProfile::SlackMentions { query, .. } => {
                if !query.is_object() {
                    return Err("assistant Slack query must be an object".to_owned());
                }
                if query.get("cursor").is_some() {
                    return Err("assistant Slack cursor is owned by the persisted scan".to_owned());
                }
                let limit = query["limit"].as_u64().unwrap_or(20);
                if !(1..=20).contains(&limit) {
                    return Err("assistant Slack page limit must be 1-20".to_owned());
                }
                page_budget += limit;
                "slack"
            }
            SourceProfile::NitrosendInbox {
                brand_sid,
                arguments,
                ..
            } => {
                if brand_sid.trim().is_empty() || !arguments.is_object() {
                    return Err(
                        "assistant Nitrosend inbox needs a brand SID and bounded arguments"
                            .to_owned(),
                    );
                }
                let page = arguments["page"].as_u64().unwrap_or(1);
                let per = arguments["per"]
                    .as_u64()
                    .ok_or("assistant Nitrosend inbox requires an explicit per-page limit")?;
                if page != 1 || !(1..=20).contains(&per) {
                    return Err("assistant Nitrosend inbox requires page 1 and per 1-20".to_owned());
                }
                if arguments["view"] != "full" {
                    return Err(
                        "assistant Nitrosend inbox requires full rows for thread readback"
                            .to_owned(),
                    );
                }
                page_budget += per;
                "nitrosend"
            }
        };
        if !profile.skills_root.join(package).join("X.yaml").is_file() {
            return Err(format!("assistant skills_root is missing {package}"));
        }
    }
    if page_budget > 20 {
        return Err("assistant combined source-page budget exceeds 20 observations".to_owned());
    }
    if profile.allowed_action_ids.len() > MAX_ACTIONS {
        return Err(format!("assistant action roster exceeds {MAX_ACTIONS}"));
    }
    if profile.work_routes.len() > 4 {
        return Err("assistant work route roster exceeds four".to_owned());
    }
    if profile
        .work_routes
        .iter()
        .filter(|route| route.kind == "work_plan")
        .count()
        > 1
    {
        return Err("assistant may configure only one work_plan route".to_owned());
    }
    if profile
        .work_routes
        .iter()
        .filter(|route| route.kind == "conversation_review")
        .count()
        > 1
    {
        return Err("assistant may configure only one conversation_review route".to_owned());
    }
    if profile
        .work_routes
        .iter()
        .any(|route| route.kind == "work_plan")
        && !profile
            .work_routes
            .iter()
            .any(|route| route.kind == "conversation_review")
    {
        return Err("assistant work_plan requires a conversation_review route".to_owned());
    }
    let mut route_ids = BTreeSet::new();
    for route in &profile.work_routes {
        if !valid_identifier(&route.route_id) || !route_ids.insert(&route.route_id) {
            return Err("assistant work route IDs must be unique bounded identifiers".to_owned());
        }
        match route.kind.as_str() {
            "github_pr_status" if !route.repositories.is_empty() && route.repositories.len() <= 8 => {}
            "conversation_review" | "work_plan" if route.repositories.is_empty() && route.credential_profile.is_none() => {}
            _ => return Err("assistant work route must be github_pr_status with 1-8 repositories or conversation_review/work_plan with no repository or credential override".to_owned()),
        }
        let mut repositories = BTreeSet::new();
        for repository in &route.repositories {
            let mut parts = repository.split('/');
            let valid_part = |part: &str| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part.len() <= 100
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
            };
            if !parts.next().is_some_and(valid_part)
                || !parts.next().is_some_and(valid_part)
                || parts.next().is_some()
                || !repositories.insert(repository)
            {
                return Err("assistant work repository must be a unique owner/name".to_owned());
            }
        }
    }
    if profile.charter.len() > 2000 {
        return Err("assistant charter exceeds 2000 bytes".to_owned());
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
            || notification.provider_grant_id.trim().is_empty()
            || notification.principal_ref.trim().is_empty()
            || !notification.channel_locator.starts_with("slack://")
        {
            return Err("assistant notification binding is incomplete".to_owned());
        }
        if notification.max_posts_total == 0
            || notification.max_posts_per_day == 0
            || !(1..=4000).contains(&notification.max_text_bytes)
        {
            return Err("assistant notification quotas or text limit are invalid".to_owned());
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

pub fn run_native_assistant(plan: AssistantPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let output = run_assistant_command(&plan, workspace);
    match output {
        Ok(value) => {
            let rendered = if plan.json {
                serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned())
            } else {
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_owned())
            };
            crate::cli_io::write_stdout_code(&format!("{rendered}\n"), 0)
        }
        Err(message) if plan.json => crate::cli_io::write_stdout_code(
            &crate::cli_error::json_failure_output(&message, "assistant_error"),
            1,
        ),
        Err(message) => {
            let _ignored = crate::cli_io::write_stderr_code(&format!("runx: {message}\n"));
            ExitCode::from(1)
        }
    }
}

fn run_assistant_command(
    plan: &AssistantPlan,
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
            let root = receipt_root(workspace);
            let grant = NotificationAuthorityGrant::new(NotificationAuthorityGrantSpec {
                authority_id: notification.authority_id.clone(),
                provider_grant_id: notification.provider_grant_id.clone(),
                principal_ref: notification.principal_ref.clone(),
                target: notification.channel_locator.clone(),
                source_set_digest: loaded.source_set_digest.clone(),
                expires_at_unix_seconds: notification.expires_at_unix_seconds,
                max_posts_total: notification.max_posts_total,
                max_posts_per_day: notification.max_posts_per_day,
                max_text_bytes: notification.max_text_bytes,
            })
            .map_err(|error| error.to_string())?;
            install_notification_authority(&root, grant).map_err(|error| error.to_string())?;
            let status = notification_authority_status(&root, &notification.authority_id)
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({"status":"installed","authority":status}))
        }
        AssistantAction::Revoke => {
            let notification = profile
                .notification
                .as_ref()
                .ok_or("assistant notification is not configured")?;
            let root = receipt_root(workspace);
            revoke_notification_authority(&root, &notification.authority_id)
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
        AssistantAction::Report => tick::report(&loaded, workspace),
        AssistantAction::DiscardNotification => tick::discard_notification(&loaded, workspace),
    }
}

pub(super) fn model_environment(
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
    // The configured endpoint may belong to another API vendor. Do not send an
    // unrelated ambient OpenAI key there through the generic fallback.
    env.remove("OPENAI_API_KEY");
    if model.auth_mode == ModelAuthMode::LocalNone {
        env.remove("RUNX_AGENT_API_KEY");
    }
    env.insert("RUNX_AGENT_PROVIDER".to_owned(), "openai".to_owned());
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
    fn source_pages_cannot_exceed_queue_commit_capacity() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills");
        let mut profile = AssistantProfile {
            schema: "runx.assistant.profile.v1".to_owned(),
            instance_id: "test".to_owned(),
            skills_root: root,
            data_source_ref: "local://runx/personal-assistant/test".to_owned(),
            inbox_data_source_ref: "local://runx/operator-inbox/test".to_owned(),
            sources: vec![
                SourceProfile::SlackMentions {
                    source_id: "slack".to_owned(),
                    query: json!({"mentions_connected_subject":true,"limit":20}),
                },
                SourceProfile::NitrosendInbox {
                    source_id: "mail".to_owned(),
                    brand_sid: "brnd_fixture".to_owned(),
                    arguments: json!({"view":"full","page":1,"per":10}),
                    credential_profile: None,
                },
            ],
            allowed_action_ids: vec![],
            work_routes: vec![],
            charter: String::new(),
            confidential_terms: vec![],
            model: ModelProfile {
                model: "local-model".to_owned(),
                endpoint_url: "http://127.0.0.1:18081/v1/chat/completions".to_owned(),
                auth_mode: ModelAuthMode::LocalNone,
                max_rounds: 4,
            },
            notification: None,
            heartbeat_seconds: 300,
            min_check_minutes: 15,
            max_check_minutes: 60,
            quiet_hours: None,
        };
        assert!(matches!(
            validate_profile(&profile),
            Err(message) if message.contains("combined source-page budget")
        ));
        profile.sources[0] = SourceProfile::SlackMentions {
            source_id: "slack".to_owned(),
            query: json!({"mentions_connected_subject":true,"limit":10}),
        };
        if let SourceProfile::NitrosendInbox { arguments, .. } = &mut profile.sources[1] {
            arguments["view"] = json!("compact");
        }
        assert!(matches!(
            validate_profile(&profile),
            Err(message) if message.contains("full rows")
        ));
    }

    #[test]
    fn operator_pinned_model_uses_configured_auth_mode() -> Result<(), String> {
        let legacy: ModelProfile = serde_json::from_value(json!({
            "model":"chosen-local-model",
            "endpoint_url":"http://127.0.0.1:18082/v1/chat/completions",
            "max_rounds":4
        }))
        .map_err(|error| format!("legacy profile must decode: {error}"))?;
        assert_eq!(legacy.auth_mode, ModelAuthMode::LocalNone);
        let mut env = std::collections::BTreeMap::from([
            ("RUNX_AGENT_API_KEY".to_owned(), "remote-secret".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "other-secret".to_owned()),
        ]);
        let mut model = ModelProfile {
            model: "chosen-local-model".to_owned(),
            endpoint_url: "http://127.0.0.1:18082/v1/chat/completions".to_owned(),
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
        assert!(!env.contains_key("OPENAI_API_KEY"));

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
        assert!(!env.contains_key("OPENAI_API_KEY"));
        assert_eq!(
            env.get("RUNX_AGENT_ENDPOINT_URL").map(String::as_str),
            Some("https://example.test/v1/chat/completions")
        );
        Ok(())
    }
}
