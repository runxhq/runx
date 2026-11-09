use runx_contracts::{JsonValue, RunOutcome};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum OutcomeClaimError {
    #[error("agent answer closure must be an object")]
    EndingNotObject,
    #[error("agent answer closure.outcome is required")]
    MissingOutcome,
    #[error("agent answer closure.outcome must be a string")]
    OutcomeNotString,
    #[error("agent answer closure.outcome {0:?} is not supported")]
    UnsupportedOutcome(String),
    #[error(
        "agent answer closure.outcome {0:?} is the runtime's to observe; an agent ends its act completed, deferred, superseded, declined, blocked or failed"
    )]
    RuntimeObserved(String),
}

pub(crate) fn parse_agent_answer_outcome(
    answer: &JsonValue,
) -> Result<RunOutcome, OutcomeClaimError> {
    let closure = answer
        .as_object()
        .and_then(|object| object.get("closure"))
        .ok_or(OutcomeClaimError::MissingOutcome)?;
    let closure = closure
        .as_object()
        .ok_or(OutcomeClaimError::EndingNotObject)?;
    let outcome = closure
        .get("outcome")
        .ok_or(OutcomeClaimError::MissingOutcome)?;
    let outcome = outcome
        .as_str()
        .ok_or(OutcomeClaimError::OutcomeNotString)?;
    agent_outcome(outcome)
}

pub(crate) fn agent_answer_outcome_or_completed(
    answer: &JsonValue,
) -> Result<RunOutcome, OutcomeClaimError> {
    match answer.as_object() {
        Some(object) if object.contains_key("closure") => parse_agent_answer_outcome(answer),
        _ => Ok(RunOutcome::Completed),
    }
}

/// Whether an agent's answer meets its turn's requirements. An agent harness that
/// meets one says so in `closure.requirements`; one that can't ends the act
/// failed with `closure.unsupported_requirement` naming it. Any other ending
/// that leaves a requirement unstated is refused, so a turn's requirement is
/// never assumed met.
pub(crate) fn check_turn_requirements(
    required: &[runx_contracts::TurnRequirement],
    answer: &JsonValue,
    outcome: RunOutcome,
) -> Result<(), String> {
    let closure = answer
        .as_object()
        .and_then(|object| object.get("closure"))
        .and_then(JsonValue::as_object);
    let named = |value: &JsonValue, field: &str| {
        value
            .as_str()
            .and_then(runx_contracts::TurnRequirement::named)
            .ok_or_else(|| {
                let shown = serde_json::to_string(value).unwrap_or_default();
                format!(
                    "agent answer {field} names no requirement: {shown}; a turn requires {}",
                    runx_contracts::TurnRequirement::ALL
                        .map(runx_contracts::TurnRequirement::as_str)
                        .join(", ")
                )
            })
    };
    let stated = match closure.and_then(|closure| closure.get("requirements")) {
        None => Vec::new(),
        Some(JsonValue::Array(values)) => values
            .iter()
            .map(|value| named(value, "closure.requirements"))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("agent answer closure.requirements must be a list".to_owned()),
    };
    if let Some(value) = closure.and_then(|closure| closure.get("unsupported_requirement")) {
        let unsupported = named(value, "closure.unsupported_requirement")?;
        if !required.contains(&unsupported) {
            return Err(format!(
                "agent answer says its agent harness can't meet {}, which the turn doesn't require",
                unsupported.as_str()
            ));
        }
        if outcome != RunOutcome::Failed {
            return Err(format!(
                "agent answer says its agent harness can't meet {} and ends the act {}; an unmet requirement ends it failed",
                unsupported.as_str(),
                outcome.label()
            ));
        }
        return Ok(());
    }
    if outcome == RunOutcome::Failed {
        return Ok(());
    }
    match required
        .iter()
        .find(|requirement| !stated.contains(requirement))
    {
        Some(missing) => Err(format!(
            "the turn requires {}, and its answer doesn't say its agent harness met it: list it in closure.requirements, or end the act failed with closure.unsupported_requirement",
            missing.as_str()
        )),
        None => Ok(()),
    }
}

/// The answer of an act that failed, saying why, and naming the requirement
/// its agent harness couldn't meet, when that's why.
pub(crate) fn failed_answer(
    reason: String,
    unsupported: Option<runx_contracts::TurnRequirement>,
) -> JsonValue {
    let mut closure = runx_contracts::JsonObject::from([
        ("outcome".to_owned(), JsonValue::String("failed".to_owned())),
        ("reason".to_owned(), JsonValue::String(reason)),
    ]);
    if let Some(requirement) = unsupported {
        closure.insert(
            "unsupported_requirement".to_owned(),
            JsonValue::String(requirement.as_str().to_owned()),
        );
    }
    JsonValue::Object(runx_contracts::JsonObject::from([(
        "closure".to_owned(),
        JsonValue::Object(closure),
    )]))
}

/// `answer`, saying its agent harness met `met`: in its closure's `requirements`,
/// with the act completed when the answer states no closure of its own.
///
/// # Errors
///
/// Returns an error when the answer isn't an object, or its closure isn't.
pub(crate) fn with_met_requirements(
    answer: JsonValue,
    met: &[runx_contracts::TurnRequirement],
) -> Result<JsonValue, String> {
    if met.is_empty() {
        return Ok(answer);
    }
    let JsonValue::Object(mut answer) = answer else {
        return Err("an answer that states its requirements is an object".to_owned());
    };
    let closure = answer.entry("closure".to_owned()).or_insert_with(|| {
        JsonValue::Object(runx_contracts::JsonObject::from([(
            "outcome".to_owned(),
            JsonValue::String("completed".to_owned()),
        )]))
    });
    let JsonValue::Object(closure) = closure else {
        return Err("agent answer closure must be an object".to_owned());
    };
    closure.insert(
        "requirements".to_owned(),
        JsonValue::Array(
            met.iter()
                .map(|requirement| JsonValue::String(requirement.as_str().to_owned()))
                .collect(),
        ),
    );
    Ok(JsonValue::Object(answer))
}

/// How an agent says its act ended. A kill or a timeout is observed by the
/// runtime that ran the act, never claimed by the agent's answer.
fn agent_outcome(outcome: &str) -> Result<RunOutcome, OutcomeClaimError> {
    match outcome {
        "completed" => Ok(RunOutcome::Completed),
        "deferred" => Ok(RunOutcome::Deferred),
        "superseded" => Ok(RunOutcome::Superseded),
        "declined" => Ok(RunOutcome::Declined),
        "blocked" => Ok(RunOutcome::Blocked),
        "failed" => Ok(RunOutcome::Failed),
        "killed" | "timed_out" => Err(OutcomeClaimError::RuntimeObserved(outcome.to_owned())),
        other => Err(OutcomeClaimError::UnsupportedOutcome(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use runx_contracts::{JsonValue, RunOutcome};

    use runx_contracts::TurnRequirement;

    use super::{
        OutcomeClaimError, agent_answer_outcome_or_completed, check_turn_requirements,
        failed_answer, with_met_requirements,
    };

    #[test]
    fn an_agent_cannot_claim_a_kill_or_a_timeout() -> Result<(), serde_json::Error> {
        let outcome = |answer: &str| {
            serde_json::from_str::<JsonValue>(answer)
                .map(|answer| agent_answer_outcome_or_completed(&answer))
        };
        for claimed in ["killed", "timed_out"] {
            assert_eq!(
                outcome(&format!(r#"{{"closure": {{"outcome": "{claimed}"}}}}"#))?,
                Err(OutcomeClaimError::RuntimeObserved(claimed.to_owned()))
            );
        }
        assert_eq!(
            outcome(r#"{"closure": {"outcome": "declined"}}"#)?,
            Ok(RunOutcome::Declined)
        );
        assert_eq!(outcome(r#"{"answer": "done"}"#)?, Ok(RunOutcome::Completed));
        Ok(())
    }

    #[test]
    fn a_turn_s_requirement_is_met_only_when_its_answer_says_so() -> Result<(), serde_json::Error> {
        let fresh = [TurnRequirement::FreshContext];
        let answer = |text: &str| serde_json::from_str::<JsonValue>(text);
        let unstated = check_turn_requirements(
            &fresh,
            &answer(r#"{"summary": "done"}"#)?,
            RunOutcome::Completed,
        );
        assert!(unstated.is_err_and(|reason| reason.contains("requires fresh_context")));
        let stated = with_met_requirements(answer(r#"{"summary": "done"}"#)?, &fresh);
        assert_eq!(
            stated.and_then(|stated| check_turn_requirements(
                &fresh,
                &stated,
                RunOutcome::Completed
            )),
            Ok(())
        );
        let unsupported = failed_answer(
            "no subagent".to_owned(),
            Some(TurnRequirement::FreshContext),
        );
        assert_eq!(
            check_turn_requirements(&fresh, &unsupported, RunOutcome::Failed),
            Ok(())
        );
        assert!(
            check_turn_requirements(&fresh, &unsupported, RunOutcome::Completed)
                .is_err_and(|reason| reason.contains("an unmet requirement ends it failed"))
        );
        let not_required = failed_answer(
            "no reviewer".to_owned(),
            Some(TurnRequirement::SeparateReviewer),
        );
        assert!(
            check_turn_requirements(&fresh, &not_required, RunOutcome::Failed)
                .is_err_and(|reason| reason.contains("doesn't require"))
        );
        let misnamed =
            answer(r#"{"closure": {"outcome": "completed", "requirements": ["telepathy"]}}"#)?;
        assert!(
            check_turn_requirements(&fresh, &misnamed, RunOutcome::Completed)
                .is_err_and(|reason| reason.contains("names no requirement"))
        );
        assert_eq!(
            check_turn_requirements(
                &fresh,
                &failed_answer("gone".to_owned(), None),
                RunOutcome::Failed
            ),
            Ok(()),
            "a failed act states no requirement"
        );
        assert_eq!(
            check_turn_requirements(
                &[],
                &answer(r#"{"summary": "done"}"#)?,
                RunOutcome::Completed
            ),
            Ok(())
        );
        Ok(())
    }
}
