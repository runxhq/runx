use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::{JsonObject, JsonValue};

use super::paths::{apply_bindings, canonical_equal, present, value_at_path};
use super::{EvidenceVerifyInput, Finding, context, effects, is_sha256, object, strings, text};
use crate::RuntimeError;

const TOOL: &str = "evidence.verify_artifact";

pub(super) fn build(inputs: &EvidenceVerifyInput) -> Result<JsonValue, RuntimeError> {
    let candidate = inputs.candidate.clone();
    let source_records = inputs.source_records.as_slice();
    let (admitted_sources, allowed_sources) =
        admitted_sources(&inputs.source_digests, source_records);
    let content_digests = content_digests(source_records);
    let claim_bindings = inputs.claim_bindings.as_slice();
    let context_requirements = inputs.context_requirements.as_slice();
    let mut findings = Vec::new();

    verify_decision(&inputs.ready_decision, &candidate, &mut findings);
    verify_claims(
        inputs.require_claim_bindings,
        claim_bindings,
        &allowed_sources,
        &content_digests,
        &mut findings,
    );
    verify_references(&inputs.reference_bindings, &allowed_sources, &mut findings);
    verify_required_fields(inputs, &candidate, &mut findings);
    context::verify(
        context_requirements,
        &inputs.context_bindings,
        inputs.require_all_contexts,
        &mut findings,
    );
    if inputs.forbid_external_effects {
        effects::verify(
            &JsonValue::Object(candidate.clone()),
            "candidate",
            &mut findings,
        );
    }

    let passed = findings.is_empty();
    let verified = verified_artifact(inputs, candidate, passed, &findings)?;
    let artifact_name = output_name(&inputs.artifact_name)?;
    let mut output = JsonObject::from([(
        "verification".to_owned(),
        verification_report(passed, findings, admitted_sources, context_requirements),
    )]);
    if let Some(verified) = verified {
        output.insert(artifact_name, JsonValue::Object(verified));
    }
    Ok(JsonValue::Object(output))
}

fn verification_report(
    passed: bool,
    findings: Vec<Finding>,
    admitted_sources: Vec<String>,
    context_requirements: &[JsonValue],
) -> JsonValue {
    let admitted_contexts = context_requirements
        .iter()
        .map(|raw| text(object(Some(raw)).get("packet_digest")))
        .filter(|digest| !digest.is_empty())
        .map(JsonValue::String)
        .collect::<Vec<_>>();
    JsonValue::Object(JsonObject::from([
        (
            "status".to_owned(),
            JsonValue::String(if passed { "pass" } else { "fail" }.to_owned()),
        ),
        (
            "findings".to_owned(),
            JsonValue::Array(findings.into_iter().map(Finding::into_json).collect()),
        ),
        (
            "admitted_source_digests".to_owned(),
            JsonValue::Array(
                admitted_sources
                    .into_iter()
                    .map(JsonValue::String)
                    .collect(),
            ),
        ),
        (
            "admitted_context_digests".to_owned(),
            JsonValue::Array(admitted_contexts),
        ),
    ]))
}

fn admitted_sources(
    source_digests: &[String],
    records: &[JsonValue],
) -> (Vec<String>, BTreeSet<String>) {
    let mut ordered = Vec::new();
    let mut admitted = BTreeSet::new();
    for digest in source_digests
        .iter()
        .map(|value| value.trim().to_owned())
        .chain(
            records
                .iter()
                .map(|record| text(object(Some(record)).get("source_digest"))),
        )
    {
        if is_sha256(&digest) && admitted.insert(digest.clone()) {
            ordered.push(digest);
        }
    }
    (ordered, admitted)
}

fn content_digests(records: &[JsonValue]) -> BTreeMap<String, String> {
    records
        .iter()
        .filter_map(|record| {
            let record = object(Some(record));
            let source = text(record.get("source_digest"));
            let content = text(record.get("content_digest"));
            (is_sha256(&source) && is_sha256(&content)).then_some((source, content))
        })
        .collect()
}

fn verify_decision(ready: &str, candidate: &JsonObject, findings: &mut Vec<Finding>) {
    if text(candidate.get("decision")) != ready.trim() {
        findings.push(Finding::new(
            "artifact.decision.not_ready",
            "candidate decision is not ready",
            None,
        ));
    }
}

fn verify_claims(
    require_claim_bindings: bool,
    bindings: &[JsonValue],
    allowed: &BTreeSet<String>,
    content_digests: &BTreeMap<String, String>,
    findings: &mut Vec<Finding>,
) {
    if require_claim_bindings && bindings.is_empty() {
        findings.push(Finding::new(
            "artifact.claims.empty",
            "ready artifact requires at least one evidence-bound claim",
            None,
        ));
    }
    for (index, raw) in bindings.iter().enumerate() {
        let binding = object(Some(raw));
        let mut digests = strings(binding.get("source_digests"));
        let single = text(binding.get("source_digest"));
        if !single.is_empty() {
            digests.push(single.clone());
        }
        if text(binding.get("claim")).is_empty() {
            findings.push(Finding::new(
                "artifact.claim.empty",
                "claim is required",
                Some(format!("claim_bindings[{index}].claim")),
            ));
        }
        if digests.is_empty() || digests.iter().any(|digest| !allowed.contains(digest)) {
            findings.push(Finding::new(
                "artifact.claim.unbound",
                "claim must bind only admitted source digests",
                Some(format!("claim_bindings[{index}].source_digests")),
            ));
        }
        if let Some(expected) = content_digests.get(&single)
            && text(binding.get("content_digest")) != *expected
        {
            findings.push(Finding::new(
                "artifact.content_digest.mismatch",
                "content digest does not match the admitted source",
                Some(format!("claim_bindings[{index}].content_digest")),
            ));
        }
    }
}

fn verify_references(
    bindings: &[JsonValue],
    allowed: &BTreeSet<String>,
    findings: &mut Vec<Finding>,
) {
    for (index, raw) in bindings.iter().enumerate() {
        let binding = object(Some(raw));
        let digests = strings(binding.get("source_digests"));
        if digests.is_empty() || digests.iter().any(|digest| !allowed.contains(digest)) {
            findings.push(Finding::new(
                "artifact.reference.unbound",
                "reference must bind only admitted source digests",
                Some(format!("reference_bindings[{index}].source_digests")),
            ));
        }
    }
}

fn verify_required_fields(
    inputs: &EvidenceVerifyInput,
    candidate: &JsonObject,
    findings: &mut Vec<Finding>,
) {
    let candidate_value = JsonValue::Object(candidate.clone());
    for (index, path) in inputs
        .required_paths
        .iter()
        .filter_map(JsonValue::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .enumerate()
    {
        if !present(value_at_path(Some(&candidate_value), path)) {
            findings.push(Finding::new(
                "artifact.required.missing",
                "required candidate field is empty",
                Some(format!("required_paths[{index}]")),
            ));
        }
    }
    for (index, raw) in inputs.required_values.iter().enumerate() {
        let binding = object(Some(raw));
        let path = text(binding.get("path"));
        if !canonical_equal(
            value_at_path(Some(&candidate_value), &path),
            binding.get("value"),
        ) {
            findings.push(Finding::new(
                "artifact.identity.changed",
                "candidate changed an authoritative input value",
                Some(format!("required_values[{index}]")),
            ));
        }
    }
    let identity = inputs.identity_source.as_ref();
    for (index, raw) in inputs.required_value_bindings.iter().enumerate() {
        let binding = object(Some(raw));
        let candidate_path = text(binding.get("candidate_path"));
        let source_path = text(binding.get("source_path"));
        if !canonical_equal(
            value_at_path(Some(&candidate_value), &candidate_path),
            value_at_path(identity, &source_path),
        ) {
            findings.push(Finding::new(
                "artifact.identity.changed",
                format!(
                    "candidate changed the authoritative value at {candidate_path}; copy it exactly"
                ),
                Some(format!("required_value_bindings[{index}]")),
            ));
        }
    }
}

fn verified_artifact(
    inputs: &EvidenceVerifyInput,
    candidate: JsonObject,
    passed: bool,
    findings: &[Finding],
) -> Result<Option<JsonObject>, RuntimeError> {
    if passed {
        let mut verified = candidate;
        verified.extend(inputs.authoritative_fields.clone().unwrap_or_default());
        // A path binding owns that path, not its parent. Applying it to an
        // empty projection and extending the candidate drops sibling fields.
        let mut verified = apply_bindings(
            verified,
            inputs.authoritative_source.as_ref(),
            &inputs.authoritative_bindings,
            "authoritative_bindings",
        )?;
        verified.insert("validation".to_owned(), validation("pass", &[]));
        return Ok(Some(verified));
    }
    let Some(fallback) = inputs.fallback_artifact.clone() else {
        return Ok(None);
    };
    let mut fallback = apply_bindings(
        fallback,
        inputs.fallback_source.as_ref(),
        &inputs.fallback_bindings,
        "fallback_bindings",
    )?;
    fallback.insert(
        "decision".to_owned(),
        JsonValue::String(inputs.blocked_decision.trim().to_owned()),
    );
    fallback.insert("validation".to_owned(), validation("fail", findings));
    Ok(Some(fallback))
}

fn validation(status: &str, findings: &[Finding]) -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("status".to_owned(), JsonValue::String(status.to_owned())),
        (
            "findings".to_owned(),
            JsonValue::Array(findings.iter().cloned().map(Finding::into_json).collect()),
        ),
    ]))
}

fn output_name(value: &str) -> Result<String, RuntimeError> {
    let name = value.trim().to_owned();
    let mut bytes = name.bytes();
    let valid = matches!(bytes.next(), Some(byte) if byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        && name.len() <= 64;
    if !valid || name == "verification" {
        return Err(invalid(
            "artifact_name must be a safe non-reserved output key",
        ));
    }
    Ok(name)
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: TOOL.to_owned(),
        message: message.into(),
    }
}
