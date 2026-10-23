//! The receipt archive: a filtered listing of the signed receipts a receipt
//! directory holds, each with its verification status.
//!
//! Receipts are the evidence a run seals, not the run's state. This listing
//! browses that evidence; whether a run is paused, running or sealed is read
//! from its record (`execution::run_record`), never from here.

use std::collections::BTreeSet;

use runx_contracts::{Receipt, SealReason};
use runx_core::policy::parse_rfc3339_moment;
use runx_receipts::{
    ReceiptFindingCode, ReceiptProofContextProvider, signed_display_identity, verify_receipt_proof,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::paths::safe_receipt_store_label;
use super::store::{LocalReceiptStore, ReceiptStoreError};
use super::{RuntimeReceiptProofContextProvider, RuntimeReceiptSignaturePolicy};
use crate::paths::RunxPaths;

pub const RECEIPT_LISTING_PROJECTOR_ID: &str = "runx-runtime.receipt-archive.v1";
pub const RECEIPT_REF_PREFIX: &str = "runx:receipt:";

/// Which receipts to list. Text filters match case-insensitively.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReceiptFilter {
    pub query: Option<String>,
    pub skill: Option<String>,
    /// How the receipt's subject ended.
    pub outcome: Option<String>,
    pub issuer: Option<String>,
    pub actor: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<usize>,
    /// Include receipts another receipt commits as a child. By default the
    /// listing shows one receipt per top-level act.
    pub include_internal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptListing {
    pub projector_id: String,
    pub store_label: String,
    pub receipts: Vec<ReceiptRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptRow {
    pub id: String,
    pub receipt_uri: String,
    /// The signed subject: the run, or one node of it.
    pub subject_uri: String,
    /// The graph that ran.
    pub name: String,
    /// How the receipt's subject ended.
    pub outcome: String,
    pub created_at: String,
    /// Why the receipt's subject ended as it did, when its outcome alone
    /// does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<SealReason>,
    pub issuer_type: String,
    /// The actor the receipt's authority names, when it names one.
    pub actor: Option<String>,
    pub verification: ReceiptVerification,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptVerification {
    pub status: String,
}

#[derive(Debug, Error)]
pub enum ReceiptListingError {
    #[error(transparent)]
    ReceiptStore(#[from] ReceiptStoreError),
    #[error("invalid {field} timestamp '{value}': expected RFC 3339 timestamp")]
    InvalidTimestamp { field: &'static str, value: String },
}

pub fn list_receipts(
    store: &LocalReceiptStore,
    paths: &RunxPaths,
    filter: &ReceiptFilter,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<ReceiptListing, ReceiptListingError> {
    let label = safe_receipt_store_label(store.root(), paths);
    let filter = ResolvedFilter::parse(filter)?;
    let receipts = match store.list_without_proof_for_history() {
        Ok(receipts) => receipts,
        Err(ReceiptStoreError::MissingStore { .. }) => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let internal = if filter.include_internal {
        BTreeSet::new()
    } else {
        child_receipt_ids(&receipts)
    };
    let mut rows = receipts
        .iter()
        .filter(|receipt| !internal.contains(receipt.id.as_ref()))
        .map(|receipt| row(receipt, signature_policy))
        .filter(|row| filter.matches(row))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(limit) = filter.limit {
        rows.truncate(limit);
    }
    Ok(ReceiptListing {
        projector_id: RECEIPT_LISTING_PROJECTOR_ID.to_owned(),
        store_label: label.as_str().to_owned(),
        receipts: rows,
    })
}

#[must_use]
pub fn receipt_uri(receipt_id: &str) -> String {
    format!("{RECEIPT_REF_PREFIX}{receipt_id}")
}

/// A receipt id from an id or a `runx:receipt:` reference.
#[must_use]
pub fn exact_receipt_id(reference: &str) -> String {
    reference
        .strip_prefix(RECEIPT_REF_PREFIX)
        .unwrap_or(reference)
        .to_owned()
}

pub(crate) fn verification_status(
    receipt: &Receipt,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> String {
    let proof_contexts = RuntimeReceiptProofContextProvider::new(signature_policy);
    let context = proof_contexts.proof_context(receipt);
    let verification = verify_receipt_proof(receipt, &context);
    if verification.findings.is_empty() {
        "verified".to_owned()
    } else if verification
        .findings
        .iter()
        .all(|finding| matches!(finding.code, ReceiptFindingCode::SignatureKeyMissing))
    {
        "unverified".to_owned()
    } else {
        "invalid".to_owned()
    }
}

fn row(receipt: &Receipt, signature_policy: RuntimeReceiptSignaturePolicy<'_>) -> ReceiptRow {
    let identity = signed_display_identity(receipt);
    let name = receipt
        .subject
        .reference
        .label
        .as_ref()
        .map_or_else(|| identity.subject_uri.clone(), ToString::to_string);
    ReceiptRow {
        id: receipt.id.to_string(),
        receipt_uri: receipt_uri(&receipt.id),
        subject_uri: identity.subject_uri,
        name,
        outcome: receipt.seal.outcome.label().to_owned(),
        created_at: receipt.created_at.to_string(),
        reason: receipt.seal.reason,
        issuer_type: identity.issuer_type,
        actor: identity.actor,
        verification: ReceiptVerification {
            status: verification_status(receipt, signature_policy),
        },
    }
}

fn child_receipt_ids(receipts: &[Receipt]) -> BTreeSet<String> {
    receipts
        .iter()
        .flat_map(|receipt| receipt.lineage.children.iter())
        .filter_map(|reference| reference.uri.strip_prefix(RECEIPT_REF_PREFIX))
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ResolvedFilter {
    query: Option<String>,
    skill: Option<String>,
    outcome: Option<String>,
    issuer: Option<String>,
    actor: Option<String>,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    limit: Option<usize>,
    include_internal: bool,
}

impl ResolvedFilter {
    fn parse(filter: &ReceiptFilter) -> Result<Self, ReceiptListingError> {
        Ok(Self {
            query: normalized(&filter.query),
            skill: normalized(&filter.skill),
            outcome: normalized(&filter.outcome),
            issuer: normalized(&filter.issuer),
            actor: normalized(&filter.actor),
            since: parse_date_filter("since", &filter.since)?,
            until: parse_date_filter("until", &filter.until)?,
            limit: filter.limit,
            include_internal: filter.include_internal,
        })
    }

    fn matches(&self, row: &ReceiptRow) -> bool {
        let name = row.name.to_lowercase();
        let subject = row.subject_uri.to_lowercase();
        self.query.as_ref().is_none_or(|query| {
            name.contains(query)
                || subject.contains(query)
                || row.id.to_lowercase().contains(query)
                || row.issuer_type.to_lowercase().contains(query)
                || row
                    .actor
                    .as_ref()
                    .is_some_and(|actor| actor.to_lowercase().contains(query))
        }) && self.skill.as_ref().is_none_or(|skill| name.contains(skill))
            && self
                .outcome
                .as_ref()
                .is_none_or(|outcome| row.outcome.to_lowercase() == *outcome)
            && self
                .issuer
                .as_ref()
                .is_none_or(|issuer| row.issuer_type.to_lowercase() == *issuer)
            && self.actor.as_ref().is_none_or(|actor| {
                row.actor
                    .as_ref()
                    .is_some_and(|row_actor| row_actor.to_lowercase() == *actor)
            })
            && self.within_window(&row.created_at)
    }

    fn within_window(&self, timestamp: &str) -> bool {
        let Some(parsed) = Timestamp::parse(timestamp) else {
            return self.since.is_none() && self.until.is_none();
        };
        self.since.is_none_or(|since| parsed >= since)
            && self.until.is_none_or(|until| parsed <= until)
    }
}

fn normalized(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|entry| entry.trim().to_lowercase())
        .filter(|entry| !entry.is_empty())
}

fn parse_date_filter(
    field: &'static str,
    value: &Option<String>,
) -> Result<Option<Timestamp>, ReceiptListingError> {
    let Some(value) = value
        .as_ref()
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
    else {
        return Ok(None);
    };
    Timestamp::parse(value)
        .map(Some)
        .ok_or_else(|| ReceiptListingError::InvalidTimestamp {
            field,
            value: value.to_owned(),
        })
}

/// An RFC 3339 instant, comparable across offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Timestamp {
    epoch_seconds: i64,
    nanos: u32,
}

impl Timestamp {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let (days, seconds_of_day, nanos) = parse_rfc3339_moment(value)?;
        let epoch_seconds = days.checked_mul(86_400)?.checked_add(seconds_of_day)?;
        Some(Self {
            epoch_seconds,
            nanos,
        })
    }
}
