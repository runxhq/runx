// Module rationale: receipt verification keeps structural and proof checks co-located for offline audit.
use std::collections::BTreeSet;

use runx_contracts::{
    ActForm, AuthorityAttenuation, CriterionBinding, Decision, ProofKind, Receipt, ReceiptAct,
    ReceiptClass, ReceiptCommitment, ReceiptEvidence, Reference, ReferenceType, Seal,
    legitimacy_authority_term,
};

mod finding;
mod projection;
mod proof;
mod verdict;

pub use finding::{ReceiptError, ReceiptFinding, ReceiptFindingCode, ReceiptVerification};
pub use projection::{
    ReceiptProofFindingSummary, ReceiptProofStatus, ReceiptProofStatusKind, receipt_proof_status,
};
pub use proof::{
    ReceiptProofContext, SignatureVerificationFailure, SignatureVerifier,
    compute_verification_summary, receipt_id_is_content_addressed, validate_receipt_proof,
    verify_receipt_proof,
};
pub use verdict::{
    ReceiptVerifyCheck, ReceiptVerifyFinding, ReceiptVerifyLineageCheck,
    ReceiptVerifySignatureCheck, ReceiptVerifySignatureMode, ReceiptVerifyVerdict,
    VERIFY_VERDICT_SCHEMA, verify_receipt_document_verdict, verify_receipt_verdict,
};

pub fn validate_receipt(receipt: &Receipt) -> Result<(), ReceiptVerification> {
    let verification = verify_receipt(receipt);
    if verification.valid {
        Ok(())
    } else {
        Err(verification)
    }
}

#[must_use]
pub fn verify_receipt(receipt: &Receipt) -> ReceiptVerification {
    let mut verifier = Verifier::default();
    verifier.check_envelope(receipt);
    verifier.check_receipt(receipt);
    verifier.finish()
}

#[derive(Default)]
struct Verifier {
    findings: Vec<ReceiptFinding>,
}

impl Verifier {
    fn finish(self) -> ReceiptVerification {
        ReceiptVerification::from_findings(self.findings)
    }

    fn check_envelope(&mut self, receipt: &Receipt) {
        self.check_non_empty("id", &receipt.id);
        self.check_non_empty("created_at", &receipt.created_at);
        self.check_non_empty("canonicalization", &receipt.canonicalization);
        self.check_non_empty("issuer.kid", &receipt.issuer.kid);
        self.check_sha256_prefix(
            "issuer.public_key_sha256",
            &receipt.issuer.public_key_sha256,
        );
        self.check_non_empty("signature.value", &receipt.signature.value);
    }

    fn check_receipt(&mut self, receipt: &Receipt) {
        self.check_authority_attenuation("authority", &receipt.authority.attenuation);
        self.check_hash_prefixes(receipt);
        if let Some(lineage) = &receipt.lineage {
            self.check_child_receipt_refs("lineage.children", &lineage.children);
        }
        self.check_marketplace_contract(receipt);
        self.check_effect_grant_evidence(receipt);
        self.check_acts(&receipt.acts);
        self.check_act_legitimacy(receipt);
        self.check_decisions(&receipt.decisions, &receipt.acts);
        self.check_seal_criteria(receipt, &receipt.seal);
    }

    fn check_marketplace_contract(&mut self, receipt: &Receipt) {
        if !receipt.evidence.is_empty() && receipt.class != ReceiptClass::Mediated {
            self.push(
                ReceiptFindingCode::ReceiptEvidenceClassInvalid,
                "evidence",
                "inner receipt evidence is valid only on mediated receipts",
            );
        }
        if !receipt.evidence.is_empty() && receipt.subject.paid_invocation.is_none() {
            self.push(
                ReceiptFindingCode::ReceiptPaidBindingMissing,
                "subject.paid_invocation",
                "composite receipts require the outer paid invocation binding",
            );
        }
        if !receipt.evidence.is_empty()
            && receipt
                .subject
                .paid_invocation
                .as_ref()
                .is_some_and(|binding| binding.mediation.is_none())
        {
            self.push(
                ReceiptFindingCode::ReceiptPaidBindingMissing,
                "subject.paid_invocation.mediation",
                "composite receipts require immutable mediated listing terms",
            );
        }

        let lineage_children = receipt
            .lineage
            .as_ref()
            .map_or(&[][..], |lineage| lineage.children.as_slice());
        let mut evidence_uris = BTreeSet::new();
        for (index, evidence) in receipt.evidence.iter().enumerate() {
            let ReceiptEvidence::InnerReceipt {
                receipt_ref,
                expected,
            } = evidence;
            let path = format!("evidence[{index}]");
            if receipt_ref.reference_type != ReferenceType::Receipt {
                self.push(
                    ReceiptFindingCode::ChildReceiptRefInvalid,
                    format!("{path}.receipt_ref.type"),
                    "inner receipt evidence must use a receipt reference",
                );
            }
            if !evidence_uris.insert(receipt_ref.uri.as_str()) {
                self.push(
                    ReceiptFindingCode::DuplicateReceiptEvidence,
                    format!("{path}.receipt_ref"),
                    "inner receipt evidence refs must be unique",
                );
            }
            if lineage_children
                .iter()
                .any(|child| child.uri == receipt_ref.uri)
            {
                self.push(
                    ReceiptFindingCode::InnerReceiptLineageConflict,
                    format!("{path}.receipt_ref"),
                    "a composite pair must use the evidence edge, not a duplicate lineage edge",
                );
            }
            if expected.parent_binding.is_none() {
                self.push(
                    ReceiptFindingCode::InnerReceiptParentBindingMissing,
                    format!("{path}.expected.parent_binding"),
                    "inner receipt evidence must bind the outer paid invocation",
                );
            }
            if let Some(outer) = receipt.subject.paid_invocation.as_ref()
                && expected.vendor_ref != outer.vendor_ref
            {
                self.push(
                    ReceiptFindingCode::InnerReceiptBindingMismatch,
                    format!("{path}.expected.vendor_ref"),
                    "inner and outer paid invocation bindings must name the same vendor",
                );
            }
        }
    }

    fn check_authority_attenuation(&mut self, path: &str, attenuation: &AuthorityAttenuation) {
        match (&attenuation.parent_authority_ref, &attenuation.subset_proof) {
            (Some(parent), Some(proof)) if proof.parent_authority_ref == *parent => {}
            (Some(_), Some(_)) => self.push(
                ReceiptFindingCode::AuthorityAttenuationInvalid,
                format!("{path}.attenuation.subset_proof.parent_authority_ref"),
                "subset proof must cite the same parent authority ref",
            ),
            (Some(_), None) => self.push(
                ReceiptFindingCode::AuthorityAttenuationInvalid,
                format!("{path}.attenuation.subset_proof"),
                "parent authority refs require a subset proof",
            ),
            (None, Some(_)) => self.push(
                ReceiptFindingCode::AuthorityAttenuationInvalid,
                format!("{path}.attenuation.subset_proof"),
                "root authority must not carry a subset proof",
            ),
            (None, None) => {}
        }
    }

    fn check_hash_prefixes(&mut self, receipt: &Receipt) {
        self.check_sha256_prefix(
            "authority.enforcement.profile_hash",
            &receipt.authority.enforcement.profile_hash,
        );
        self.check_sha256_prefix("idempotency.intent_key", &receipt.idempotency.intent_key);
        self.check_sha256_prefix(
            "idempotency.trigger_fingerprint",
            &receipt.idempotency.trigger_fingerprint,
        );
        self.check_sha256_prefix(
            "idempotency.content_hash",
            &receipt.idempotency.content_hash,
        );
        self.check_sha256_prefix("digest", &receipt.digest);
        for (index, commitment) in receipt.subject.commitments.iter().enumerate() {
            self.check_commitment(&format!("subject.commitments[{index}]"), commitment);
        }
    }

    fn check_child_receipt_refs(&mut self, path: &str, refs: &[Reference]) {
        for (index, reference) in refs.iter().enumerate() {
            if reference.reference_type != ReferenceType::Receipt {
                self.push(
                    ReceiptFindingCode::ChildReceiptRefInvalid,
                    format!("{path}[{index}].type"),
                    "child receipt refs must use type receipt",
                );
            }
        }
    }

    fn check_effect_grant_evidence(&mut self, receipt: &Receipt) {
        if !receipt_has_effect_evidence(receipt) || !receipt.authority.grant_refs.is_empty() {
            return;
        }
        self.push(
            ReceiptFindingCode::EffectGrantEvidenceMissing,
            "authority.grant_refs",
            "receipts carrying effect evidence must identify the grant or capability refs that admitted the effect",
        );
    }

    fn check_acts(&mut self, acts: &[ReceiptAct]) {
        for (index, act) in acts.iter().enumerate() {
            let act_path = format!("acts[{index}]");
            if act.id.is_empty() {
                self.push(
                    ReceiptFindingCode::ActFormDetailsInvalid,
                    format!("{act_path}.id"),
                    "acts must carry a non-empty id",
                );
            }
            match act.form {
                ActForm::Revision => {
                    if act.revision.is_none() || act.verification.is_some() {
                        self.push(
                            ReceiptFindingCode::ActFormDetailsInvalid,
                            act_path,
                            "revision acts require revision details and must not carry verification details",
                        );
                    }
                }
                ActForm::Verification => {
                    if act.verification.is_none() || act.revision.is_some() {
                        self.push(
                            ReceiptFindingCode::ActFormDetailsInvalid,
                            act_path,
                            "verification acts require verification details and must not carry revision details",
                        );
                    }
                }
                ActForm::Reply | ActForm::Review | ActForm::Observation => {
                    if act.revision.is_some() || act.verification.is_some() {
                        self.push(
                            ReceiptFindingCode::ActFormDetailsInvalid,
                            act_path,
                            "reply, review, and observation acts must not carry revision or verification details",
                        );
                    }
                }
            }
        }
    }

    /// An act whose legitimacy names an execution term acts under a term the
    /// receipt's authority holds.
    fn check_act_legitimacy(&mut self, receipt: &Receipt) {
        for (index, act) in receipt.acts.iter().enumerate() {
            let Some(term_id) = legitimacy_authority_term(&act.intent.legitimacy) else {
                continue;
            };
            if !receipt
                .authority
                .terms
                .iter()
                .any(|term| term.term_id.as_str() == term_id)
            {
                self.push(
                    ReceiptFindingCode::ActLegitimacyTermMissing,
                    format!("acts[{index}].intent.legitimacy"),
                    "an act's legitimacy must name an execution term in the receipt's authority",
                );
            }
        }
    }

    /// The reasoning is inline; the `selected_act_id` integrity property is
    /// checked against the inline `acts[]` (no journal).
    fn check_decisions(&mut self, decisions: &[Decision], acts: &[ReceiptAct]) {
        let act_ids = act_ids(acts);
        for (index, decision) in decisions.iter().enumerate() {
            if let Some(act_id) = &decision.selected_act_id
                && !act_ids.contains(act_id.as_str())
            {
                self.push(
                    ReceiptFindingCode::DecisionSelectedActMissing,
                    format!("decisions[{index}].selected_act_id"),
                    "selected act id must refer to an act in the receipt",
                );
            }
        }
    }

    fn check_seal_criteria(&mut self, receipt: &Receipt, seal: &Seal) {
        let act_criteria = act_criterion_ids(&receipt.acts);
        for (index, criterion) in seal.criteria.iter().enumerate() {
            let criterion_path = format!("seal.criteria[{index}]");
            // A rolled-up seal criterion must be backed by a per-act criterion
            // binding (or declared success criterion) of the same id.
            if !receipt.acts.is_empty() && !act_criteria.contains(criterion.criterion_id.as_str()) {
                self.push(
                    ReceiptFindingCode::SealCriterionUnbound,
                    format!("{criterion_path}.criterion_id"),
                    "seal criterion must roll up an act criterion binding",
                );
            }
        }
    }

    fn check_commitment(&mut self, path: &str, commitment: &ReceiptCommitment) {
        self.check_sha256_prefix(&format!("{path}.value"), &commitment.value);
    }

    fn check_sha256_prefix(&mut self, path: &str, value: &str) {
        if !value.starts_with("sha256:") {
            self.push(
                ReceiptFindingCode::HashCommitmentInvalid,
                path,
                "hash values must use the sha256: prefix",
            );
        }
    }

    fn check_non_empty(&mut self, path: &str, value: &str) {
        if value.is_empty() {
            self.push(
                ReceiptFindingCode::EmptyEnvelopeField,
                path,
                "receipt envelope fields must not be empty",
            );
        }
    }

    fn push(
        &mut self,
        code: ReceiptFindingCode,
        path: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.findings.push(ReceiptFinding {
            code,
            path: path.into(),
            message: message.into(),
        });
    }
}

fn act_criterion_ids(acts: &[ReceiptAct]) -> BTreeSet<String> {
    acts.iter()
        .flat_map(|act| {
            act.criterion_bindings
                .iter()
                .map(|binding| binding.criterion_id.as_str().to_owned())
                .chain(
                    act.intent
                        .success_criteria
                        .iter()
                        .map(|criterion| criterion.criterion_id.as_str().to_owned()),
                )
        })
        .collect()
}

fn act_ids(acts: &[ReceiptAct]) -> BTreeSet<String> {
    acts.iter().map(|act| act.id.as_str().to_owned()).collect()
}

fn receipt_has_effect_evidence(receipt: &Receipt) -> bool {
    receipt.signals.iter().any(reference_is_effect_evidence)
        || receipt.decisions.iter().any(decision_has_effect_evidence)
        || receipt.acts.iter().any(act_has_effect_evidence)
        || receipt
            .seal
            .criteria
            .iter()
            .any(binding_has_effect_evidence)
}

fn decision_has_effect_evidence(decision: &Decision) -> bool {
    decision
        .inputs
        .signal_refs
        .iter()
        .any(reference_is_effect_evidence)
        || decision
            .inputs
            .target_ref
            .as_ref()
            .is_some_and(reference_is_effect_evidence)
        || decision
            .inputs
            .opportunity_refs
            .iter()
            .any(reference_is_effect_evidence)
        || decision
            .inputs
            .selection_ref
            .as_ref()
            .is_some_and(reference_is_effect_evidence)
        || decision
            .proposed_intent
            .derived_from
            .iter()
            .any(reference_is_effect_evidence)
        || decision
            .selected_harness_ref
            .as_ref()
            .is_some_and(reference_is_effect_evidence)
        || decision
            .justification
            .evidence_refs
            .iter()
            .any(reference_is_effect_evidence)
        || decision
            .artifact_refs
            .iter()
            .any(reference_is_effect_evidence)
}

fn act_has_effect_evidence(act: &ReceiptAct) -> bool {
    act.intent
        .derived_from
        .iter()
        .any(reference_is_effect_evidence)
        || act.source_refs.iter().any(reference_is_effect_evidence)
        || act.target_refs.iter().any(reference_is_effect_evidence)
        || act.artifact_refs.iter().any(reference_is_effect_evidence)
        || act
            .context_ref
            .as_ref()
            .is_some_and(reference_is_effect_evidence)
        || act
            .criterion_bindings
            .iter()
            .any(binding_has_effect_evidence)
}

fn binding_has_effect_evidence(binding: &CriterionBinding) -> bool {
    binding
        .evidence_refs
        .iter()
        .any(reference_is_effect_evidence)
        || binding
            .verification_refs
            .iter()
            .any(reference_is_effect_evidence)
}

fn reference_is_effect_evidence(reference: &Reference) -> bool {
    reference.proof_kind.as_ref() == Some(&ProofKind::EffectEvidence)
}
