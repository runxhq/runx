//! Effect custody: one stream per act.
//!
//! An act is one operation on one instance's target, named by the caller's
//! request key. Every attempt of the act shares its stream, from whichever
//! run makes it, so an outcome one run left unknown holds the next run that
//! tries the act. A read changes nothing another run could duplicate; it
//! carries no key and is named by its exact intent within its run.
//!
//! A mutation is recorded before it is sent and settled after: attempted,
//! then read back and finalized, rejected (the provider refused before
//! executing), or unknown. An unknown outcome is held. The next attempt of
//! the same intent resumes from the record: a cached readback finalizes
//! without resending, a recorded acknowledgement is read back without
//! resending, and anything else is resent only when the operation's
//! guarantees or the use's tolerance allow it.
//!
//! The record pins the execution term, instance and implementation the call
//! was admitted against. Recovery under different evidence holds; it never
//! silently continues under a newer one.
//!
//! The attempts from one first attempt to its settlement are a sequence, and
//! a sequence takes one use of bounded evidence however many attempts recover
//! it. Repeating a settled act opens a new sequence and takes a new use.

use std::sync::Arc;

use runx_contracts::connector::Digest;
use runx_contracts::{JsonObject, JsonValue, OperationIntent, Reference, sha256_prefixed};

use super::operation::packet::Readback;
use serde::{Deserialize, Serialize};

use super::store::{Append, Expected, KernelStore, KernelStoreError, StreamEvent};
use super::usage::UseCover;

/// Who approved an attempt, recorded so a recovery keeps the same lane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApprovalRecord {
    pub actor: ApprovalActor,
    pub key: String,
    /// When this call first accepted the approval evidence. This is the
    /// person's approval time only for an exact human resolution.
    pub recorded_at: String,
}

impl ApprovalRecord {
    pub(crate) fn new(actor: ApprovalActor, key: String) -> Self {
        Self {
            actor,
            key,
            recorded_at: crate::time::now_iso8601(),
        }
    }

    /// The approval as a receipt names it: who approved, and the key of the
    /// exact intent approved.
    pub(crate) fn uri(&self) -> String {
        format!("runx:approval:{}:{}", self.actor.as_str(), self.key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApprovalActor {
    /// A person approved this exact intent.
    Human,
}

impl ApprovalActor {
    /// The actor's wire name.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
        }
    }
}

/// The provider's acknowledgement of a local HTTP write, kept until readback
/// establishes finality. Recovery state, never proof of success.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Acknowledgement {
    pub response: JsonObject,
    pub write_result: JsonValue,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum EffectEvent {
    Attempted {
        intent_digest: String,
        authority_term_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mandate_ref: Option<Reference>,
        idempotency_key: String,
        /// The binding digest of the instance attempted under: its account,
        /// principal and credential material.
        instance_binding: String,
        attempt: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval: Option<ApprovalRecord>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        implementation_digest: Option<String>,
    },
    Acknowledged {
        acknowledgement: Acknowledgement,
    },
    ReadBack {
        readback: Readback,
    },
    Unknown {
        reason: String,
    },
    Rejected {
        code: String,
    },
    Finalized {
        receipt_id: String,
    },
}

impl StreamEvent for EffectEvent {
    const KIND: &'static str = "effect";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Attempted { .. } => "attempted",
            Self::Acknowledged { .. } => "acknowledged",
            Self::ReadBack { .. } => "read_back",
            Self::Unknown { .. } => "unknown",
            Self::Rejected { .. } => "rejected",
            Self::Finalized { .. } => "finalized",
        }
    }
}

/// An attempt that has not settled.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pending {
    pub intent_digest: String,
    pub authority_term_id: String,
    pub mandate_ref: Option<Reference>,
    pub idempotency_key: String,
    pub instance_binding: String,
    pub attempt: u32,
    pub approval: Option<ApprovalRecord>,
    pub implementation_digest: Option<String>,
    pub acknowledgement: Option<Acknowledgement>,
    pub readback: Option<Readback>,
    pub unknown: bool,
}

/// What the next attempt records.
pub(crate) struct Attempt<'a> {
    pub intent_digest: &'a str,
    pub authority_term_id: &'a str,
    pub mandate_ref: Option<&'a Reference>,
    pub idempotency_key: &'a str,
    /// The instance's binding digest: a recovery under an instance confirmed
    /// again with other material is another attempt, never this one.
    pub instance_binding: &'a str,
    pub attempt: u32,
    pub approval: Option<&'a ApprovalRecord>,
    pub implementation_digest: Option<&'a str>,
    /// The stream version admission read. The attempt appends only onto it,
    /// so a call admitted before another run attempted the same act is
    /// refused, never sent.
    pub admitted_at: u64,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CustodyError {
    #[error("{0}")]
    Mismatch(&'static str),
    #[error(transparent)]
    Store(#[from] KernelStoreError),
    #[error(transparent)]
    Spend(#[from] super::spend::SpendError),
}

/// The act's custody as last recorded: its unsettled attempt, if any, the
/// stream version the attempt's sequence opened from, or the next one opens
/// from, and the version this read saw.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Custody {
    pub pending: Option<Pending>,
    pub sequence: u64,
    pub head: u64,
}

/// The custody stream of one act.
#[derive(Clone)]
pub(crate) struct EffectCustody {
    store: Arc<KernelStore>,
    key: String,
}

impl std::fmt::Debug for EffectCustody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EffectCustody")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl PartialEq for EffectCustody {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.store.path() == other.store.path()
    }
}

impl EffectCustody {
    /// The custody of the act `intent` performs in run `run_id`.
    pub(crate) fn for_act(
        store: Arc<KernelStore>,
        intent: &OperationIntent,
        intent_digest: &Digest,
        run_id: &str,
    ) -> Self {
        let key = match &intent.request_key_digest {
            Some(request_key) => named_act(
                intent.connector.as_str(),
                intent.operation.as_str(),
                intent.instance_id.as_str(),
                intent.target.as_str(),
                request_key,
            ),
            None => sha256_prefixed(format!("read\0{run_id}\0{intent_digest}").as_bytes()),
        };
        Self { store, key }
    }

    /// The key of the use an attempt of `intent_digest` takes in `sequence`.
    pub(crate) fn use_key(&self, sequence: u64, intent_digest: &str) -> String {
        sha256_prefixed(format!("{}\0{sequence}\0{intent_digest}", self.key).as_bytes())
    }

    /// The unsettled attempt, if any.
    pub(crate) fn pending(&self) -> Result<Option<Pending>, CustodyError> {
        Ok(self.custody()?.pending)
    }

    /// The act's custody. A settled stream answers from its verified latest
    /// event, so an act repeated many times never rereads its history.
    pub(crate) fn custody(&self) -> Result<Custody, CustodyError> {
        let latest = self.store.latest::<EffectEvent>(&self.key)?;
        let settled = latest.as_ref().is_none_or(|last| {
            matches!(
                last.event,
                EffectEvent::Finalized { .. } | EffectEvent::Rejected { .. }
            )
        });
        if settled {
            let head = latest.map_or(0, |last| last.version);
            return Ok(Custody {
                pending: None,
                sequence: head,
                head,
            });
        }
        let mut sequence = 0;
        let mut head = 0;
        let mut pending: Option<Pending> = None;
        for recorded in self.store.read::<EffectEvent>(&self.key)? {
            head = recorded.version;
            match recorded.event {
                EffectEvent::Attempted {
                    intent_digest,
                    authority_term_id,
                    mandate_ref,
                    idempotency_key,
                    instance_binding,
                    attempt,
                    approval,
                    implementation_digest,
                } => {
                    let prior = pending.take();
                    pending = Some(Pending {
                        intent_digest,
                        authority_term_id,
                        mandate_ref,
                        idempotency_key,
                        instance_binding,
                        attempt,
                        approval,
                        implementation_digest,
                        acknowledgement: prior
                            .as_ref()
                            .and_then(|prior| prior.acknowledgement.clone()),
                        readback: prior.and_then(|prior| prior.readback),
                        unknown: false,
                    });
                }
                EffectEvent::Acknowledged { acknowledgement } => {
                    if let Some(pending) = pending.as_mut() {
                        pending.acknowledgement = Some(acknowledgement);
                    }
                }
                EffectEvent::ReadBack { readback } => {
                    if let Some(pending) = pending.as_mut() {
                        pending.readback = Some(readback);
                    }
                }
                EffectEvent::Unknown { .. } => {
                    if let Some(pending) = pending.as_mut() {
                        pending.unknown = true;
                    }
                }
                EffectEvent::Rejected { .. } | EffectEvent::Finalized { .. } => {
                    pending = None;
                    sequence = recorded.version;
                }
            }
        }
        Ok(Custody {
            pending,
            sequence,
            head,
        })
    }

    /// Record an attempt, every evidence use it takes and what it spends in
    /// one commit.
    pub(crate) fn attempt(
        &self,
        attempt: &Attempt<'_>,
        bounded: &[UseCover],
        spend: Option<&super::spend::SpendCover>,
    ) -> Result<(), CustodyError> {
        if let Some(pending) = self.pending()? {
            if pending.intent_digest != attempt.intent_digest
                || pending.authority_term_id != attempt.authority_term_id
                || pending.mandate_ref.as_ref() != attempt.mandate_ref
                || pending.idempotency_key != attempt.idempotency_key
                || pending.instance_binding != attempt.instance_binding
            {
                return Err(CustodyError::Mismatch(
                    "the pending attempt has different intent, authority, or instance evidence",
                ));
            }
            if pending.approval.is_some() && pending.approval.as_ref() != attempt.approval {
                return Err(CustodyError::Mismatch(
                    "the pending attempt's approval changed during recovery",
                ));
            }
        }
        let mut batch = Vec::with_capacity(2 + bounded.len());
        for cover in bounded {
            if let Some(append) = cover.reservation()? {
                batch.push(append);
            }
        }
        if let Some(append) = spend
            .map(super::spend::SpendCover::reservation)
            .transpose()?
            .flatten()
        {
            batch.push(append);
        }
        batch.push(Self::append_at(
            &self.key,
            attempt.admitted_at,
            &EffectEvent::Attempted {
                intent_digest: attempt.intent_digest.to_owned(),
                authority_term_id: attempt.authority_term_id.to_owned(),
                mandate_ref: attempt.mandate_ref.cloned(),
                idempotency_key: attempt.idempotency_key.to_owned(),
                instance_binding: attempt.instance_binding.to_owned(),
                attempt: attempt.attempt,
                approval: attempt.approval.cloned(),
                implementation_digest: attempt.implementation_digest.map(str::to_owned),
            },
        )?);
        match self.store.append(batch) {
            Ok(_) => Ok(()),
            Err(KernelStoreError::Conflict { key, .. }) if key == self.key => {
                Err(CustodyError::Mismatch(
                    "another call attempted this act after this one was admitted; run it again to see where it stands",
                ))
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn acknowledged(
        &self,
        acknowledgement: Acknowledgement,
    ) -> Result<(), CustodyError> {
        self.commit(&EffectEvent::Acknowledged { acknowledgement })
    }

    pub(crate) fn read_back(&self, readback: Readback) -> Result<(), CustodyError> {
        self.commit(&EffectEvent::ReadBack { readback })
    }

    pub(crate) fn unknown(&self, reason: String) -> Result<(), CustodyError> {
        self.commit(&EffectEvent::Unknown { reason })
    }

    /// A definitive rejection settles the attempt. Release its evidence use
    /// only when the provider proves no execution; otherwise the use is kept.
    /// `spend` closes what the attempt reserved against the run's limit.
    pub(crate) fn rejected(
        &self,
        code: String,
        bounded: &[UseCover],
        release: bool,
        spend: Option<Append>,
    ) -> Result<(), CustodyError> {
        let mut batch = Vec::with_capacity(2 + bounded.len());
        for cover in bounded {
            batch.push(if release {
                cover.release(&self.store)?
            } else {
                cover.consume(&self.store)?
            });
        }
        batch.extend(spend);
        batch.push(self.append(&EffectEvent::Rejected { code })?);
        self.store.append(batch)?;
        Ok(())
    }

    /// What settles the act once the step that consumed it is recorded: the
    /// receipt that sealed it, the evidence uses it consumes and the spend
    /// `(run, attempt)` it reserved.
    pub(crate) fn settlement(
        &self,
        receipt_id: String,
        bounded: &[UseCover],
        spend: Option<(String, String)>,
    ) -> Settlement {
        Settlement {
            custody: self.clone(),
            receipt_id,
            bounded: bounded.to_vec(),
            spend,
        }
    }

    /// The store the act's custody is kept in.
    pub(crate) fn store(&self) -> &KernelStore {
        &self.store
    }

    fn commit(&self, event: &EffectEvent) -> Result<(), CustodyError> {
        self.store.append(vec![self.append(event)?])?;
        Ok(())
    }

    fn append(&self, event: &EffectEvent) -> Result<Append, KernelStoreError> {
        Self::append_at(
            &self.key,
            self.store.version::<EffectEvent>(&self.key)?,
            event,
        )
    }

    fn append_at(key: &str, version: u64, event: &EffectEvent) -> Result<Append, KernelStoreError> {
        Append::kernel(
            key.to_owned(),
            if version == 0 {
                Expected::Absent
            } else {
                Expected::Version(version)
            },
            event,
        )
    }
}

/// A sealed act's settlement. The run record commits it in the same
/// transaction as the step or call that consumed the effect, so a crash
/// leaves both or neither: a step replayed after one never finds its act
/// settled and sends it again.
#[derive(Clone, Debug)]
pub struct Settlement {
    custody: EffectCustody,
    receipt_id: String,
    bounded: Vec<UseCover>,
    spend: Option<(String, String)>,
}

impl Settlement {
    /// The appends that settle the act, for a batch committed through
    /// `store`, which must be the store that keeps the act's custody. An act
    /// this receipt already settled needs none.
    pub(crate) fn appends(&self, store: &KernelStore) -> Result<Vec<Append>, CustodyError> {
        if store.path() != self.custody.store.path() {
            return Err(CustodyError::Mismatch(
                "the act's custody is kept in another store than the run that consumed it",
            ));
        }
        if let Some(latest) = store.latest::<EffectEvent>(&self.custody.key)?
            && matches!(&latest.event, EffectEvent::Finalized { receipt_id } if *receipt_id == self.receipt_id)
        {
            return Ok(Vec::new());
        }
        let mut batch = Vec::with_capacity(2 + self.bounded.len());
        for cover in &self.bounded {
            batch.push(cover.consume(store)?);
        }
        if let Some((run_id, spend_key)) = &self.spend {
            batch.extend(super::spend::settle(store, run_id, spend_key)?);
        }
        batch.push(self.custody.append(&EffectEvent::Finalized {
            receipt_id: self.receipt_id.clone(),
        })?);
        Ok(batch)
    }
}

fn named_act(
    connector: &str,
    operation: &str,
    instance_id: &str,
    target: &str,
    request_key: &Digest,
) -> String {
    sha256_prefixed(
        format!("act\0{connector}\0{operation}\0{instance_id}\0{target}\0{request_key}").as_bytes(),
    )
}

#[cfg(test)]
mod tests;
