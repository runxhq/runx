//! `verdict.criteria` and `verdict.decide`: a criteria set made by a
//! deterministic step, and a verdict read by the one rule
//! (`runx.verdict.v1`). A reader decides a verdict against the subject and the
//! criteria set it expects; a verdict that judges another subject or answers
//! another set is refused, and a refused verdict answers nothing, which isn't
//! `inconclusive`.

use runx_contracts::schema::NonEmptyVec;
use runx_contracts::{Criteria, Pinned, Verdict, VerdictCriterion, VerdictDecision};
use serde::{Deserialize, Serialize};

use super::NativeInvocation;
use super::capability::{NativeCapability, TypedNativeCapability};
use crate::RuntimeError;
use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct CriteriaInput {
    criteria: NonEmptyVec<VerdictCriterion>,
}

impl CapabilityInput for CriteriaInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct CriteriaOutput {
    criteria: Criteria,
}

impl CapabilityOutput for CriteriaOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct DecideInput {
    verdict: Verdict,
    /// What the reader expects judged.
    subject: Pinned,
    /// The criteria set the reader expects answered, and the ref it goes by.
    criteria_ref: String,
    criteria: Criteria,
}

impl CapabilityInput for DecideInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct DecideOutput {
    decision: VerdictDecision,
}

impl CapabilityOutput for DecideOutput {}

const CRITERIA_FIELDS: &[CapabilityField] = &[field(
    "criteria",
    "The checks, each an id, whether it's required, and the question that must be answered yes.",
)];

const DECIDE_FIELDS: &[CapabilityField] = &[
    field("verdict", "The runx.verdict.v1 to decide."),
    field(
        "subject",
        "What the reader expects judged: its ref and revision.",
    ),
    field("criteria_ref", "The ref the expected criteria set goes by."),
    field("criteria", "The expected runx.criteria.v1 set."),
];

const fn field(name: &'static str, description: &'static str) -> CapabilityField {
    CapabilityField { name, description }
}

static CRITERIA: TypedNativeCapability<CriteriaInput, CriteriaOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "verdict.criteria",
        source: "runx-runtime/verdict",
        summary: "Make a runx.criteria.v1 set from its checks, its revision the digest of what it holds.",
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "criteria",
            packet: "runx.criteria.v1",
        },
        fields: CRITERIA_FIELDS,
    },
    criteria,
);

static DECIDE: TypedNativeCapability<DecideInput, DecideOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "verdict.decide",
        source: "runx-runtime/verdict",
        summary: "Decide a runx.verdict.v1 by the one rule against the subject and criteria set the reader expects, or refuse it.",
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "decision",
            packet: "runx.verdict.decision.v1",
        },
        fields: DECIDE_FIELDS,
    },
    decide,
);

pub(super) const CAPABILITIES: &[&dyn NativeCapability] = &[&CRITERIA, &DECIDE];

fn criteria(
    invocation: &NativeInvocation<'_, CriteriaInput>,
) -> Result<CriteriaOutput, RuntimeError> {
    let criteria = Criteria::new(invocation.inputs.criteria.clone()).map_err(|refused| {
        RuntimeError::SkillFailed {
            skill_name: "verdict.criteria".to_owned(),
            message: refused.to_string(),
        }
    })?;
    criteria
        .check()
        .map_err(|refused| RuntimeError::SkillFailed {
            skill_name: "verdict.criteria".to_owned(),
            message: refused.to_string(),
        })?;
    Ok(CriteriaOutput { criteria })
}

fn decide(invocation: &NativeInvocation<'_, DecideInput>) -> Result<DecideOutput, RuntimeError> {
    let inputs = invocation.inputs;
    Ok(DecideOutput {
        decision: VerdictDecision::of(
            &inputs.verdict,
            &inputs.subject,
            &inputs.criteria_ref,
            &inputs.criteria,
        ),
    })
}
