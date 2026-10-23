use runx_contracts::{JsonNumber, JsonObject, JsonValue, ReceiptProof, ReceiptQuery};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

use super::NativeInvocation;
use super::capability::{NativeCapability, TypedNativeCapability, decode_typed_output};
use crate::RuntimeError;

use crate::services::ReceiptQueryInput;

impl CapabilityInput for ReceiptQueryInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("limit".to_owned(), JsonValue::Number(JsonNumber::U64(1000))),
            ("verify_chain".to_owned(), JsonValue::Bool(false)),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct ReceiptProofInput {
    receipt_ids: Vec<String>,
}

impl CapabilityInput for ReceiptProofInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct ReceiptQueryOutput {
    receipt_query: ReceiptQuery,
}

impl CapabilityOutput for ReceiptQueryOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct ReceiptProofOutput {
    receipt_proof: ReceiptProof,
}

impl CapabilityOutput for ReceiptProofOutput {}

const QUERY_FIELDS: &[CapabilityField] = &[
    field(
        "query",
        "Case-insensitive subject, id, issuer, actor, or artifact search.",
    ),
    field("skill", "Skill or subject filter."),
    field("outcome", "Exact closure outcome filter."),
    field("issuer", "Exact receipt issuer filter: local or hosted."),
    field("actor", "Exact signed actor filter."),
    field("since", "RFC3339 inclusive lower time bound."),
    field("until", "RFC3339 inclusive upper time bound."),
    field(
        "period",
        "Relative lookback used only when since is absent.",
    ),
    field(
        "as_of",
        "Optional RFC3339 anchor for reproducible lookback.",
    ),
    field("limit", "Maximum result rows from 1 to 10000."),
    field(
        "receipt_ids",
        "One to one hundred exact receipt ids for detailed reads.",
    ),
    field("verify_chain", "Verify bounded matched receipt trees."),
];

const PROOF_FIELDS: &[CapabilityField] = &[field(
    "receipt_ids",
    "One to one hundred exact content-addressed receipt ids.",
)];

const fn field(name: &'static str, description: &'static str) -> CapabilityField {
    CapabilityField { name, description }
}

static QUERY: TypedNativeCapability<ReceiptQueryInput, ReceiptQueryOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "receipt.query",
            source: "runx-runtime/receipts",
            summary: "Query native receipt history with bounded detail and optional tree proof.",
            scopes: &["receipt.read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Named {
                output: "receipt_query",
                packet: "runx.receipt.query.v1",
            },
            fields: QUERY_FIELDS,
        },
        query_receipts,
    );

static PROVE: TypedNativeCapability<ReceiptProofInput, ReceiptProofOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "receipt.prove",
            source: "runx-runtime/receipts",
            summary: "Resolve exact receipts, verify each child tree, and return redacted proof.",
            scopes: &["receipt.read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Named {
                output: "receipt_proof",
                packet: "runx.receipt.proof.v1",
            },
            fields: PROOF_FIELDS,
        },
        prove_receipts,
    );

pub(super) const CAPABILITIES: &[&dyn NativeCapability] = &[&QUERY, &PROVE];

fn query_receipts(
    invocation: &NativeInvocation<'_, ReceiptQueryInput>,
) -> Result<ReceiptQueryOutput, RuntimeError> {
    let query = crate::services::query_receipts(
        invocation.inputs,
        invocation.env,
        invocation.skill_directory,
    )?;
    decode_typed_output(
        "receipt.query",
        JsonValue::Object(JsonObject::from([(
            "receipt_query".to_owned(),
            JsonValue::Object(query),
        )])),
    )
}

fn prove_receipts(
    invocation: &NativeInvocation<'_, ReceiptProofInput>,
) -> Result<ReceiptProofOutput, RuntimeError> {
    let proof = crate::services::prove_receipts(
        &invocation.inputs.receipt_ids,
        invocation.env,
        invocation.skill_directory,
    )?;
    decode_typed_output(
        "receipt.prove",
        JsonValue::Object(JsonObject::from([(
            "receipt_proof".to_owned(),
            JsonValue::Object(proof),
        )])),
    )
}
