use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

use runx_contracts::JsonObject;
use runx_contracts::JsonValue;

use crate::RuntimeError;
use crate::credentials::CredentialDelivery;
use crate::effects::EffectToolRequest;
use crate::effects::RuntimeEffectRegistry;

mod argv;
mod artifacts;
mod attestation;
mod authoring;
mod capability;
mod cli;
mod command;
mod data;
mod evidence;
mod files;
mod git;
mod handoff;
mod input;
mod policy;
mod process;
mod project_artifacts;
mod receipt_tools;
mod registry;
mod run_tools;
mod snapshot;
mod verdict;
mod workspace;

mod catalog;

use capability::NativeCapability;
use capability::RawNativeInvocation;
pub(super) use input::{invalid_input, non_empty, root_name};
pub(super) use workspace::resolve_repo_root_for;

pub(crate) use catalog::artifacts;
pub(crate) use catalog::inventory;
pub(crate) use catalog::{inspect, list_items, search};
pub use snapshot::{NativeCapabilitySnapshot, native_capability_snapshot};

const CAPABILITY_GROUPS: &[&[&dyn NativeCapability]] = &[
    command::CAPABILITIES,
    authoring::CAPABILITIES,
    files::CAPABILITIES,
    git::CAPABILITIES,
    cli::CAPABILITIES,
    data::CAPABILITIES,
    handoff::CAPABILITIES,
    evidence::CAPABILITIES,
    attestation::CAPABILITIES,
    artifacts::CAPABILITIES,
    project_artifacts::CAPABILITIES,
    receipt_tools::CAPABILITIES,
    registry::CAPABILITIES,
    run_tools::CAPABILITIES,
    verdict::CAPABILITIES,
    policy::CAPABILITIES,
    process::CAPABILITIES,
];

fn core_capabilities() -> impl Iterator<Item = &'static dyn NativeCapability> {
    CAPABILITY_GROUPS
        .iter()
        .flat_map(|group| group.iter().copied())
}

struct CapabilityIndex {
    by_id: BTreeMap<&'static str, &'static dyn NativeCapability>,
    duplicate_ids: BTreeSet<&'static str>,
}

fn capability_index() -> &'static CapabilityIndex {
    static INDEX: OnceLock<CapabilityIndex> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut by_id = BTreeMap::new();
        let mut duplicate_ids = BTreeSet::new();
        for capability in core_capabilities() {
            let id = capability.definition().id;
            if by_id.insert(id, capability).is_some() {
                duplicate_ids.insert(id);
            }
        }
        CapabilityIndex {
            by_id,
            duplicate_ids,
        }
    })
}

pub(super) struct NativeInvocation<'a, I: ?Sized = JsonObject> {
    pub inputs: &'a I,
    pub observed_at: &'a str,
    pub env: &'a BTreeMap<String, String>,
    pub skill_directory: &'a Path,
    pub credential_delivery: &'a CredentialDelivery,
    pub local_artifacts: &'a crate::services::LocalArtifactService,
    pub effects: &'a RuntimeEffectRegistry,
    /// How far the run's file, git, command and authoring tools reach.
    pub reach: runx_contracts::Reach,
    /// The call the tool is invoked for, when a run makes it.
    pub call: Option<&'a crate::tool_catalogs::ToolCall>,
}

pub(crate) struct NativeToolInvocation<'a> {
    pub tool: &'a str,
    pub observed_at: &'a str,
    pub inputs: JsonObject,
    pub env: &'a BTreeMap<String, String>,
    pub skill_directory: &'a Path,
    pub credential_delivery: &'a CredentialDelivery,
    pub local_artifacts: &'a crate::services::LocalArtifactService,
    pub effect_admission: Option<&'a crate::effects::EffectAdmission>,
    pub effects: &'a RuntimeEffectRegistry,
    pub reach: runx_contracts::Reach,
    pub call: Option<&'a crate::tool_catalogs::ToolCall>,
}

pub(crate) struct NativeToolInvocationResult {
    pub(crate) result: Result<JsonValue, RuntimeError>,
    pub(crate) ephemeral: Option<JsonValue>,
    pub(crate) execution_boundary: runx_contracts::ExecutionBoundaryKind,
}

pub(crate) fn invoke(request: NativeToolInvocation<'_>) -> Option<NativeToolInvocationResult> {
    if let Some(tool) = definition(request.tool) {
        let invocation = RawNativeInvocation {
            inputs: request.inputs,
            observed_at: request.observed_at,
            env: request.env,
            skill_directory: request.skill_directory,
            credential_delivery: request.credential_delivery,
            local_artifacts: request.local_artifacts,
            effects: request.effects,
            reach: request.reach,
            call: request.call,
        };
        return Some(NativeToolInvocationResult {
            result: tool.invoke(invocation),
            ephemeral: None,
            execution_boundary: tool.execution_boundary(),
        });
    }

    // Only an effect capability runs here.
    request.effects.capability(request.tool)?;
    // An effect capability runs in the runtime's process, behind its native
    // capability boundary; the effect does not say where it ran.
    let execution_boundary = runx_contracts::ExecutionBoundaryKind::NativeCapability;
    request
        .effects
        .invoke_tool(EffectToolRequest {
            tool: request.tool,
            observed_at: request.observed_at,
            inputs: &request.inputs,
            env: request.env,
            skill_directory: request.skill_directory,
            credential_delivery: request.credential_delivery,
            admission: request.effect_admission,
        })
        .map(|result| match result {
            Ok(output) => NativeToolInvocationResult {
                result: Ok(output.value),
                ephemeral: output.ephemeral,
                execution_boundary,
            },
            Err(error) => NativeToolInvocationResult {
                result: Err(error),
                ephemeral: None,
                execution_boundary,
            },
        })
}

fn definition(tool: &str) -> Option<&'static dyn NativeCapability> {
    let index = capability_index();
    if index.duplicate_ids.contains(tool) {
        return None;
    }
    index.by_id.get(tool).copied()
}

/// What a call of core tool `tool` with `inputs` would change or run,
/// without running it; none for a tool that only reads or that runx does not
/// own.
pub(crate) fn local_effect(
    tool: &str,
    inputs: JsonObject,
    env: &BTreeMap<String, String>,
    skill_directory: &Path,
    effects: &RuntimeEffectRegistry,
    reach: runx_contracts::Reach,
) -> Result<Option<crate::capability::LocalEffect>, RuntimeError> {
    let Some(tool) = definition(tool) else {
        return Ok(None);
    };
    tool.local_effect(RawNativeInvocation {
        inputs,
        observed_at: "",
        env,
        skill_directory,
        credential_delivery: &CredentialDelivery::none(),
        local_artifacts: &crate::services::LocalArtifactService::default(),
        effects,
        reach,
        call: None,
    })
}

pub(crate) fn is_core_tool(tool: &str) -> bool {
    definition(tool).is_some()
}

/// The sources of the core tools that reach the workspace's files: its files
/// and git history, skill authoring, commands and a CLI's help. The rest
/// compute, evidence over the packets it's given among them, or reach runx's
/// own state, a detached process or a registry.
const WORKSPACE_SOURCES: [&str; 6] = [
    "runx-runtime/filesystem",
    "runx-runtime/fs",
    "runx-runtime/git",
    "runx-runtime/authoring",
    "runx-runtime/command",
    "runx-runtime/cli",
];

/// How a call of core tool `tool` takes the workspace's lock: shared to read
/// the workspace, alone to change it or to run a command runx waits on, and
/// not at all when it doesn't reach the workspace.
pub(crate) fn workspace_lock_mode(tool: &str) -> Option<crate::workspace_lock::LockMode> {
    let capability = definition(tool)?;
    let definition = capability.definition();
    if !WORKSPACE_SOURCES.contains(&definition.source) {
        return None;
    }
    let runs_a_command = capability.execution_boundary()
        == runx_contracts::ExecutionBoundaryKind::TrustedHostProcess
        || definition.source == "runx-runtime/cli";
    Some(if definition.effect.mutating() || runs_a_command {
        crate::workspace_lock::LockMode::Alone
    } else {
        crate::workspace_lock::LockMode::Shared
    })
}

/// Whether `tool` is a native tool that only reads.
/// Whether `tool` is a native tool that runs a host process to do its work.
pub(crate) fn runs_a_process(tool: &str) -> bool {
    definition(tool).is_some_and(|capability| {
        capability.execution_boundary() == runx_contracts::ExecutionBoundaryKind::TrustedHostProcess
    })
}

pub(crate) fn only_reads(tool: &str) -> bool {
    definition(tool).is_some_and(|capability| !capability.definition().effect.mutating())
}

/// An input-only computation has no mutable state to reread. Every other
/// native read, including a new capability, must compare through its owner.
pub(crate) fn reads_only_inputs(tool: &str) -> bool {
    definition(tool).is_some_and(|capability| capability.reads_only_inputs())
}

/// Whether `tool` is a safe read (D33): a native tool that only reads, in
/// runx's own process. A crash that leaves one unsettled reruns it with
/// nobody asked. A read a process carries out isn't one.
pub(crate) fn safe_read(tool: &str) -> bool {
    only_reads(tool) && resends_safely(tool)
}

/// Whether a call of core tool `tool` that may have run can run again
/// unseen: it runs in runx's own process, where it only reads, or every
/// change it makes is journaled and bound to what it replaces, so a resend
/// either finds it rolled back or is refused or repeats the same bytes. A
/// tool a process carries out cannot, a read included: whether it only read
/// rests on its author's promise.
pub(crate) fn resends_safely(tool: &str) -> bool {
    definition(tool).is_some_and(|capability| {
        capability.execution_boundary() == runx_contracts::ExecutionBoundaryKind::NativeCapability
    })
}

pub(crate) fn execution_boundary(
    tool: &str,
    effects: &crate::effects::RuntimeEffectRegistry,
) -> Option<runx_contracts::ExecutionBoundaryKind> {
    definition(tool)
        .map(NativeCapability::execution_boundary)
        .or_else(|| {
            effects
                .capability(tool)
                .map(|_| runx_contracts::ExecutionBoundaryKind::NativeCapability)
        })
}

#[cfg(test)]
pub(super) fn fixture_input<I: crate::CapabilityInput>(
    inputs: JsonObject,
) -> Result<I, serde_json::Error> {
    let mut normalized = I::defaults();
    normalized.extend(inputs);
    serde_json::from_value(serde_json::to_value(normalized)?)
}

#[cfg(test)]
pub(super) fn fixture_local_artifacts() -> &'static crate::services::LocalArtifactService {
    static SERVICE: OnceLock<crate::services::LocalArtifactService> = OnceLock::new();
    SERVICE.get_or_init(crate::services::LocalArtifactService::default)
}

#[cfg(test)]
mod tests;
