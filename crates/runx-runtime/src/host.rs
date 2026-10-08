use runx_contracts::{ExecutionEvent, ResolutionRequest, ResolutionResponse};

use crate::RuntimeError;

pub trait Host {
    fn report(&mut self, event: ExecutionEvent) -> Result<(), RuntimeError>;

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError>;

    fn log(&mut self, message: String) -> Result<(), RuntimeError>;
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
