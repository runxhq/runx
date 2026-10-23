use runx_contracts::schema::BoundedString;
use runx_contracts::{
    ATTESTATION_MAX_ACTION_BYTES, ATTESTATION_MAX_CLAIM_BYTES, ATTESTATION_MAX_PRINCIPAL_BYTES,
    Attestation, AttestationDecision, AttestationEvidence, AttestationGap,
    AttestationProofBoundary, AttestationSchema, Digest, JsonObject, JsonValue,
    canonical_stable_json,
};
use serde::Serialize;

use super::{NativeInvocation, invalid_input};
use crate::RuntimeError;

mod capability;
mod evidence;
mod output;

use capability::AttestationInput;
pub(super) use capability::CAPABILITIES;
use output::AttestationOutput;

use evidence::parse_evidence;

const TOOL: &str = "receipt.attest";

fn prepare(
    invocation: &NativeInvocation<'_, AttestationInput>,
) -> Result<AttestationOutput, RuntimeError> {
    Ok(AttestationOutput {
        attestation: build(invocation.inputs)?,
    })
}

fn build(inputs: &AttestationInput) -> Result<Attestation, RuntimeError> {
    let action = bounded_text::<ATTESTATION_MAX_ACTION_BYTES>(&inputs.action, "action")?;
    let principal =
        bounded_text::<ATTESTATION_MAX_PRINCIPAL_BYTES>(&inputs.principal, "principal")?;
    let claim = bounded_text::<ATTESTATION_MAX_CLAIM_BYTES>(&inputs.claim, "claim")?;
    let scope = inputs.scope.clone().unwrap_or_default();
    let mut gaps = Vec::new();
    require(
        action.as_ref(),
        "attestation.action.missing",
        "action is required",
        &mut gaps,
    );
    require(
        principal.as_ref(),
        "attestation.principal.missing",
        "principal is required",
        &mut gaps,
    );
    require(
        claim.as_ref(),
        "attestation.claim.missing",
        "claim is required",
        &mut gaps,
    );
    let evidence = parse_evidence(&inputs.evidence, &mut gaps);
    let attestation_digest = attestation_digest(&AttestationCore {
        action: action.as_ref(),
        claim: claim.as_ref(),
        principal: principal.as_ref(),
        evidence: &evidence,
        scope: &scope,
    })?;
    Ok(Attestation {
        schema: AttestationSchema::V1,
        decision: attestation_decision(&gaps),
        action,
        claim,
        principal,
        evidence: runx_contracts::schema::BoundedVec::new(evidence)
            .ok_or_else(|| invalid_input(TOOL, "evidence exceeds its item limit"))?,
        scope,
        attestation_digest,
        gaps,
        proof_boundary: AttestationProofBoundary::default(),
    })
}

fn attestation_decision(gaps: &[AttestationGap]) -> AttestationDecision {
    if gaps.is_empty() {
        AttestationDecision::ReadyToSeal
    } else if gaps.iter().any(|gap| gap.code.ends_with(".missing")) {
        AttestationDecision::NeedsAgent
    } else {
        AttestationDecision::NeedsMoreEvidence
    }
}

/// What the attestation digest covers.
#[derive(Serialize)]
struct AttestationCore<'a> {
    action: Option<&'a BoundedString<ATTESTATION_MAX_ACTION_BYTES>>,
    claim: Option<&'a BoundedString<ATTESTATION_MAX_CLAIM_BYTES>>,
    principal: Option<&'a BoundedString<ATTESTATION_MAX_PRINCIPAL_BYTES>>,
    evidence: &'a [AttestationEvidence],
    scope: &'a JsonObject,
}

fn attestation_digest(core: &AttestationCore<'_>) -> Result<Digest, RuntimeError> {
    let canonical = JsonValue::from_serialize(core)
        .map_err(|error| error.to_string())
        .and_then(|core| canonical_stable_json(&core).map_err(|error| error.to_string()))
        .map_err(|error| {
            invalid_input(
                TOOL,
                format!("attestation canonicalization failed: {error}"),
            )
        })?;
    Ok(Digest::of(canonical.as_bytes()))
}

/// The trimmed value, or `None` when it is empty.
fn bounded_text<const MAX_BYTES: usize>(
    value: &str,
    field: &str,
) -> Result<Option<BoundedString<MAX_BYTES>>, RuntimeError> {
    let value = value.trim();
    if value.len() > MAX_BYTES {
        return Err(invalid_input(
            TOOL,
            format!("{field} exceeds the {MAX_BYTES}-byte limit"),
        ));
    }
    Ok(BoundedString::new(value))
}

fn require<T>(value: Option<&T>, code: &str, message: &str, gaps: &mut Vec<AttestationGap>) {
    if value.is_none() {
        gap(gaps, code, message);
    }
}

fn gap(gaps: &mut Vec<AttestationGap>, code: &str, message: impl Into<String>) {
    gaps.push(AttestationGap {
        code: code.into(),
        message: message.into().into(),
    });
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;
