//! What a root run has executing at once, at every depth: one pool that the
//! steps and calls of its whole tree draw from, whichever script or child run
//! makes them, so the run's `max_concurrency` bounds the tree, not each
//! script in it. Only work that runs by itself takes a place, a tool or an
//! operation. A skill call, a script or an agent step works through steps
//! and calls of its own, which take theirs, so nothing waits on a place while
//! it holds one.

use std::sync::{Condvar, Mutex};

use crate::RuntimeError;

#[derive(Debug, Default)]
pub(crate) struct CallPool {
    executing: Mutex<usize>,
    freed: Condvar,
}

impl CallPool {
    /// Take a place among at most `capacity` executing at once, waiting for
    /// one to free when none is left. The place is held until it's dropped.
    pub(crate) fn enter(&self, capacity: usize) -> Result<PoolPlace<'_>, RuntimeError> {
        let poisoned = |_| RuntimeError::EngineInvariant {
            context: "taking a place in the run's pool",
            message: "a call that held a place panicked".to_owned(),
        };
        let mut executing = self.executing.lock().map_err(poisoned)?;
        while *executing >= capacity.max(1) {
            executing = self.freed.wait(executing).map_err(poisoned)?;
        }
        *executing += 1;
        Ok(PoolPlace(self))
    }
}

/// A place in the run's pool, given back when dropped.
pub(crate) struct PoolPlace<'a>(&'a CallPool);

impl Drop for PoolPlace<'_> {
    fn drop(&mut self) {
        if let Ok(mut executing) = self.0.executing.lock() {
            *executing -= 1;
        }
        self.0.freed.notify_one();
    }
}
