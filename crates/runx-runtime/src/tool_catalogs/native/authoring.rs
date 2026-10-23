use crate::{CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect};

use super::capability::{NativeCapability, TypedNativeCapability};

mod handlers;
mod inputs;
mod outputs;

use handlers::{apply_skill, bind_skill, inspect_skill, plan_skill, validate_skill};
use inputs::{
    APPLY_FIELDS, ApplyInput, BIND_FIELDS, BindInput, INSPECT_FIELDS, InspectInput, PLAN_FIELDS,
    PlanInput, VALIDATE_FIELDS, ValidateInput,
};
use outputs::{ApplyOutput, AuthoringInspectOutput, BindOutput, PlanOutput, ValidateOutput};

const VALIDATE_TOOL: &str = "runx.skill.validate";
const PLAN_TOOL: &str = "runx.skill.plan";
const BIND_TOOL: &str = "runx.skill.bind";
const APPLY_TOOL: &str = "runx.skill.apply";

static INSPECT: TypedNativeCapability<InspectInput, AuthoringInspectOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "runx.skill.inspect",
            source: "runx-runtime/authoring",
            summary: "Inspect one package and its catalog without spawning a subprocess.",
            scopes: &["fs.read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Named {
                output: "authoring_context",
                packet: "runx.skill_lab.authoring_context.v1",
            },
            fields: INSPECT_FIELDS,
        },
        inspect_skill,
    );

static VALIDATE: TypedNativeCapability<ValidateInput, ValidateOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: VALIDATE_TOOL,
        source: "runx-runtime/authoring",
        summary: "Validate and safely harness an existing or inline skill package.",
        scopes: &["fs.read"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "skill_validation",
            packet: "runx.skill.validation.v1",
        },
        fields: VALIDATE_FIELDS,
    },
    validate_skill,
);

static PLAN: TypedNativeCapability<PlanInput, PlanOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: PLAN_TOOL,
        source: "runx-runtime/authoring",
        summary: "Validate and digest-bind one skill architecture decision.",
        scopes: &[],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "architecture_plan",
            packet: "runx.skill.architecture_plan.v1",
        },
        fields: PLAN_FIELDS,
    },
    plan_skill,
);

static BIND: TypedNativeCapability<BindInput, BindOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: BIND_TOOL,
        source: "runx-runtime/authoring",
        summary: "Bind one closed content draft to its native architecture plan.",
        scopes: &[],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "change_bundle",
            packet: "runx.skill.change_bundle.v1",
        },
        fields: BIND_FIELDS,
    },
    bind_skill,
);

static APPLY: TypedNativeCapability<ApplyInput, ApplyOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: APPLY_TOOL,
        source: "runx-runtime/authoring",
        summary: "Transactionally apply one validated, bounded skill change bundle.",
        scopes: &["fs.write", "fs.delete"],
        effect: CapabilityEffect::Mutate,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "apply_result",
            packet: "runx.skill.apply_result.v1",
        },
        fields: APPLY_FIELDS,
    },
    apply_skill,
)
.describing(handlers::apply_skill_effect);

pub(super) const CAPABILITIES: &[&dyn NativeCapability] =
    &[&INSPECT, &PLAN, &BIND, &VALIDATE, &APPLY];
