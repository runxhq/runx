#![cfg_attr(not(feature = "catalog"), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

use runx_contracts::JsonObject;
#[cfg(feature = "catalog")]
use runx_contracts::JsonValue;

#[cfg(feature = "catalog")]
use crate::RuntimeError;
use crate::credentials::CredentialDelivery;
#[cfg(feature = "catalog")]
use crate::effects::EffectToolRequest;
#[cfg(feature = "catalog")]
use crate::effects::RuntimeEffectRegistry;

mod artifacts;
mod attestation;
#[cfg(feature = "catalog")]
mod authoring;
mod capability;
#[cfg(feature = "cli-tool")]
mod cli;
#[cfg(feature = "cli-tool")]
mod command;
mod data;
mod event_store;
pub use event_store::{
    EventStoreMigrationProof, EventStoreMigrationRequest, EventStoreMigrationStatus,
    migrate_event_store,
};
mod evidence;
mod files;
#[cfg(feature = "cli-tool")]
mod git;
mod handoff;
#[cfg(feature = "catalog")]
mod hosted_artifacts;
mod input;
mod policy;
mod receipt_tools;
#[cfg(feature = "catalog")]
mod snapshot;
mod workspace;

mod catalog;

use capability::NativeCapability;
#[cfg(feature = "catalog")]
use capability::RawNativeInvocation;
#[cfg(feature = "catalog")]
pub(crate) use event_store::{
    MAX_DATA_OPERATION_RESULT_BYTES, PreparedOperation as PreparedDataOperation,
    native_adapter as is_native_data_adapter, operation_name as data_operation_name,
    prepare_operation as prepare_data_operation,
};
pub(super) use input::{invalid_input, required_string};
pub(super) use workspace::resolve_repo_root_for;

pub(crate) use catalog::artifacts;
#[cfg(feature = "catalog")]
pub(crate) use catalog::inventory;
pub(crate) use catalog::{inspect, list_items, search};
#[cfg(feature = "catalog")]
pub use snapshot::{NativeCapabilitySnapshot, native_capability_snapshot};

const CAPABILITY_GROUPS: &[&[&dyn NativeCapability]] = &[
    #[cfg(feature = "cli-tool")]
    command::CAPABILITIES,
    #[cfg(feature = "catalog")]
    authoring::CAPABILITIES,
    files::CAPABILITIES,
    #[cfg(feature = "cli-tool")]
    git::CAPABILITIES,
    #[cfg(feature = "cli-tool")]
    cli::CAPABILITIES,
    data::CAPABILITIES,
    handoff::CAPABILITIES,
    event_store::CAPABILITIES,
    evidence::CAPABILITIES,
    attestation::CAPABILITIES,
    artifacts::CAPABILITIES,
    #[cfg(feature = "catalog")]
    hosted_artifacts::CAPABILITIES,
    receipt_tools::CAPABILITIES,
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
    /// Runtime-owned binding resolved before native dispatch. Capability input
    /// schemas must never expose this routing state to an operator or agent.
    pub data_source_binding: Option<&'a JsonObject>,
    pub env: &'a BTreeMap<String, String>,
    pub skill_directory: &'a Path,
    pub credential_delivery: &'a CredentialDelivery,
    pub local_artifacts: &'a crate::services::LocalArtifactService,
    #[cfg(feature = "catalog")]
    pub effects: &'a RuntimeEffectRegistry,
}

impl<I: ?Sized> NativeInvocation<'_, I> {
    fn harness_http_responses(
        &self,
    ) -> Option<&BTreeMap<String, crate::http::RuntimeHttpResponse>> {
        #[cfg(feature = "catalog")]
        {
            self.effects.harness_http_responses()
        }
        #[cfg(not(feature = "catalog"))]
        {
            None
        }
    }
}

#[cfg(feature = "catalog")]
pub(crate) struct NativeToolInvocation<'a> {
    pub tool_ref: &'a str,
    pub observed_at: &'a str,
    pub inputs: JsonObject,
    pub scopes: &'a [String],
    pub data_source_binding: Option<JsonObject>,
    pub env: &'a BTreeMap<String, String>,
    pub skill_directory: &'a Path,
    pub credential_delivery: &'a CredentialDelivery,
    pub local_artifacts: &'a crate::services::LocalArtifactService,
    pub effect_admission: Option<&'a crate::effects::EffectAdmission>,
    pub effects: &'a RuntimeEffectRegistry,
}

#[cfg(feature = "catalog")]
pub(crate) struct NativeToolInvocationResult {
    pub(crate) result: Result<JsonValue, RuntimeError>,
    pub(crate) ephemeral: Option<JsonValue>,
    pub(crate) execution_boundary: runx_contracts::ExecutionBoundaryKind,
}

#[cfg(feature = "catalog")]
pub(crate) fn invoke(request: NativeToolInvocation<'_>) -> Option<NativeToolInvocationResult> {
    if let Some(tool) = definition(request.tool_ref) {
        let invocation = RawNativeInvocation {
            inputs: request.inputs,
            scopes: request.scopes,
            data_source_binding: request.data_source_binding,
            observed_at: request.observed_at,
            env: request.env,
            skill_directory: request.skill_directory,
            credential_delivery: request.credential_delivery,
            local_artifacts: request.local_artifacts,
            effects: request.effects,
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
    let execution_boundary = request
        .effects
        .capability_execution_boundary(request.tool_ref)
        .unwrap_or(runx_contracts::ExecutionBoundaryKind::NativeCapability);
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

pub(crate) fn is_core_tool(tool_ref: &str) -> bool {
    definition(tool_ref).is_some()
}

pub(crate) fn execution_boundary(
    tool_ref: &str,
    effects: &crate::effects::RuntimeEffectRegistry,
) -> Option<runx_contracts::ExecutionBoundaryKind> {
    definition(tool_ref)
        .map(NativeCapability::execution_boundary)
        .or_else(|| effects.capability_execution_boundary(tool_ref))
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
