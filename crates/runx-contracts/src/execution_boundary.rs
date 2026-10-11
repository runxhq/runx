//! Runtime-observed execution boundaries.
//!
//! Permission requests describe what an act needs. This contract records where
//! the admitted act actually ran, without implying authority or containment
//! that the executing adapter did not enforce.

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, RunxSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBoundaryKind {
    TrustedHostProcess,
    DeterministicWorker,
    NativeCapability,
    RemoteProvider,
}

impl ExecutionBoundaryKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrustedHostProcess => "trusted_host_process",
            Self::DeterministicWorker => "deterministic_worker",
            Self::NativeCapability => "native_capability",
            Self::RemoteProvider => "remote_provider",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBoundaryObservation {
    pub kind: ExecutionBoundaryKind,
}
