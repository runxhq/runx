use std::collections::BTreeSet;
use std::path::Path;

use runx_contracts::{AuthorityTerm, Receipt, Reference};
use serde::{Deserialize, Serialize};

use super::RuntimeReceiptSignaturePolicy;
use super::archive::{
    ReceiptVerification, closure_status, exact_receipt_id, receipt_uri, verification_status,
};
use super::paths::safe_receipt_store_label;
use super::store::{LocalReceiptStore, ReceiptStoreError};

pub const RECEIPT_INSPECTION_SCHEMA: &str = "runx.receipt_inspection.v1";
pub const RECEIPT_INSPECTION_PROJECTOR_ID: &str = "runx-runtime.receipt-inspection.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptInspectionProjection {
    pub schema: String,
    pub projector_id: String,
    pub store_label: String,
    pub receipt: ReceiptInspection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptInspection {
    pub id: String,
    pub receipt_ref: String,
    pub subject_kind: String,
    pub subject_ref: String,
    pub created_at: String,
    pub status: String,
    pub verification: ReceiptVerification,
    pub authority: ReceiptAuthorityInspection,
    pub decisions: Vec<ReceiptDecisionInspection>,
    pub acts: Vec<ReceiptActInspection>,
    pub artifact_refs: Vec<String>,
    pub lineage_refs: Vec<String>,
    pub seal_reason_code: String,
    pub seal_summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptAuthorityInspection {
    pub actor_ref: String,
    pub grant_refs: Vec<String>,
    pub scope_refs: Vec<String>,
    pub exercised_scopes: Vec<ReceiptExercisedScope>,
    pub authority_proof_refs: Vec<String>,
    pub approval_refs: Vec<String>,
    pub term_count: usize,
    pub parent_authority_ref: Option<String>,
    pub subset_proof_present: bool,
    pub enforcement_profile_hash: String,
    pub redaction_refs: Vec<String>,
    pub credential_ref_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReceiptExercisedScope {
    pub scope: String,
    pub source: String,
    pub term_id: Option<String>,
    pub resource_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptDecisionInspection {
    pub id: String,
    pub choice: String,
    pub selected_act_id: Option<String>,
    pub summary: String,
    pub evidence_refs: Vec<String>,
    pub artifact_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptActInspection {
    pub id: String,
    pub form: String,
    pub purpose: String,
    pub legitimacy: String,
    pub summary: String,
    pub disposition: String,
    pub reason_code: String,
    pub source_refs: Vec<String>,
    pub target_refs: Vec<String>,
    pub artifact_refs: Vec<String>,
    pub criterion_statuses: Vec<ReceiptCriterionInspection>,
    pub context_ref_present: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptCriterionInspection {
    pub criterion_id: String,
    pub status: String,
    pub evidence_refs: Vec<String>,
    pub verification_refs: Vec<String>,
}

pub fn inspect_local_receipt(
    store: &LocalReceiptStore,
    workspace_base: &Path,
    project_runx_dir: &Path,
    receipt_reference: &str,
) -> Result<ReceiptInspectionProjection, ReceiptStoreError> {
    inspect_local_receipt_with_policy(
        store,
        workspace_base,
        project_runx_dir,
        receipt_reference,
        RuntimeReceiptSignaturePolicy::local_development(),
    )
}

pub fn inspect_local_receipt_with_policy(
    store: &LocalReceiptStore,
    workspace_base: &Path,
    project_runx_dir: &Path,
    receipt_reference: &str,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<ReceiptInspectionProjection, ReceiptStoreError> {
    let receipt_id = exact_receipt_id(receipt_reference);
    let receipt = store.read_exact_without_proof_for_history(&receipt_id)?;
    let store_label = safe_receipt_store_label(store.root(), workspace_base, project_runx_dir);
    Ok(ReceiptInspectionProjection {
        schema: RECEIPT_INSPECTION_SCHEMA.to_owned(),
        projector_id: RECEIPT_INSPECTION_PROJECTOR_ID.to_owned(),
        store_label: store_label.as_str().to_owned(),
        receipt: project_receipt_inspection_with_policy(&receipt, signature_policy),
    })
}

#[must_use]
pub fn project_receipt_inspection(receipt: &Receipt) -> ReceiptInspection {
    project_receipt_inspection_with_policy(
        receipt,
        RuntimeReceiptSignaturePolicy::local_development(),
    )
}

#[must_use]
pub fn project_receipt_inspection_with_policy(
    receipt: &Receipt,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> ReceiptInspection {
    let decisions = decision_inspections(receipt);
    let acts = act_inspections(receipt);
    let artifact_refs = inspection_artifact_refs(&decisions, &acts);

    ReceiptInspection {
        id: receipt.id.to_string(),
        receipt_ref: receipt_uri(&receipt.id),
        subject_kind: receipt.subject.kind.to_string(),
        subject_ref: receipt.subject.reference.uri.to_string(),
        created_at: receipt.created_at.to_string(),
        status: closure_status(&receipt.seal.disposition),
        verification: ReceiptVerification {
            status: verification_status(receipt, signature_policy),
        },
        authority: authority_inspection(receipt),
        decisions,
        acts,
        artifact_refs,
        lineage_refs: inspection_lineage_refs(receipt),
        seal_reason_code: receipt.seal.reason_code.to_string(),
        seal_summary: receipt.seal.summary.to_string(),
    }
}

fn decision_inspections(receipt: &Receipt) -> Vec<ReceiptDecisionInspection> {
    receipt
        .decisions
        .iter()
        .map(|decision| ReceiptDecisionInspection {
            id: decision.decision_id.to_string(),
            choice: wire_label(&decision.choice),
            selected_act_id: decision.selected_act_id.as_ref().map(ToString::to_string),
            summary: decision.justification.summary.to_string(),
            evidence_refs: refs(&decision.justification.evidence_refs),
            artifact_refs: refs(&decision.artifact_refs),
        })
        .collect()
}

fn act_inspections(receipt: &Receipt) -> Vec<ReceiptActInspection> {
    receipt
        .acts
        .iter()
        .map(|act| ReceiptActInspection {
            id: act.id.to_string(),
            form: wire_label(&act.form),
            purpose: act.intent.purpose.to_string(),
            legitimacy: act.intent.legitimacy.to_string(),
            summary: act.summary.to_string(),
            disposition: closure_status(&act.closure.disposition),
            reason_code: act.closure.reason_code.to_string(),
            source_refs: refs(&act.source_refs),
            target_refs: refs(&act.target_refs),
            artifact_refs: refs(&act.artifact_refs),
            criterion_statuses: act
                .criterion_bindings
                .iter()
                .map(|criterion| ReceiptCriterionInspection {
                    criterion_id: criterion.criterion_id.to_string(),
                    status: wire_label(&criterion.status),
                    evidence_refs: refs(&criterion.evidence_refs),
                    verification_refs: refs(&criterion.verification_refs),
                })
                .collect(),
            context_ref_present: act.context_ref.is_some(),
        })
        .collect()
}

fn inspection_artifact_refs(
    decisions: &[ReceiptDecisionInspection],
    acts: &[ReceiptActInspection],
) -> Vec<String> {
    decisions
        .iter()
        .flat_map(|decision| decision.artifact_refs.iter().cloned())
        .chain(
            acts.iter()
                .flat_map(|act| act.artifact_refs.iter().cloned()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn inspection_lineage_refs(receipt: &Receipt) -> Vec<String> {
    receipt
        .lineage
        .as_ref()
        .into_iter()
        .flat_map(|lineage| {
            lineage
                .parent
                .iter()
                .chain(lineage.previous.iter())
                .chain(lineage.children.iter())
        })
        .map(|reference| reference.uri.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn authority_inspection(receipt: &Receipt) -> ReceiptAuthorityInspection {
    let authority = &receipt.authority;
    let scope_refs = refs(&authority.scope_refs);
    let mut exercised_scopes = authority
        .scope_refs
        .iter()
        .map(|reference| ReceiptExercisedScope {
            scope: scope_from_reference(reference),
            source: "scope_ref".to_owned(),
            term_id: None,
            resource_ref: Some(reference.uri.to_string()),
        })
        .chain(authority.terms.iter().flat_map(term_scopes))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    exercised_scopes.sort();
    let approval_refs = authority
        .terms
        .iter()
        .flat_map(|term| term.approvals.iter())
        .map(|approval| approval.approval_ref.uri.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    ReceiptAuthorityInspection {
        actor_ref: authority.actor_ref.uri.to_string(),
        grant_refs: refs(&authority.grant_refs),
        scope_refs,
        exercised_scopes,
        authority_proof_refs: refs(&authority.authority_proof_refs),
        approval_refs,
        term_count: authority.terms.len(),
        parent_authority_ref: authority
            .attenuation
            .parent_authority_ref
            .as_ref()
            .map(|reference| reference.uri.to_string()),
        subset_proof_present: authority.attenuation.subset_proof.is_some(),
        enforcement_profile_hash: authority.enforcement.profile_hash.to_string(),
        redaction_refs: refs(&authority.enforcement.redaction_refs),
        credential_ref_count: authority
            .terms
            .iter()
            .filter(|term| term.credential_ref.is_some())
            .count(),
    }
}

fn term_scopes(term: &AuthorityTerm) -> impl Iterator<Item = ReceiptExercisedScope> + '_ {
    let family = wire_label(&term.resource_family);
    term.verbs.iter().map(move |verb| ReceiptExercisedScope {
        scope: format!("{family}:{}", wire_label(verb)),
        source: "authority_term".to_owned(),
        term_id: Some(term.term_id.to_string()),
        resource_ref: Some(term.resource_ref.uri.to_string()),
    })
}

fn scope_from_reference(reference: &Reference) -> String {
    let uri = reference.uri.as_str();
    uri.strip_prefix("runx:scope_admission:")
        .or_else(|| uri.strip_prefix("runx:scope:"))
        .unwrap_or(uri)
        .replace('.', ":")
}

fn refs(references: &[Reference]) -> Vec<String> {
    references
        .iter()
        .map(|reference| reference.uri.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn wire_label<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}
