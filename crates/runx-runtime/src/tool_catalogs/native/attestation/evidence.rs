use std::collections::BTreeSet;

use runx_contracts::schema::BoundedString;
use runx_contracts::{
    ATTESTATION_MAX_EVIDENCE, ATTESTATION_MAX_LOCATOR_BYTES, ATTESTATION_MAX_PROVES_BYTES,
    AttestationEvidence, AttestationGap, Digest, JsonObject, JsonValue,
};

use super::gap;

pub(super) fn parse_evidence(
    evidence: &[JsonValue],
    gaps: &mut Vec<AttestationGap>,
) -> Vec<AttestationEvidence> {
    if evidence.is_empty() {
        gap(
            gaps,
            "attestation.evidence.missing",
            "at least one evidence locator is required",
        );
    }
    if evidence.len() > ATTESTATION_MAX_EVIDENCE {
        gap(
            gaps,
            "attestation.evidence.limit",
            format!("evidence is limited to {ATTESTATION_MAX_EVIDENCE} locators"),
        );
    }
    let mut seen_locators = BTreeSet::new();
    evidence
        .iter()
        .take(ATTESTATION_MAX_EVIDENCE)
        .enumerate()
        .filter_map(|(index, candidate)| {
            parse_evidence_item(candidate, index, &mut seen_locators, gaps)
        })
        .collect()
}

fn parse_evidence_item(
    candidate: &JsonValue,
    index: usize,
    seen_locators: &mut BTreeSet<String>,
    gaps: &mut Vec<AttestationGap>,
) -> Option<AttestationEvidence> {
    let Some(item) = candidate.as_object() else {
        gap(
            gaps,
            "attestation.evidence.object",
            format!("evidence[{index}] must be an object"),
        );
        return None;
    };
    let extras = item
        .keys()
        .filter(|key| !matches!(key.as_str(), "locator" | "digest" | "proves"))
        .cloned()
        .collect::<Vec<_>>();
    if !extras.is_empty() {
        gap(
            gaps,
            "attestation.evidence.raw_fields",
            format!(
                "evidence[{index}] contains unsupported fields: {}",
                extras.join(", ")
            ),
        );
    }
    Some(AttestationEvidence {
        locator: locator(item, index, seen_locators, gaps),
        digest: digest(item, index, gaps),
        proves: bounded_item_text::<ATTESTATION_MAX_PROVES_BYTES>(item, index, "proves", gaps)
            .or_else(|| {
                gap(
                    gaps,
                    "attestation.evidence.proves",
                    format!("evidence[{index}].proves is required"),
                );
                None
            }),
    })
}

/// The item's locator, unless it is missing or could carry a secret.
fn locator(
    item: &JsonObject,
    index: usize,
    seen_locators: &mut BTreeSet<String>,
    gaps: &mut Vec<AttestationGap>,
) -> Option<BoundedString<ATTESTATION_MAX_LOCATOR_BYTES>> {
    let Some(locator) =
        bounded_item_text::<ATTESTATION_MAX_LOCATOR_BYTES>(item, index, "locator", gaps)
    else {
        gap(
            gaps,
            "attestation.evidence.locator",
            format!("evidence[{index}].locator is required"),
        );
        return None;
    };
    if !opaque_locator(locator.as_str()) {
        gap(
            gaps,
            "attestation.evidence.locator",
            format!("evidence[{index}].locator must be an opaque non-secret locator"),
        );
        return None;
    }
    if !seen_locators.insert(locator.as_str().to_owned()) {
        gap(
            gaps,
            "attestation.evidence.duplicate",
            format!("duplicate evidence locator: {}", locator.as_str()),
        );
    }
    Some(locator)
}

fn digest(item: &JsonObject, index: usize, gaps: &mut Vec<AttestationGap>) -> Option<Digest> {
    let digest = item
        .get("digest")
        .and_then(JsonValue::as_str)
        .and_then(|value| Digest::new(value.trim()));
    if digest.is_none() {
        gap(
            gaps,
            "attestation.evidence.digest",
            format!("evidence[{index}].digest must be sha256"),
        );
    }
    digest
}

/// The item's trimmed text field, or `None` when it is missing, empty, or
/// over `MAX_BYTES`; only the last earns its own gap here.
fn bounded_item_text<const MAX_BYTES: usize>(
    item: &JsonObject,
    index: usize,
    field: &str,
    gaps: &mut Vec<AttestationGap>,
) -> Option<BoundedString<MAX_BYTES>> {
    let value = item.get(field).and_then(JsonValue::as_str)?.trim();
    if value.len() > MAX_BYTES {
        gap(
            gaps,
            "attestation.evidence.limit",
            format!("evidence[{index}].{field} exceeds the {MAX_BYTES}-byte limit"),
        );
        return None;
    }
    BoundedString::new(value)
}

fn opaque_locator(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    !value.chars().any(char::is_whitespace)
        && !lower.starts_with("sk-")
        && !lower.starts_with("bearer")
        && !lower.starts_with("-----begin")
}
