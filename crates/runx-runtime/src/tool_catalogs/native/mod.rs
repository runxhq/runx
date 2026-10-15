use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

use runx_contracts::JsonObject;
use runx_contracts::JsonValue;

use crate::RuntimeError;
use crate::credentials::CredentialDelivery;
use crate::effects::EffectToolRequest;
use crate::effects::RuntimeEffectRegistry;

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
mod receipt_tools;
mod run_tools;
mod snapshot;
mod workspace;

mod catalog;

use capability::NativeCapability;
use capability::RawNativeInvocation;
pub(super) use input::{invalid_input, required_string};
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
    receipt_tools::CAPABILITIES,
    run_tools::CAPABILITIES,
    policy::CAPABILITIES,
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
    pub bounds: crate::services::WorkspaceBounds,
}

pub(crate) struct NativeToolInvocation<'a> {
    pub tool_ref: &'a str,
    pub observed_at: &'a str,
    pub inputs: JsonObject,
    pub scopes: &'a [String],
    pub env: &'a BTreeMap<String, String>,
    pub skill_directory: &'a Path,
    pub credential_delivery: &'a CredentialDelivery,
    pub local_artifacts: &'a crate::services::LocalArtifactService,
    pub effect_admission: Option<&'a crate::effects::EffectAdmission>,
    pub effects: &'a RuntimeEffectRegistry,
    pub bounds: crate::services::WorkspaceBounds,
}

pub(crate) struct NativeToolInvocationResult {
    pub(crate) result: Result<JsonValue, RuntimeError>,
    pub(crate) ephemeral: Option<JsonValue>,
    pub(crate) execution_boundary: runx_contracts::ExecutionBoundaryKind,
}

pub(crate) fn invoke(request: NativeToolInvocation<'_>) -> Option<NativeToolInvocationResult> {
    if let Some(tool) = definition(request.tool_ref) {
        let invocation = RawNativeInvocation {
            inputs: request.inputs,
            scopes: request.scopes,
            observed_at: request.observed_at,
            env: request.env,
            skill_directory: request.skill_directory,
            credential_delivery: request.credential_delivery,
            local_artifacts: request.local_artifacts,
            effects: request.effects,
            bounds: request.bounds,
        };
        return Some(NativeToolInvocationResult {
            result: tool.invoke(invocation),
            ephemeral: None,
            execution_boundary: tool.execution_boundary(),
        });
    }

    let capability = request.effects.capability(request.tool_ref)?;
    if let Err(error) = crate::capability::enforce_required_scopes(
        capability.definition().id,
        capability.definition().scopes.iter().copied(),
        request.scopes,
    ) {
        return Some(NativeToolInvocationResult {
            result: Err(error),
            ephemeral: None,
            execution_boundary: runx_contracts::ExecutionBoundaryKind::NativeCapability,
        });
    }
    // An effect capability runs in the runtime's process, behind its native
    // capability boundary; the effect does not say where it ran.
    let execution_boundary = runx_contracts::ExecutionBoundaryKind::NativeCapability;
    request
        .effects
        .invoke_tool(EffectToolRequest {
            tool_ref: request.tool_ref,
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

fn definition(tool_ref: &str) -> Option<&'static dyn NativeCapability> {
    let index = capability_index();
    if index.duplicate_ids.contains(tool_ref) {
        return None;
    }
    index.by_id.get(tool_ref).copied()
}

#[cfg(test)]
pub(super) fn required_scopes(tool_ref: &str) -> Option<&'static [&'static str]> {
    definition(tool_ref).map(|capability| capability.definition().scopes)
}

/// What a call of core tool `tool_ref` with `inputs` would change or run,
/// without running it; none for a tool that only reads or that runx does not
/// own.
pub(crate) fn local_effect(
    tool_ref: &str,
    inputs: JsonObject,
    env: &BTreeMap<String, String>,
    skill_directory: &Path,
    effects: &RuntimeEffectRegistry,
    bounds: crate::services::WorkspaceBounds,
) -> Result<Option<crate::capability::LocalEffect>, RuntimeError> {
    let Some(tool) = definition(tool_ref) else {
        return Ok(None);
    };
    tool.local_effect(RawNativeInvocation {
        inputs,
        scopes: &[],
        observed_at: "",
        env,
        skill_directory,
        credential_delivery: &CredentialDelivery::none(),
        local_artifacts: &crate::services::LocalArtifactService::default(),
        effects,
        bounds,
    })
}

pub(crate) fn is_core_tool(tool_ref: &str) -> bool {
    definition(tool_ref).is_some()
}

/// Whether the core tool `tool_ref` only reads, so a call of it that may
/// have run can run again unseen.
pub(crate) fn only_reads(tool_ref: &str) -> bool {
    definition(tool_ref).is_some_and(|capability| !capability.definition().effect.mutating())
}

pub(crate) fn execution_boundary(
    tool_ref: &str,
    effects: &crate::effects::RuntimeEffectRegistry,
) -> Option<runx_contracts::ExecutionBoundaryKind> {
    definition(tool_ref)
        .map(NativeCapability::execution_boundary)
        .or_else(|| {
            effects
                .capability(tool_ref)
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
