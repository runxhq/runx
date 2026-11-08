use super::super::types::{
    SequentialWorkflow, SequentialWorkflowStep, SequentialWorkflowStepDefinition, WorkflowState,
    WorkflowStepState,
};

#[must_use]
pub fn create_sequential_workflow(
    workflow_id: impl Into<String>,
    steps: &[SequentialWorkflowStepDefinition],
) -> SequentialWorkflow {
    SequentialWorkflow {
        workflow_id: workflow_id.into(),
        state: WorkflowState::Pending,
        steps: steps
            .iter()
            .map(|step| SequentialWorkflowStep {
                step_id: step.id.clone(),
                state: WorkflowStepState::Pending,
                attempts: 0,
                started_at: None,
                completed_at: None,
                receipt_id: None,
                outputs: None,
                error: None,
            })
            .collect(),
    }
}
