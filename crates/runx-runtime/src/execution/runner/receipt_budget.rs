//! A root run's receipt tree as a budget, checked as `max_calls` is: each
//! receipt the run will seal is counted when what seals it is admitted, at
//! every depth, and what would take the tree past its bound is refused
//! before it runs. A run that did its work therefore always seals, and the
//! bound a verifier holds a tree to is never reached at the seal.
//!
//! What a run's record already holds is counted as a fact, never refused:
//! a run's own receipt and its recorded steps when the run is first read, a
//! step's recorded calls when its record is first read, a step or call that
//! waits, and the recorded tree of a child run that waits. What the run
//! admits from there is taken from what's left: a step's receipt when it
//! starts, a call's when it's admitted, and a new child run's own when it
//! starts. A step or call that ends without a receipt gives its count back;
//! one that waits keeps it, since it seals later.
//!
//! Each of these is counted under its own key, once, whichever path reaches
//! it first: the tally of one drive, or of a session's passes, never counts
//! a receipt twice.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use runx_contracts::CallTarget;

use crate::RuntimeError;
use crate::execution::run_record::{
    CallOutcome, Hold, RecordedCall, Replayed, RunLog, RunRecord, call_step_id,
};

/// What a root run's tree has drawn of its receipt budget: the receipts it
/// holds or will hold, each part counted once by its key. Its child runs,
/// and every pass of a session that works it, draw from the same tally.
#[derive(Debug, Default)]
pub(crate) struct ReceiptTally(Mutex<Counted>);

#[derive(Debug, Default)]
struct Counted {
    receipts: usize,
    keys: BTreeSet<String>,
}

impl ReceiptTally {
    /// Count `receipts` a run's record already holds under `key`, once:
    /// they are its tree's, whatever is left.
    pub(crate) fn count(&self, key: String, receipts: usize) {
        let mut counted = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if counted.keys.insert(key) {
            counted.receipts = counted.receipts.saturating_add(receipts);
        }
    }

    /// Take `receipts` under `key` within `bound`; false, taking none, when
    /// they don't fit. What `key` already counts is not taken again.
    pub(crate) fn take(&self, key: String, receipts: usize, bound: usize) -> bool {
        let mut counted = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if counted.keys.contains(&key) {
            return true;
        }
        match counted.receipts.checked_add(receipts) {
            Some(total) if total <= bound => {
                counted.receipts = total;
                counted.keys.insert(key);
                true
            }
            _ => false,
        }
    }

    /// Give back the `receipts` `key` counts, for something that sealed none.
    pub(crate) fn give_back(&self, key: &str, receipts: usize) {
        let mut counted = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if counted.keys.remove(key) {
            counted.receipts = counted.receipts.saturating_sub(receipts);
        }
    }
}

/// The key of run `run_id`'s own receipt and the steps its record holds.
pub(crate) fn run_key(run_id: &str) -> String {
    format!("run:{run_id}")
}

/// The key of the receipt attempt `attempt` of step `step_id` seals.
pub(crate) fn step_key(run_id: &str, step_id: &str, attempt: u32) -> String {
    format!("step:{run_id}/{step_id}/{attempt}")
}

/// The key of the completed calls a step's record holds.
pub(crate) fn calls_key(run_id: &str, step_id: &str, attempt: u32) -> String {
    format!("calls:{run_id}/{step_id}/{attempt}")
}

/// The key of the receipt call `index` of a step seals.
pub(crate) fn call_key(run_id: &str, step_id: &str, attempt: u32, index: u32) -> String {
    format!("call:{run_id}/{step_id}/{attempt}/{index}")
}

/// The receipts a run's record holds of its own: the run's and each step it
/// recorded, with every receipt each carries.
pub(crate) fn recorded_receipts(record: &RunRecord) -> usize {
    1 + record
        .facts
        .iter()
        .map(|fact| match fact {
            Replayed::Step(run) => run.receipts(),
            Replayed::Skipped(_) => 0,
        })
        .sum::<usize>()
}

/// Count, as facts, what the run `log` records and what it waits at: its
/// own receipt and its recorded steps, the step it waits at and that step's
/// calls, and the same of each child run one of them waits on. A step's
/// record counts its waiting children's trees this way as it loads, since a
/// turn's agent may make a call before any of them is entered again; a child
/// entered later counts nothing twice.
pub(crate) fn count_recorded_tree(tally: &ReceiptTally, log: &RunLog) -> Result<(), RuntimeError> {
    let Some(record) = log.load()? else {
        return Ok(());
    };
    tally.count(run_key(log.run_id()), recorded_receipts(&record));
    if record.sealed.is_some() {
        return Ok(());
    }
    let Some(suspension) = &record.suspension else {
        return Ok(());
    };
    let (step_id, attempt) = (suspension.step_id.as_str(), suspension.attempt);
    tally.count(step_key(log.run_id(), step_id, attempt), 1);
    let calls = record
        .calls
        .iter()
        .filter(|call| call.step_id == step_id && call.attempt == attempt)
        .collect::<Vec<_>>();
    count_recorded_calls(tally, log, step_id, attempt, &calls)?;
    if let Hold::Child(_) = &suspension.hold {
        count_recorded_tree(tally, &log.child(step_id)?)?;
    }
    Ok(())
}

/// Count, as facts, the calls step `step_id`'s attempt `attempt` of the run
/// `log` records: each completed call's receipts, the one each open call
/// seals later, and the recorded tree of each child run an open call waits
/// on.
pub(crate) fn count_recorded_calls(
    tally: &ReceiptTally,
    log: &RunLog,
    step_id: &str,
    attempt: u32,
    calls: &[&RecordedCall],
) -> Result<(), RuntimeError> {
    let completed = calls
        .iter()
        .filter_map(|call| match &call.outcome {
            CallOutcome::Completed(run) => Some(run.receipts()),
            _ => None,
        })
        .sum();
    tally.count(calls_key(log.run_id(), step_id, attempt), completed);
    for call in calls {
        if !seals_a_receipt(&call.target) {
            continue;
        }
        match &call.outcome {
            CallOutcome::Pending(_) | CallOutcome::Claimed(_) => {}
            CallOutcome::Child(_) => {
                count_recorded_tree(tally, &log.child(&call_step_id(step_id, call.index))?)?;
            }
            _ => continue,
        }
        tally.count(call_key(log.run_id(), step_id, attempt, call.index), 1);
    }
    Ok(())
}

/// Whether a call naming `target` seals a receipt once it completes: a
/// script's wait and read are its ledger's own, and seal none.
pub(crate) fn seals_a_receipt(target: &CallTarget) -> bool {
    !matches!(target, CallTarget::Wait { .. } | CallTarget::Read { .. })
}
