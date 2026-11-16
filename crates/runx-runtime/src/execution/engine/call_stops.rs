//! The stops of the calls a run is executing, by address, so its host
//! cancels one call alone while the others go on (D36). A call's stop is
//! registered when it's admitted to execute, under its address in its run,
//! `<step>/<n>`, a turn's call and a script's alike, and released when its
//! driver records how it ended. A cancel between the two is recorded with
//! it: a call that hasn't started, or one that only reads, settles
//! `cancelled`, and its stop ends a read in flight; an effect in flight runs
//! to its end and settles as it actually ended, with the cancel beside it.
//! A cancel of a skill call in flight reaches its child run, which heeds it
//! by the same rule at every depth and starts no further step.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::interrupt::Caller;

/// The executing calls of a run and the child runs below it, by address,
/// and the skill calls each turn has admitted to start a child run.
#[derive(Debug, Default)]
pub(crate) struct CallStops {
    executing: Mutex<BTreeMap<String, Executing>>,
    /// The skill calls this process admitted to start a child run, by the
    /// turn they're its own or its scripts' calls of, as its step and
    /// attempt. A turn's fanout counts them with the children its record
    /// holds, so calls admitted at once, on any of its streams, can't pass
    /// its bound together.
    children: Mutex<BTreeMap<(String, u32), BTreeSet<String>>>,
}

#[derive(Debug)]
struct Executing {
    work: CallWork,
    started: bool,
    cancelled: bool,
    /// What a read's execution, or a skill call's child run, heeds: a read
    /// stops, and a child run starts no further step.
    stop: Arc<AtomicBool>,
}

/// What a call's execution does, which says how a cancel in flight settles
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CallWork {
    /// It only reads, so a cancel settles it `cancelled` wherever it stands.
    Reads,
    /// It may change something: in flight, it runs to its end.
    Changes,
    /// It runs a child run, which the cancel reaches.
    RunsAChild,
}

/// How a cancel settles the call it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallCancel {
    /// It hadn't started, or it only reads: it settles `cancelled`.
    Cancelled,
    /// It's in flight and may change something: it settles as it actually
    /// ends, with the cancel recorded beside it. A skill call's child run
    /// starts no further step, and the call settles `cancelled` if the child
    /// committed no effect.
    SettlesAsItEnds,
}

impl CallStops {
    fn executing(&self) -> MutexGuard<'_, BTreeMap<String, Executing>> {
        self.executing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Admit the skill call at `address` to start one of `turn`'s child
    /// runs, within `limit`: none when the children the turn's record holds
    /// as started, which `started` reads, and those admitted here, but for
    /// this call, already reach it. The record is read, and the admission
    /// taken, under one lock, so a call admitted at once on another stream
    /// is counted by one or the other.
    pub(crate) fn admit_child<E>(
        &self,
        turn: (String, u32),
        address: &str,
        limit: usize,
        started: impl FnOnce() -> Result<BTreeSet<String>, E>,
    ) -> Result<bool, E> {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        let started = started()?;
        let admitted = children.entry(turn).or_default();
        let counted = started
            .union(admitted)
            .filter(|child| child.as_str() != address)
            .count();
        if counted >= limit {
            return Ok(false);
        }
        admitted.insert(address.to_owned());
        Ok(true)
    }

    /// Give back what `admit_child` took for the call at `address`, once its
    /// record says how it settled: a child it started is counted from there.
    pub(crate) fn release_child(&self, turn: &(String, u32), address: &str) {
        if let Some(admitted) = self
            .children
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(turn)
        {
            admitted.remove(address);
        }
    }

    /// Register the call at `address`, which does `work`, until its driver
    /// records it.
    pub(crate) fn register(self: &Arc<Self>, address: String, work: CallWork) -> CallStop {
        let stop = Arc::new(AtomicBool::new(false));
        self.executing().insert(
            address.clone(),
            Executing {
                work,
                started: false,
                cancelled: false,
                stop: Arc::clone(&stop),
            },
        );
        CallStop {
            stops: Arc::clone(self),
            address,
            caller: (work == CallWork::RunsAChild)
                .then(|| Arc::new(Caller::new(Arc::clone(&stop)))),
            stop,
            work,
        }
    }

    /// Cancel the call at `address`, if it's executing.
    pub(crate) fn cancel(&self, address: &str) -> Option<CallCancel> {
        let mut executing = self.executing();
        let call = executing.get_mut(address)?;
        call.cancelled = true;
        if !call.started || call.work != CallWork::Changes {
            call.stop.store(true, Ordering::Release);
        }
        if !call.started || call.work == CallWork::Reads {
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
    work: CallWork,
    /// A skill call's, which its child run runs under.
    caller: Option<Arc<Caller>>,
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
        (self.work == CallWork::Reads).then_some(&self.stop)
    }

    /// The call a skill call's child run runs under.
    pub(crate) fn caller(&self) -> Option<&Arc<Caller>> {
        self.caller.as_ref()
    }

    /// Whether its host cancelled it, as far as it has got.
    pub(crate) fn cancelled(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// Whether a cancel settles it `cancelled` however far it got: it only
    /// reads, or it ran a child run that committed no effect.
    pub(crate) fn changed_nothing(&self) -> bool {
        match self.work {
            CallWork::Reads => true,
            CallWork::Changes => false,
            CallWork::RunsAChild => self
                .caller
                .as_ref()
                .is_some_and(|caller| !caller.effected()),
        }
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

    /// A turn's fanout counts the children its record holds and those
    /// admitted here at once, a call never against itself, and a call that
    /// settles without a child gives back what it took.
    #[test]
    fn calls_admitted_at_once_cannot_pass_a_turns_fanout_together() {
        let stops = CallStops::default();
        let turn = ("answer".to_owned(), 1);
        let none = || Ok::<_, ()>(BTreeSet::new());
        assert_eq!(
            stops.admit_child(turn.clone(), "answer/0/0", 1, none),
            Ok(true)
        );
        assert_eq!(
            stops.admit_child(turn.clone(), "answer/1/0", 1, none),
            Ok(false)
        );
        assert_eq!(
            stops.admit_child(turn.clone(), "answer/0/0", 1, none),
            Ok(true)
        );
        let recorded = || Ok::<_, ()>(BTreeSet::from(["answer/0/0".to_owned()]));
        stops.release_child(&turn, "answer/0/0");
        assert_eq!(
            stops.admit_child(turn.clone(), "answer/1/0", 1, recorded),
            Ok(false)
        );
        assert_eq!(stops.admit_child(turn, "answer/1/0", 2, recorded), Ok(true));
    }

    #[test]
    fn a_cancel_settles_by_where_the_call_stands() {
        let stops = Arc::new(CallStops::default());
        let queued = stops.register("batch/0".to_owned(), CallWork::Changes);
        let effect = stops.register("batch/1".to_owned(), CallWork::Changes);
        let read = stops.register("batch/2".to_owned(), CallWork::Reads);
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
        let earlier = stops.register("batch/0".to_owned(), CallWork::Changes);
        let again = stops.register("batch/0".to_owned(), CallWork::Changes);
        drop(earlier);
        assert_eq!(stops.cancel("batch/0"), Some(CallCancel::Cancelled));
        assert!(again.release());
    }
}
