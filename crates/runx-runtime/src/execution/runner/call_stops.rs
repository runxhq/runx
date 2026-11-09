//! The stops of the calls a run is executing, by address, so its host
//! cancels one call alone while the others go on (D36). A call's stop is
//! registered when it's admitted to execute, under its address in its run,
//! `<step>/<n>`, a turn's call and a script's alike, and released when its
//! driver records how it ended. A cancel between the two is recorded with
//! it: a call that hasn't started, or one that only reads, settles
//! `cancelled`, and its stop ends a read in flight; an effect in flight runs
//! to its end and settles as it actually ended, with the cancel beside it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The executing calls of a run and the child runs below it, by address.
#[derive(Debug, Default)]
pub(crate) struct CallStops(Mutex<BTreeMap<String, Executing>>);

#[derive(Debug)]
struct Executing {
    /// It only reads, so a cancel settles it `cancelled` wherever it stands.
    reads: bool,
    started: bool,
    cancelled: bool,
    /// What a read's execution works under, so a cancel stops whatever in it
    /// heeds a stop.
    stop: Arc<AtomicBool>,
}

/// How a cancel settles the call it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallCancel {
    /// It hadn't started, or it only reads: it settles `cancelled`.
    Cancelled,
    /// Its effect is in flight: it settles as it actually ends, with the
    /// cancel recorded beside it.
    SettlesAsItEnds,
}

impl CallStops {
    fn executing(&self) -> MutexGuard<'_, BTreeMap<String, Executing>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Register the call at `address` until its driver records it.
    pub(crate) fn register(self: &Arc<Self>, address: String, reads: bool) -> CallStop {
        let stop = Arc::new(AtomicBool::new(false));
        self.executing().insert(
            address.clone(),
            Executing {
                reads,
                started: false,
                cancelled: false,
                stop: Arc::clone(&stop),
            },
        );
        CallStop {
            stops: Arc::clone(self),
            address,
            stop,
            reads,
        }
    }

    /// Cancel the call at `address`, if it's executing.
    pub(crate) fn cancel(&self, address: &str) -> Option<CallCancel> {
        let mut executing = self.executing();
        let call = executing.get_mut(address)?;
        call.cancelled = true;
        if call.reads || !call.started {
            call.stop.store(true, Ordering::Release);
            Some(CallCancel::Cancelled)
        } else {
            Some(CallCancel::SettlesAsItEnds)
        }
    }
}

/// One executing call's place among its run's stops, held from its admission
/// to its record.
#[derive(Debug)]
pub(crate) struct CallStop {
    stops: Arc<CallStops>,
    address: String,
    stop: Arc<AtomicBool>,
    reads: bool,
}

impl CallStop {
    /// Start the call, unless its host cancelled it first: whether it's
    /// started, now or already.
    pub(crate) fn start(&self) -> bool {
        let mut executing = self.stops.executing();
        match executing.get_mut(&self.address) {
            Some(call) if Arc::ptr_eq(&call.stop, &self.stop) => {
                if !call.started {
                    call.started = !call.cancelled;
                }
                call.started
            }
            _ => true,
        }
    }

    /// The stop the call's execution works under: its own when it only
    /// reads. An effect has none, so it runs to its end.
    pub(crate) fn working(&self) -> Option<&Arc<AtomicBool>> {
        self.reads.then_some(&self.stop)
    }

    /// Whether its host cancelled it, as far as it has got.
    pub(crate) fn cancelled(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    pub(crate) fn reads(&self) -> bool {
        self.reads
    }

    /// Release the call to record it, under its driver: whether its host
    /// cancelled it. A cancel after this names no executing call.
    pub(crate) fn release(self) -> bool {
        self.remove().is_some_and(|call| call.cancelled)
    }

    fn remove(&self) -> Option<Executing> {
        let mut executing = self.stops.executing();
        match executing.get(&self.address) {
            Some(call) if Arc::ptr_eq(&call.stop, &self.stop) => executing.remove(&self.address),
            _ => None,
        }
    }
}

impl Drop for CallStop {
    fn drop(&mut self) {
        self.remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_settles_by_where_the_call_stands() {
        let stops = Arc::new(CallStops::default());
        let queued = stops.register("batch/0".to_owned(), false);
        let effect = stops.register("batch/1".to_owned(), false);
        let read = stops.register("batch/2".to_owned(), true);
        assert!(effect.start() && read.start());
        assert!(effect.start(), "a call starts once");
        assert_eq!(stops.cancel("batch/0"), Some(CallCancel::Cancelled));
        assert_eq!(stops.cancel("batch/1"), Some(CallCancel::SettlesAsItEnds));
        assert_eq!(stops.cancel("batch/2"), Some(CallCancel::Cancelled));
        assert!(!queued.start(), "a cancelled call never starts");
        assert!(
            effect.start(),
            "a cancel of a started call leaves it started"
        );
        assert!(!effect.cancelled(), "an effect's work isn't stopped");
        assert!(read.cancelled(), "a read's work is");
        assert!(queued.release() && effect.release() && read.release());
        assert_eq!(
            stops.cancel("batch/1"),
            None,
            "a recorded call is not executing"
        );
    }

    #[test]
    fn a_call_registered_again_is_not_released_by_its_earlier_place() {
        let stops = Arc::new(CallStops::default());
        let earlier = stops.register("batch/0".to_owned(), false);
        let again = stops.register("batch/0".to_owned(), false);
        drop(earlier);
        assert_eq!(stops.cancel("batch/0"), Some(CallCancel::Cancelled));
        assert!(again.release());
    }
}
