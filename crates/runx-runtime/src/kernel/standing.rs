//! Standing approvals: a person's confirmation of a bounded kind of intent.
//!
//! A standing approval lets bounded repeats of one operation skip a per-call
//! question. It is approval evidence, never authority: the call still needs
//! the instance's capability. Its record and its usage live in the kernel
//! store. Usage is appended in the same commit as the effect attempt it
//! pays for, so a quota can never be spent by an attempt that was not
//! recorded, nor an attempt recorded without its use.
//!
//! The one kind today is `notification`: the payload is exactly a target and
//! a text, the target equals the approval's, and the text stays within the
//! approval's size and mention rules.

use std::sync::Arc;

use runx_contracts::connector::{ConnectorRef, InstanceId, OperationName};
use runx_contracts::{AuthorityUseLimit, sha256_prefixed};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::custody::EffectCustody;
use super::store::{Append, Expected, KernelStore, KernelStoreError, Recorded, StreamEvent};
use super::usage::{UsageError, UsageLedger, UseCover};

/// The standing approval a run may use, named by its host.
pub const STANDING_APPROVAL_ID_ENV: &str = "RUNX_STANDING_APPROVAL_ID";
/// The digest of the delegate allowed to use it (the assistant's source set).
pub const STANDING_DELEGATE_DIGEST_ENV: &str = "RUNX_STANDING_DELEGATE_DIGEST";

const MAX_LIFETIME_SECONDS: u64 = 30 * 24 * 60 * 60;

/// What the operator grants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandingApproval {
    pub approval_id: String,
    pub connector: ConnectorRef,
    pub operation: OperationName,
    /// The instance whose capability the approved calls run under.
    pub instance_id: InstanceId,
    pub target: String,
    /// The delegate allowed to use the approval. Renewing it to a new digest
    /// keeps usage and expiry.
    pub delegate_digest: String,
    pub expires_at_unix_seconds: u64,
    pub max_total: u32,
    pub max_per_day: u32,
    pub max_text_bytes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_mention: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum StandingEvent {
    Granted {
        approval: StandingApproval,
        granted_at_unix_seconds: u64,
    },
    Renewed {
        delegate_digest: String,
    },
    Revoked {
        revoked_at_unix_seconds: u64,
    },
}

impl StreamEvent for StandingEvent {
    const KIND: &'static str = "standing_approval";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Granted { .. } => "granted",
            Self::Renewed { .. } => "renewed",
            Self::Revoked { .. } => "revoked",
        }
    }
}

/// An approval's current state, folded from its stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StandingStatus {
    pub approval: StandingApproval,
    pub revoked_at_unix_seconds: Option<u64>,
    pub used_total: usize,
    #[serde(skip)]
    version: u64,
}

impl StandingStatus {
    fn active_at(&self, now: u64) -> Result<(), StandingError> {
        if self.revoked_at_unix_seconds.is_some() {
            return Err(StandingError::Denied("the standing approval is revoked"));
        }
        if now >= self.approval.expires_at_unix_seconds {
            return Err(StandingError::Denied("the standing approval is expired"));
        }
        Ok(())
    }
}

/// One call a standing approval is asked to cover.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StandingCall<'a> {
    pub connector: &'a str,
    pub operation: &'a str,
    pub instance_id: &'a str,
    pub target: &'a str,
    pub delegate_digest: &'a str,
    pub text: &'a str,
    pub intent_digest: &'a str,
    /// The use the act's attempt sequence takes.
    pub use_key: &'a str,
    pub run_id: &'a str,
}

/// Proof that a standing approval covers a call, and the usage to append.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StandingCover {
    pub approval_id: String,
    pub approval_key: String,
    pub usage: UseCover,
}

#[derive(Debug, Error)]
pub enum StandingError {
    #[error("standing approval is invalid: {0}")]
    Invalid(&'static str),
    #[error("standing approval denied: {0}")]
    Denied(&'static str),
    #[error(transparent)]
    Store(#[from] KernelStoreError),
}

/// The standing approvals in one kernel store.
#[derive(Clone)]
pub struct StandingApprovals {
    store: Arc<KernelStore>,
}

impl StandingApprovals {
    #[must_use]
    pub fn new(store: Arc<KernelStore>) -> Self {
        Self { store }
    }

    /// Grant a standing approval. Granting the same terms again is a no-op;
    /// granting them with a new delegate digest renews it. Different terms
    /// under an existing id are refused. A renewal waits until all uses of
    /// the old delegate have been consumed or released.
    pub fn grant(&self, approval: StandingApproval) -> Result<(), StandingError> {
        let now = now_unix_seconds();
        validate(&approval, now)?;
        let key = approval.approval_id.clone();
        let Some(status) = self.status(&key)? else {
            self.store.append(vec![Append::kernel(
                key,
                Expected::Absent,
                &StandingEvent::Granted {
                    approval,
                    granted_at_unix_seconds: now,
                },
            )?])?;
            return Ok(());
        };
        let same_terms = StandingApproval {
            delegate_digest: approval.delegate_digest.clone(),
            ..status.approval.clone()
        } == approval;
        if !same_terms || status.active_at(now).is_err() {
            return Err(StandingError::Denied(
                "the approval id already has different terms or is no longer active",
            ));
        }
        if status.approval.delegate_digest == approval.delegate_digest {
            return Ok(());
        }
        let ledger = UsageLedger::new(Arc::clone(&self.store));
        let usage_key = format!("standing:{key}");
        let usage = ledger.status(&usage_key).map_err(standing_usage_error)?;
        if usage.open_total > 0 {
            return Err(StandingError::Denied(
                "a use of this standing approval is still open",
            ));
        }
        self.store.append(vec![
            ledger.fence(&usage_key, usage.version)?,
            Append::kernel(
                key,
                Expected::Version(status.version),
                &StandingEvent::Renewed {
                    delegate_digest: approval.delegate_digest,
                },
            )?,
        ])?;
        Ok(())
    }

    pub fn revoke(&self, approval_id: &str) -> Result<(), StandingError> {
        let status = self
            .status(approval_id)?
            .ok_or(StandingError::Denied("the approval id is unknown"))?;
        if status.revoked_at_unix_seconds.is_some() {
            return Ok(());
        }
        self.store.append(vec![Append::kernel(
            approval_id,
            Expected::Version(status.version),
            &StandingEvent::Revoked {
                revoked_at_unix_seconds: now_unix_seconds(),
            },
        )?])?;
        Ok(())
    }

    pub fn status(&self, approval_id: &str) -> Result<Option<StandingStatus>, StandingError> {
        let Some(mut status) = fold(&self.store.read::<StandingEvent>(approval_id)?)? else {
            return Ok(None);
        };
        status.used_total = UsageLedger::new(Arc::clone(&self.store))
            .status(&format!("standing:{approval_id}"))
            .map_err(standing_usage_error)?
            .active_total;
        Ok(Some(status))
    }

    /// Whether the approval's act named by `idempotency_key` was ever
    /// attempted. An attempt is recorded, with its use, before anything is
    /// sent; an unknown approval is not proof of absence.
    pub fn attempted(
        &self,
        approval_id: &str,
        idempotency_key: &str,
    ) -> Result<bool, StandingError> {
        let status = self
            .status(approval_id)?
            .ok_or(StandingError::Denied("the approval id is unknown"))?;
        let approval = &status.approval;
        Ok(EffectCustody::for_named_act(
            Arc::clone(&self.store),
            approval.connector.as_str(),
            approval.operation.as_str(),
            approval.instance_id.as_str(),
            &approval.target,
            idempotency_key,
        )
        .attempted()?)
    }

    /// Check that the approval covers this exact call and that usage remains.
    /// Nothing is written: the use is appended with the effect attempt.
    pub(crate) fn cover(
        &self,
        approval_id: &str,
        call: &StandingCall<'_>,
    ) -> Result<StandingCover, StandingError> {
        let status = self
            .status(approval_id)?
            .ok_or(StandingError::Denied("the approval id is unknown"))?;
        let approval = &status.approval;
        if approval.connector.as_str() != call.connector
            || approval.operation.as_str() != call.operation
            || approval.instance_id.as_str() != call.instance_id
            || approval.target != call.target
            || approval.delegate_digest != call.delegate_digest
        {
            return Err(StandingError::Denied(
                "the operation, instance, target or delegate differs from the approval",
            ));
        }
        check_text(approval, call.text)?;
        let approval_key = sha256_prefixed(format!("{approval_id}\0{}", call.use_key).as_bytes());
        let now = now_unix_seconds();
        let usage = UsageLedger::new(Arc::clone(&self.store))
            .cover(
                &format!("standing:{approval_id}"),
                &AuthorityUseLimit {
                    total: Some(u64::from(approval.max_total)),
                    per_day: Some(u64::from(approval.max_per_day)),
                },
                call.use_key,
                call.intent_digest,
                call.run_id,
                now,
            )
            .map_err(standing_usage_error)?;
        if !usage.recorded() {
            status.active_at(now)?;
        }
        Ok(StandingCover {
            approval_id: approval_id.to_owned(),
            approval_key,
            usage,
        })
    }
}

impl StandingCover {
    /// The usage append that rides in the attempt's commit, against the
    /// version the cover was checked at. `None` when already recorded.
    pub(crate) fn use_append(&self) -> Result<Option<Append>, KernelStoreError> {
        self.usage.reservation()
    }

    /// Release the use after a refusal the provider proved happened before
    /// execution.
    pub(crate) fn release_append(&self, store: &KernelStore) -> Result<Append, KernelStoreError> {
        self.usage.release(store)
    }

    pub(crate) fn consume_append(&self, store: &KernelStore) -> Result<Append, KernelStoreError> {
        self.usage.consume(store)
    }
}

fn fold(events: &[Recorded<StandingEvent>]) -> Result<Option<StandingStatus>, StandingError> {
    let mut status: Option<StandingStatus> = None;
    for recorded in events {
        match (&mut status, &recorded.event) {
            (None, StandingEvent::Granted { approval, .. }) => {
                status = Some(StandingStatus {
                    approval: approval.clone(),
                    revoked_at_unix_seconds: None,
                    used_total: 0,
                    version: 0,
                });
            }
            (Some(status), StandingEvent::Renewed { delegate_digest }) => {
                status.approval.delegate_digest.clone_from(delegate_digest);
            }
            (
                Some(status),
                StandingEvent::Revoked {
                    revoked_at_unix_seconds,
                },
            ) => {
                status.revoked_at_unix_seconds = Some(*revoked_at_unix_seconds);
            }
            _ => {
                return Err(StandingError::Invalid(
                    "the approval history is out of order",
                ));
            }
        }
        if let Some(status) = status.as_mut() {
            status.version = recorded.version;
        }
    }
    Ok(status)
}

fn check_text(approval: &StandingApproval, text: &str) -> Result<(), StandingError> {
    let without_mention = match approval.allowed_mention.as_deref() {
        Some(mention) => {
            if text.matches(mention).count() > 1 {
                return Err(StandingError::Denied(
                    "the text repeats its approved mention",
                ));
            }
            text.replace(mention, "")
        }
        None => text.to_owned(),
    };
    if text.is_empty()
        || text.len() > approval.max_text_bytes as usize
        || ["<!", "<@", "@channel", "@here", "@everyone"]
            .iter()
            .any(|mention| without_mention.contains(mention))
    {
        return Err(StandingError::Denied("the text violates the approval"));
    }
    Ok(())
}

fn validate(approval: &StandingApproval, now: u64) -> Result<(), StandingError> {
    let id = &approval.approval_id;
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(StandingError::Invalid(
            "the approval id must be a safe identifier",
        ));
    }
    if approval.target.is_empty()
        || approval.target.len() > 512
        || approval.target.chars().any(char::is_control)
        || approval.target.contains("/../")
    {
        return Err(StandingError::Invalid(
            "the target must be an exact locator",
        ));
    }
    let digest = &approval.delegate_digest;
    if digest.len() != 71
        || !digest.starts_with("sha256:")
        || !digest[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(StandingError::Invalid(
            "the delegate digest must be SHA-256",
        ));
    }
    if approval.expires_at_unix_seconds <= now
        || approval.expires_at_unix_seconds > now.saturating_add(MAX_LIFETIME_SECONDS)
    {
        return Err(StandingError::Invalid("the expiry must be within 30 days"));
    }
    if approval.max_total == 0
        || approval.max_total > 1000
        || approval.max_per_day == 0
        || approval.max_per_day > 100
        || approval.max_text_bytes == 0
        || approval.max_text_bytes > 4000
    {
        return Err(StandingError::Invalid(
            "the quotas exceed the notification ceiling",
        ));
    }
    if approval.allowed_mention.as_ref().is_some_and(|mention| {
        mention.is_empty()
            || mention.len() > 100
            || mention.chars().any(char::is_control)
            || ["<!", "@channel", "@here", "@everyone"]
                .iter()
                .any(|broadcast| mention.contains(broadcast))
    }) {
        return Err(StandingError::Invalid("the approved mention is invalid"));
    }
    Ok(())
}

fn standing_usage_error(error: UsageError) -> StandingError {
    match error {
        UsageError::TotalExhausted => StandingError::Denied("the total quota is exhausted"),
        UsageError::DailyExhausted => StandingError::Denied("the daily quota is exhausted"),
        UsageError::InvalidHistory => StandingError::Invalid("the usage history is invalid"),
        UsageError::Store(error) => StandingError::Store(error),
    }
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests;
