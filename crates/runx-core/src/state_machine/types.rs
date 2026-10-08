// Module rationale: state-machine wire types stay colocated so single-step and
// sequential-graph serde surfaces are reviewed together.
use runx_contracts::{AuthorityVerb, JsonObject, Reference};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Admitted,
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphStepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    /// A `when` condition the runtime resolved to false selected this step out;
    /// it never runs and counts as terminal for graph-completion purposes.
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SingleStepState {
    pub step_id: String,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorityAdmissionWitness {
    pub verb: AuthorityVerb,
    pub parent_term_id: String,
    pub child_term_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_ref: Option<Reference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepAdmissionWitness {
    pub step_id: String,
    pub receipt_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<AuthorityAdmissionWitness>,
}

impl StepAdmissionWitness {
    #[must_use]
    pub fn local_runtime(step_id: impl Into<String>, receipt_id: impl Into<String>) -> Self {
        Self {
            step_id: step_id.into(),
            receipt_id: receipt_id.into(),
            authority: None,
        }
    }

    #[must_use]
    pub fn with_authority(
        step_id: impl Into<String>,
        receipt_id: impl Into<String>,
        authority: AuthorityAdmissionWitness,
    ) -> Self {
        Self {
            step_id: step_id.into(),
            receipt_id: receipt_id.into(),
            authority: Some(authority),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SingleStepEvent {
    Admit,
    Start {
        at: String,
    },
    Succeed {
        at: String,
        admission_witness: Box<StepAdmissionWitness>,
    },
    Fail {
        at: String,
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequentialGraphStepDefinition {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_from: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequentialGraphStepState {
    pub step_id: String,
    pub status: GraphStepStatus,
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequentialGraphState {
    pub graph_id: String,
    pub status: GraphStatus,
    pub steps: Vec<SequentialGraphStepState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SequentialGraphEvent {
    StartStep {
        step_id: String,
        at: String,
    },
    StepSucceeded {
        at: String,
        admission_witness: Box<StepAdmissionWitness>,
        #[serde(skip_serializing_if = "Option::is_none")]
        outputs: Option<JsonObject>,
    },
    StepFailed {
        step_id: String,
        at: String,
        error: String,
    },
    StepSkipped {
        step_id: String,
        at: String,
    },
    Complete,
    FailGraph {
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SequentialGraphPlan {
    RunStep {
        step_id: String,
        attempt: u32,
        context_from: Vec<String>,
    },
    Complete,
    Failed {
        step_id: String,
        reason: String,
    },
    Blocked {
        step_id: String,
        reason: String,
    },
}
