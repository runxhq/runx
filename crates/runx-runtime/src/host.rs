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
