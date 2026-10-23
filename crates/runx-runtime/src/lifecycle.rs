//! The events a run reports to its host as it executes: each step starting
//! and closing, and the run's seal. The run record is the durable account;
//! these are the live notifications.
use runx_contracts::{ExecutionEvent, JsonObject, JsonValue, Receipt, RunOutcome};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LifecycleEvent {
    StepStarted {
        step_id: String,
    },
    StepClosed {
        step_id: String,
        outcome: RunOutcome,
    },
    RunSealed {
        receipt_id: String,
        subject_uri: String,
        outcome: RunOutcome,
        message: String,
    },
}

impl LifecycleEvent {
    pub(crate) fn step_started(step_id: &str) -> Self {
        Self::StepStarted {
            step_id: step_id.to_owned(),
        }
    }

    pub(crate) fn step_completed(step_id: &str) -> Self {
        Self::StepClosed {
            step_id: step_id.to_owned(),
            outcome: RunOutcome::Completed,
        }
    }

    pub(crate) fn step_failed(step_id: &str) -> Self {
        Self::StepClosed {
            step_id: step_id.to_owned(),
            outcome: RunOutcome::Failed,
        }
    }

    pub(crate) fn graph_completed(graph_name: &str, receipt: &Receipt) -> Self {
        Self::sealed(receipt, format!("graph {graph_name} completed"))
    }

    /// The graph stopped at `step_id` with its receipt's outcome.
    pub(crate) fn graph_stopped(graph_name: &str, step_id: &str, receipt: &Receipt) -> Self {
        Self::sealed(
            receipt,
            format!(
                "graph {graph_name} {} at {step_id}",
                receipt.seal.outcome.label()
            ),
        )
    }

    fn sealed(receipt: &Receipt, message: String) -> Self {
        Self::RunSealed {
            receipt_id: receipt.id.to_string(),
            subject_uri: receipt.subject.reference.uri.to_string(),
            outcome: receipt.seal.outcome,
            message,
        }
    }

    pub(crate) fn into_execution_event(self) -> ExecutionEvent {
        match self {
            Self::StepStarted { step_id } => ExecutionEvent::StepStarted {
                message: format!("step {step_id} started"),
                data: Some(event_data([
                    ("kind", "step_started".to_owned()),
                    ("step_id", step_id),
                ])),
            },
            Self::StepClosed { step_id, outcome } => {
                let data = Some(event_data([
                    ("kind", "step_closed".to_owned()),
                    ("step_id", step_id.clone()),
                    ("outcome", outcome.label().to_owned()),
                ]));
                if outcome == RunOutcome::Completed {
                    ExecutionEvent::StepCompleted {
                        message: format!("step {step_id} completed"),
                        data,
                    }
                } else {
                    ExecutionEvent::Warning {
                        message: format!("step {step_id} {}", outcome.label()),
                        data,
                    }
                }
            }
            Self::RunSealed {
                receipt_id,
                subject_uri,
                outcome,
                message,
            } => ExecutionEvent::Completed {
                message,
                data: Some(event_data([
                    ("kind", "run_sealed".to_owned()),
                    ("receipt_id", receipt_id),
                    ("subject_uri", subject_uri),
                    ("outcome", outcome.label().to_owned()),
                ])),
            },
        }
    }
}

fn event_data<const N: usize>(fields: [(&'static str, String); N]) -> JsonValue {
    JsonValue::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), JsonValue::String(value)))
            .collect::<JsonObject>(),
    )
}
