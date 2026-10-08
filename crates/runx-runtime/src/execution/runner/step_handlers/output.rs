//! Step output projection helpers. Translate the invocation's typed claim and
//! declared run-outputs / artifact-emits into the typed step projection that
//! downstream graph state machines and receipt sealers consume.

use runx_contracts::JsonObject;
use runx_parser::{GraphStep, SkillArtifactContract};

use crate::RuntimeError;
use crate::adapter::InvocationOutput;
use crate::execution::output_projection::{StepOutputProjection, project_step_claim};
use crate::output_contract::{project_artifact_outputs, project_declared_output_claim};

/// Project a step's output from its producing runner contract.
///
/// The addressable surface is sourced from the contract, never from the step
/// kind: declared `run.outputs` plus the effective artifact packets. The
/// effective artifact contract is the step's own inline `artifacts` when present,
/// otherwise `extra_artifacts` (the invoked sub-skill / tool runner contract).
pub(super) fn build_step_output_projection(
    step: &GraphStep,
    output: &InvocationOutput,
    extra_outputs: Option<&JsonObject>,
    extra_artifacts: Option<&SkillArtifactContract>,
) -> Result<StepOutputProjection, RuntimeError> {
    // A failed invocation produced diagnostics, not its declared success
    // contract. Preserve that failure for sealing instead of replacing it with
    // a secondary "declared output was not returned" projection error.
    if !output.succeeded() {
        return Ok(project_step_claim(JsonObject::new()));
    }
    let declared_outputs = step.outputs.as_ref().or(extra_outputs);
    let artifacts = step.artifacts.as_ref().or(extra_artifacts);
    let claim = project_declared_output_claim(&step.id, &output.value, declared_outputs, artifacts)
        .map_err(|error| RuntimeError::InvalidRunStep {
            step_id: step.id.clone(),
            reason: error.to_string(),
        })?;
    Ok(project_step_claim(claim))
}

/// Return only the step outputs declared by its runner or artifact contract.
/// Effect supervisors must inspect this same addressable surface that downstream
/// graph steps consume, never the adapter's transport-level stdout shape.
pub(super) fn contract_output_claim(projection: &StepOutputProjection) -> &JsonObject {
    &projection.outputs
}

/// Project only caller-only values that are actually present. Ephemeral output
/// is an overlay, so it never owes the complete durable output contract.
pub(super) fn build_ephemeral_step_output(
    step: &GraphStep,
    output: &InvocationOutput,
    extra_outputs: Option<&JsonObject>,
    extra_artifacts: Option<&SkillArtifactContract>,
) -> JsonObject {
    let Some(value) = output.ephemeral.as_value() else {
        return JsonObject::new();
    };
    let fields = value.as_object();
    let declared_outputs = step.outputs.as_ref().or(extra_outputs);
    let mut projected = JsonObject::new();
    if let (Some(fields), Some(declared)) = (fields, declared_outputs) {
        for name in declared.keys() {
            if let Some(value) = fields.get(name) {
                projected.insert(name.clone(), value.clone());
            }
        }
    }
    projected.extend(project_artifact_outputs(
        value,
        step.artifacts.as_ref().or(extra_artifacts),
    ));
    projected
}

#[cfg(test)]
mod tests {
    use runx_contracts::{JsonObject, JsonValue};
    use runx_parser::{GraphStep, parse_graph_yaml, validate_graph};

    use super::build_step_output_projection;
    use crate::RuntimeError;
    use crate::adapter::InvocationOutput;

    #[test]
    fn failed_invocation_preserves_its_diagnostic_instead_of_enforcing_success_outputs()
    -> Result<(), Box<dyn std::error::Error>> {
        let step = declared_output_step()?;
        let output = InvocationOutput::runtime_failure(
            JsonValue::Object(JsonObject::from([(
                "provider_error".to_owned(),
                JsonValue::String("credits depleted".to_owned()),
            )])),
            "credits depleted",
            4,
            JsonObject::new(),
        );

        let projection = build_step_output_projection(&step, &output, None, None)?;

        assert!(projection.outputs.is_empty());
        assert_eq!(
            output.failure_message().as_deref(),
            Some("credits depleted")
        );
        Ok(())
    }

    #[test]
    fn successful_invocation_still_owes_every_declared_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let step = declared_output_step()?;
        let output = InvocationOutput::runtime_success(JsonValue::Null, 4, JsonObject::new());

        let error = match build_step_output_projection(&step, &output, None, None) {
            Err(error) => error,
            Ok(_) => return Err("success without the declared output did not fail".into()),
        };

        assert!(matches!(
            error,
            RuntimeError::InvalidRunStep { reason, .. }
                if reason.contains("declared run output \"result\" was not returned")
        ));
        Ok(())
    }

    #[test]
    fn successful_invocation_may_omit_an_optional_declared_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let graph = validate_graph(parse_graph_yaml(
            r#"
name: optional-output-contract
steps:
  - id: produce
    tool: { module: produce.mjs, export: run }
    outputs:
      result:
        type: object
        required: false
"#,
        )?)?;
        let step = graph
            .steps
            .into_iter()
            .next()
            .ok_or("validated output-contract graph omitted its step")?;
        let output = InvocationOutput::runtime_success(
            JsonValue::Object(JsonObject::new()),
            4,
            JsonObject::new(),
        );

        let projection = build_step_output_projection(&step, &output, None, None)?;

        assert!(projection.outputs.is_empty());
        Ok(())
    }

    fn declared_output_step() -> Result<GraphStep, Box<dyn std::error::Error>> {
        let graph = validate_graph(parse_graph_yaml(
            r#"
name: output-contract
steps:
  - id: produce
    tool: { module: produce.mjs, export: run }
    outputs:
      result: object
"#,
        )?)?;
        graph
            .steps
            .into_iter()
            .next()
            .ok_or_else(|| "validated output-contract graph omitted its step".into())
    }
}
