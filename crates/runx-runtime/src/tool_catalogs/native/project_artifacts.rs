//! Explicit custody of immutable local result values, using the same store as
//! CLI results and bounded caller turns. No application state or new backend.
use super::capability::{NativeCapability, TypedNativeCapability};
use super::{NativeInvocation, invalid_input};
use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput, RuntimeError,
};
use runx_contracts::Reach;
use runx_contracts::{JsonValue, LocalArtifact, ProjectArtifact};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct StoreInput {
    value: JsonValue,
}
impl CapabilityInput for StoreInput {}
impl CapabilityOutput for ProjectArtifact {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct OpenInput {
    artifact: ProjectArtifact,
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

fn store(invocation: &NativeInvocation<'_, StoreInput>) -> Result<ProjectArtifact, RuntimeError> {
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
    let paths = crate::paths::RunxPaths::resolve(invocation.env, invocation.skill_directory);
    let artifact = crate::services::persist_json_bytes(
        &paths.artifacts(),
        &call.run_id,
        "values",
        "value",
        bytes,
    )
    .map_err(|error| invalid_input(TOOL, error))?;
    artifact
        .deserialize_into()
        .map_err(|_| invalid_input(TOOL, "stored artifact descriptor is invalid"))
}

fn open(invocation: &NativeInvocation<'_, OpenInput>) -> Result<LocalArtifact, RuntimeError> {
    const TOOL: &str = "artifact.open";
    require_inputs_reach(TOOL, invocation.reach)?;
    if invocation.inputs.artifact.bytes > crate::services::MAX_ARTIFACT_BYTES {
        return Err(invalid_input(
            TOOL,
            "artifact exceeds the native snapshot admission byte ceiling",
        ));
    }
    let paths = crate::paths::RunxPaths::resolve(invocation.env, invocation.skill_directory);
    invocation
        .local_artifacts
        .open_project_artifact(&paths.artifacts(), &invocation.inputs.artifact)
        .map(Into::into)
        .map_err(|error| invalid_input(TOOL, error))
}

static STORE: TypedNativeCapability<StoreInput, ProjectArtifact> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "artifact.store",
        source: "runx-runtime/project-artifacts",
        summary: "Retain one canonical JSON result in this project's immutable artifact store under the native run identity.",
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "project_artifact",
            packet: "runx.project_artifact.v1",
        },
        fields: &[CapabilityField {
            name: "value",
            description: "Explicit result value, encoded once as canonical JSON; use bounded parts for large results.",
        }],
    },
    store,
);
static OPEN: TypedNativeCapability<OpenInput, LocalArtifact> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "artifact.open",
        source: "runx-runtime/project-artifacts",
        summary: "Verify a retained project artifact and admit its immutable bytes for existing bounded page reads.",
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "local_artifact",
            packet: "runx.local_artifact.v1",
        },
        fields: &[CapabilityField {
            name: "artifact",
            description: "Exact native ProjectArtifact descriptor from the current project; a reference grants no authority.",
        }],
    },
    open,
);
pub(in crate::tool_catalogs::native) const CAPABILITIES: &[&dyn NativeCapability] =
    &[&STORE, &OPEN];

#[cfg(test)]
mod tests;
