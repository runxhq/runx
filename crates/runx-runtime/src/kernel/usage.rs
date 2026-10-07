//! One usage ledger for bounded authority and standing approval evidence.
//!
//! A cover is checked before execution; its reservation is committed in the
//! same batch as the effect attempt. The stream version makes concurrent
//! admissions race safely, and a retry of the same call never uses it twice.

use std::collections::BTreeMap;
use std::sync::Arc;

use runx_contracts::{AuthorityUseLimit, sha256_prefixed};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::store::{Append, Expected, KernelStore, KernelStoreError, StreamEvent};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum UsageEvent {
    Reserved {
        use_key: String,
        intent_digest: String,
        run_id: String,
        day: u64,
    },
    Released {
        use_key: String,
    },
    Consumed {
        use_key: String,
    },
    Fence,
}

impl StreamEvent for UsageEvent {
    const KIND: &'static str = "evidence_usage";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Reserved { .. } => "reserved",
            Self::Released { .. } => "released",
            Self::Consumed { .. } => "consumed",
            Self::Fence => "fence",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Use {
    intent_digest: String,
    run_id: String,
    day: u64,
    consumed: bool,
}

#[derive(Debug, Default)]
pub(crate) struct UsageStatus {
    pub active_total: usize,
    pub open_total: usize,
    uses: BTreeMap<String, Use>,
    pub version: u64,
}

impl UsageStatus {
    pub(crate) fn has(&self, use_key: &str) -> bool {
        self.uses.contains_key(use_key)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UseCover {
    evidence_key: String,
    use_key: String,
    intent_digest: String,
    run_id: String,
    day: u64,
    expected: u64,
    recorded: bool,
}

#[derive(Debug, Error)]
pub(crate) enum UsageError {
    #[error("the evidence use key belongs to another intent")]
    KeyConflict,
    #[error("the total evidence use limit is exhausted")]
    TotalExhausted,
    #[error("the daily evidence use limit is exhausted")]
    DailyExhausted,
    #[error("the evidence usage history is invalid")]
    InvalidHistory,
    #[error(transparent)]
    Store(#[from] KernelStoreError),
}

#[derive(Clone)]
pub(crate) struct UsageLedger {
    store: Arc<KernelStore>,
}

impl UsageLedger {
    pub(crate) fn new(store: Arc<KernelStore>) -> Self {
        Self { store }
    }

    pub(crate) fn status(&self, evidence_key: &str) -> Result<UsageStatus, UsageError> {
        let mut status = UsageStatus::default();
        for recorded in self.store.read::<UsageEvent>(evidence_key)? {
            match recorded.event {
                UsageEvent::Reserved {
                    use_key,
                    intent_digest,
                    run_id,
                    day,
                } => {
                    if status
                        .uses
                        .insert(
                            use_key,
                            Use {
                                intent_digest,
                                run_id,
                                day,
                                consumed: false,
                            },
                        )
                        .is_some()
                    {
                        return Err(UsageError::InvalidHistory);
                    }
                }
                UsageEvent::Released { use_key } => {
                    if status.uses.get(&use_key).is_none_or(|used| used.consumed) {
                        return Err(UsageError::InvalidHistory);
                    }
                    status.uses.remove(&use_key);
                }
                UsageEvent::Consumed { use_key } => {
                    let Some(used) = status.uses.get_mut(&use_key) else {
                        return Err(UsageError::InvalidHistory);
                    };
                    used.consumed = true;
                }
                UsageEvent::Fence => {}
            }
            status.version = recorded.version;
        }
        status.active_total = status.uses.len();
        status.open_total = status.uses.values().filter(|used| !used.consumed).count();
        Ok(status)
    }

    pub(crate) fn cover(
        &self,
        evidence_key: &str,
        limit: &AuthorityUseLimit,
        use_key: &str,
        intent_digest: &str,
        run_id: &str,
        now_unix_seconds: u64,
    ) -> Result<UseCover, UsageError> {
        let status = self.status(evidence_key)?;
        let day = now_unix_seconds / 86_400;
        if let Some(used) = status.uses.get(use_key) {
            if used.intent_digest != intent_digest || used.run_id != run_id {
                return Err(UsageError::KeyConflict);
            }
            return Ok(UseCover {
                evidence_key: evidence_key.to_owned(),
                use_key: use_key.to_owned(),
                intent_digest: intent_digest.to_owned(),
                run_id: run_id.to_owned(),
                day: used.day,
                expected: status.version,
                recorded: true,
            });
        }
        if limit
            .total
            .is_some_and(|total| status.uses.len() as u64 >= total)
        {
            return Err(UsageError::TotalExhausted);
        }
        if limit.per_day.is_some_and(|per_day| {
            status.uses.values().filter(|used| used.day == day).count() as u64 >= per_day
        }) {
            return Err(UsageError::DailyExhausted);
        }
        Ok(UseCover {
            evidence_key: evidence_key.to_owned(),
            use_key: use_key.to_owned(),
            intent_digest: intent_digest.to_owned(),
            run_id: run_id.to_owned(),
            day,
            expected: status.version,
            recorded: false,
        })
    }

    /// Fence an approval renewal against a concurrent reservation.
    pub(crate) fn fence(
        &self,
        evidence_key: &str,
        version: u64,
    ) -> Result<Append, KernelStoreError> {
        Append::kernel(
            evidence_key.to_owned(),
            if version == 0 {
                Expected::Absent
            } else {
                Expected::Version(version)
            },
            &UsageEvent::Fence,
        )
    }
}

impl UseCover {
    pub(crate) fn receipt_ref(&self) -> String {
        format!(
            "runx:evidence_use:{}",
            sha256_prefixed(format!("{}\0{}", self.evidence_key, self.use_key).as_bytes())
        )
    }

    pub(crate) fn recorded(&self) -> bool {
        self.recorded
    }

    pub(crate) fn reservation(&self) -> Result<Option<Append>, KernelStoreError> {
        if self.recorded {
            return Ok(None);
        }
        Append::kernel(
            self.evidence_key.clone(),
            if self.expected == 0 {
                Expected::Absent
            } else {
                Expected::Version(self.expected)
            },
            &UsageEvent::Reserved {
                use_key: self.use_key.clone(),
                intent_digest: self.intent_digest.clone(),
                run_id: self.run_id.clone(),
                day: self.day,
            },
        )
        .map(Some)
    }

    pub(crate) fn release(&self, store: &KernelStore) -> Result<Append, KernelStoreError> {
        Append::kernel(
            self.evidence_key.clone(),
            Expected::Version(store.version::<UsageEvent>(&self.evidence_key)?),
            &UsageEvent::Released {
                use_key: self.use_key.clone(),
            },
        )
    }

    pub(crate) fn consume(&self, store: &KernelStore) -> Result<Append, KernelStoreError> {
        Append::kernel(
            self.evidence_key.clone(),
            Expected::Version(store.version::<UsageEvent>(&self.evidence_key)?),
            &UsageEvent::Consumed {
                use_key: self.use_key.clone(),
            },
        )
    }
}

#[cfg(test)]
mod tests;
