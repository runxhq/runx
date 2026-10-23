use serde::Serialize;

use crate::{CapabilityApproval, CapabilityArtifacts, CapabilityEffect};

use super::core_capabilities;

const SNAPSHOT_SCHEMA: &str = "runx.native_capability_snapshot.v1";

#[derive(Serialize)]
pub struct NativeCapabilitySnapshot {
    schema: &'static str,
    capability_count: usize,
    capabilities: Vec<SnapshotCapability>,
}

#[derive(Serialize)]
struct SnapshotCapability {
    id: &'static str,
    source: &'static str,
    scopes: Vec<&'static str>,
    effect: &'static str,
    approval: &'static str,
    artifacts: Option<SnapshotArtifacts>,
    execution_boundary: &'static str,
}

#[derive(Serialize)]
struct SnapshotArtifacts {
    kind: &'static str,
    output: &'static str,
    packet: &'static str,
}

/// Project the complete runtime-owned native catalog for the shipping
/// `catalog` feature profile. Effect-owned extensions are composition-root
/// inputs and retain their owner-specific contract validation.
#[must_use]
pub fn native_capability_snapshot() -> NativeCapabilitySnapshot {
    let mut capabilities = core_capabilities()
        .map(|capability| {
            let definition = capability.definition();
            let mut scopes = definition.scopes.to_vec();
            scopes.sort_unstable();
            SnapshotCapability {
                id: definition.id,
                source: definition.source,
                scopes,
                effect: match definition.effect {
                    CapabilityEffect::Read => "read",
                    CapabilityEffect::Mutate => "mutate",
                },
                approval: match definition.approval {
                    CapabilityApproval::None => "none",
                    CapabilityApproval::Effect => "effect",
                },
                artifacts: match definition.artifacts {
                    CapabilityArtifacts::None => None,
                    CapabilityArtifacts::Named { output, packet } => Some(SnapshotArtifacts {
                        kind: "named",
                        output,
                        packet,
                    }),
                    CapabilityArtifacts::Wrapped { output, packet } => Some(SnapshotArtifacts {
                        kind: "wrapped",
                        output,
                        packet,
                    }),
                },
                execution_boundary: capability.execution_boundary().as_str(),
            }
        })
        .collect::<Vec<_>>();
    capabilities.sort_by_key(|capability| capability.id);

    NativeCapabilitySnapshot {
        schema: SNAPSHOT_SCHEMA,
        capability_count: capabilities.len(),
        capabilities,
    }
}
