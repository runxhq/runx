use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use runx_contracts::{
    ExecutionEvent, JsonValue, ResolutionRequest, ResolutionResponse, ResolutionResponseActor,
};

use crate::RuntimeError;
use crate::execution::engine::turn::Turn;

pub trait Host {
    fn report(&mut self, event: ExecutionEvent) -> Result<(), RuntimeError>;

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError>;

    fn log(&mut self, message: String) -> Result<(), RuntimeError>;

    /// Work a turn its host hasn't answered: settle the calls
    /// this host now answers, make calls, or hold the turn's answer for
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

    /// Whether this host's agent is handed each call's result as it settles:
    /// an attached session's, which answers the call with it. Its record is
    /// durable before it's handed over.
    fn answers_each_call(&self) -> bool {
        false
    }

    /// Whether this pass only records what a call already did. A stop keeps
    /// a driver from admitting any further step or call, never from
    /// recording what one did, so such a pass re-enters the step it is
    /// suspended at even after a stop.
    fn records(&self) -> bool {
        false
    }

    /// Whether this pass carries work on apart from the run's driver: it
    /// records nothing of a ledger's, a call's settlement or a step's hold,
    /// only what the child runs it goes on in write of their own.
    fn executes(&self) -> bool {
        false
    }

    /// Whether the call at `call`, an address in the run, is on this pass's
    /// way: every call is, but for a pass that executes or concludes one act,
    /// which goes only through the calls whose child runs hold it.
    fn leads_to(&self, call: &str) -> bool {
        let _ = call;
        true
    }

    /// Whether call `index` of the step at `step`, an address in the run,
    /// executes apart from this pass, which never takes it further.
    fn executing(&self, step: &str, index: u32) -> bool {
        let _ = (step, index);
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
/// script's, serves each apart too, from the same answers. The managed
/// agent a run consents to works the turns the call opens here, as it works
/// the driver's, so turns opened at once are worked at once.
pub struct DetachedHost {
    answers: Arc<Mutex<DetachedAnswers>>,
    #[cfg(feature = "agent")]
    managed: Option<Arc<crate::adapters::managed_agent::ManagedAgent>>,
    /// The answers its managed agent gave the call's turns, by address.
    given: BTreeMap<String, JsonValue>,
    reported: Vec<ExecutionEvent>,
    logged: Vec<String>,
}

impl DetachedHost {
    #[must_use]
    pub fn new(answers: DetachedAnswers) -> Self {
        Self {
            answers: Arc::new(Mutex::new(answers)),
            #[cfg(feature = "agent")]
            managed: None,
            given: BTreeMap::new(),
            reported: Vec::new(),
            logged: Vec::new(),
        }
    }

    /// This host with the managed agent its run consents to, which works
    /// the turns of the call it serves.
    #[cfg(feature = "agent")]
    pub(crate) fn worked_by(
        self,
        managed: Option<Arc<crate::adapters::managed_agent::ManagedAgent>>,
    ) -> Self {
        Self { managed, ..self }
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
        if let ResolutionRequest::Turn { id, .. } = &request
            && let Some(answer) = self.given.remove(id.as_str())
        {
            return Ok(Some(ResolutionResponse {
                actor: ResolutionResponseActor::Agent,
                payload: answer,
            }));
        }
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
    /// answers and the same managed agent, and what it reports comes back to
    /// this host.
    fn detached(&self) -> Option<DetachedHost> {
        Some(Self {
            answers: Arc::clone(&self.answers),
            #[cfg(feature = "agent")]
            managed: self.managed.clone(),
            given: BTreeMap::new(),
            reported: Vec::new(),
            logged: Vec::new(),
        })
    }

    /// The managed agent, when the run consents to one, works a turn once
    /// nothing in it waits on a human.
    fn drive_turn(&mut self, turn: &mut dyn Turn) -> Result<(), RuntimeError> {
        #[cfg(feature = "agent")]
        if let Some(managed) = self.managed.clone()
            && let Some(output) = managed.drive(turn, self)?
        {
            self.given
                .insert(turn.invocation().id.as_str().to_owned(), output);
        }
        #[cfg(not(feature = "agent"))]
        let _ = turn;
        Ok(())
    }

    /// Only a managed agent at a loopback endpoint is known to run here.
    fn reads_local_only(&self) -> bool {
        #[cfg(feature = "agent")]
        {
            self.managed.as_ref().is_some_and(|agent| agent.is_local())
        }
        #[cfg(not(feature = "agent"))]
        {
            false
        }
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
