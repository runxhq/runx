use runx_contracts::{
    JsonObject, OperationalPolicyValidationFinding, PolicyLint, PolicyLintEngine, PolicyLintStatus,
};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

use super::NativeInvocation;
use super::capability::{NativeCapability, TypedNativeCapability};
use crate::RuntimeError;

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct PolicyLintInput {
    policy: JsonObject,
}

impl CapabilityInput for PolicyLintInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct PolicyLintOutput {
    policy_lint: PolicyLint,
}

impl CapabilityOutput for PolicyLintOutput {}

const FIELDS: &[CapabilityField] = &[CapabilityField {
    name: "policy",
    description: "Exact in-memory runx.operational_policy.v1 candidate.",
}];

static LINT: TypedNativeCapability<PolicyLintInput, PolicyLintOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "policy.lint",
        source: "runx-core/policy",
        summary: "Parse and lint one operational policy through the canonical policy engine.",
        scopes: &[],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "policy_lint",
            packet: "runx.policy.lint.v1",
        },
        fields: FIELDS,
    },
    lint_policy,
);

pub(super) const CAPABILITIES: &[&dyn NativeCapability] = &[&LINT];

fn lint_policy(
    invocation: &NativeInvocation<'_, PolicyLintInput>,
) -> Result<PolicyLintOutput, RuntimeError> {
    let parsed = serde_json::from_slice::<runx_contracts::OperationalPolicy>(
        &serde_json::to_vec(&invocation.inputs.policy)
            .map_err(|source| RuntimeError::json("serializing policy lint input", source))?,
    );
    let policy_lint = match parsed {
        Ok(policy) => match runx_contracts::project_operational_policy_readback(&policy) {
            Ok(readback) => PolicyLint {
                status: if readback.valid {
                    PolicyLintStatus::Pass
                } else {
                    PolicyLintStatus::Fail
                },
                engine: PolicyLintEngine::RunxPolicy,
                findings: readback.findings.clone(),
                readback: Some(readback),
            },
            Err(error) => lint_failure(error.finding().clone()),
        },
        Err(error) => lint_failure(OperationalPolicyValidationFinding {
            code: "policy.contract.invalid".into(),
            path: "$".into(),
            message: format!("policy does not match runx.operational_policy.v1: {error}").into(),
        }),
    };
    Ok(PolicyLintOutput { policy_lint })
}

fn lint_failure(finding: OperationalPolicyValidationFinding) -> PolicyLint {
    PolicyLint {
        status: PolicyLintStatus::Fail,
        engine: PolicyLintEngine::RunxPolicy,
        findings: vec![finding],
        readback: None,
    }
}
