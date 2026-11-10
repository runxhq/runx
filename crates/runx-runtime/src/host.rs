use std::sync::{Arc, Mutex};

use runx_contracts::{ExecutionEvent, ResolutionRequest, ResolutionResponse};

use crate::RuntimeError;
use crate::execution::runner::turn::Turn;

pub trait Host {
    fn report(&mut self, event: ExecutionEvent) -> Result<(), RuntimeError>;

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError>;

    fn log(&mut self, message: String) -> Result<(), RuntimeError>;

    /// Work the turn an unanswered agent act holds open: settle the calls
    /// this host now answers, make calls, or hold the act's answer for
    /// `resolve`. A host that drives no agent leaves the turn as it is.
    fn drive_turn(&mut self, turn: &mut dyn Turn) -> Result<(), RuntimeError> {
        let _ = turn;
        Ok(())
    }

    /// Whether the model that works this host's turns is known to run on
    /// this machine, so a turn may carry its `local_only` context entries.
    /// A host whose model may run anywhere never gets one (memory.md).
    fn reads_local_only(&self) -> bool {
        false
    }

    /// Whether this pass only records what a call already did. A stop keeps
    /// a driver from admitting any further step or call, never from
    /// recording what one did, so such a pass re-enters the step it is
    /// suspended at even after a stop.
    fn records(&self) -> bool {
        false
    }

    /// A host for one call that runs apart from the run's driver, on a
    /// thread of its own: it answers what this host would, by address, and
    /// keeps what the call reports for the driver to take back. `None` when
    /// this host can't serve a call apart, so its calls run on the driver.
    fn detached(&self) -> Option<DetachedHost> {
        None
    }
}

/// What answers a request for a call that runs apart from its driver.
pub type DetachedAnswers =
    Box<dyn FnMut(ResolutionRequest) -> Result<Option<ResolutionResponse>, RuntimeError> + Send>;

/// A host serving one call apart from the run's driver. What the call
/// reports and logs is kept, and the driver hands it to its own host when it
/// records the call. A call it serves that runs calls of its own, a child
/// script's, serves each apart too, from the same answers.
pub struct DetachedHost {
    answers: Arc<Mutex<DetachedAnswers>>,
    reported: Vec<ExecutionEvent>,
    logged: Vec<String>,
}

impl DetachedHost {
    #[must_use]
    pub fn new(answers: DetachedAnswers) -> Self {
        Self {
            answers: Arc::new(Mutex::new(answers)),
            reported: Vec::new(),
            logged: Vec::new(),
        }
    }

    /// Hand what the call reported and logged to the driver's `host`.
    pub(crate) fn hand_back(self, host: &mut dyn Host) -> Result<(), RuntimeError> {
        for event in self.reported {
            host.report(event)?;
        }
        for message in self.logged {
            host.log(message)?;
        }
        Ok(())
    }
}

impl Host for DetachedHost {
    fn report(&mut self, event: ExecutionEvent) -> Result<(), RuntimeError> {
        self.reported.push(event);
        Ok(())
    }

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError> {
        let mut answers = self
            .answers
            .lock()
            .map_err(|_| RuntimeError::EngineInvariant {
                context: "answering a call apart from its driver",
                message: "a call answered from the same answers panicked".to_owned(),
            })?;
        (answers)(request)
    }

    fn log(&mut self, message: String) -> Result<(), RuntimeError> {
        self.logged.push(message);
        Ok(())
    }

    /// A call of the call this host serves runs apart from it on the same
    /// answers, and what it reports comes back to this host.
    fn detached(&self) -> Option<DetachedHost> {
        Some(Self {
            answers: Arc::clone(&self.answers),
            reported: Vec::new(),
            logged: Vec::new(),
        })
    }
}

#[derive(Default)]
pub struct NoopHost;

impl Host for NoopHost {
    fn report(&mut self, _event: ExecutionEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        _request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError> {
        Ok(None)
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }
}
