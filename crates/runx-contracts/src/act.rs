//! Act algebra: intents, the checks the runtime binds to an act, and act forms.
pub mod assignment;
pub mod call;
pub mod result;

use serde::{Deserialize, Serialize};

use crate::schema::{NonEmptyString, RunxSchema};
use crate::{Reference, TermLegitimacy};

/// Why an act was done and under what it was legitimate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub purpose: NonEmptyString,
    /// The execution term the act was admitted under.
    pub legitimacy: TermLegitimacy,
}

/// A check the runtime runs and binds to a receipt. The runtime is the only
/// writer of criteria, so they are its own closed vocabulary: each names what
/// was checked, and its binding states the result and the evidence.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, RunxSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Criterion {
    /// The step's outcome: verified when it succeeded, failed when it did not.
    StepOutcome,
    /// The result satisfied the step's declared output contract.
    OutputContractVerified,
    /// The agent invocation used its resolved voice profile.
    VoiceProfileApplied,
    /// Every packet output satisfied its declared packet schema.
    PacketSchemasVerified,
}

impl Criterion {
    /// The criterion's wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StepOutcome => "step_outcome",
            Self::OutputContractVerified => "output_contract_verified",
            Self::VoiceProfileApplied => "voice_profile_applied",
            Self::PacketSchemasVerified => "packet_schemas_verified",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CriterionStatus {
    Verified,
    Failed,
}

/// What kind of act a governed call is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActForm {
    #[default]
    Observation,
    Reply,
    Review,
}

/// One check's result and the references that prove it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CriterionBinding {
    pub criterion_id: Criterion,
    pub status: CriterionStatus,
    pub evidence_refs: Vec<Reference>,
    pub verification_refs: Vec<Reference>,
}
