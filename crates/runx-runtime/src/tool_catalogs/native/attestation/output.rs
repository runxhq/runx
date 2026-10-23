use runx_contracts::Attestation;
use serde::{Deserialize, Serialize};

use crate::CapabilityOutput;

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AttestationOutput {
    pub(super) attestation: Attestation,
}

impl CapabilityOutput for AttestationOutput {}
