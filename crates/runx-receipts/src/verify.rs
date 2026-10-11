// Module rationale: receipt verification keeps structural and proof checks co-located for offline audit.
use std::collections::BTreeSet;

use runx_contracts::{
    Criterion, CriterionBinding, Decision, ProofKind, Receipt, ReceiptAct, Reference,
    ReferenceType, Seal, legitimacy_authority_term,
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
    receipt_id_is_content_addressed, validate_receipt_proof, verify_receipt_proof,
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
        self.check_non_empty("issuer.kid", &receipt.issuer.kid);
        self.check_sha256_prefix(
            "issuer.public_key_sha256",
            &receipt.issuer.public_key_sha256,
        );
        self.check_non_empty("signature.value", &receipt.signature.value);
    }

    fn check_receipt(&mut self, receipt: &Receipt) {
        self.check_sha256_prefix("digest", &receipt.digest);
        self.check_child_receipt_refs("lineage.children", &receipt.lineage.children);
        self.check_effect_grant_evidence(receipt);
        self.check_act_legitimacy(receipt);
        self.check_decisions(&receipt.decisions, &receipt.acts);
        self.check_seal_criteria(receipt, &receipt.seal);
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
            if !act_ids.contains(decision.selected_act_id.as_str()) {
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
            // binding of the same criterion.
            if !receipt.acts.is_empty() && !act_criteria.contains(&criterion.criterion_id) {
                self.push(
                    ReceiptFindingCode::SealCriterionUnbound,
                    format!("{criterion_path}.criterion_id"),
                    "seal criterion must roll up an act criterion binding",
                );
            }
        }
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

fn act_criterion_ids(acts: &[ReceiptAct]) -> BTreeSet<Criterion> {
    acts.iter()
        .flat_map(|act| &act.criterion_bindings)
        .map(|binding| binding.criterion_id)
        .collect()
}

fn act_ids(acts: &[ReceiptAct]) -> BTreeSet<String> {
    acts.iter().map(|act| act.id.as_str().to_owned()).collect()
}

/// Effect evidence lives in an act's references and criteria, or in the
/// seal's criteria for a step that made no governed call. A decision carries
/// none of its own: it names the act that holds it.
fn receipt_has_effect_evidence(receipt: &Receipt) -> bool {
    receipt.acts.iter().any(act_has_effect_evidence)
        || receipt
            .seal
            .criteria
            .iter()
            .any(binding_has_effect_evidence)
}

fn act_has_effect_evidence(act: &ReceiptAct) -> bool {
    act.target_refs.iter().any(reference_is_effect_evidence)
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
