use std::collections::BTreeSet;

use runx_contracts::{
    AuthorityTerm, CriterionBinding, Receipt, ReceiptIssuerType, Reference, SealReason, SubjectKind,
};
use serde::{Deserialize, Serialize};

use super::RuntimeReceiptSignaturePolicy;
use super::archive::{ReceiptVerification, exact_receipt_id, receipt_uri, verification_status};
use super::paths::safe_receipt_store_label;
use super::store::{LocalReceiptStore, ReceiptStoreError};
use crate::paths::RunxPaths;

pub const RECEIPT_INSPECTION_SCHEMA: &str = "runx.receipt_inspection.v1";
pub const RECEIPT_INSPECTION_PROJECTOR_ID: &str = "runx-runtime.receipt-inspection.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptInspectionProjection {
    pub schema: String,
    pub projector_id: String,
    pub store_label: String,
    pub receipt: ReceiptInspection,
}

/// A receipt as a reader inspects it. Each `_uri` is the URI of the receipt
/// `Reference` it projects; the receipt itself keeps its own field names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptInspection {
    pub id: String,
    pub receipt_uri: String,
    pub subject_kind: SubjectKind,
    pub subject_uri: String,
    pub created_at: String,
    /// How the receipt's subject ended.
    pub outcome: String,
    /// The kind of identity that signed the receipt. A verified local issuer
    /// is a runx home's own key; whether that suffices is the reader's call.
    pub issuer_type: ReceiptIssuerType,
    pub verification: ReceiptVerification,
    pub authority: ReceiptAuthorityInspection,
    pub decisions: Vec<ReceiptDecisionInspection>,
    pub acts: Vec<ReceiptActInspection>,
    pub artifact_uris: Vec<String>,
    pub lineage_uris: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal_reason: Option<SealReason>,
    /// The seal's criteria: how the step ended and the evidence of it,
    /// whether or not the step made a governed call.
    pub seal_criteria: Vec<ReceiptCriterionInspection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptAuthorityInspection {
    pub actor_uri: Option<String>,
    pub grant_uris: Vec<String>,
    pub scope_uris: Vec<String>,
    pub exercised_scopes: Vec<ReceiptExercisedScope>,
    pub approval_uris: Vec<String>,
    pub term_count: usize,
    pub credential_ref_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReceiptExercisedScope {
    pub scope: String,
    pub source: String,
    pub term_id: Option<String>,
    pub resource_uri: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptDecisionInspection {
    pub id: String,
    pub choice: String,
    pub selected_act_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptActInspection {
    pub id: String,
    pub form: String,
    pub purpose: String,
    pub legitimacy: String,
    /// What the caller told a reader about this call, when it said anything.
    pub reason: Option<String>,
    pub target_uris: Vec<String>,
    pub artifact_uris: Vec<String>,
    pub criterion_statuses: Vec<ReceiptCriterionInspection>,
    pub context_ref_present: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptCriterionInspection {
    pub criterion_id: String,
    pub status: String,
    pub evidence_uris: Vec<String>,
    pub verification_uris: Vec<String>,
}

pub fn inspect_local_receipt(
    store: &LocalReceiptStore,
    paths: &RunxPaths,
    receipt_reference: &str,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<ReceiptInspectionProjection, ReceiptStoreError> {
    let receipt_id = exact_receipt_id(receipt_reference);
    let receipt = store.read_exact_without_proof_for_history(&receipt_id)?;
    let store_label = safe_receipt_store_label(store.root(), paths);
    Ok(ReceiptInspectionProjection {
        schema: RECEIPT_INSPECTION_SCHEMA.to_owned(),
        projector_id: RECEIPT_INSPECTION_PROJECTOR_ID.to_owned(),
        store_label: store_label.as_str().to_owned(),
        receipt: project_receipt_inspection(&receipt, signature_policy),
    })
}

#[must_use]
pub fn project_receipt_inspection(
    receipt: &Receipt,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> ReceiptInspection {
    let decisions = decision_inspections(receipt);
    let acts = act_inspections(receipt);
    let artifact_uris = inspection_artifact_uris(&acts);

    ReceiptInspection {
        id: receipt.id.to_string(),
        receipt_uri: receipt_uri(&receipt.id),
        subject_kind: receipt.subject.kind,
        subject_uri: receipt.subject.reference.uri.to_string(),
        created_at: receipt.created_at.to_string(),
        outcome: receipt.seal.outcome.label().to_owned(),
        issuer_type: receipt.issuer.issuer_type.clone(),
        verification: ReceiptVerification {
            status: verification_status(receipt, signature_policy),
        },
        authority: authority_inspection(receipt),
        decisions,
        acts,
        artifact_uris,
        lineage_uris: inspection_lineage_uris(receipt),
        seal_reason: receipt.seal.reason,
        seal_criteria: criterion_inspections(&receipt.seal.criteria),
    }
}

fn decision_inspections(receipt: &Receipt) -> Vec<ReceiptDecisionInspection> {
    receipt
        .decisions
        .iter()
        .map(|decision| ReceiptDecisionInspection {
            id: decision.decision_id.to_string(),
            choice: wire_label(&decision.choice),
            selected_act_id: decision.selected_act_id.to_string(),
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
            reason: act.reason.as_ref().map(ToString::to_string),
            target_uris: uris(&act.target_refs),
            artifact_uris: uris(&act.artifact_refs),
            criterion_statuses: criterion_inspections(&act.criterion_bindings),
            context_ref_present: act.context_ref.is_some(),
        })
        .collect()
}

fn criterion_inspections(criteria: &[CriterionBinding]) -> Vec<ReceiptCriterionInspection> {
    criteria
        .iter()
        .map(|criterion| ReceiptCriterionInspection {
            criterion_id: criterion.criterion_id.as_str().to_owned(),
            status: wire_label(&criterion.status),
            evidence_uris: uris(&criterion.evidence_refs),
            verification_uris: uris(&criterion.verification_refs),
        })
        .collect()
}

fn inspection_artifact_uris(acts: &[ReceiptActInspection]) -> Vec<String> {
    acts.iter()
        .flat_map(|act| act.artifact_uris.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn inspection_lineage_uris(receipt: &Receipt) -> Vec<String> {
    receipt
        .lineage
        .children
        .iter()
        .map(|reference| reference.uri.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn authority_inspection(receipt: &Receipt) -> ReceiptAuthorityInspection {
    let authority = &receipt.authority;
    let scope_uris = uris(&authority.scope_refs);
    let mut exercised_scopes = authority
        .scope_refs
        .iter()
        .map(|reference| ReceiptExercisedScope {
            scope: scope_from_reference(reference),
            source: "scope_ref".to_owned(),
            term_id: None,
            resource_uri: Some(reference.uri.to_string()),
        })
        .chain(authority.terms.iter().flat_map(term_scopes))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    exercised_scopes.sort();
    let approval_uris = authority
        .terms
        .iter()
        .flat_map(|term| term.approvals.iter())
        .map(|approval| approval.approval_ref.uri.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    ReceiptAuthorityInspection {
        actor_uri: authority
            .actor_ref
            .as_ref()
            .map(|actor| actor.uri.to_string()),
        grant_uris: uris(&authority.grant_refs),
        scope_uris,
        exercised_scopes,
        approval_uris,
        term_count: authority.terms.len(),
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
        resource_uri: Some(term.resource_ref.uri.to_string()),
    })
}

fn scope_from_reference(reference: &Reference) -> String {
    let uri = reference.uri.as_str();
    uri.strip_prefix("runx:scope_admission:")
        .or_else(|| uri.strip_prefix("runx:scope:"))
        .unwrap_or(uri)
        .replace('.', ":")
}

fn uris(references: &[Reference]) -> Vec<String> {
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
