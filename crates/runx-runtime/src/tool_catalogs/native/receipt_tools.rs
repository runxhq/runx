use runx_contracts::{JsonNumber, JsonObject, JsonValue, ReceiptProof, ReceiptQuery, ReceiptRead};
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
struct ReceiptReadInput {
    receipt_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_receipt_id: Option<String>,
    json_pointer: String,
    max_bytes: u64,
}

impl CapabilityInput for ReceiptReadInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("json_pointer".to_owned(), JsonValue::String(String::new())),
            (
                "max_bytes".to_owned(),
                JsonValue::Number(JsonNumber::U64(
                    crate::services::DEFAULT_ARTIFACT_PAGE_BYTES as u64,
                )),
            ),
        ])
    }
}

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

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct ReceiptReadOutput {
    receipt_read: ReceiptRead,
}

impl CapabilityOutput for ReceiptReadOutput {}

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

static READ: TypedNativeCapability<ReceiptReadInput, ReceiptReadOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "receipt.read",
            source: "runx-runtime/receipts",
            summary: "Read a receipt's exact retained declared claim after native proof and signed digest verification.",
            scopes: &["receipt.claim.read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Named {
                output: "receipt_read",
                packet: "runx.receipt.read.v1",
            },
            fields: &[
                field(
                    "receipt_id",
                    "One exact content-addressed receipt id; only its current workspace record is read.",
                ),
                field(
                    "root_receipt_id",
                    "Optional original root. Verify that complete signed tree and read only its root or a descendant; unrelated receipts are refused before retained-state access.",
                ),
                field(
                    "json_pointer",
                    "RFC 6901 pointer into declared outputs; empty selects the complete claim. Diagnostics and inputs are excluded.",
                ),
                field(
                    "max_bytes",
                    "Positive complete-selection byte budget; defaults to the native artifact page budget and may reach the kernel blob ceiling. Never truncates.",
                ),
            ],
        },
        read_receipt,
    );

pub(super) const CAPABILITIES: &[&dyn NativeCapability] = &[&QUERY, &PROVE, &READ];

fn read_receipt(
    invocation: &NativeInvocation<'_, ReceiptReadInput>,
) -> Result<ReceiptReadOutput, RuntimeError> {
    if invocation.bounds == crate::services::WorkspaceBounds::Agent {
        return Err(super::invalid_input(
            "receipt.read",
            "retained receipt claims are protected operator state; this host's agent can read redacted receipt metadata instead",
        ));
    }
    Ok(ReceiptReadOutput {
        receipt_read: crate::services::read_receipt_claim(
            &invocation.inputs.receipt_id,
            invocation.inputs.root_receipt_id.as_deref(),
            &invocation.inputs.json_pointer,
            invocation.inputs.max_bytes,
            invocation.env,
            invocation.skill_directory,
        )?,
    })
}

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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::credentials::CredentialDelivery;
    use crate::effects::RuntimeEffectRegistry;
    use crate::services::{LocalArtifactService, WorkspaceBounds};

    #[test]
    fn receipt_read_requires_claim_authority_and_preserves_the_host_agent_state_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let environment = BTreeMap::from([(
            crate::paths::RUNX_HOME_ENV.to_owned(),
            directory.path().join("unused-home").display().to_string(),
        )]);
        let effects = RuntimeEffectRegistry::default();
        let artifacts = LocalArtifactService::default();
        let delivery = CredentialDelivery::none();
        for (scopes, bounds, reason) in [
            (
                vec!["receipt.read".to_owned()],
                WorkspaceBounds::Operator,
                "receipt.claim.read",
            ),
            (
                vec!["receipt.claim.read".to_owned()],
                WorkspaceBounds::Agent,
                "protected operator state",
            ),
        ] {
            let result = crate::tool_catalogs::native::invoke(
                crate::tool_catalogs::native::NativeToolInvocation {
                    tool: "receipt.read",
                    observed_at: "2026-11-03T00:00:00Z",
                    inputs: JsonObject::from([
                        (
                            "receipt_id".to_owned(),
                            JsonValue::String(format!("sha256:{}", "a".repeat(64))),
                        ),
                        (
                            "root_receipt_id".to_owned(),
                            JsonValue::String(format!("sha256:{}", "b".repeat(64))),
                        ),
                    ]),
                    scopes: &scopes,
                    env: &environment,
                    skill_directory: directory.path(),
                    credential_delivery: &delivery,
                    local_artifacts: &artifacts,
                    effects: &effects,
                    bounds,
                    call: None,
                    effect_admission: None,
                },
            )
            .ok_or("receipt.read was not registered")?;
            let error = result.result.err().ok_or("protected read was allowed")?;
            assert!(error.to_string().contains(reason), "{error}");
            assert!(!directory.path().join("unused-home").exists());
        }
        Ok(())
    }
}
