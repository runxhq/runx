//! A verdict names who judged: the package, method and run of the step that
//! returned it (`runx.verdict.v1`). The runtime names it where every step
//! kind's output becomes its claim, so an evaluator can't claim to be
//! another: a verdict that names some other evaluator fails its step.
//! Whether a verdict answers what its reader asked, its subject and its
//! criteria set, is the reader's to decide (`verdict.decide`).

use std::path::Path;

use runx_contracts::{Evaluator, JsonObject, JsonValue, VERDICT_SCHEMA};
use runx_parser::{OutputArtifacts, WorkflowStep};

use super::super::Runtime;
use crate::RuntimeError;
use crate::execution::run_record::Origin;

/// Who the verdicts a step returns name as their evaluator.
pub(super) enum Evaluating<'a> {
    /// The step itself, as `evaluator` works it out when the step returns a
    /// verdict.
    Step(&'a dyn Fn() -> Result<Evaluator, RuntimeError>),
    /// A skill step's child run, which named the verdicts it returned as its
    /// own steps returned them.
    Child,
}

/// Name the step's evaluator in each verdict `claim` holds: each output
/// `artifacts` keeps as a `runx.verdict.v1`, in its packet envelope.
pub(super) fn name_verdicts(
    claim: &mut JsonObject,
    artifacts: Option<&OutputArtifacts>,
    evaluating: &Evaluating<'_>,
    step_id: &str,
) -> Result<(), RuntimeError> {
    let Evaluating::Step(evaluator) = evaluating else {
        return Ok(());
    };
    let Some(artifacts) = artifacts else {
        return Ok(());
    };
    let verdicts = artifacts
        .packets
        .iter()
        .filter(|(output, packet)| {
            packet.as_str() == VERDICT_SCHEMA && claim.contains_key(output.as_str())
        })
        .map(|(output, _)| output.clone())
        .collect::<Vec<_>>();
    if verdicts.is_empty() {
        return Ok(());
    }
    let evaluator = evaluator()?;
    let named =
        JsonValue::from_serialize(&evaluator).map_err(|error| RuntimeError::StepFailed {
            step_id: step_id.to_owned(),
            reason: format!("naming its verdict's evaluator: {error}"),
        })?;
    for output in verdicts {
        let Some(JsonValue::Object(verdict)) =
            claim.get_mut(&output).and_then(|envelope| match envelope {
                JsonValue::Object(envelope) => envelope.get_mut("data"),
                _ => None,
            })
        else {
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
                    step_id: step_id.to_owned(),
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

/// The evaluator of `step`: its package, its method and its run.
pub(super) fn evaluator<A>(
    runtime: &Runtime<A>,
    workflow_dir: &Path,
    step: &WorkflowStep,
) -> Result<Evaluator, RuntimeError> {
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
