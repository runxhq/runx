// The dispatch boundary materializes one step's inputs and hands the step to
// its handler.
use std::path::Path;

use runx_contracts::Reference;
use runx_parser::GraphStep;

use super::super::graph::{
    LoadedStepSkill, materialize_step_act_with_index,
    materialize_step_invocation_inputs_with_index,
    materialize_step_invocation_provenance_with_index,
};
use super::super::graph_index::PriorRunIndex;
use super::step_handlers::{StepRunRequest, run_step_with_inputs};
use super::{Runtime, StepRun};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::host::Host;

pub(super) struct LoadedStepExecutionRequest<'a, A: SkillAdapter> {
    pub(super) runtime: &'a Runtime<A>,
    pub(super) graph_dir: &'a Path,
    pub(super) graph_name: &'a str,
    pub(super) step: &'a GraphStep,
    pub(super) attempt: u32,
    pub(super) loaded_skill: Option<LoadedStepSkill>,
    pub(super) policy_approval_refs: Vec<Reference>,
    pub(super) host: &'a mut dyn Host,
}

pub(super) fn run_step_with_loaded_skill_index<A>(
    request: LoadedStepExecutionRequest<'_, A>,
    prior_run_index: &PriorRunIndex<'_>,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let inputs = materialize_step_invocation_inputs_with_index(request.step, prior_run_index)?;
    // A skill step's act is its child run's; an operation step's is already
    // one of its call's fields.
    let child_act = match request.step.skill() {
        Some(_) => materialize_step_act_with_index(request.step, prior_run_index)?
            .map(|act| {
                runx_contracts::CallAct::parse(&runx_contracts::JsonValue::Object(act)).map_err(
                    |reason| RuntimeError::InvalidRunStep {
                        step_id: request.step.id.clone(),
                        reason,
                    },
                )
            })
            .transpose()?,
        None => None,
    };
    let mut provenance =
        materialize_step_invocation_provenance_with_index(request.step, prior_run_index)?;
    provenance.extend_from_slice(request.runtime.inherited_provenance(&request.step.id));
    run_step_with_loaded_skill_inputs(request, inputs, child_act, provenance)
}

fn run_step_with_loaded_skill_inputs<A>(
    request: LoadedStepExecutionRequest<'_, A>,
    inputs: runx_contracts::JsonObject,
    child_act: Option<runx_contracts::CallAct>,
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
        host,
    } = request;
    let request = StepRunRequest {
        runtime,
        graph_dir,
        graph_name,
        step,
        attempt,
        inputs,
        child_act,
        provenance,
        policy_approval_refs,
        host,
    };
    match loaded_skill {
        Some(skill) => super::step_handlers::run_step_with_loaded_skill_inputs(request, skill),
        None => run_step_with_inputs(request),
    }
}
