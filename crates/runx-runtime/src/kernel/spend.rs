//! The run's money limit, in integer micro-USD.
//!
//! A run authority may bound what a run spends. Before anything is sent, a
//! priced call, or a call that moves a USD or USDC amount, reserves what it
//! spends in its root run's ledger, in the same commit as the call's attempt.
//! The reservation settles with the call's receipt, or is released when the
//! call proved nothing billable. A child run spends from its root's ledger, so
//! the limit covers the whole run and no child can widen it. A reservation is
//! keyed by the act's attempt sequence, so recovering an attempt never
//! reserves twice.

use std::collections::BTreeMap;

use runx_contracts::OperationAmount;
use runx_contracts::connector::{ConnectorOperation, CostBasis};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::store::{Append, Expected, KernelStore, KernelStoreError, StreamEvent};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum SpendEvent {
    Reserved {
        spend_key: String,
        intent_digest: String,
        run_id: String,
        usd_micro: u64,
    },
    Settled {
        spend_key: String,
    },
    Released {
        spend_key: String,
    },
}

impl StreamEvent for SpendEvent {
    const KIND: &'static str = "run_spend";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Reserved { .. } => "reserved",
            Self::Settled { .. } => "settled",
            Self::Released { .. } => "released",
        }
    }
}

/// What a run has committed against its limit: reserved and settled.
#[derive(Debug, Default)]
pub(crate) struct SpendStatus {
    pub committed_usd_micro: u64,
    spends: BTreeMap<String, Spend>,
    version: u64,
}

#[derive(Clone, Copy, Debug)]
struct Spend {
    usd_micro: u64,
    settled: bool,
}

#[derive(Debug, Error)]
pub(crate) enum SpendError {
    #[error(
        "cost_limit_exceeded: the call costs {usd_micro} micro-USD and the run has {remaining} of its {limit} left"
    )]
    Exceeded {
        limit: u64,
        remaining: u64,
        usd_micro: u64,
    },
    #[error("the run's spend history is invalid")]
    InvalidHistory,
    #[error(transparent)]
    Store(#[from] KernelStoreError),
}

/// What one call spends, and whether nothing is billable when it fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Charge {
    pub usd_micro: u64,
    /// Billable only when the provider answers as the operation accepts.
    pub on_success: bool,
}

/// What a call spends against a money limit: its operation's quote and the
/// amount it moves. USDC's six decimals are micro-USD and USD is counted in
/// cents; a payment in anything else cannot be counted, so a limited run
/// refuses it.
pub(crate) fn charge(
    operation: &ConnectorOperation,
    amount: Option<&OperationAmount>,
) -> Result<Charge, String> {
    let moved = match amount {
        None => 0,
        Some(amount) => match amount.unit.as_str() {
            "USDC" => amount.units,
            "USD" => amount
                .units
                .checked_mul(10_000)
                .ok_or_else(|| "the call's USD amount is too large to count".to_owned())?,
            unit => {
                return Err(format!(
                    "a payment in {unit} cannot be counted against the run's money limit; only USD and USDC can"
                ));
            }
        },
    };
    let quote = operation.cost.as_ref().map_or(0, |cost| cost.usd_micro);
    Ok(Charge {
        usd_micro: quote
            .checked_add(moved)
            .ok_or_else(|| "the call's cost is too large to count".to_owned())?,
        on_success: moved == 0
            && operation
                .cost
                .as_ref()
                .is_some_and(|cost| cost.per == CostBasis::Success),
    })
}

/// The root of a run's tree, whose ledger every run in it spends from.
pub(crate) fn root_run(run_id: &str) -> &str {
    run_id.split('/').next().unwrap_or(run_id)
}

/// What the run whose root is `root` has committed against its limit.
pub(crate) fn status(store: &KernelStore, root: &str) -> Result<SpendStatus, SpendError> {
    let mut status = SpendStatus::default();
    for recorded in store.read::<SpendEvent>(root)? {
        match recorded.event {
            SpendEvent::Reserved {
                spend_key,
                usd_micro,
                ..
            } => {
                let spend = Spend {
                    usd_micro,
                    settled: false,
                };
                if status.spends.insert(spend_key, spend).is_some() {
                    return Err(SpendError::InvalidHistory);
                }
            }
            SpendEvent::Settled { spend_key } => match status.spends.get_mut(&spend_key) {
                Some(spend) if !spend.settled => spend.settled = true,
                _ => return Err(SpendError::InvalidHistory),
            },
            SpendEvent::Released { spend_key } => {
                if status
                    .spends
                    .get(&spend_key)
                    .is_none_or(|spend| spend.settled)
                {
                    return Err(SpendError::InvalidHistory);
                }
                status.spends.remove(&spend_key);
            }
        }
        status.version = recorded.version;
    }
    status.committed_usd_micro = status
        .spends
        .values()
        .try_fold(0_u64, |total, spend| total.checked_add(spend.usd_micro))
        .ok_or(SpendError::InvalidHistory)?;
    Ok(status)
}

/// Cover `usd_micro` for the attempt `spend_key` of a call in `run_id`,
/// within `limit`. An attempt already reserved is covered by its reservation.
pub(crate) fn cover(
    store: &KernelStore,
    run_id: &str,
    limit: u64,
    spend_key: &str,
    intent_digest: &str,
    usd_micro: u64,
) -> Result<SpendCover, SpendError> {
    let root = root_run(run_id);
    let status = status(store, root)?;
    let recorded = status.spends.contains_key(spend_key);
    let remaining = limit.saturating_sub(status.committed_usd_micro);
    if !recorded && usd_micro > remaining {
        return Err(SpendError::Exceeded {
            limit,
            remaining,
            usd_micro,
        });
    }
    Ok(SpendCover {
        root: root.to_owned(),
        spend_key: spend_key.to_owned(),
        intent_digest: intent_digest.to_owned(),
        run_id: run_id.to_owned(),
        usd_micro,
        expected: status.version,
        recorded,
    })
}

/// Settle the attempt's reservation: what it spent stays spent. None when it
/// holds no open reservation.
pub(crate) fn settle(
    store: &KernelStore,
    run_id: &str,
    spend_key: &str,
) -> Result<Option<Append>, SpendError> {
    close(store, run_id, spend_key, |spend_key| SpendEvent::Settled {
        spend_key,
    })
}

/// Release the attempt's reservation: the call proved nothing billable.
/// None when it holds no open reservation.
pub(crate) fn release(
    store: &KernelStore,
    run_id: &str,
    spend_key: &str,
) -> Result<Option<Append>, SpendError> {
    close(store, run_id, spend_key, |spend_key| SpendEvent::Released {
        spend_key,
    })
}

fn close(
    store: &KernelStore,
    run_id: &str,
    spend_key: &str,
    event: impl FnOnce(String) -> SpendEvent,
) -> Result<Option<Append>, SpendError> {
    let root = root_run(run_id);
    let status = status(store, root)?;
    if status
        .spends
        .get(spend_key)
        .is_none_or(|spend| spend.settled)
    {
        return Ok(None);
    }
    Ok(Some(Append::kernel(
        root.to_owned(),
        Expected::Version(status.version),
        &event(spend_key.to_owned()),
    )?))
}

/// Whether a commit failed because another call of the run spent first.
pub(crate) fn moved(error: &KernelStoreError) -> bool {
    matches!(error, KernelStoreError::Conflict { kind, .. } if kind == SpendEvent::KIND)
}

/// One call's reservation against its run's limit, committed with the
/// call's attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpendCover {
    root: String,
    spend_key: String,
    intent_digest: String,
    run_id: String,
    pub usd_micro: u64,
    expected: u64,
    recorded: bool,
}

impl SpendCover {
    /// The reservation; none when the attempt already holds it.
    pub(crate) fn reservation(&self) -> Result<Option<Append>, KernelStoreError> {
        if self.recorded {
            return Ok(None);
        }
        Append::kernel(
            self.root.clone(),
            if self.expected == 0 {
                Expected::Absent
            } else {
                Expected::Version(self.expected)
            },
            &SpendEvent::Reserved {
                spend_key: self.spend_key.clone(),
                intent_digest: self.intent_digest.clone(),
                run_id: self.run_id.clone(),
                usd_micro: self.usd_micro,
            },
        )
        .map(Some)
    }
}

#[cfg(test)]
mod tests;
