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

use super::standing::StandingCover;
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApprovalActor {
    /// A person approved this exact intent.
    Human,
    /// A standing approval covered it.
    Standing,
    /// A paid external job the host admitted carries it.
    PaidExternalJob,
}

impl ApprovalActor {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Human => "exact approval",
            Self::Standing => "standing approval",
            Self::PaidExternalJob => "paid external-job authority",
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
        instance_id: String,
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
    pub instance_id: String,
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
    pub instance_id: &'a str,
    pub attempt: u32,
    pub approval: Option<&'a ApprovalRecord>,
    pub implementation_digest: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CustodyError {
    #[error("{0}")]
    Mismatch(&'static str),
    #[error(transparent)]
    Store(#[from] KernelStoreError),
}

/// The act's custody as last recorded: its unsettled attempt, if any, and the
/// stream version the attempt's sequence opened from, or the next one opens
/// from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Custody {
    pub pending: Option<Pending>,
    pub sequence: u64,
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

    /// The custody of the act the caller names with `request_key`.
    pub(crate) fn for_named_act(
        store: Arc<KernelStore>,
        connector: &str,
        operation: &str,
        instance_id: &str,
        target: &str,
        request_key: &str,
    ) -> Self {
        let key = named_act(
            connector,
            operation,
            instance_id,
            target,
            &Digest::of(request_key.as_bytes()),
        );
        Self { store, key }
    }

    pub(crate) fn store(&self) -> &Arc<KernelStore> {
        &self.store
    }

    /// Whether any attempt of the act was recorded. An attempt is recorded
    /// before anything is sent.
    pub(crate) fn attempted(&self) -> Result<bool, KernelStoreError> {
        Ok(self.store.version::<EffectEvent>(&self.key)? > 0)
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
            return Ok(Custody {
                pending: None,
                sequence: latest.map_or(0, |last| last.version),
            });
        }
        let mut sequence = 0;
        let mut pending: Option<Pending> = None;
        for recorded in self.store.read::<EffectEvent>(&self.key)? {
            match recorded.event {
                EffectEvent::Attempted {
                    intent_digest,
                    authority_term_id,
                    mandate_ref,
                    idempotency_key,
                    instance_id,
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
                        instance_id,
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
        Ok(Custody { pending, sequence })
    }

    /// Record an attempt and every evidence use it takes in one commit.
    pub(crate) fn attempt(
        &self,
        attempt: &Attempt<'_>,
        standing: Option<&StandingCover>,
        bounded: &[UseCover],
    ) -> Result<(), CustodyError> {
        if let Some(pending) = self.pending()? {
            if pending.intent_digest != attempt.intent_digest
                || pending.authority_term_id != attempt.authority_term_id
                || pending.mandate_ref.as_ref() != attempt.mandate_ref
                || pending.idempotency_key != attempt.idempotency_key
                || pending.instance_id != attempt.instance_id
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
        if let Some(append) = standing
            .map(StandingCover::use_append)
            .transpose()?
            .flatten()
        {
            batch.push(append);
        }
        batch.push(self.append(&EffectEvent::Attempted {
            intent_digest: attempt.intent_digest.to_owned(),
            authority_term_id: attempt.authority_term_id.to_owned(),
            mandate_ref: attempt.mandate_ref.cloned(),
            idempotency_key: attempt.idempotency_key.to_owned(),
            instance_id: attempt.instance_id.to_owned(),
            attempt: attempt.attempt,
            approval: attempt.approval.cloned(),
            implementation_digest: attempt.implementation_digest.map(str::to_owned),
        })?);
        self.store.append(batch)?;
        Ok(())
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
    pub(crate) fn rejected(
        &self,
        code: String,
        standing: Option<&StandingCover>,
        bounded: &[UseCover],
        release: bool,
    ) -> Result<(), CustodyError> {
        let mut batch = Vec::with_capacity(2 + bounded.len());
        for cover in bounded {
            batch.push(if release {
                cover.release(&self.store)?
            } else {
                cover.consume(&self.store)?
            });
        }
        if let Some(cover) = standing {
            batch.push(if release {
                cover.release_append(&self.store)?
            } else {
                cover.consume_append(&self.store)?
            });
        }
        batch.push(self.append(&EffectEvent::Rejected { code })?);
        self.store.append(batch)?;
        Ok(())
    }

    pub(crate) fn finalized(
        &self,
        receipt_id: String,
        standing: Option<&StandingCover>,
        bounded: &[UseCover],
    ) -> Result<(), CustodyError> {
        let mut batch = Vec::with_capacity(2 + bounded.len());
        for cover in bounded {
            batch.push(cover.consume(&self.store)?);
        }
        if let Some(cover) = standing {
            batch.push(cover.consume_append(&self.store)?);
        }
        batch.push(self.append(&EffectEvent::Finalized { receipt_id })?);
        self.store.append(batch)?;
        Ok(())
    }

    fn commit(&self, event: &EffectEvent) -> Result<(), CustodyError> {
        self.store.append(vec![self.append(event)?])?;
        Ok(())
    }

    fn append(&self, event: &EffectEvent) -> Result<Append, KernelStoreError> {
        let version = self.store.version::<EffectEvent>(&self.key)?;
        Append::kernel(
            self.key.clone(),
            if version == 0 {
                Expected::Absent
            } else {
                Expected::Version(version)
            },
            event,
        )
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
