//! A root run's receipt tree as a budget, checked as `max_calls` is: each
//! receipt the run will seal is counted when what seals it is admitted, at
//! every depth, and what would take the tree past its bound is refused
//! before it runs. A run that did its work therefore always seals, and the
//! bound a verifier holds a tree to is never reached at the seal.
//!
//! A drive counts what its run's record already holds as it replays it, and
//! takes what it admits from there: a step's receipt when the step starts, a
//! call's when it's admitted, and a child run's own when it starts. A step or
//! call that stops for good without a receipt gives its count back; one that
//! waits keeps it, since it seals later.

use std::sync::atomic::{AtomicUsize, Ordering};

/// What a root run's tree has drawn of its receipt budget: the receipts it
/// holds or will hold, its root's own included. Its child runs draw from the
/// same tally.
#[derive(Debug)]
pub(crate) struct ReceiptTally(AtomicUsize);

impl Default for ReceiptTally {
    /// A root run's, holding its own receipt.
    fn default() -> Self {
        Self(AtomicUsize::new(1))
    }
}

impl ReceiptTally {
    /// Take `receipts` more within `bound`; false, taking none, when they
    /// don't fit.
    pub(crate) fn take(&self, receipts: usize, bound: usize) -> bool {
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |counted| {
                counted
                    .checked_add(receipts)
                    .filter(|total| *total <= bound)
            })
            .is_ok()
    }

    /// Count `receipts` the run's record already holds: they are its tree's,
    /// whatever is left.
    pub(crate) fn count(&self, receipts: usize) {
        self.0.fetch_add(receipts, Ordering::SeqCst);
    }

    /// Give back `receipts` taken for something that sealed none.
    pub(crate) fn give_back(&self, receipts: usize) {
        self.0.fetch_sub(receipts, Ordering::SeqCst);
    }
}
