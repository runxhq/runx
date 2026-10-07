// The typed dispatch boundary materializes one step's inputs and preserves the
// compiler-checked distinction between sealable outcomes and engine faults.
use std::path::Path;

use runx_contracts::Reference;
use runx_parser::GraphStep;

use super::super::graph::{
    LoadedStepSkill, materialize_step_invocation_inputs,
    materialize_step_invocation_inputs_with_index, materialize_step_invocation_provenance,
    materialize_step_invocation_provenance_with_index,
};
use super::super::graph_index::PriorRunIndex;
use super::step_handlers::{StepRunRequest, run_step_with_inputs};
use super::{GraphCheckpoint, Runtime, StepRun};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::host::Host;

#[derive(Debug)]
pub(super) enum StepFault {
    /// The step itself failed and the runtime may seal that failure as the
    /// branch outcome when the graph's failure policy allows continuation.
    Sealable(RuntimeError),
    /// An engine fault or native suspension must escape the graph unchanged.
    /// Synthesizing a failed step receipt would lose recovery custody.
    Fatal(RuntimeError),
}

impl StepFault {
    pub(super) fn into_runtime_error(self) -> RuntimeError {
        match self {
            Self::Sealable(error) | Self::Fatal(error) => error,
        }
    }

    pub(super) fn at_graph_step(self, step_id: &str) -> Self {
        match self {
            Self::Sealable(error) => Self::Sealable(error.at_graph_step(step_id)),
            Self::Fatal(error) => Self::Fatal(error.at_graph_step(step_id)),
        }
    }
}

impl From<RuntimeError> for StepFault {
    fn from(error: RuntimeError) -> Self {
        if error.is_fatal_step_fault() {
            Self::Fatal(error)
        } else {
            Self::Sealable(error)
        }
    }
}

pub(super) struct LoadedStepExecutionRequest<'a, A: SkillAdapter> {
    pub(super) runtime: &'a Runtime<A>,
    pub(super) graph_dir: &'a Path,
    pub(super) graph_name: &'a str,
    pub(super) step: &'a GraphStep,
    pub(super) attempt: u32,
    pub(super) loaded_skill: Option<LoadedStepSkill>,
    pub(super) policy_approval_refs: Vec<Reference>,
    pub(super) child_checkpoint: Option<Box<GraphCheckpoint>>,
    pub(super) host: &'a mut dyn Host,
}

pub(super) fn run_step_with_loaded_skill<A>(
    request: LoadedStepExecutionRequest<'_, A>,
    prior_runs: &[StepRun],
) -> Result<StepRun, StepFault>
where
    A: SkillAdapter,
{
    let inputs =
        materialize_step_invocation_inputs(request.step, prior_runs).map_err(StepFault::from)?;
    let provenance = materialize_step_invocation_provenance(request.step, prior_runs)
        .map_err(StepFault::from)?;
    run_step_with_loaded_skill_inputs(request, inputs, provenance).map_err(StepFault::from)
}

pub(super) fn run_step_with_loaded_skill_index<A>(
    request: LoadedStepExecutionRequest<'_, A>,
    prior_run_index: &PriorRunIndex<'_>,
) -> Result<StepRun, StepFault>
where
    A: SkillAdapter,
{
    let inputs = materialize_step_invocation_inputs_with_index(request.step, prior_run_index)
        .map_err(StepFault::from)?;
    let provenance =
        materialize_step_invocation_provenance_with_index(request.step, prior_run_index)
            .map_err(StepFault::from)?;
    run_step_with_loaded_skill_inputs(request, inputs, provenance).map_err(StepFault::from)
}

fn run_step_with_loaded_skill_inputs<A>(
    request: LoadedStepExecutionRequest<'_, A>,
    inputs: runx_contracts::JsonObject,
    provenance: Vec<runx_contracts::ProvenanceEntry>,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let LoadedStepExecutionRequest {
        runtime,
        graph_dir,
        graph_name,
        step,
        attempt,
        loaded_skill,
        policy_approval_refs,
        child_checkpoint,
        host,
    } = request;
    let request = StepRunRequest {
        runtime,
        graph_dir,
        graph_name,
        step,
        attempt,
        inputs,
        provenance,
        policy_approval_refs,
        child_checkpoint,
        host,
    };
    match loaded_skill {
        Some(skill) => super::step_handlers::run_step_with_loaded_skill_inputs(request, skill),
        None => run_step_with_inputs(request),
    }
}

#[cfg(test)]
mod tests {
    use super::StepFault;
    use crate::RuntimeError;

    #[test]
    fn invocation_failure_is_sealable() {
        let fault = StepFault::from(RuntimeError::SkillFailed {
            skill_name: "fixture".to_owned(),
            message: "provider rejected the act".to_owned(),
        });
        assert!(matches!(fault, StepFault::Sealable(_)));
    }

    #[test]
    fn receipt_failure_is_fatal() {
        let fault = StepFault::from(RuntimeError::ReceiptInvalid {
            message: "digest mismatch".to_owned(),
        });
        assert!(matches!(fault, StepFault::Fatal(_)));
    }

    #[test]
    fn parallel_host_interaction_is_fatal() {
        let fault = StepFault::from(RuntimeError::ParallelHostInteraction {
            operation: "resolve",
        });
        assert!(matches!(fault, StepFault::Fatal(_)));
    }

    #[test]
    fn explicitly_wrapped_engine_failure_is_fatal() -> Result<(), Box<dyn std::error::Error>> {
        let source = serde_json::from_str::<serde_json::Value>("{")
            .err()
            .ok_or("fixture unexpectedly contained valid JSON")?;
        let fault = StepFault::from(RuntimeError::engine(
            "sealing a graph step receipt",
            RuntimeError::Json {
                context: "serializing invocation diagnostics".to_owned(),
                source,
            },
        ));
        assert!(matches!(fault, StepFault::Fatal(_)));
        Ok(())
    }
}
