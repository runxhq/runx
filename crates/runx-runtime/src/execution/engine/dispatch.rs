// The dispatch boundary materializes one step's inputs and hands the step to
// its handler.
use std::path::Path;

use runx_contracts::Reference;
use runx_parser::WorkflowStep;

use super::super::workflow::{
    LoadedStepSkill, bound_step_labels, materialize_ask_reason_with_index,
    materialize_step_act_with_index, materialize_step_context_entries_with_index,
    materialize_step_invocation_inputs_with_index,
    materialize_step_invocation_provenance_with_index,
};
use super::super::workflow_index::PriorRunIndex;
use super::step_handlers::{StepRunRequest, run_step_with_inputs};
use super::{Runtime, StepRun};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::host::Host;

pub(super) struct LoadedStepExecutionRequest<'a, A: SkillAdapter> {
    pub(super) runtime: &'a Runtime<A>,
    pub(super) workflow_dir: &'a Path,
    pub(super) workflow_name: &'a str,
    pub(super) step: &'a WorkflowStep,
    pub(super) attempt: u32,
    pub(super) loaded_skill: Option<LoadedStepSkill>,
    pub(super) admission_refs: Vec<Reference>,
    /// The floor holds the step's write under `effects: ask`.
    pub(super) held_by_effects: bool,
    pub(super) host: &'a mut dyn Host,
    /// How a claimed step whose run stopped is taken further, when it is.
    pub(super) reconciliation: Option<crate::effects::Reconciliation>,
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
                    |reason| RuntimeError::StepFailed {
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
    let context_entries =
        materialize_step_context_entries_with_index(request.step, prior_run_index)?;
    let ask_reason = materialize_ask_reason_with_index(request.step, prior_run_index)?;
    // What the step is given: its inputs, the artifacts they name, and the
    // earlier steps' outputs it binds, each as that step labelled it.
    let given = runx_contracts::ArtifactLabels::derived_from([
        &request
            .runtime
            .local_artifacts
            .labels_of(&runx_contracts::JsonValue::Object(inputs.clone())),
        &bound_step_labels(request.step, prior_run_index),
    ]);
    run_step_with_loaded_skill_inputs(
        request,
        StepValues {
            inputs,
            child_act,
            provenance,
            context_entries,
            ask_reason,
            given,
        },
    )
}

/// What a step runs with, each read where its inputs are.
struct StepValues {
    inputs: runx_contracts::JsonObject,
    child_act: Option<runx_contracts::CallAct>,
    provenance: Vec<runx_contracts::ProvenanceEntry>,
    context_entries: Vec<runx_contracts::ContextEntry>,
    ask_reason: Option<String>,
    given: runx_contracts::ArtifactLabels,
}

fn run_step_with_loaded_skill_inputs<A>(
    request: LoadedStepExecutionRequest<'_, A>,
    values: StepValues,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let LoadedStepExecutionRequest {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        loaded_skill,
        admission_refs,
        held_by_effects,
        host,
        reconciliation,
    } = request;
    let StepValues {
        inputs,
        child_act,
        provenance,
        context_entries,
        ask_reason,
        given,
    } = values;
    let request = StepRunRequest {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        inputs,
        child_act,
        provenance,
        admission_refs,
        held_by_effects,
        host,
        context_entries,
        ask_reason,
        given,
        reconciliation,
    };
    match loaded_skill {
        Some(skill) => super::step_handlers::run_step_with_loaded_skill_inputs(request, skill),
        None => run_step_with_inputs(request),
    }
}
