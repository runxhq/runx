use crate::{CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect};

use super::capability::{NativeCapability, TypedNativeCapability};

mod handlers;
mod inputs;
mod outputs;

use handlers::{apply_skill, bind_skill, inspect_skill, validate_skill};
use inputs::{
    APPLY_FIELDS, ApplyInput, BIND_FIELDS, BindInput, INSPECT_FIELDS, InspectInput,
    VALIDATE_FIELDS, ValidateInput,
};
use outputs::{ApplyOutput, AuthoringInspectOutput, BindOutput, ValidateOutput};

const VALIDATE_TOOL: &str = "runx.skill.validate";
const BIND_TOOL: &str = "runx.skill.bind";
const APPLY_TOOL: &str = "runx.skill.apply";

static INSPECT: TypedNativeCapability<InspectInput, AuthoringInspectOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "runx.skill.inspect",
            source: "runx-runtime/authoring",
            summary: "Inspect one package and its catalog without spawning a subprocess.",
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
        summary: "Validate an existing, inline or released skill package and run its cases, each reported passed, failed or inconclusive.",
        // The candidate's test reads the registry children its closure
        // names from the registry the operator selects.
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

static BIND: TypedNativeCapability<BindInput, BindOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: BIND_TOOL,
        source: "runx-runtime/authoring",
        summary: "Check one authored change's decision against its mode and bind it to the package state it was authored against.",
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
        summary: "Validate one bound skill change as the staged package it makes, and write it unless the mode is design.",
        // Applying validates the candidate first, test included.
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

pub(super) const CAPABILITIES: &[&dyn NativeCapability] = &[&INSPECT, &BIND, &VALIDATE, &APPLY];
