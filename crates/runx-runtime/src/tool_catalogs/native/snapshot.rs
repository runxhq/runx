use serde::Serialize;

use crate::{CapabilityApproval, CapabilityArtifacts, CapabilityEffect};

use super::core_capabilities;

const SNAPSHOT_SCHEMA: &str = "runx.native_capability_snapshot.v1";
const SNAPSHOT_PROFILE: &str = "runx-runtime/catalog";

#[derive(Serialize)]
pub struct NativeCapabilitySnapshot {
    schema: &'static str,
    profile: SnapshotProfile,
    capability_count: usize,
    capabilities: Vec<SnapshotCapability>,
}

#[derive(Serialize)]
struct SnapshotProfile {
    name: &'static str,
    features: Vec<&'static str>,
}

#[derive(Serialize)]
struct SnapshotCapability {
    id: &'static str,
    owner: &'static str,
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
                owner: definition.owner,
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
        profile: SnapshotProfile {
            name: SNAPSHOT_PROFILE,
            features: enabled_roster_features(),
        },
        capability_count: capabilities.len(),
        capabilities,
    }
}

fn enabled_roster_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    if cfg!(feature = "async-http") {
        features.push("async-http");
    }
    if cfg!(feature = "catalog") {
        features.push("catalog");
    }
    if cfg!(feature = "process-tool") {
        features.push("process-tool");
    }
    features
}
