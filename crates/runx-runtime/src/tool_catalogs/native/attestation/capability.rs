use runx_contracts::{JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput,
};

use super::super::capability::{NativeCapability, TypedNativeCapability};
use super::output::AttestationOutput;

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AttestationInput {
    pub(super) action: String,
    pub(super) principal: String,
    pub(super) claim: String,
    pub(super) evidence: Vec<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) scope: Option<JsonObject>,
}

impl CapabilityInput for AttestationInput {}

const FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "action",
        description: "What happened outside Runx.",
    },
    CapabilityField {
        name: "principal",
        description: "Actor named by the attestation.",
    },
    CapabilityField {
        name: "claim",
        description: "Exact assertion bound into the receipt.",
    },
    CapabilityField {
        name: "evidence",
        description: "Bounded opaque evidence locators with digests and proof claims.",
    },
    CapabilityField {
        name: "scope",
        description: "Optional downstream reliance bounds.",
    },
];

static ATTEST: TypedNativeCapability<AttestationInput, AttestationOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "receipt.attest",
            source: "runx-runtime/receipts",
            summary: "Validate and digest one bounded off-runtime action attestation for sealing.",
            scopes: &[],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Named {
                output: "attestation",
                packet: "runx.attestation.v1",
            },
            fields: FIELDS,
        },
        super::prepare,
    );

pub(in crate::tool_catalogs::native) const CAPABILITIES: &[&dyn NativeCapability] = &[&ATTEST];
