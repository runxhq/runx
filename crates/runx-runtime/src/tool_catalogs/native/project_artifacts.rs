//! Explicit custody of immutable local result values, using the same store as
//! CLI results and bounded caller turns. No application state or new backend.
use super::capability::{NativeCapability, TypedNativeCapability};
use super::{NativeInvocation, invalid_input};
use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, RuntimeError,
};
use runx_contracts::Reach;
use runx_contracts::{Artifact, ArtifactLabels, JsonValue};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct StoreInput {
    value: JsonValue,
    /// Where the stored value may be shared; when unsaid, what the value's
    /// own artifacts allow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    labels: Option<ArtifactLabels>,
}
impl CapabilityInput for StoreInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct OpenInput {
    artifact: Artifact,
}
impl CapabilityInput for OpenInput {}

fn require_inputs_reach(tool: &str, reach: Reach) -> Result<(), RuntimeError> {
    if reach == Reach::Workspace {
        return Err(invalid_input(
            tool,
            "retained project artifacts are protected operator state; a run with reach: inputs and the declared artifact scope reaches them",
        ));
    }
    Ok(())
}

fn store(invocation: &NativeInvocation<'_, StoreInput>) -> Result<Artifact, RuntimeError> {
    const TOOL: &str = "artifact.store";
    require_inputs_reach(TOOL, invocation.reach)?;
    let call = invocation
        .call
        .ok_or_else(|| invalid_input(TOOL, "storage requires the native call identity"))?;
    let bytes = runx_contracts::to_canonical_bytes(&invocation.inputs.value)
        .map_err(|_| invalid_input(TOOL, "value cannot be encoded as canonical JSON"))?;
    if bytes.len() > crate::kernel::store::MAX_BLOB_BYTES {
        return Err(invalid_input(
            TOOL,
            "value exceeds the native artifact value byte ceiling; persist bounded parts",
        ));
    }
    let labels = invocation
        .inputs
        .labels
        .unwrap_or_else(|| ArtifactLabels::of_value(&invocation.inputs.value));
    let paths = crate::paths::RunxPaths::resolve(invocation.env, invocation.skill_directory);
    crate::services::persist_json_bytes(
        &paths.artifacts(),
        &call.run_id,
        "values",
        "value",
        bytes,
        labels,
    )
    .map_err(|error| invalid_input(TOOL, error))
}

fn open(invocation: &NativeInvocation<'_, OpenInput>) -> Result<Artifact, RuntimeError> {
    const TOOL: &str = "artifact.open";
    require_inputs_reach(TOOL, invocation.reach)?;
    if invocation.inputs.artifact.size_bytes > crate::services::MAX_ARTIFACT_BYTES {
        return Err(invalid_input(
            TOOL,
            "artifact exceeds the native snapshot admission byte ceiling",
        ));
    }
    let paths = crate::paths::RunxPaths::resolve(invocation.env, invocation.skill_directory);
    invocation
        .local_artifacts
        .open_project_artifact(&paths.artifacts(), &invocation.inputs.artifact)
        .map_err(|error| invalid_input(TOOL, error))
}

static STORE: TypedNativeCapability<StoreInput, Artifact> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "artifact.store",
        source: "runx-runtime/project-artifacts",
        summary: "Retain one canonical JSON result in this project's immutable artifact store under the native run identity.",
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "artifact",
            packet: runx_contracts::ARTIFACT_SCHEMA,
        },
        fields: &[
            CapabilityField {
                name: "value",
                description: "Explicit result value, encoded once as canonical JSON; use bounded parts for large results.",
            },
            CapabilityField {
                name: "labels",
                description: "Where the stored value may be shared. Defaults to local_only when any artifact in the value is local_only, and otherwise configured_model.",
            },
        ],
    },
    store,
);
static OPEN: TypedNativeCapability<OpenInput, Artifact> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "artifact.open",
        source: "runx-runtime/project-artifacts",
        summary: "Verify a retained project artifact and admit its immutable bytes for existing bounded page reads.",
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "artifact",
            packet: runx_contracts::ARTIFACT_SCHEMA,
        },
        fields: &[CapabilityField {
            name: "artifact",
            description: "Exact artifact descriptor artifact.store retained in the current project; the opened artifact keeps its labels. A reference grants no authority.",
        }],
    },
    open,
);
pub(in crate::tool_catalogs::native) const CAPABILITIES: &[&dyn NativeCapability] =
    &[&STORE, &OPEN];

#[cfg(test)]
mod tests;
