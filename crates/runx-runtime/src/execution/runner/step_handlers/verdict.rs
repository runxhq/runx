//! A verdict names who judged: the package, method and run of the step that
//! returned it (`runx.verdict.v1`). The runtime sets it, so an evaluator
//! can't claim to be another: a verdict that names some other evaluator is
//! refused, and its step fails. Whether a verdict answers what its reader
//! asked, its subject and its criteria set, is the reader's to decide
//! (`verdict.decide`).

use std::path::Path;

use runx_contracts::{Evaluator, JsonValue, VERDICT_SCHEMA};
use runx_parser::{OutputArtifacts, WorkflowStep};

use super::super::Runtime;
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::execution::run_record::Origin;

/// Name the step that returned them as the evaluator of the verdicts in
/// `value`: each output `artifacts` keeps as a `runx.verdict.v1`.
pub(super) fn name_verdicts<A>(
    runtime: &Runtime<A>,
    workflow_dir: &Path,
    step: &WorkflowStep,
    artifacts: Option<&OutputArtifacts>,
    value: &mut JsonValue,
) -> Result<(), RuntimeError>
where
    A: SkillAdapter,
{
    let Some(artifacts) = artifacts else {
        return Ok(());
    };
    let outputs = artifacts
        .packets
        .iter()
        .filter(|(_, packet)| packet.as_str() == VERDICT_SCHEMA)
        .map(|(output, _)| output.clone())
        .collect::<Vec<_>>();
    let JsonValue::Object(fields) = value else {
        return Ok(());
    };
    if outputs.iter().all(|output| !fields.contains_key(output)) {
        return Ok(());
    }
    let evaluator = evaluator(runtime, workflow_dir, step)?;
    let named = JsonValue::from_serialize(&evaluator).map_err(|error| failed(step, error))?;
    for output in outputs {
        let Some(JsonValue::Object(verdict)) = fields.get_mut(&output) else {
            continue;
        };
        match verdict.get("evaluator") {
            None => {
                verdict.insert("evaluator".to_owned(), named.clone());
            }
            Some(stated) if stated == &named => {}
            Some(stated) => {
                let stated = serde_json::to_string(stated).unwrap_or_default();
                return Err(RuntimeError::StepFailed {
                    step_id: step.id.clone(),
                    reason: format!(
                        "the verdict it returns as {output} names {stated} as its evaluator, and {}#{} in run {} returned it; runx names who judged",
                        evaluator.package, evaluator.method, evaluator.run
                    ),
                });
            }
        }
    }
    Ok(())
}

/// The step's package, its method and its run.
fn evaluator<A>(
    runtime: &Runtime<A>,
    workflow_dir: &Path,
    step: &WorkflowStep,
) -> Result<Evaluator, RuntimeError>
where
    A: SkillAdapter,
{
    let options = &runtime.options;
    let package = options
        .chain_at(workflow_dir)?
        .last()
        .map(|link| link.package.as_str().to_owned())
        .ok_or_else(|| RuntimeError::StepFailed {
            step_id: step.id.clone(),
            reason: "it returns a verdict, and no package runs it to name as the evaluator"
                .to_owned(),
        })?;
    let record = options
        .run
        .load()?
        .ok_or_else(|| RuntimeError::StepFailed {
            step_id: step.id.clone(),
            reason: "it returns a verdict before its run is recorded".to_owned(),
        })?;
    // A child run records the method it runs as its `skill`.
    let method = match record.origin {
        Origin::Run(started) => started.method,
        Origin::Child(started) => started.skill,
        Origin::Evaluation(started) => started.target,
    };
    Ok(Evaluator {
        package: package.into(),
        method: method.into(),
        run: options.run.run_id().into(),
    })
}

fn failed(step: &WorkflowStep, error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::StepFailed {
        step_id: step.id.clone(),
        reason: format!("naming its verdict's evaluator: {error}"),
    }
}
