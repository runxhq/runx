//! The CLI is the local host for one finite assistant turn. Skills retain
//! source, queue, storage, model judgment, and notification ownership.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, SecondsFormat, TimeZone, Timelike, Utc};
use ring::rand::{SecureRandom, SystemRandom};
use runx_contracts::sha256_prefixed;
use runx_runtime::{
    ASSISTANT_CONFIDENTIAL_TERMS_ENV, ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV, ManagedAgentPolicy,
    NOTIFICATION_AUTHORITY_ID_ENV, NOTIFICATION_SOURCE_SET_DIGEST_ENV,
    PROVIDER_PERMISSION_GRANT_ID_ENV, PROVIDER_PERMISSION_GRANTED_SCOPES_ENV,
    PROVIDER_PERMISSION_PRINCIPAL_REF_ENV, SkillRunRequest, WorkspaceEnv,
    assistant_text_is_safe as private_text_is_safe, encode_provider_scopes_env, now_iso8601,
    resolve_project_runx_dir, resolve_runx_workspace_base, resolve_skill_credential_for_path,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{LoadedProfile, QuietHours, SourceProfile, WorkRoute, model_environment};

const CONTROL_SCHEMA: &str = "runx.assistant.control.v1";
const MAX_OBSERVATIONS: usize = 20;
const MAX_DEFERRED_TURNS: usize = 16;
const MAX_MAIL_CONTEXT_CHARS: usize = 20_000;
const MAX_WORK_ARTIFACT_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    schema: String,
    instance_id: String,
    profile_revision: String,
    paused: bool,
    next_due_unix_seconds: u64,
    #[serde(default)]
    next_delivery_due_unix_seconds: u64,
    #[serde(default)]
    next_worker_due_unix_seconds: u64,
    active_run_id: Option<String>,
    #[serde(default)]
    pending_turn: Option<PendingTurn>,
    pending_intent: Option<PendingIntent>,
    #[serde(default)]
    deferred_turns: Vec<DeferredTurn>,
    last_handled_material_digest: Option<String>,
    last_delivered_material_digest: Option<String>,
    last_handled_receipt: Option<String>,
    #[serde(default)]
    handled_occurrence_digests: Vec<String>,
    #[serde(default)]
    last_blocker: Option<String>,
    #[serde(default)]
    scan_retry_from_first_page: bool,
    #[serde(default)]
    last_fresh_scan_at: BTreeMap<String, u64>,
    #[serde(default)]
    confirmed_memory: Vec<ConfirmedMemory>,
    #[serde(default)]
    work: Vec<WorkAssignment>,
    #[serde(default)]
    last_review_ref: Option<Value>,
    #[serde(default)]
    last_review_at: Option<String>,
    #[serde(default)]
    last_review_coverage_incomplete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkAssignment {
    id: String,
    #[serde(default)]
    parent_work_id: Option<String>,
    source_ref: String,
    source_digest: String,
    thread_locator: String,
    route_id: String,
    target_ref: String,
    status: String,
    #[serde(default)]
    retry_after_unix_seconds: u64,
    receipt: Option<String>,
    result_ref: Option<Value>,
    #[serde(default)]
    runs: BTreeMap<String, BoundWorkRun>,
    #[serde(default)]
    failed_skill: Option<String>,
    #[serde(default)]
    private_update_status: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundWorkRun {
    run_id: String,
    package_digest: String,
    input_ref: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmedMemory {
    memory_id: String,
    text: String,
    confirmed_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingIntent {
    artifact_ref: Value,
    material_digest: String,
    logical_run_id: String,
    selected_digests: Vec<String>,
    #[serde(default)]
    delivery_package_digest: String,
    #[serde(default)]
    work_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingTurn {
    pages: Vec<PinnedPage>,
    review_ref: Option<Value>,
    #[serde(default)]
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
    #[serde(default)]
    scan_lane: ScanLane,
    #[serde(default)]
    fetched_at_unix_seconds: u64,
    next_cursor: Option<String>,
    messages: Vec<Value>,
    observations: Vec<Value>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScanLane {
    #[default]
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
        .filter(|item| item.status == "pending")
        .map(|item| item.retry_after_unix_seconds)
        .min()
}

fn due_work_index(work: &[WorkAssignment], now: u64) -> Option<usize> {
    work.iter()
        .position(|item| item.status == "pending" && item.retry_after_unix_seconds <= now)
}

fn evictable_work_index(work: &[WorkAssignment], protected_id: Option<&str>) -> Option<usize> {
    work.iter().position(|item| {
        item.status != "pending"
            && !needs_work_update(item)
            && protected_id != Some(item.id.as_str())
            && !work.iter().any(|child| {
                child.status == "pending"
                    && child.parent_work_id.as_deref() == Some(item.id.as_str())
            })
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

fn project_runx_dir(workspace: &WorkspaceEnv) -> PathBuf {
    let base = resolve_runx_workspace_base(workspace.env(), workspace.cwd());
    resolve_project_runx_dir(workspace.env(), &base)
}

fn control_default(loaded: &LoadedProfile) -> Control {
    Control {
        schema: CONTROL_SCHEMA.to_owned(),
        instance_id: loaded.profile.instance_id.clone(),
        profile_revision: loaded.revision.clone(),
        paused: true,
        next_due_unix_seconds: 0,
        next_delivery_due_unix_seconds: 0,
        next_worker_due_unix_seconds: 0,
        active_run_id: None,
        pending_turn: None,
        pending_intent: None,
        deferred_turns: Vec::new(),
        last_handled_material_digest: None,
        last_delivered_material_digest: None,
        last_handled_receipt: None,
        handled_occurrence_digests: Vec::new(),
        last_blocker: None,
        scan_retry_from_first_page: false,
        last_fresh_scan_at: BTreeMap::new(),
        confirmed_memory: Vec::new(),
        work: Vec::new(),
        last_review_ref: None,
        last_review_at: None,
        last_review_coverage_incomplete: false,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one typed boundary for the finite skill turn"
)]
fn run_skill(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    name: &str,
    runner: &str,
    inputs: Value,
    model: bool,
    credential_profile: Option<&str>,
    delivery: bool,
) -> Result<(Value, String), String> {
    run_skill_with_id(
        loaded,
        workspace,
        name,
        runner,
        inputs,
        model,
        credential_profile,
        delivery,
        None,
        None,
    )
    .map_err(String::from)
}

#[derive(Debug)]
struct SkillRunFailure {
    message: String,
    terminal_receipt: Option<String>,
    skill: Option<String>,
}

impl From<String> for SkillRunFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            terminal_receipt: None,
            skill: None,
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
        Self {
            message: message.into(),
            terminal_receipt: Some(receipt.to_owned()),
            skill: Some(skill.to_owned()),
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
    credential_profile: Option<&str>,
    delivery: bool,
    run_id: Option<&str>,
    pinned_package_digest: Option<&str>,
) -> Result<(Value, String), SkillRunFailure> {
    let source_path = loaded.profile.skills_root.join(name);
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
    env.insert(
        ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV.to_owned(),
        "required".to_owned(),
    );
    env.insert(
        ASSISTANT_CONFIDENTIAL_TERMS_ENV.to_owned(),
        serde_json::to_string(&loaded.profile.confidential_terms)
            .map_err(|error| format!("encoding assistant egress policy: {error}"))?,
    );
    if delivery {
        let notify = loaded
            .profile
            .notification
            .as_ref()
            .ok_or("notification is not configured")?;
        env.insert(
            PROVIDER_PERMISSION_GRANT_ID_ENV.to_owned(),
            notify.provider_grant_id.clone(),
        );
        env.insert(
            PROVIDER_PERMISSION_PRINCIPAL_REF_ENV.to_owned(),
            notify.principal_ref.clone(),
        );
        env.insert(
            PROVIDER_PERMISSION_GRANTED_SCOPES_ENV.to_owned(),
            encode_provider_scopes_env(&[
                "channel.post".to_owned(),
                "channel.post.read".to_owned(),
            ])
            .map_err(|error| error.to_string())?,
        );
        env.insert(
            NOTIFICATION_AUTHORITY_ID_ENV.to_owned(),
            notify.authority_id.clone(),
        );
        env.insert(
            NOTIFICATION_SOURCE_SET_DIGEST_ENV.to_owned(),
            loaded.source_set_digest.clone(),
        );
    }
    let credential =
        resolve_skill_credential_for_path(&path, Some(runner), credential_profile, workspace)
            .map_err(|error| format!("{name} credential resolution failed: {error}"))?;
    if credential
        .as_ref()
        .is_some_and(|item| !item.resolution.is_ready())
    {
        return Err(format!("{name} requires a configured credential").into());
    }
    let input_map: BTreeMap<String, runx_contracts::JsonValue> = serde_json::from_value(inputs)
        .map_err(|error| format!("assistant skill inputs are invalid: {error}"))?;
    let request = SkillRunRequest {
        skill_path: path,
        receipt_dir: None,
        run_id: run_id.map(str::to_owned),
        answers_path: None,
        inputs: input_map,
        env: env.clone(),
        cwd: workspace.cwd().to_path_buf(),
        managed_agent: if model {
            ManagedAgentPolicy::inline(loaded.profile.model.max_rounds)
                .map_err(|error| error.to_string())?
        } else {
            ManagedAgentPolicy::HostDriven
        },
        local_credential: credential
            .as_ref()
            .and_then(|context| context.resolution.descriptor().cloned()),
    };
    let orchestrator =
        crate::runtime::local_orchestrator(&env).map_err(|error| error.to_string())?;
    let result = orchestrator
        .run_skill_with_runner(&request, runner)
        .map_err(|error| format!("{name}#{runner}: {error}"))?;
    if !result.succeeded() {
        let receipt = result.receipt_refs.first().cloned();
        return Err(SkillRunFailure {
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
            skill: Some(name.to_owned()),
        });
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

fn pinned_skill_path_by_digest(
    workspace: &WorkspaceEnv,
    name: &str,
    digest: &str,
) -> Result<PathBuf, String> {
    let path = project_runx_dir(workspace)
        .join("assistant")
        .join("skill-packages")
        .join(name)
        .join(digest.trim_start_matches("sha256:"));
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("reading pinned {name} skill package: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "pinned {name} skill package is unavailable or changed"
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("protecting pinned {name} skill package: {error}"))?;
    }
    resolve_pinned_skill(&path, name, digest)
}

fn package_digest(path: &Path) -> Result<String, String> {
    let inspected = runx_runtime::inspect_skill_package(path, None, None)
        .map_err(|error| format!("inspecting assistant skill package: {error}"))?;
    inspected
        .as_object()
        .and_then(|object| object.get("package_digest"))
        .and_then(runx_contracts::JsonValue::as_str)
        .map(str::to_owned)
        .ok_or("assistant skill inspection has no package digest".to_owned())
}

fn resolve_pinned_skill(root: &Path, name: &str, digest: &str) -> Result<PathBuf, String> {
    if root.join("SKILL.md").is_file() {
        if package_digest(root)? != digest {
            return Err(format!("pinned {name} skill package has changed"));
        }
        return Ok(root.to_path_buf());
    }
    let bytes = fs::read(root.join(".assistant-bundle.json"))
        .map_err(|error| format!("reading pinned {name} closure binding: {error}"))?;
    let digests: BTreeMap<String, String> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("decoding pinned {name} closure binding: {error}"))?;
    if digests.len() < 2 || !digests.contains_key(name) {
        return Err(format!("pinned {name} closure is incomplete"));
    }
    let bindings = digests
        .iter()
        .map(|(skill, package_digest)| (skill.clone(), (package_digest.clone(), root.join(skill))))
        .collect::<BTreeMap<_, _>>();
    if super::skill_binding_digest(&bindings)? != digest {
        return Err(format!("pinned {name} closure binding has changed"));
    }
    for (skill, (expected, path)) in &bindings {
        if !super::valid_identifier(skill)
            || fs::symlink_metadata(path)
                .map_err(|error| format!("reading pinned {skill} package: {error}"))?
                .file_type()
                .is_symlink()
            || package_digest(path)? != *expected
        {
            return Err(format!("pinned {skill} closure package has changed"));
        }
    }
    Ok(root.join(name))
}

/// A native checkpoint resumes from this exact content-addressed directory.
/// Patch installs can replace the source package without changing a running
/// checkpoint's code. The digest is checked again before every invocation.
fn pinned_skill_path(
    workspace: &WorkspaceEnv,
    source: &Path,
    name: &str,
    expected_digest: &str,
) -> Result<PathBuf, String> {
    let inspected = runx_runtime::inspect_skill_package(source, None, None)
        .map_err(|error| format!("inspecting assistant skill package: {error}"))?;
    let bindings = super::inspected_skill_bindings(&inspected)?;
    let digest = super::skill_binding_digest(&bindings)?;
    if digest != expected_digest {
        return Err(format!(
            "assistant {name} skill changed during this turn; retry with fresh bindings"
        ));
    }
    let root = project_runx_dir(workspace)
        .join("assistant")
        .join("skill-packages")
        .join(name);
    let target = root.join(digest.trim_start_matches("sha256:"));
    fs::create_dir_all(&root)
        .map_err(|error| format!("creating assistant skill package store: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for directory in [
            root.parent().and_then(Path::parent),
            root.parent(),
            Some(root.as_path()),
        ]
        .into_iter()
        .flatten()
        {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("protecting assistant skill package store: {error}"))?;
        }
    }
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o700))
                    .map_err(|error| format!("protecting pinned skill package: {error}"))?;
            }
            return resolve_pinned_skill(&target, name, &digest);
        }
        Ok(_) => {
            return Err(format!(
                "pinned {name} skill package is not a regular directory"
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("checking pinned skill package: {error}")),
    }
    let mut nonce = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| "creating assistant skill package nonce failed".to_owned())?;
    let stage = root.join(format!(".stage-{:032x}", u128::from_le_bytes(nonce)));
    fs::create_dir(&stage).map_err(|error| format!("staging assistant skill package: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("protecting staged assistant skill package: {error}"))?;
    }
    let staged = (|| {
        if bindings.len() == 1 {
            copy_skill_files(source, &stage)?;
        } else {
            let parent = source
                .parent()
                .ok_or("assistant skill has no package parent")?
                .canonicalize()
                .map_err(|error| format!("resolving assistant skill root: {error}"))?;
            let mut manifest = BTreeMap::new();
            for (skill, (package_digest, path)) in &bindings {
                let canonical = path
                    .canonicalize()
                    .map_err(|error| format!("resolving {skill} closure package: {error}"))?;
                if !super::valid_identifier(skill)
                    || canonical.parent() != Some(parent.as_path())
                    || canonical.file_name().and_then(|name| name.to_str()) != Some(skill)
                {
                    return Err("assistant skill closure leaves its local sibling root".to_owned());
                }
                let destination = stage.join(skill);
                fs::create_dir(&destination)
                    .map_err(|error| format!("staging {skill} closure package: {error}"))?;
                copy_skill_files(&canonical, &destination)?;
                manifest.insert(skill.clone(), package_digest.clone());
            }
            fs::write(
                stage.join(".assistant-bundle.json"),
                serde_json::to_vec(&manifest)
                    .map_err(|error| format!("encoding assistant closure binding: {error}"))?,
            )
            .map_err(|error| format!("writing assistant closure binding: {error}"))?;
        }
        if resolve_pinned_skill(&stage, name, &digest).is_err() {
            return Err("assistant skill changed while its package was being pinned".to_owned());
        }
        match fs::rename(&stage, &target) {
            Ok(()) => Ok(()),
            Err(_) if target.exists() && resolve_pinned_skill(&target, name, &digest).is_ok() => {
                Ok(())
            }
            Err(error) => Err(format!("committing assistant skill package: {error}")),
        }
    })();
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    staged?;
    resolve_pinned_skill(&target, name, &digest)
}

fn copy_skill_files(source: &Path, target: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|error| format!("reading skill package: {error}"))? {
        let entry = entry.map_err(|error| format!("reading skill package entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("checking skill package entry: {error}"))?;
        let destination = target.join(entry.file_name());
        if file_type.is_dir() {
            fs::create_dir(&destination)
                .map_err(|error| format!("creating skill package directory: {error}"))?;
            copy_skill_files(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), destination)
                .map_err(|error| format!("copying skill package file: {error}"))?;
        } else {
            return Err(
                "assistant skill packages cannot contain symbolic links or special files"
                    .to_owned(),
            );
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
    let digest = loaded
        .skill_bindings
        .get(skill)
        .ok_or_else(|| format!("assistant has no inspected {skill} skill binding"))?;
    let binding = sha256_prefixed(
        &serde_json::to_vec(&(&item.id, digest))
            .map_err(|error| format!("encoding work run binding: {error}"))?,
    );
    Ok(format!("{prefix}{}", binding.trim_start_matches("sha256:")))
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
    if let Some(bound) = record.state.work[index].runs.get(skill) {
        pinned_skill_path_by_digest(workspace, skill, &bound.package_digest)?;
        let packet = read_bound_work_inputs(workspace, bound)?;
        return Ok((
            bound.clone(),
            packet["inputs"].clone(),
            serde_json::from_value(packet["evidence_receipts"].clone())
                .map_err(|error| format!("decoding bound work evidence: {error}"))?,
        ));
    }
    let (inputs, evidence_receipts) = build()?;
    let packet = json!({"inputs":inputs,"evidence_receipts":evidence_receipts});
    if serde_json::to_vec(&packet)
        .map_err(|error| format!("encoding bound work inputs: {error}"))?
        .len()
        > 64 * 1024
    {
        return Err("assistant work inputs exceed 64 KiB".to_owned());
    }
    let digest = loaded
        .skill_bindings
        .get(skill)
        .ok_or_else(|| format!("assistant has no inspected {skill} skill binding"))?;
    pinned_skill_path(
        workspace,
        &loaded.profile.skills_root.join(skill),
        skill,
        digest,
    )?;
    let run_id = work_run_id(loaded, prefix, &record.state.work[index], skill)?;
    let value = serde_json::from_value(packet.clone())
        .map_err(|error| format!("normalizing bound work inputs: {error}"))?;
    let reference = crate::skill::output::persist_json_artifact(
        &project_runx_dir(workspace),
        &run_id,
        "assistant-work-inputs",
        "inputs",
        &value,
    )?;
    let bound = BoundWorkRun {
        run_id,
        package_digest: digest.clone(),
        input_ref: serde_json::to_value(reference)
            .map_err(|error| format!("encoding bound work input reference: {error}"))?,
    };
    record.state.work[index]
        .runs
        .insert(skill.to_owned(), bound.clone());
    write_control(loaded, workspace, record, "work_run_bound")?;
    Ok((bound, packet["inputs"].clone(), evidence_receipts))
}

fn read_bound_work_inputs(workspace: &WorkspaceEnv, bound: &BoundWorkRun) -> Result<Value, String> {
    let reference = serde_json::from_value(bound.input_ref.clone())
        .map_err(|error| format!("decoding bound work input reference: {error}"))?;
    let value = crate::skill::output::read_json_artifact(&project_runx_dir(workspace), &reference)?;
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

fn nitrosend_read_result(output: &Value) -> Result<&Value, String> {
    let evidence = result_data(output, "provider_evidence")?;
    if evidence["decision"] != "ok" {
        return Err(
            "Nitrosend read was not accepted by the selected account or provider".to_owned(),
        );
    }
    evidence["result"]
        .as_object()
        .ok_or("Nitrosend accepted read lacks an object result".to_owned())?;
    Ok(&evidence["result"])
}

fn read_control(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<ControlRecord, String> {
    let (output, _) = run_skill(
        loaded,
        workspace,
        "data-store",
        "read_events",
        json!({
            "data_source_ref": loaded.profile.data_source_ref,
            "resource": "assistant_control",
            "aggregate_id": loaded.profile.instance_id,
            "limit": 1
        }),
        false,
        None,
        false,
    )?;
    let data = result_data(&output, "data_operation_result")?;
    let rows = data["rows"]
        .as_array()
        .ok_or("assistant control read has no rows array")?;
    let Some(row) = rows.first() else {
        return Ok(ControlRecord {
            version: 0,
            state: control_default(loaded),
        });
    };
    let version = row["version"]
        .as_u64()
        .ok_or("assistant control row lacks version")?;
    let state: Control = serde_json::from_value(row["event"]["payload"]["state"].clone())
        .map_err(|error| format!("assistant control state is invalid: {error}"))?;
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
    if state.work.len() > 40
        || state.work.iter().any(|item| {
            !matches!(item.status.as_str(), "pending" | "completed" | "held")
                || item.id.len() > 80
                || item.parent_work_id.as_ref().is_some_and(|id| id.len() > 80)
                || item.source_ref.len() > 300
                || item.target_ref.len() > 300
                || item.private_update_status.as_deref().is_some_and(|status| {
                    !matches!(status, "local_only" | "delivered") || item.status != "completed"
                })
        })
    {
        return Err("assistant work state is invalid".to_owned());
    }
    if let Some(intent) = state.pending_intent.as_ref() {
        let mut seen = BTreeSet::new();
        if intent.work_ids.len() > 3
            || intent.work_ids.iter().any(|id| {
                !seen.insert(id)
                    || !state.work.iter().any(|item| {
                        item.id == *id
                            && needs_work_update(item)
                            && work_result_digest(item)
                                .is_some_and(|digest| intent.selected_digests.contains(&digest))
                    })
            })
        {
            return Err("assistant notification work binding is invalid".to_owned());
        }
    }
    Ok(ControlRecord { version, state })
}

fn persist_work_result(
    workspace: &WorkspaceEnv,
    item: &WorkAssignment,
    result: &Value,
) -> Result<Value, String> {
    let value = serde_json::from_value(result.clone())
        .map_err(|error| format!("encoding assistant work result: {error}"))?;
    let reference = crate::skill::output::persist_json_artifact(
        &project_runx_dir(workspace),
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

fn read_work_result(
    workspace: &WorkspaceEnv,
    item: &WorkAssignment,
) -> Result<Option<Value>, String> {
    let Some(reference) = item.result_ref.as_ref() else {
        return Ok(None);
    };
    let reference = serde_json::from_value(reference.clone())
        .map_err(|error| format!("decoding assistant work result reference: {error}"))?;
    let value = crate::skill::output::read_json_artifact(&project_runx_dir(workspace), &reference)?;
    serde_json::to_value(value)
        .map(Some)
        .map_err(|error| format!("decoding assistant work result: {error}"))
}

fn work_result_digest(item: &WorkAssignment) -> Option<String> {
    let receipt = item.receipt.as_deref()?;
    Some(sha256_prefixed(
        format!("assistant-work-result:{}:{receipt}", item.id).as_bytes(),
    ))
}

fn needs_work_update(item: &WorkAssignment) -> bool {
    item.status == "completed"
        && item.private_update_status.is_none()
        && item.receipt.is_some()
        && item.result_ref.is_some()
}

fn pending_work_update_indices(work: &[WorkAssignment]) -> Vec<usize> {
    work.iter()
        .enumerate()
        .filter(|(_, item)| needs_work_update(item))
        .map(|(index, _)| index)
        .collect()
}

fn work_update_item(workspace: &WorkspaceEnv, item: &WorkAssignment) -> Result<Value, String> {
    let result = read_work_result(workspace, item)?.ok_or("completed work has no result")?;
    let summary = match result["kind"].as_str() {
        Some("conversation_review") => {
            let status = result["status"]
                .as_str()
                .ok_or("conversation work lacks status")?;
            let prefix = match status {
                "reply" => "Unsent reply draft",
                "follow_up" => "Follow-up proposed but not queued",
                "needs_context" => "Conversation needs context",
                "no_action" => "Conversation needs no action",
                _ => return Err("conversation work has an invalid outcome".to_owned()),
            };
            let summary = result["summary"]
                .as_str()
                .ok_or("conversation work lacks summary")?;
            format!("{prefix}; unverified draft summary: {summary}")
        }
        Some("coding_intake") => {
            let summary = result["summary"]
                .as_str()
                .ok_or("coding intake lacks summary")?;
            format!("Coding request triaged; no code changed; unverified draft summary: {summary}")
        }
        Some("work_plan") => {
            let decision = result["decision"]
                .as_str()
                .ok_or("work plan lacks decision")?;
            let change_set = result["change_set_id"]
                .as_str()
                .ok_or("work plan lacks change set")?;
            format!("Coding plan {decision}; no code changed: {change_set}")
        }
        None => {
            let state = result["state"].as_str().ok_or("PR work lacks state")?;
            let title = result["title"].as_str().ok_or("PR work lacks title")?;
            format!("Pull request {state}: {title}")
        }
        _ => return Err("completed work has an unsupported result kind".to_owned()),
    };
    let digest = work_result_digest(item).ok_or("completed work lacks a receipt")?;
    Ok(json!({
        "source_ref":format!("runx://assistant/work/{}", item.id),
        "source_digest":digest,
        "source_kind":"work_result",
        "priority":"medium",
        "summary":summary.chars().take(1000).collect::<String>(),
        "receipt":item.receipt,
        "work_id":item.id,
    }))
}

fn write_control(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    reason: &str,
) -> Result<(), String> {
    record.state.next_worker_due_unix_seconds = next_work_due(&record.state.work).unwrap_or(0);
    let event = json!({
        "type": "assistant.control.snapshot",
        "payload": {"state": record.state, "reason": reason}
    });
    let key = sha256_prefixed(
        &serde_json::to_vec(&json!([loaded.profile.instance_id, record.version, &event]))
            .map_err(|error| error.to_string())?,
    );
    let (output, _) = run_skill(
        loaded,
        workspace,
        "data-store",
        "append_event",
        json!({
            "data_source_ref": loaded.profile.data_source_ref,
            "resource": "assistant_control",
            "aggregate_id": loaded.profile.instance_id,
            "expected_version": record.version,
            "idempotency_key": format!("assistant:control:{key}"),
            "event": event,
            "observed_at": now_iso8601()
        }),
        false,
        None,
        false,
    )?;
    let data = result_data(&output, "data_operation_result")?;
    let after = data["after_version"]
        .as_u64()
        .ok_or("assistant control append lacks after_version")?;
    if after != record.version + 1 {
        return Err("assistant control append did not advance exactly one version".to_owned());
    }
    record.version = after;
    Ok(())
}

fn lock(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<File, String> {
    let dir = project_runx_dir(workspace).join("assistant");
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
    let dir = project_runx_dir(workspace).join("assistant");
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
            runx_runtime::notification_authority_status(
                &super::receipt_root(workspace),
                &notify.authority_id,
            )
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
        "pending_private_work_updates":pending_work_update_indices(&record.state.work).len(),
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
        "recent_work":record.state.work.iter().rev().filter(|item| item.status == "completed").take(5).collect::<Vec<_>>()
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
    let exact = serde_json::to_value(crate::skill::output::read_json_artifact(
        &project_runx_dir(workspace),
        &reference,
    )?)
    .map_err(|error| format!("decoding exact notification: {error}"))?;
    if exact["channel_locator"] != notification.channel_locator {
        return Err("pending notification destination differs from profile".to_owned());
    }
    let key = exact["idempotency_key"]
        .as_str()
        .ok_or("pending notification lacks idempotency key")?;
    if runx_runtime::notification_intent_has_reservation(
        &super::receipt_root(workspace),
        &notification.authority_id,
        key,
    )
    .map_err(|error| error.to_string())?
    {
        return Err(
            "notification has a native reservation; reconcile its provider outcome before discarding"
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
            projection
                .as_object_mut()
                .ok_or("assistant work item is not an object")?
                .remove("result_ref");
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
        None,
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
    if let Some(existing) = record
        .state
        .confirmed_memory
        .iter_mut()
        .find(|item| item.memory_id == memory_id)
    {
        existing.text = text;
        existing.confirmed_at = now_iso8601();
    } else {
        if record.state.confirmed_memory.len() >= 20 {
            return Err("assistant confirmed memory limit is 20 entries".to_owned());
        }
        record.state.confirmed_memory.push(ConfirmedMemory {
            memory_id: memory_id.to_owned(),
            text,
            confirmed_at: now_iso8601(),
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
        || record
            .state
            .work
            .iter()
            .any(|item| item.status == "pending")
    {
        return Err(
            "assistant memory cannot change during pending work or profile drift".to_owned(),
        );
    }
    Ok(())
}

fn reset_for_context_change(state: &mut Control) {
    state.next_due_unix_seconds = 0;
    state.last_handled_material_digest = None;
    state.last_handled_receipt = None;
    state.handled_occurrence_digests.clear();
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
        if record.state.pending_intent.is_some() || record.state.pending_turn.is_some() {
            return Err(
                "cannot resume a pinned assistant turn or delivery with changed bindings"
                    .to_owned(),
            );
        }
        if record
            .state
            .work
            .iter()
            .any(|item| item.status == "pending" && !item.runs.is_empty())
        {
            return Err(
                "cannot rebind an active work run to changed operator configuration".to_owned(),
            );
        }
    }
    record.state.paused = paused;
    if !paused {
        if record.state.profile_revision != loaded.revision {
            record.state.active_run_id = None;
            record.state.last_handled_material_digest = None;
            record.state.last_delivered_material_digest = None;
            record.state.handled_occurrence_digests.clear();
            record.state.scan_retry_from_first_page = true;
            record.state.last_fresh_scan_at.clear();
        }
        record.state.profile_revision = loaded.revision.clone();
        record.state.next_due_unix_seconds = 0;
        record.state.next_delivery_due_unix_seconds = 0;
        for item in &mut record.state.work {
            if item.status == "pending" {
                item.retry_after_unix_seconds = 0;
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
        write_control(
            loaded,
            workspace,
            &mut record,
            "restart_unpinned_legacy_turn",
        )?;
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
    let new_digests = unreviewed_digests(
        &record.state.handled_occurrence_digests,
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
    if refresh_failed_work_after_patch(&mut record.state.work, &loaded.skill_bindings) {
        write_control(loaded, workspace, &mut record, "work_patch_rebound")?;
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
    let result: Result<Value, SkillRunFailure> = match lane {
        Some(WorkerLane::Delivery) => {
            deliver_pending(loaded, workspace, &mut record).map_err(SkillRunFailure::from)
        }
        Some(WorkerLane::Assignment) => dispatch_pending_work(loaded, workspace, &mut record),
        None => unreachable!(),
    };
    if let Err(error) = &result {
        // A concurrent intake may have advanced the control version. Never
        // overwrite it with a stale worker snapshot; retry the same native
        // run identity after a bounded delay.
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
                .find(|item| item.id == work_id && item.status == "pending")
        {
            apply_assignment_failure(item, error, loaded.profile.min_check_minutes);
        }
        write_control(loaded, workspace, &mut current, "worker_held")?;
    }
    result.map_err(String::from)
}

fn refresh_failed_work_after_patch(
    work: &mut [WorkAssignment],
    bindings: &BTreeMap<String, String>,
) -> bool {
    let mut changed = false;
    for item in work {
        let Some(failed_skill) = item.failed_skill.as_ref() else {
            continue;
        };
        if item.status != "held" || item.receipt.is_none() {
            continue;
        }
        let Some(bound) = item.runs.get(failed_skill) else {
            continue;
        };
        let Some(current_digest) = bindings.get(failed_skill) else {
            continue;
        };
        if current_digest == &bound.package_digest {
            continue;
        }
        let failed_skill = failed_skill.clone();
        item.status = "pending".to_owned();
        item.retry_after_unix_seconds = 0;
        item.receipt = None;
        item.failed_skill = None;
        item.runs.remove(&failed_skill);
        changed = true;
    }
    changed
}

fn apply_assignment_failure(item: &mut WorkAssignment, error: &SkillRunFailure, minutes: u64) {
    if let Some(receipt) = &error.terminal_receipt {
        item.status = "held".to_owned();
        item.receipt = Some(receipt.clone());
        item.failed_skill = error.skill.clone();
    } else {
        item.retry_after_unix_seconds = scheduled_due(minutes);
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
                .filter_map(|index| record.state.work[*index].receipt.clone())
                .collect::<Vec<_>>();
            for index in due {
                record.state.work[index].private_update_status = Some("local_only".to_owned());
            }
            record.state.last_handled_material_digest = Some(material_digest);
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
    let observations = observations
        .into_iter()
        .filter(|item| {
            item["source_digest"].as_str().is_some_and(|digest| {
                !record
                    .state
                    .handled_occurrence_digests
                    .iter()
                    .any(|handled| handled == digest)
            })
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
        let reference = crate::skill::output::persist_json_artifact(
            &project_runx_dir(workspace),
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
        record.state.last_handled_material_digest = Some(material_digest);
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
    record.state.last_handled_material_digest = Some(material_digest);
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
            if result["kind"] == "conversation_review" {
                if result["summary"].as_str().is_none()
                    || result["status"].as_str().is_none()
                    || result["checked_at"].as_str().is_none()
                {
                    return Ok(None);
                }
                return Ok(Some(json!({
                    "source_ref":work.source_ref,
                    "source_digest":work.source_digest,
                    "route_id":work.route_id,
                    "target_ref":work.target_ref,
                    "kind":"conversation_review",
                    "summary":result["summary"],
                    "status":result["status"],
                    "source_complete":result["source_complete"],
                    "checked_at":result["checked_at"],
                    "receipt":receipt,
                })));
            }
            if result["kind"] == "coding_intake" {
                if result["summary"].as_str().is_none()
                    || result["recommended_lane"].as_str().is_none()
                    || result["checked_at"].as_str().is_none()
                {
                    return Ok(None);
                }
                return Ok(Some(json!({
                    "source_ref":work.source_ref,
                    "source_digest":work.source_digest,
                    "route_id":work.route_id,
                    "target_ref":work.target_ref,
                    "kind":"coding_intake",
                    "summary":result["summary"],
                    "recommended_lane":result["recommended_lane"],
                    "source_complete":result["source_complete"],
                    "checked_at":result["checked_at"],
                    "receipt":receipt,
                })));
            }
            if result["kind"] == "work_plan" {
                if result["change_set_id"].as_str().is_none()
                    || !matches!(result["decision"].as_str(), Some("ready" | "blocked"))
                {
                    return Ok(None);
                }
                return Ok(Some(json!({
                    "source_ref":work.source_ref,
                    "source_digest":work.source_digest,
                    "route_id":work.route_id,
                    "target_ref":work.target_ref,
                    "kind":"work_plan",
                    "decision":result["decision"],
                    "change_set_id":result["change_set_id"],
                    "checked_at":result["checked_at"],
                    "receipt":receipt,
                })));
            }
            if !matches!(result["state"].as_str(), Some("open" | "closed"))
                || result["title"].as_str().is_none()
                || result["checked_at"].as_str().is_none()
            {
                return Ok(None);
            }
            Ok(Some(json!({
                "source_ref":work.source_ref,
                "source_digest":work.source_digest,
                "route_id":work.route_id,
                "target_ref":work.target_ref,
                "state":result["state"],
                "title":result["title"],
                "checked_at":result["checked_at"],
                "receipt":receipt
            })))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let mut candidates = work_candidates(loaded, observations);
    candidates.retain(|candidate| {
        !record.state.work.iter().any(|work| {
            work.status == "completed"
                && candidate["source_digest"] == work.source_digest
                && candidate["route_id"] == work.route_id
                && candidate["target_ref"] == work.target_ref
        })
    });
    let (review, receipt) = run_skill(
        loaded,
        workspace,
        "attention-review",
        "review",
        json!({
            "objective":"Identify new attention. Set brief iff items is nonempty; idle requires empty items, recommended_action_ids and work_proposals. Use high, medium or low for item priority. Propose at most three exact optional checks from work_candidates; selected mail/chat items are reviewed through the configured conversation route by the host. Use work_results to avoid stale claims. This source-page scan is partial; do not claim complete coverage. Propose only allowed_action_ids; if empty, recommend none.",
            "as_of":now_iso8601(),
            "evidence":model_evidence,
            "user_context":{"charter":loaded.profile.charter,"confirmed_memory":record.state.confirmed_memory},
            "current_actions":dispositions,
            "allowed_action_ids":loaded.profile.allowed_action_ids,
            "work_candidates":candidates,
            "work_results":work_results,
            "min_check_minutes":loaded.profile.min_check_minutes,
            "max_check_minutes":loaded.profile.max_check_minutes
        }),
        true,
        None,
        false,
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
    let value = crate::skill::output::read_json_artifact(&project_runx_dir(workspace), &reference)?;
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
            record.state.last_fresh_scan_at.insert(
                source.id().to_owned(),
                if page.fetched_at_unix_seconds == 0 {
                    now_seconds()
                } else {
                    page.fetched_at_unix_seconds
                },
            );
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
        None,
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

fn parse_pr_target(target: &str) -> Option<(String, u64)> {
    let path = target.strip_prefix("https://github.com/")?;
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next()? != "pull" {
        return None;
    }
    let number = parts
        .next()?
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)?;
    if parts.next().is_some()
        || [owner, repo].iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || part.len() > 100
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        })
    {
        return None;
    }
    Some((format!("{owner}/{repo}"), number))
}

fn work_candidates(loaded: &LoadedProfile, observations: &[Value]) -> Vec<Value> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    for observation in observations {
        let Some(summary) = observation["summary"].as_str() else {
            continue;
        };
        for (start, _) in summary.match_indices("https://github.com/") {
            let raw = summary[start..]
                .split(|ch: char| ch.is_whitespace() || "<>|#?".contains(ch))
                .next()
                .unwrap_or("")
                .trim_end_matches(['.', ',', ';', ')', '/']);
            let Some((repository, number)) = parse_pr_target(raw) else {
                continue;
            };
            let target = format!("https://github.com/{repository}/pull/{number}");
            for route in &loaded.profile.work_routes {
                if route.kind == "github_pr_status"
                    && route
                        .repositories
                        .iter()
                        .any(|allowed| allowed == &repository)
                    && seen.insert((
                        observation["source_ref"].to_string(),
                        route.route_id.clone(),
                        target.clone(),
                    ))
                {
                    candidates.push(json!({
                        "source_ref":observation["source_ref"],
                        "source_digest":observation["source_digest"],
                        "route_id":route.route_id,
                        "target_ref":target
                    }));
                    if candidates.len() == 20 {
                        return candidates;
                    }
                }
            }
        }
    }
    candidates
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
        .ok_or("attention packet lacks work proposals")?;
    if proposals.len() > 3 {
        return Err("attention packet exceeds work proposal limit".to_owned());
    }
    let candidates = work_candidates(loaded, observations);
    let mut requested: Vec<(Value, bool)> = proposals
        .iter()
        .cloned()
        .map(|proposal| (proposal, false))
        .collect();
    if let Some(route) = loaded
        .profile
        .work_routes
        .iter()
        .find(|route| route.kind == "conversation_review")
    {
        for item in packet["items"]
            .as_array()
            .ok_or("attention packet lacks selected items")?
        {
            let Some(observation) = observations.iter().find(|observation| {
                observation["source_ref"] == item["source_ref"]
                    && observation["source_digest"] == item["source_digest"]
            }) else {
                return Err("selected conversation source is no longer present".to_owned());
            };
            if matches!(observation["source_kind"].as_str(), Some("chat" | "mail")) {
                requested.push((
                    json!({
                        "source_ref":observation["source_ref"],
                        "source_digest":observation["source_digest"],
                        "route_id":route.route_id,
                        "target_ref":observation["thread_locator"],
                    }),
                    true,
                ));
            }
        }
    }
    if requested.is_empty() {
        return Ok(());
    }
    let dispositions = current_dispositions(loaded, workspace, observations)?;
    let mut changed = false;
    for (proposal, selected_conversation) in &requested {
        let model_candidate = candidates.iter().any(|candidate| {
            candidate["source_ref"] == proposal["source_ref"]
                && candidate["source_digest"] == proposal["source_digest"]
                && candidate["route_id"] == proposal["route_id"]
                && candidate["target_ref"] == proposal["target_ref"]
        });
        let valid_route = loaded.profile.work_routes.iter().any(|route| {
            route.kind == "conversation_review" && route.route_id == proposal["route_id"]
        });
        if !(if *selected_conversation {
            valid_route
        } else {
            model_candidate
        }) {
            return Err("work proposal is no longer an exact configured candidate".to_owned());
        }
        let source_ref = proposal["source_ref"]
            .as_str()
            .ok_or("work proposal lacks source")?;
        if !dispositions
            .iter()
            .any(|item| item["source_ref"] == source_ref && item["disposition"] == "open")
        {
            return Err("work proposal source is not an open operator-inbox action".to_owned());
        }
        let observation = observations
            .iter()
            .find(|item| item["source_ref"] == source_ref)
            .ok_or("work proposal source is missing")?;
        if *selected_conversation
            && (!matches!(observation["source_kind"].as_str(), Some("chat" | "mail"))
                || observation["source_digest"] != proposal["source_digest"]
                || observation["thread_locator"] != proposal["target_ref"])
        {
            return Err("selected conversation no longer matches its source thread".to_owned());
        }
        let route_id = proposal["route_id"]
            .as_str()
            .ok_or("work proposal lacks route")?;
        let target_ref = proposal["target_ref"]
            .as_str()
            .ok_or("work proposal lacks target")?;
        let source_digest = proposal["source_digest"]
            .as_str()
            .ok_or("work proposal lacks digest")?;
        let id = sha256_prefixed(
            &serde_json::to_vec(&json!([
                loaded.profile.instance_id,
                source_digest,
                route_id,
                target_ref
            ]))
            .map_err(|error| error.to_string())?,
        );
        if record.state.work.iter().any(|item| item.id == id) {
            continue;
        }
        while record.state.work.len() >= 40 {
            let index = evictable_work_index(&record.state.work, None)
                .ok_or("assistant work ledger is full of pending assignments")?;
            record.state.work.remove(index);
        }
        record.state.work.push(WorkAssignment {
            id,
            parent_work_id: None,
            source_ref: source_ref.to_owned(),
            source_digest: source_digest.to_owned(),
            thread_locator: observation["thread_locator"]
                .as_str()
                .ok_or("work source lacks thread")?
                .to_owned(),
            route_id: route_id.to_owned(),
            target_ref: target_ref.to_owned(),
            status: "pending".to_owned(),
            retry_after_unix_seconds: 0,
            receipt: None,
            result_ref: None,
            runs: BTreeMap::new(),
            failed_skill: None,
            private_update_status: None,
        });
        changed = true;
    }
    if changed {
        write_control(loaded, workspace, record, "work_admitted")?;
    }
    Ok(())
}

fn dispatch_pending_work(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
) -> Result<Value, SkillRunFailure> {
    let index = due_work_index(&record.state.work, now_seconds()).ok_or("no due assistant work")?;
    let item = record.state.work[index].clone();
    let route = loaded
        .profile
        .work_routes
        .iter()
        .find(|route| route.route_id == item.route_id)
        .ok_or("pending work route is no longer configured")?;
    let dispositions = current_dispositions(
        loaded,
        workspace,
        &[json!({
            "source_ref":item.source_ref,"thread_locator":item.thread_locator
        })],
    )?;
    if !dispositions
        .iter()
        .any(|state| state["disposition"] == "open")
    {
        record.state.work[index].status = "held".to_owned();
        record.state.work[index].runs.clear();
        record.state.work[index].failed_skill = None;
        write_control(loaded, workspace, record, "work_source_closed")?;
        return Ok(
            json!({"status":"work_held","work_id":item.id,"reason":"source action is no longer open"}),
        );
    }
    let (result, receipt) = match route.kind.as_str() {
        "github_pr_status" => run_pr_status(loaded, workspace, record, index, &item, route)?,
        "conversation_review" => {
            let memory = record.state.confirmed_memory.clone();
            run_conversation_review(loaded, workspace, record, index, &item, &memory)?
        }
        "work_plan" => run_work_plan(loaded, workspace, record, index, &item)?,
        _ => return Err("pending work has an unsupported route".into()),
    };
    let child = if route.kind == "conversation_review" {
        plan_child_assignment(
            &loaded.profile.instance_id,
            loaded
                .profile
                .work_routes
                .iter()
                .find(|route| route.kind == "work_plan"),
            &item,
            &result,
        )
        .map_err(|error| {
            let skill = if result["kind"] == "coding_intake" {
                "issue-intake"
            } else {
                "conversation-review"
            };
            SkillRunFailure::sealed_validation(skill, &receipt, error)
        })?
    } else {
        None
    };
    record.state.work[index].status = "completed".to_owned();
    record.state.work[index].receipt = Some(receipt.clone());
    record.state.work[index].result_ref = Some(persist_work_result(workspace, &item, &result)?);
    record.state.work[index].runs.clear();
    record.state.work[index].failed_skill = None;
    record.state.next_due_unix_seconds = 0;
    if let Some(child) = child
        && !record.state.work.iter().any(|entry| entry.id == child.id)
    {
        while record.state.work.len() >= 40 {
            let evict = evictable_work_index(&record.state.work, Some(&item.id))
                .ok_or("assistant work ledger cannot retain planning parent")?;
            record.state.work.remove(evict);
        }
        record.state.work.push(child);
    }
    record.state.last_blocker = None;
    write_control(loaded, workspace, record, "work_completed")?;
    Ok(json!({"status":"work_completed","work_id":item.id,"receipt":receipt,"result":result}))
}

fn run_pr_status(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    index: usize,
    item: &WorkAssignment,
    route: &WorkRoute,
) -> Result<(Value, String), SkillRunFailure> {
    let (repository, number) = parse_pr_target(&item.target_ref)
        .ok_or("pending work target is not a canonical GitHub pull request")?;
    if !route
        .repositories
        .iter()
        .any(|allowed| allowed == &repository)
    {
        return Err("pending work target is outside its configured route".into());
    }
    let (bound, inputs, _) = bound_work_inputs(
        loaded,
        workspace,
        record,
        index,
        "github-sync",
        "run_assistant_work_",
        || {
            Ok((
                json!({"repo":repository,"resources":{"kind":"prs","refs":[format!("pulls/{number}")],"include_body":false}}),
                vec![],
            ))
        },
    )?;
    let (output, receipt) = run_skill_with_id(
        loaded,
        workspace,
        "github-sync",
        "pull",
        inputs,
        false,
        route.credential_profile.as_deref(),
        false,
        Some(&bound.run_id),
        Some(&bound.package_digest),
    )?;
    let checked = (|| -> Result<Value, String> {
        let result = &result_data(&output, "provider_operation")?["result"];
        let items = result["items"]
            .as_array()
            .ok_or("GitHub PR check lacks items")?;
        let pr = items
            .first()
            .ok_or("GitHub PR check returned no requested pull request")?;
        if result["repository"] != repository
            || items.len() != 1
            || pr["repository"] != repository
            || pr["number"]
                .as_u64()
                .or_else(|| pr["number"].as_str().and_then(|value| value.parse().ok()))
                != Some(number)
            || pr["url"] != item.target_ref
            || !matches!(pr["state"].as_str(), Some("open" | "closed"))
        {
            return Err("GitHub PR check lacks bounded repository readback".to_owned());
        }
        let bytes = serde_json::to_vec(result).map_err(|error| error.to_string())?;
        Ok(json!({
            "repository":repository,
            "pull_ref":format!("pulls/{number}"),
            "state":pr["state"],
            "title":pr["title"],
            "url":pr["url"],
            "checked_at":now_iso8601(),
            "result_digest":sha256_prefixed(&bytes)
        }))
    })()
    .map_err(|error| SkillRunFailure::sealed_validation("github-sync", &receipt, error))?;
    Ok((checked, receipt))
}

fn bounded_work_artifact(value: &Value) -> Result<(), String> {
    let size = serde_json::to_vec(value)
        .map_err(|error| error.to_string())?
        .len();
    if size > MAX_WORK_ARTIFACT_BYTES {
        return Err("assistant work artifact exceeds 32 KiB".to_owned());
    }
    Ok(())
}

fn plannable_change_set<'a>(item: &WorkAssignment, result: &'a Value) -> Option<&'a Value> {
    let change_set = result.get("change_set")?;
    if result["kind"] != "coding_intake"
        || result["source_complete"] != true
        || result["needs_human"] != false
        || result["recommended_lane"] != "work-plan"
        || result["commence_decision"] != "approve"
        || result["action_decision"] != "proceed_to_plan"
        || change_set["thread_locator"] != item.thread_locator
        || change_set["change_set_id"] != result["change_set_id"]
        || change_set["recommended_lane"] != "work-plan"
        || change_set["commence_decision"] != "approve"
        || change_set["action_decision"] != "proceed_to_plan"
    {
        return None;
    }
    Some(change_set)
}

fn plan_child_assignment(
    instance_id: &str,
    route: Option<&WorkRoute>,
    parent: &WorkAssignment,
    result: &Value,
) -> Result<Option<WorkAssignment>, String> {
    let Some(route) = route else {
        return Ok(None);
    };
    let Some(change_set) = plannable_change_set(parent, result) else {
        return Ok(None);
    };
    let target_ref = change_set["change_set_id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 200)
        .ok_or("plannable change set has no bounded identity")?;
    let id = sha256_prefixed(
        &serde_json::to_vec(&json!([instance_id, parent.id, route.route_id, target_ref]))
            .map_err(|error| error.to_string())?,
    );
    Ok(Some(WorkAssignment {
        id,
        parent_work_id: Some(parent.id.clone()),
        source_ref: parent.source_ref.clone(),
        source_digest: parent.source_digest.clone(),
        thread_locator: parent.thread_locator.clone(),
        route_id: route.route_id.clone(),
        target_ref: target_ref.to_owned(),
        status: "pending".to_owned(),
        retry_after_unix_seconds: 0,
        receipt: None,
        result_ref: None,
        runs: BTreeMap::new(),
        failed_skill: None,
        private_update_status: None,
    }))
}

fn run_work_plan(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    index: usize,
    item: &WorkAssignment,
) -> Result<(Value, String), SkillRunFailure> {
    let parent = item
        .parent_work_id
        .as_deref()
        .and_then(|id| record.state.work.iter().find(|entry| entry.id == id))
        .filter(|entry| {
            entry.status == "completed"
                && entry.receipt.is_some()
                && entry.source_ref == item.source_ref
                && entry.source_digest == item.source_digest
                && entry.thread_locator == item.thread_locator
        })
        .cloned()
        .ok_or("work plan lacks a completed source-intake parent")?;
    let source_result =
        read_work_result(workspace, &parent)?.ok_or("work plan parent has no intake result")?;
    let change_set = plannable_change_set(&parent, &source_result)
        .filter(|set| set["change_set_id"] == item.target_ref)
        .ok_or("work plan parent does not authorize this exact planning target")?;
    bounded_work_artifact(change_set)?;
    let objective = change_set["summary"]
        .as_str()
        .filter(|summary| !summary.is_empty() && summary.len() <= 2000)
        .ok_or("work plan parent lacks a bounded objective")?;
    let (bound, inputs, _) = bound_work_inputs(
        loaded,
        workspace,
        record,
        index,
        "work-plan",
        "run_assistant_plan_",
        || {
            Ok((
                json!({
                    "objective":objective,
                    "project_context":loaded.profile.charter,
                    "thread_locator":item.thread_locator,
                    "change_set":change_set,
                }),
                vec![],
            ))
        },
    )?;
    let (output, receipt) = run_skill_with_id(
        loaded,
        workspace,
        "work-plan",
        "work-plan",
        inputs.clone(),
        true,
        None,
        false,
        Some(&bound.run_id),
        Some(&bound.package_digest),
    )?;
    let plan = result_data(&output, "work_plan")
        .map_err(|error| SkillRunFailure::sealed_validation("work-plan", &receipt, error))?;
    if plan["change_set"] != inputs["change_set"]
        || plan["evidence"]["source_change_set_preserved"] != true
        || plan["evidence"]["source_thread_locator_preserved"] != true
        || !matches!(plan["decision"].as_str(), Some("ready" | "blocked"))
        || (plan["decision"] == "ready" && plan["validation"]["status"] != "pass")
    {
        return Err(SkillRunFailure::sealed_validation(
            "work-plan",
            &receipt,
            "work plan did not preserve the admitted parent change set",
        ));
    }
    bounded_work_artifact(plan)
        .map_err(|error| SkillRunFailure::sealed_validation("work-plan", &receipt, error))?;
    Ok((
        json!({
            "kind":"work_plan",
            "effect_status":"plan_only",
            "parent_work_id":parent.id,
            "parent_receipt":parent.receipt,
            "change_set_id":item.target_ref,
            "decision":plan["decision"],
            "plan":plan,
            "checked_at":now_iso8601(),
        }),
        receipt,
    ))
}

struct HydratedSource {
    title: String,
    body: String,
    context: Value,
    complete: bool,
    receipts: Vec<String>,
}

fn bounded_mail_context(grounding: &Value) -> Result<(Vec<Value>, bool), String> {
    let messages = grounding["messages"]
        .as_array()
        .filter(|messages| messages.len() <= 25)
        .ok_or("mail intake returned an unbounded grounding window")?;
    let mut complete = grounding["omitted_before"]["count"] == 0
        && grounding["returned_count"].as_u64() == Some(messages.len() as u64);
    let mut remaining = MAX_MAIL_CONTEXT_CHARS;
    let mut context = Vec::with_capacity(messages.len());
    for message in messages {
        let body = message["text_body"].as_str().unwrap_or("");
        let limit = remaining.min(4000);
        let excerpt = body.chars().take(limit).collect::<String>();
        remaining -= excerpt.chars().count();
        if body.is_empty()
            || body.chars().nth(limit).is_some()
            || message["body_truncated"] != false
            || message["occurred_at"].as_str().is_none()
            || message["direction"].as_str().is_none()
            || !matches!(message["attachments"].as_array(), Some(attachments) if attachments.is_empty())
        {
            complete = false;
        }
        context.push(json!({
            "message_ref":message["id"].to_string(),
            "direction":message["direction"].as_str().unwrap_or(""),
            "occurred_at":message["occurred_at"].as_str().unwrap_or(""),
            "text":excerpt,
        }));
    }
    Ok((context, complete))
}

fn run_conversation_review(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    record: &mut ControlRecord,
    index: usize,
    item: &WorkAssignment,
    memory: &[ConfirmedMemory],
) -> Result<(Value, String), SkillRunFailure> {
    if item.target_ref != item.thread_locator {
        return Err("source intake target differs from its recorded thread".into());
    }
    let mut operator_context = loaded.profile.charter.clone();
    for entry in memory {
        operator_context.push_str("\nConfirmed operator context: ");
        operator_context.push_str(&entry.text);
    }
    let (review_run, review_inputs, source_receipts) = bound_work_inputs(
        loaded,
        workspace,
        record,
        index,
        "conversation-review",
        "run_assistant_conversation_",
        || {
            let source = if item.source_ref.starts_with("slack://") {
                hydrate_slack_source(loaded, workspace, item)?
            } else if item.source_ref.starts_with("nitrosend://") {
                hydrate_mail_source(loaded, workspace, item)?
            } else {
                return Err("source intake has an unsupported source locator".to_owned());
            };
            Ok((
                json!({
                    "source_ref":item.source_ref,
                    "source_digest":item.source_digest,
                    "thread_locator":item.thread_locator,
                    "title":source.title,
                    "body":source.body,
                    "thread":source.context,
                    "source_complete":source.complete,
                    "operator_context":operator_context,
                }),
                source.receipts,
            ))
        },
    )?;
    if review_inputs["source_ref"] != item.source_ref
        || review_inputs["source_digest"] != item.source_digest
        || review_inputs["thread_locator"] != item.thread_locator
    {
        return Err("bound conversation inputs differ from the admitted source".into());
    }
    let source_complete = review_inputs["source_complete"] == true;
    let (review, review_receipt) = run_skill_with_id(
        loaded,
        workspace,
        "conversation-review",
        "review",
        review_inputs.clone(),
        true,
        None,
        false,
        Some(&review_run.run_id),
        Some(&review_run.package_digest),
    )?;
    let conversation = result_data(&review, "conversation_packet").map_err(|error| {
        SkillRunFailure::sealed_validation("conversation-review", &review_receipt, error)
    })?;
    if conversation["source_ref"] != item.source_ref
        || conversation["source_digest"] != item.source_digest
        || conversation["thread_locator"] != item.thread_locator
    {
        return Err(SkillRunFailure::sealed_validation(
            "conversation-review",
            &review_receipt,
            "conversation review is not bound to the admitted source",
        ));
    }
    let status = conversation["status"]
        .as_str()
        .ok_or("conversation review lacks a status")
        .map_err(|error| {
            SkillRunFailure::sealed_validation("conversation-review", &review_receipt, error)
        })?;
    if conversation["validation"]["status"] != "pass" && status != "needs_context" {
        return Err(SkillRunFailure::sealed_validation(
            "conversation-review",
            &review_receipt,
            "conversation review failed validation",
        ));
    }
    if status != "coding" {
        return Ok((
            json!({
                "kind":"conversation_review",
                "status":status,
                "summary":conversation["summary"],
                "reason":conversation["reason"],
                "draft_message":conversation["draft_message"],
                "follow_up_task":conversation["follow_up_task"],
                "follow_up_status":conversation["follow_up_status"],
                "effect_status":"draft_only",
                "source_complete":source_complete,
                "source_receipts":source_receipts,
                "checked_at":now_iso8601(),
            }),
            review_receipt,
        ));
    }
    if !source_complete {
        return Err(SkillRunFailure::sealed_validation(
            "conversation-review",
            &review_receipt,
            "incomplete conversation cannot enter coding intake",
        ));
    }
    let (intake_run, intake_inputs, _) = bound_work_inputs(
        loaded,
        workspace,
        record,
        index,
        "issue-intake",
        "run_assistant_coding_intake_",
        || {
            Ok((
                json!({
                    "thread_title":review_inputs["title"],
                    "thread_body":review_inputs["body"],
                    "thread_locator":item.thread_locator,
                    "thread":review_inputs["thread"],
                    "operator_context":review_inputs["operator_context"],
                }),
                source_receipts.clone(),
            ))
        },
    )?;
    let (output, receipt) = run_skill_with_id(
        loaded,
        workspace,
        "issue-intake",
        "intake",
        intake_inputs,
        true,
        None,
        false,
        Some(&intake_run.run_id),
        Some(&intake_run.package_digest),
    )?;
    let result = (|| -> Result<Value, String> {
    let report = output
        .pointer("/result/intake_report")
        .filter(|value| value.is_object())
        .ok_or("issue-intake did not return a validated intake report")?;
    let summary = report["summary"]
        .as_str()
        .ok_or("intake report lacks summary")?;
    let reply = report["suggested_reply"]
        .as_str()
        .ok_or("intake report lacks suggested reply")?;
    let lane = report["recommended_lane"]
        .as_str()
        .ok_or("intake report lacks a lane")?;
    let change_set = output
        .pointer("/result/change_set")
        .filter(|value| value.is_object())
        .ok_or("issue-intake did not return its parent change set")?;
    let change_set_id = change_set["change_set_id"]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 200)
        .ok_or("intake change set lacks a bounded identity")?;
    if change_set["thread_locator"] != item.thread_locator
        || change_set["recommended_lane"] != lane
        || change_set["category"] != report["category"]
        || change_set["severity"] != report["severity"]
    {
        return Err("intake change set differs from its source or decision".to_owned());
    }
    bounded_work_artifact(change_set)?;
    if summary.len() > 1000
        || reply.len() > 4000
        || !matches!(
            lane,
            "issue-to-pr" | "work-plan" | "reply-only" | "manual-review"
        )
    {
        return Err("intake report exceeds assistant's bounded handoff".to_owned());
    }
    let needs_human = report["needs_human"] == true || !source_complete;
    Ok(json!({
        "kind":"coding_intake",
        "effect_status":"draft_only",
        "conversation_receipt":review_receipt,
        "coding_request_quote":conversation["request_quote"],
        "summary":summary,
        "suggested_reply":reply,
        "recommended_lane":if source_complete { lane } else { "manual-review" },
        "change_set_id":change_set_id,
        "change_set":change_set,
        "commence_decision":if needs_human { json!("needs_human") } else { change_set["commence_decision"].clone() },
        "action_decision":if needs_human { json!("stop") } else { change_set["action_decision"].clone() },
        "needs_human":needs_human,
        "source_complete":source_complete,
        "source_receipts":source_receipts,
        "checked_at":now_iso8601(),
    }))
    })()
    .map_err(|error| SkillRunFailure::sealed_validation("issue-intake", &receipt, error))?;
    Ok((result, receipt))
}

fn hydrate_slack_source(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    item: &WorkAssignment,
) -> Result<HydratedSource, String> {
    let tenant = item
        .thread_locator
        .strip_prefix("slack://")
        .and_then(|rest| rest.split('/').next())
        .filter(|value| !value.is_empty())
        .ok_or("source intake has an invalid Slack thread locator")?;
    let action = read_action_state(loaded, workspace, &item.thread_locator)?
        .ok_or("Slack intake has no canonical operator-inbox action")?;
    let latest = &action["latest_message"];
    if action["status"] != "open"
        || action["thread_locator"] != item.thread_locator
        || latest["message_locator"] != item.source_ref
    {
        return Err("Slack intake source differs from its canonical open action".to_owned());
    }
    let mut cursor: Option<String> = None;
    let mut receipts = Vec::new();
    let mut messages_context = Vec::new();
    let mut source: Option<String> = None;
    let mut connected_subject = None;
    let mut complete = false;
    let mut truncated = false;
    for _ in 0..3 {
        let mut inputs = json!({"thread_locator":item.thread_locator,"limit":15});
        if let Some(value) = cursor.as_ref() {
            inputs["cursor"] = json!(value);
        }
        let (output, receipt) = run_skill(
            loaded,
            workspace,
            "slack",
            "read_thread",
            inputs,
            false,
            None,
            false,
        )?;
        let page = &result_data(&output, "provider_operation")?["result"];
        let observed_tenant = page["external_tenant_ref"].as_str();
        if page["thread_locator"] != item.thread_locator
            || !matches!(observed_tenant, Some(value) if value == tenant || value == format!("slack:workspace:{tenant}"))
            || action["external_tenant_ref"] != page["external_tenant_ref"]
        {
            return Err(
                "Slack intake readback differs from the recorded thread or tenant".to_owned(),
            );
        }
        let subject = page["connected_subject_ref"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or("Slack intake lacks connected subject")?;
        if connected_subject
            .as_deref()
            .is_some_and(|previous| previous != subject)
        {
            return Err("Slack intake changed connected subject during hydration".to_owned());
        }
        connected_subject = Some(subject.to_owned());
        if action["connected_subject_ref"] != subject {
            return Err("Slack intake action differs from the connected subject".to_owned());
        }
        let messages = page["messages"]
            .as_array()
            .filter(|messages| messages.len() <= 15)
            .ok_or("Slack intake returned an unbounded thread page")?;
        let channel = item
            .thread_locator
            .rsplit_once('/')
            .map(|(channel, _)| channel)
            .ok_or("Slack intake target lacks a channel")?;
        for message in messages {
            if !message["message_locator"]
                .as_str()
                .is_some_and(|locator| locator.starts_with(&format!("{channel}/")))
            {
                return Err("Slack intake page contains another channel".to_owned());
            }
            if let Some(preview) = message["preview"].as_str() {
                if preview.chars().nth(1000).is_some()
                    || messages_context.len() >= 30
                    || message["occurred_at"].as_str().is_none()
                    || message["author"]["external_id"].as_str().is_none()
                {
                    truncated = true;
                }
                if messages_context.len() < 30 {
                    messages_context.push(json!({
                        "message_ref":message["message_locator"],
                        "occurred_at":message["occurred_at"].as_str().unwrap_or(""),
                        "author_ref":message["author"]["external_id"].as_str().unwrap_or(""),
                        "is_self":message["author"]["external_id"] == subject,
                        "text":preview.chars().take(1000).collect::<String>(),
                    }));
                }
            } else {
                truncated = true;
            }
            if message["message_locator"] == item.source_ref {
                // Search and thread previews can render markup differently.
                // The queue digest identifies the selected occurrence; the
                // fresh read is bound by tenant, locator, time, and author.
                if message_for_queue(message)?["occurred_at"] != latest["occurred_at"] {
                    return Err("Slack intake source occurrence time changed".to_owned());
                }
                if message["author"]["external_id"] != action["requester"]["external_id"]
                    || message["author"]["external_id"] == subject
                {
                    return Err("Slack intake source author changed or is self-authored".to_owned());
                }
                source = message["preview"]
                    .as_str()
                    .map(|preview| preview.chars().take(1000).collect());
            }
        }
        receipts.push(receipt);
        cursor = page["next_cursor"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        complete = cursor.is_none() && !truncated;
        if cursor.is_none() {
            break;
        }
    }
    let body = source.ok_or("Slack intake source was not found in three bounded thread pages")?;
    let title = body
        .lines()
        .next()
        .unwrap_or("Slack request")
        .chars()
        .take(200)
        .collect();
    Ok(HydratedSource {
        title,
        body,
        context: json!({"thread_locator":item.thread_locator,"source_ref":item.source_ref,"source_digest":item.source_digest,"messages":messages_context,"coverage_incomplete":!complete}),
        complete,
        receipts,
    })
}

fn hydrate_mail_source(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
    item: &WorkAssignment,
) -> Result<HydratedSource, String> {
    let rest = item
        .thread_locator
        .strip_prefix("nitrosend://")
        .ok_or("source intake has an invalid mail thread locator")?;
    let (brand_sid, conversation) = rest
        .split_once('/')
        .ok_or("source intake has an invalid mail conversation")?;
    let conversation_id = conversation
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or("source intake has an invalid mail conversation id")?;
    let source = loaded
        .profile
        .sources
        .iter()
        .find_map(|source| match source {
            SourceProfile::NitrosendInbox {
                brand_sid: configured,
                credential_profile,
                ..
            } if configured == brand_sid => Some(credential_profile.as_deref()),
            _ => None,
        })
        .ok_or("mail intake brand is outside configured sources")?;
    let (output, receipt) = run_skill(
        loaded,
        workspace,
        "nitrosend",
        "inbox",
        json!({"command":"get_thread","arguments":{"conversation_id":conversation_id},"brand_sid":brand_sid}),
        false,
        source,
        false,
    )?;
    let thread = nitrosend_read_result(&output)?;
    let detail = &thread["thread"];
    let row = json!({
        "conversation_id":conversation_id,
        "last_message_at":detail["last_message_at"],
        "updated_at":detail["updated_at"],
        "subject":detail["subject"],
        "preview":detail["preview"],
    });
    let (observation, _) = normalize_mail_observation(brand_sid, &row, thread)
        .ok_or("mail intake thread cannot be grounded to its latest message")?;
    if observation["source_ref"] != item.source_ref
        || observation["source_digest"] != item.source_digest
    {
        return Err("mail intake source changed after observation".to_owned());
    }
    let message_id = item
        .source_ref
        .rsplit('/')
        .next()
        .and_then(|id| id.parse::<u64>().ok())
        .ok_or("mail intake has an invalid source message")?;
    let grounding = &thread["thread_grounding"];
    let (messages, context_complete) = bounded_mail_context(grounding)?;
    let message = grounding["messages"]
        .as_array()
        .and_then(|messages| messages.iter().find(|message| message["id"] == message_id))
        .ok_or("mail intake source is absent from grounded thread")?;
    let body = message["text_body"]
        .as_str()
        .filter(|body| !body.trim().is_empty())
        .ok_or("mail intake source has no grounded body")?;
    let complete =
        context_complete && message["body_truncated"] == false && body.chars().nth(4000).is_none();
    Ok(HydratedSource {
        title: detail["subject"]
            .as_str()
            .unwrap_or("Mail request")
            .chars()
            .take(200)
            .collect(),
        body: body.chars().take(4000).collect(),
        context: json!({"thread_locator":item.thread_locator,"source_ref":item.source_ref,"source_digest":item.source_digest,"messages":messages,"omitted_before_count":grounding["omitted_before"]["count"],"coverage_incomplete":!complete}),
        complete,
        receipts: vec![receipt],
    })
}

fn render_notification_plain_text(text: &str) -> String {
    let mut rendered = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find('<') {
        rendered.push_str(&remaining[..start]);
        let token = &remaining[start + 1..];
        if let Some(end) = token.find('>') {
            let inner = &token[..end];
            let display = inner
                .split_once('|')
                .map(|(_, label)| label)
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| {
                    if inner.starts_with('@') {
                        "person"
                    } else if inner.starts_with('!') {
                        "group"
                    } else {
                        "link"
                    }
                });
            rendered.push_str(display);
            remaining = &token[end + 1..];
        } else {
            rendered.push('(');
            remaining = token;
        }
    }
    rendered.push_str(remaining);
    rendered
        .replace('&', " and ")
        .replace('>', ")")
        .replace('`', "'")
        .replace("@channel", "＠channel")
        .replace("@here", "＠here")
        .replace("@everyone", "＠everyone")
}

fn private_notification_text(
    packet: &Value,
    max_bytes: usize,
    coverage_incomplete: bool,
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
            let summary = render_notification_plain_text(
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
        if compact.len() <= max_bytes && !has_active_notification_mention(&compact) {
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

fn has_active_notification_mention(text: &str) -> bool {
    ["<", ">", "&", "<@", "<!", "@channel", "@here", "@everyone"]
        .iter()
        .any(|needle| text.contains(needle))
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
    let text =
        private_notification_text(packet, notify.max_text_bytes as usize, coverage_incomplete)?;
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
    let work_ids = packet["items"]
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
                .find(|work| {
                    work.id == id
                        && work.status == "completed"
                        && work.private_update_status.is_none()
                })
                .ok_or("work update is not a pending completed assignment")?;
            if item["source_digest"].as_str() != work_result_digest(assignment).as_deref()
                || item["receipt"].as_str() != assignment.receipt.as_deref()
            {
                return Err("work update differs from its sealed receipt".to_owned());
            }
            Ok(id.to_owned())
        })
        .collect::<Result<Vec<_>, String>>()?;
    if work_ids.len() > 3 || work_ids.iter().collect::<BTreeSet<_>>().len() != work_ids.len() {
        return Err("work update selection is invalid".to_owned());
    }
    let delivery_package_digest = loaded
        .skill_bindings
        .get("slack-notify")
        .ok_or("assistant has no inspected slack-notify binding")?;
    pinned_skill_path(
        workspace,
        &loaded.profile.skills_root.join("slack-notify"),
        "slack-notify",
        delivery_package_digest,
    )?;
    let (planned, _) = run_skill_with_id(
        loaded,
        workspace,
        "slack-notify",
        "plan_exact",
        json!({
            "channel":notify.channel_locator,
            "content":{"message":text},
            "principal":notify.principal_ref
        }),
        false,
        None,
        false,
        None,
        Some(delivery_package_digest),
    )
    .map_err(String::from)?;
    let plan = result_data(&planned, "notify_plan")?;
    if plan["decision"] != "ready_for_provider" {
        return Err("slack-notify did not admit the exact private brief".to_owned());
    }
    let logical = record
        .state
        .active_run_id
        .as_ref()
        .ok_or("assistant run has no identity")?;
    let idempotency_key = notification_uuid(logical)?;
    let exact = json!({
        "notification_plan":plan,
        "channel_locator":notify.channel_locator,
        "text":text,
        "idempotency_key":idempotency_key
    });
    let exact_json: runx_contracts::JsonValue = serde_json::from_value(exact)
        .map_err(|error| format!("encoding exact notification intent: {error}"))?;
    let artifact = crate::skill::output::persist_json_artifact(
        &project_runx_dir(workspace),
        logical,
        "assistant",
        "notification-intent",
        &exact_json,
    )?;
    record.state.pending_intent = Some(PendingIntent {
        artifact_ref: serde_json::to_value(artifact)
            .map_err(|error| format!("encoding notification artifact reference: {error}"))?,
        material_digest: material_digest.to_owned(),
        logical_run_id: logical.clone(),
        selected_digests,
        delivery_package_digest: delivery_package_digest.clone(),
        work_ids,
    });
    record.state.pending_turn = None;
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
    let reference = serde_json::from_value(pending.artifact_ref)
        .map_err(|error| format!("decoding notification artifact reference: {error}"))?;
    let exact = serde_json::to_value(crate::skill::output::read_json_artifact(
        &project_runx_dir(workspace),
        &reference,
    )?)
    .map_err(|error| format!("decoding notification intent: {error}"))?;
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
    admit_private_text(loaded, workspace, exact_text)?;
    let delivery_run_id = format!(
        "run_assistant_deliver_{}",
        sha256_prefixed(pending.logical_run_id.as_bytes()).trim_start_matches("sha256:")
    );
    let (delivered, receipt) = run_skill_with_id(
        loaded,
        workspace,
        "slack-notify",
        "deliver",
        exact,
        false,
        None,
        true,
        Some(&delivery_run_id),
        (!pending.delivery_package_digest.is_empty())
            .then_some(pending.delivery_package_digest.as_str()),
    )?;
    let readback = result_data(&delivered, "provider_operation")
        .or_else(|_| result_data(&delivered, "notify_delivery"))?;
    if !verified_notification_readback(readback, &expected_digest, &expected_channel) {
        return Err("notification delivery lacks verified provider readback".to_owned());
    }
    record.state.last_delivered_material_digest = Some(pending.material_digest.clone());
    record.state.last_handled_material_digest = Some(pending.material_digest);
    record.state.last_handled_receipt = Some(receipt.clone());
    record.state.last_blocker = None;
    record.state.scan_retry_from_first_page = false;
    let work_digests = pending
        .work_ids
        .iter()
        .filter_map(|id| {
            record
                .state
                .work
                .iter()
                .find(|work| work.id == *id)
                .and_then(work_result_digest)
        })
        .collect::<BTreeSet<_>>();
    for digest in &pending.selected_digests {
        if !work_digests.contains(digest) {
            mark_handled(&mut record.state, digest);
        }
    }
    for id in &pending.work_ids {
        let assignment = record
            .state
            .work
            .iter_mut()
            .find(|work| {
                work.id == *id && work.status == "completed" && work.private_update_status.is_none()
            })
            .ok_or("delivered work update lacks pending assignment")?;
        if !pending
            .selected_digests
            .contains(&work_result_digest(assignment).ok_or("delivered work lacks receipt")?)
        {
            return Err("delivered work update is not bound to selected digest".to_owned());
        }
        assignment.private_update_status = Some("delivered".to_owned());
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

fn verified_notification_readback(readback: &Value, digest: &str, channel: &str) -> bool {
    readback["status"] == "success"
        && readback["operation"] == "channel.post.read"
        && readback["finality"] == "confirmed"
        && readback["result"]["content_digest"] == digest
        && readback["result"]["conversation_locator"] == channel
        && readback["result"]["message_locator"]
            .as_str()
            .is_some_and(|locator| locator.starts_with(&format!("{channel}/")))
        && readback["readback_ref"]
            .as_str()
            .is_some_and(|reference| reference.starts_with("runx:provider-readback:sha256:"))
}

fn mark_handled(state: &mut Control, digest: &str) {
    if !state
        .handled_occurrence_digests
        .iter()
        .any(|item| item == digest)
    {
        state.handled_occurrence_digests.push(digest.to_owned());
        if state.handled_occurrence_digests.len() > 500 {
            state.handled_occurrence_digests.remove(0);
        }
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
            let artifact_ref = crate::skill::output::persist_json_artifact(
                &project_runx_dir(workspace),
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
                record_observation(loaded, workspace, message)?;
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
    let value = crate::skill::output::read_json_artifact(&project_runx_dir(workspace), &reference)?;
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
    let (name, runner, inputs, credential) = match source {
        SourceProfile::SlackMentions { query, .. } => {
            let mut query = query.clone();
            if let Some(next) = &continuation {
                query["cursor"] = json!(next.cursor);
            }
            ("slack", "search", json!({"query":query}), None)
        }
        SourceProfile::NitrosendInbox {
            brand_sid,
            arguments,
            credential_profile,
            ..
        } => {
            let mut arguments = arguments.clone();
            if let Some(next) = &continuation {
                let page = next
                    .cursor
                    .strip_prefix("page:")
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|page| *page >= 2 && *page <= 10_000)
                    .ok_or("Nitrosend scan continuation is invalid")?;
                arguments["page"] = json!(page);
            }
            (
                "nitrosend",
                "inbox",
                json!({"command":"list_mailbox","arguments":arguments,"brand_sid":brand_sid}),
                credential_profile.as_deref(),
            )
        }
    };
    let (output, _) = run_skill(
        loaded, workspace, name, runner, inputs, false, credential, false,
    )?;
    let page = match source {
        SourceProfile::SlackMentions { .. } => {
            &result_data(&output, "provider_operation")?["result"]
        }
        SourceProfile::NitrosendInbox { .. } => nitrosend_read_result(&output)?,
    };
    let items = match source {
        SourceProfile::SlackMentions { .. } => page["messages"].as_array(),
        SourceProfile::NitrosendInbox { .. } => page["items"].as_array(),
    }
    .ok_or("source result lacks bounded message page")?;
    let expected_limit = match source {
        SourceProfile::SlackMentions { query, .. } => query["limit"].as_u64().unwrap_or(20),
        SourceProfile::NitrosendInbox { arguments, .. } => arguments["per"].as_u64().unwrap_or(0),
    };
    if items.len() > expected_limit as usize || items.len() > MAX_OBSERVATIONS {
        return Err("source page exceeds assistant's bounded observation limit".to_owned());
    }
    let mut page_messages = Vec::new();
    let mut observations = Vec::new();
    for item in items {
        let (evidence, message) = match source {
            SourceProfile::SlackMentions { .. } => normalize_slack_observation(item),
            SourceProfile::NitrosendInbox {
                brand_sid,
                credential_profile,
                ..
            } => {
                let conversation_id = item["conversation_id"]
                    .as_u64()
                    .filter(|id| *id > 0)
                    .ok_or("Nitrosend mailbox row lacks a positive conversation id")?;
                let (output, _) = run_skill(
                    loaded,
                    workspace,
                    "nitrosend",
                    "inbox",
                    json!({"command":"get_thread","arguments":{"conversation_id":conversation_id},"brand_sid":brand_sid}),
                    false,
                    credential_profile.as_deref(),
                    false,
                )?;
                let thread = nitrosend_read_result(&output)?;
                normalize_mail_observation(brand_sid, item, thread)
            }
        }
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
        next_cursor: page_cursor(page),
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
        None,
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

fn normalize_slack_observation(item: &Value) -> Option<(Value, Value)> {
    let locator = item["message_locator"].as_str()?.to_owned();
    let thread = item["thread_locator"].as_str()?.to_owned();
    let occurred = item["occurred_at"].as_str()?.to_owned();
    let summary = item["preview"]
        .as_str()?
        .chars()
        .take(1000)
        .collect::<String>();
    observation("chat", thread, locator, occurred, summary, item.clone())
}

fn normalize_mail_observation(
    brand_sid: &str,
    row: &Value,
    thread_result: &Value,
) -> Option<(Value, Value)> {
    let conversation_id = row["conversation_id"].as_u64().filter(|id| *id > 0)?;
    let thread = &thread_result["thread"];
    let grounding = &thread_result["thread_grounding"];
    if thread_result["status"] != "ok"
        || thread_result["purpose"] != "read"
        || thread["conversation_id"].as_u64() != Some(conversation_id)
        || grounding["conversation_id"].as_u64() != Some(conversation_id)
    {
        return None;
    }
    let row_time = DateTime::parse_from_rfc3339(row["last_message_at"].as_str()?).ok()?;
    let thread_time = DateTime::parse_from_rfc3339(thread["last_message_at"].as_str()?).ok()?;
    let row_updated = DateTime::parse_from_rfc3339(row["updated_at"].as_str()?).ok()?;
    let thread_updated = DateTime::parse_from_rfc3339(thread["updated_at"].as_str()?).ok()?;
    if row_time != thread_time
        || row_updated != thread_updated
        || row["subject"] != thread["subject"]
        || row["preview"] != thread["preview"]
    {
        return None;
    }
    let messages = grounding["messages"].as_array()?;
    let latest = messages
        .iter()
        .filter(|message| {
            message["id"].as_u64().is_some_and(|id| id > 0)
                && message["occurred_at"]
                    .as_str()
                    .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
                    .is_some_and(|time| time == row_time)
        })
        .max_by_key(|message| message["id"].as_u64())?;
    let direction = latest["direction"].as_str()?;
    if direction != "inbound" && direction != "outbound" {
        return None;
    }
    if thread["latest_activity"]["direction"].as_str()? != direction {
        return None;
    }
    // The queue's ISO parser accepts millisecond precision; the provider can
    // return microseconds. The provider message ID remains the occurrence key.
    let occurred = row_time
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let message_id = latest["id"].as_u64()?;
    let thread_locator = format!("nitrosend://{brand_sid}/{conversation_id}");
    let locator = format!("{thread_locator}/messages/{message_id}");
    let connected_subject = format!("nitrosend:{brand_sid}");
    let author = if direction == "inbound" {
        latest["sender_address"].as_str()?.to_owned()
    } else {
        connected_subject.clone()
    };
    let summary = format!(
        "{} — {}",
        row["subject"].as_str().unwrap_or("Mail"),
        row["preview"].as_str().unwrap_or("")
    )
    .chars()
    .take(1000)
    .collect::<String>();
    let message = json!({
        "provider":"nitrosend",
        "external_tenant_ref":brand_sid,
        "connected_subject_ref":connected_subject,
        "message_locator":locator,
        "thread_locator":thread_locator,
        "author":{"external_id":author},
        "conversation":{"external_id":conversation_id.to_string(),"type":"mail"},
        "occurred_at":occurred,
        "preview":summary,
        "context":[]
    });
    observation("mail", thread_locator, locator, occurred, summary, message)
}

fn observation(
    kind: &str,
    thread: String,
    locator: String,
    occurred: String,
    summary: String,
    message: Value,
) -> Option<(Value, Value)> {
    if summary.trim().is_empty() {
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
        None,
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
            "triage":{"kind":match message["provider"].as_str() {
                Some("slack") => "direct_mention", _ => "operator_selected_query"
            },"reason":"Observed in a configured personal source."}
        }),
        false,
        None,
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
        None,
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
                "provider":match source { SourceProfile::SlackMentions { .. } => "slack", _ => "nitrosend" },
                "query_digest":query_digest,
                "page_index":page.page_index,
                "status":if page.next_cursor.is_some() {"truncated"} else {"complete"},
                "next_cursor":page.next_cursor
            },
            "messages":messages
        }),
        false,
        None,
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

fn page_cursor(page: &Value) -> Option<String> {
    page["next_cursor"]
        .as_str()
        .or_else(|| page["pagination"]["next_cursor"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            let current = page["pagination"]["page"].as_u64()?;
            let total = page["pagination"]["total_pages"].as_u64()?;
            (current < total).then(|| format!("page:{}", current + 1))
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{
        DeferredTurn, PagePacket, PendingTurn, ScanLane, SkillRunFailure, WorkAssignment,
        WorkerLane, apply_assignment_failure, bounded_mail_context, bounded_work_artifact,
        due_work_index, due_worker_lane, evictable_work_index, fresh_scan_due, message_for_queue,
        next_work_due, normalize_mail_observation, notification_uuid, package_digest, page_cursor,
        parse_pr_target, persist_work_result, pinned_skill_path, plan_child_assignment,
        plannable_change_set, private_notification_text, private_text_is_safe, quiet_hour,
        read_work_result, refresh_failed_work_after_patch, scan_continuation_from_value,
        source_scan_digest, unreviewed_digests, verified_notification_readback,
    };
    use crate::assistant::{QuietHours, SourceProfile, WorkRoute};

    #[test]
    fn fresh_page_keeps_the_backlog_scan_head_and_old_page_binding() -> Result<(), String> {
        let source = SourceProfile::SlackMentions {
            source_id: "mentions".to_owned(),
            query: json!({"mentions_connected_subject":true,"limit":3}),
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
        let old_page: PagePacket = serde_json::from_value(json!({
            "source_id":"mentions","query_digest":backlog,"scan_id":"old",
            "page_index":2,"next_cursor":"cursor","messages":[],"observations":[]
        }))
        .map_err(|error| error.to_string())?;
        assert_eq!(old_page.scan_lane, ScanLane::Backlog);
        assert_eq!(old_page.fetched_at_unix_seconds, 0);
        assert!(!fresh_scan_due(Some(100), 100 + 29 * 60, 15));
        assert!(fresh_scan_due(Some(100), 100 + 30 * 60, 15));
        assert!(fresh_scan_due(None, 100, 15));
        Ok(())
    }

    #[test]
    fn sealed_failed_work_is_held_and_transient_work_can_retry() -> Result<(), String> {
        let mut work: WorkAssignment = serde_json::from_value(json!({
            "id":"sha256:assignment", "source_ref":"slack://message", "source_digest":"sha256:source",
            "thread_locator":"slack://thread", "route_id":"review",
            "target_ref":"slack://thread", "status":"pending",
            "receipt":null, "result_ref":null
        }))
        .map_err(|error| error.to_string())?;
        apply_assignment_failure(
            &mut work,
            &SkillRunFailure {
                message: "model failed".to_owned(),
                terminal_receipt: Some("sha256:failed".to_owned()),
                skill: Some("issue-intake".to_owned()),
            },
            15,
        );
        assert_eq!(work.status, "held");
        assert_eq!(work.receipt.as_deref(), Some("sha256:failed"));
        work.runs.insert(
            "issue-intake".to_owned(),
            super::BoundWorkRun {
                run_id: "run_old".to_owned(),
                package_digest: "sha256:old".to_owned(),
                input_ref: json!(null),
            },
        );
        work.runs.insert(
            "conversation-review".to_owned(),
            super::BoundWorkRun {
                run_id: "run_completed_phase".to_owned(),
                package_digest: "sha256:completed".to_owned(),
                input_ref: json!({"artifact":"completed-phase-inputs"}),
            },
        );
        let current = BTreeMap::from([("issue-intake".to_owned(), "sha256:old".to_owned())]);
        assert!(!refresh_failed_work_after_patch(
            std::slice::from_mut(&mut work),
            &current
        ));
        let unrelated_patch = BTreeMap::from([
            ("issue-intake".to_owned(), "sha256:old".to_owned()),
            ("conversation-review".to_owned(), "sha256:new".to_owned()),
        ]);
        assert!(!refresh_failed_work_after_patch(
            std::slice::from_mut(&mut work),
            &unrelated_patch
        ));
        let patched = BTreeMap::from([("issue-intake".to_owned(), "sha256:new".to_owned())]);
        assert!(refresh_failed_work_after_patch(
            std::slice::from_mut(&mut work),
            &patched
        ));
        assert_eq!(work.status, "pending");
        assert!(!work.runs.contains_key("issue-intake"));
        assert_eq!(
            work.runs["conversation-review"].run_id,
            "run_completed_phase"
        );
        assert!(work.receipt.is_none());

        apply_assignment_failure(
            &mut work,
            &SkillRunFailure {
                message: "temporary transport error".to_owned(),
                terminal_receipt: None,
                skill: None,
            },
            15,
        );
        assert_eq!(work.status, "pending");
        assert!(work.retry_after_unix_seconds > 0);
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
        for name in ["send-as", "slack-notify"] {
            let destination = source.join(name);
            std::fs::create_dir(&destination).map_err(|error| error.to_string())?;
            super::copy_skill_files(&official.join(name), &destination)?;
        }
        let workspace = runx_runtime::WorkspaceEnv::load_process(root.clone())
            .map_err(|error| error.to_string())?;
        let skill = source.join("slack-notify");
        let binding = |path: &std::path::Path| -> Result<String, String> {
            let inspected = runx_runtime::inspect_skill_package(path, None, None)
                .map_err(|error| error.to_string())?;
            super::super::skill_binding_digest(&super::super::inspected_skill_bindings(&inspected)?)
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
        let mut item: WorkAssignment = serde_json::from_value(json!({
            "id":"work-1", "source_ref":"slack://message", "source_digest":"digest",
            "thread_locator":"slack://thread", "route_id":"intake",
            "target_ref":"slack://thread", "status":"completed",
            "receipt":format!("sha256:{}", "a".repeat(64)), "result_ref":null
        }))
        .map_err(|error| error.to_string())?;
        let result = json!({"kind":"coding_intake","summary":"Check the bounded request","change_set":{"summary":"private context"}});
        item.result_ref = Some(persist_work_result(&workspace, &item, &result)?);
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
        )?;
        assert!(text.contains("WORK: Coding request triaged; no code changed"));
        assert!(text.contains(&format!("receipt sha256:{}", "a".repeat(64))));
        assert!(!text.contains(&format!(
            "source {}",
            update["source_digest"].as_str().unwrap_or_default()
        )));
        assert!(text.contains("runx assistant work"));
        item.private_update_status = Some("local_only".to_owned());
        assert!(super::pending_work_update_indices(&[item.clone()]).is_empty());
        assert_eq!(evictable_work_index(&[item.clone()], None), Some(0));
        item.private_update_status = Some("delivered".to_owned());
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
        let old: WorkAssignment = serde_json::from_value(json!({
            "id":"first", "source_ref":"slack://one", "source_digest":"digest",
            "thread_locator":"slack://thread", "route_id":"intake",
            "target_ref":"slack://thread", "status":"pending",
            "receipt":null, "result_ref":null
        }))
        .map_err(|error| error.to_string())?;
        assert_eq!(old.retry_after_unix_seconds, 0);
        let mut work = vec![
            old.clone(),
            WorkAssignment {
                id: "second".to_owned(),
                ..old
            },
        ];
        work[0].retry_after_unix_seconds = 120;
        assert_eq!(next_work_due(&work), Some(0));
        assert_eq!(due_work_index(&work, 60), Some(1));
        work[1].status = "completed".to_owned();
        assert_eq!(next_work_due(&work), Some(120));
        assert_eq!(due_work_index(&work, 60), None);
        assert_eq!(due_work_index(&work, 120), Some(0));
        Ok(())
    }

    #[test]
    fn planning_requires_exact_completed_intake_authority() -> Result<(), String> {
        let parent: WorkAssignment = serde_json::from_value(json!({
            "id":"intake", "source_ref":"slack://message", "source_digest":"digest",
            "thread_locator":"slack://thread", "route_id":"intake",
            "target_ref":"slack://thread", "status":"completed",
            "receipt":"sha256:receipt", "result_ref":null
        }))
        .map_err(|error| error.to_string())?;
        let approved = json!({
            "kind":"coding_intake", "source_complete":true, "needs_human":false,
            "recommended_lane":"work-plan", "commence_decision":"approve",
            "action_decision":"proceed_to_plan", "change_set_id":"change-1",
            "change_set":{
                "change_set_id":"change-1", "thread_locator":"slack://thread",
                "recommended_lane":"work-plan", "commence_decision":"approve",
                "action_decision":"proceed_to_plan", "summary":"Plan a bounded fix"
            }
        });
        assert_eq!(
            plannable_change_set(&parent, &approved),
            Some(&approved["change_set"])
        );
        let route = WorkRoute {
            route_id: "planning".to_owned(),
            kind: "work_plan".to_owned(),
            repositories: Vec::new(),
            credential_profile: None,
        };
        let planned = plan_child_assignment("test-instance", Some(&route), &parent, &approved)?
            .ok_or("approved intake did not enqueue planning")?;
        assert_eq!(planned.parent_work_id.as_deref(), Some("intake"));
        assert_eq!(planned.target_ref, "change-1");
        assert_eq!(planned.status, "pending");
        assert_eq!(
            planned.id,
            plan_child_assignment("test-instance", Some(&route), &parent, &approved)?
                .ok_or("repeated planning proposal vanished")?
                .id
        );
        assert!(plan_child_assignment("test-instance", None, &parent, &approved)?.is_none());
        for (path, denied) in [
            ("source_complete", json!(false)),
            ("needs_human", json!(true)),
            ("action_decision", json!("proceed_to_build")),
            ("recommended_lane", json!("issue-to-pr")),
            ("change_set_id", json!("another")),
        ] {
            let mut candidate = approved.clone();
            candidate[path] = denied;
            assert!(plannable_change_set(&parent, &candidate).is_none());
        }
        let mut drifted = approved.clone();
        drifted["change_set"]["thread_locator"] = json!("slack://other");
        assert!(plannable_change_set(&parent, &drifted).is_none());
        let mut drifted = approved.clone();
        drifted["change_set"]["action_decision"] = json!("stop");
        assert!(plannable_change_set(&parent, &drifted).is_none());
        assert!(bounded_work_artifact(&json!({"text":"x".repeat(33 * 1024)})).is_err());

        let child = WorkAssignment {
            id: "plan".to_owned(),
            parent_work_id: Some(parent.id.clone()),
            status: "pending".to_owned(),
            ..parent.clone()
        };
        let older = WorkAssignment {
            id: "older".to_owned(),
            parent_work_id: None,
            ..parent.clone()
        };
        assert_eq!(evictable_work_index(&[parent, child, older], None), Some(2));
        Ok(())
    }

    #[test]
    fn mail_context_is_complete_only_when_all_grounded_messages_are_available() -> Result<(), String>
    {
        let mut grounding = json!({
            "returned_count":2,
            "omitted_before":{"count":0},
            "messages":[
                {"id":1,"direction":"inbound","occurred_at":"2026-10-01T00:00:00Z","text_body":"Question","body_truncated":false,"attachments":[]},
                {"id":2,"direction":"outbound","occurred_at":"2026-10-01T01:00:00Z","text_body":"Answer","body_truncated":false,"attachments":[]}
            ]
        });
        let (messages, complete) = bounded_mail_context(&grounding)?;
        assert!(complete);
        assert_eq!(messages.len(), 2);
        grounding["omitted_before"]["count"] = json!(1);
        assert!(!bounded_mail_context(&grounding)?.1);
        grounding["omitted_before"]["count"] = json!(0);
        grounding["messages"][0]["body_truncated"] = json!(true);
        assert!(!bounded_mail_context(&grounding)?.1);
        grounding["messages"][0]["body_truncated"] = json!(false);
        grounding["messages"][0]["attachments"] = json!([{"name":"file"}]);
        assert!(!bounded_mail_context(&grounding)?.1);
        Ok(())
    }

    #[test]
    fn pending_delivery_preserves_only_new_attention_and_old_turns_decode() -> Result<(), String> {
        let previous: PendingTurn = serde_json::from_value(json!({"pages":[],"review_ref":null}))
            .map_err(|error| error.to_string())?;
        assert!(!previous.deferred);
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
        Ok(())
    }

    #[test]
    fn mailbox_requires_a_grounded_message_and_current_thread() -> Result<(), &'static str> {
        let row = json!({
            "conversation_id":42,
            "last_message_at":"2026-10-02T09:00:00Z",
            "last_inbound_at":"2026-10-02T09:00:00Z",
            "updated_at":"2026-10-02T09:00:01Z",
            "external_participant_address":"sender@example.test",
            "subject":"Decision requested",
            "preview":"Please review the draft."
        });
        let thread = json!({
            "status":"ok",
            "purpose":"read",
            "thread":{
                "conversation_id":42,
                "last_message_at":"2026-10-02T09:00:00.000000Z",
                "updated_at":"2026-10-02T09:00:01.000000Z",
                "subject":"Decision requested",
                "preview":"Please review the draft.",
                "latest_activity":{"direction":"inbound"}
            },
            "thread_grounding":{
                "conversation_id":42,
                "messages":[{"id":77,"direction":"inbound","occurred_at":"2026-10-02T09:00:00.000000Z","sender_address":"sender@example.test"}]
            }
        });
        let (evidence, message) = normalize_mail_observation("brnd_fixture", &row, &thread)
            .ok_or("grounded mail observation must normalize")?;
        assert_eq!(evidence["source_kind"], "mail");
        assert_eq!(message["thread_locator"], "nitrosend://brnd_fixture/42");
        assert_eq!(
            message["message_locator"],
            "nitrosend://brnd_fixture/42/messages/77"
        );
        assert_eq!(message["conversation"]["external_id"], "42");
        assert_eq!(message["occurred_at"], "2026-10-02T09:00:00.000Z");
        let mut stale = row.clone();
        stale["last_message_at"] = json!("2026-10-02T09:00:02Z");
        assert!(normalize_mail_observation("brnd_fixture", &stale, &thread).is_none());
        let mut missing_message = thread.clone();
        missing_message["thread_grounding"]["messages"] = json!([]);
        assert!(normalize_mail_observation("brnd_fixture", &row, &missing_message).is_none());
        let pinned_before_fix = json!({"occurred_at":"2026-10-02T09:00:00.123456Z"});
        assert_eq!(
            message_for_queue(&pinned_before_fix)
                .map_err(|_| "pinned page time must canonicalize")?["occurred_at"],
            "2026-10-02T09:00:00.123Z"
        );
        Ok(())
    }

    #[test]
    fn quiet_hours_and_paginated_coverage_cross_expected_boundaries() -> Result<(), &'static str> {
        let hours = QuietHours {
            start_hour: 22,
            end_hour: 8,
        };
        assert!(quiet_hour(&hours, 23));
        assert!(quiet_hour(&hours, 7));
        assert!(!quiet_hour(&hours, 8));
        assert_eq!(
            page_cursor(&json!({"pagination":{"page":1,"total_pages":66}})),
            Some("page:2".to_owned())
        );
        assert_eq!(
            page_cursor(&json!({"pagination":{"page":66,"total_pages":66}})),
            None
        );
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
    fn pr_work_targets_require_an_exact_canonical_read_target() {
        assert_eq!(
            parse_pr_target("https://github.com/example/project/pull/12"),
            Some(("example/project".to_owned(), 12))
        );
        for target in [
            "http://github.com/example/project/pull/12",
            "https://github.com/example/project/issues/12",
            "https://github.com/example/project/pull/12/commits",
            "https://github.com/example/project/pull/0",
            "https://github.com/example/project/pull/12?state=all",
            "https://github.com/example/../pull/12",
        ] {
            assert!(parse_pr_target(target).is_none(), "accepted {target}");
        }
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
        let text = private_notification_text(&packet, 2000, true)?;
        assert!(text.len() <= 2000);
        assert!(text.contains("Source scan is partial"));
        assert!(!super::has_active_notification_mention(&text));
        assert!(text.contains("Kam"));
        assert!(text.contains("the link"));
        for index in 0..5 {
            assert!(text.contains(&format!("sha256:{index:064x}")));
            assert!(!text.contains(&format!("slack://workspace/channel/{index}")));
        }
        assert!(private_notification_text(&packet, 100, true).is_err());
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
            "operation":"channel.post.read",
            "finality":"confirmed",
            "readback_ref":format!("runx:provider-readback:sha256:{}", "b".repeat(64)),
            "result":{
                "content_digest":digest,
                "conversation_locator":channel,
                "message_locator":"slack://T123/C456/1791095930.389929"
            }
        });
        assert!(verified_notification_readback(&readback, &digest, channel));
        let mut wrong = readback.clone();
        wrong["result"]["content_digest"] = json!(format!("sha256:{}", "c".repeat(64)));
        assert!(!verified_notification_readback(&wrong, &digest, channel));
        wrong = readback;
        wrong["operation"] = json!("channel.post");
        assert!(!verified_notification_readback(&wrong, &digest, channel));
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
