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
