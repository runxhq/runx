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
#[cfg(any(feature = "agent", feature = "mcp"))]
pub(crate) fn with_met_requirements(
    answer: JsonValue,
    met: &[runx_contracts::TurnRequirement],
) -> Result<JsonValue, String> {
    if met.is_empty() {
        return Ok(answer);
    }
    stating(
        answer,
        "requirements",
        JsonValue::Array(
            met.iter()
                .map(|requirement| JsonValue::String(requirement.as_str().to_owned()))
                .collect(),
        ),
    )
}

/// `answer`, saying `model` answered it: in its closure's `model`, with the
/// act completed when the answer states no closure of its own.
///
/// # Errors
///
/// Returns an error when the answer isn't an object, or its closure isn't.
#[cfg(feature = "agent")]
pub(crate) fn with_stated_model(answer: JsonValue, model: &str) -> Result<JsonValue, String> {
    stating(answer, "model", JsonValue::String(model.to_owned()))
}

/// `answer` with `value` at its closure's `key`, the act completed when the
/// answer states no closure of its own.
#[cfg(any(feature = "agent", feature = "mcp"))]
fn stating(answer: JsonValue, key: &str, value: JsonValue) -> Result<JsonValue, String> {
    let JsonValue::Object(mut answer) = answer else {
        return Err(format!("an answer that states its {key} is an object"));
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
    closure.insert(key.to_owned(), value);
    Ok(JsonValue::Object(answer))
}

/// The longest model name an answer may state.
const MAX_MODEL: usize = 200;

/// The model an agent turn's answer says answered it, in its closure's
/// `model`: its agent harness or runx's driver states it, and a turn whose
/// answer states none has no model of record.
///
/// # Errors
///
/// Returns an error when the closure states a model that isn't a name.
pub(crate) fn stated_model(answer: &JsonValue) -> Result<Option<&str>, String> {
    let stated = answer
        .as_object()
        .and_then(|answer| answer.get("closure"))
        .and_then(JsonValue::as_object)
        .and_then(|closure| closure.get("model"));
    match stated {
        None => Ok(None),
        Some(JsonValue::String(model))
            if !model.trim().is_empty()
                && model.chars().count() <= MAX_MODEL
                && !model.chars().any(char::is_control) =>
        {
            Ok(Some(model.as_str()))
        }
        Some(_) => Err(format!(
            "agent answer closure.model names the model that answered, in 1 to {MAX_MODEL} characters"
        )),
    }
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

    #[cfg(any(feature = "agent", feature = "mcp"))]
    use super::with_met_requirements;
    use super::{
        OutcomeClaimError, agent_answer_outcome_or_completed, check_turn_requirements,
        failed_answer,
    };

    /// runx's driver states the model that answered a turn, and the model
    /// an answer states is read back as it was stated; one that isn't a
    /// name refuses the answer.
    #[cfg(feature = "agent")]
    #[test]
    fn a_turns_answer_states_the_model_that_answered() -> Result<(), Box<dyn std::error::Error>> {
        let answer = |value: serde_json::Value| serde_json::from_value::<JsonValue>(value);
        let stated = super::with_stated_model(
            answer(serde_json::json!({"draft": "Hello."}))?,
            "claude-haiku-4-5",
        )?;
        assert_eq!(super::stated_model(&stated), Ok(Some("claude-haiku-4-5")));
        assert_eq!(
            agent_answer_outcome_or_completed(&stated),
            Ok(RunOutcome::Completed)
        );
        assert_eq!(
            super::stated_model(&answer(serde_json::json!({"draft": "Hello."}))?),
            Ok(None)
        );
        for named in [
            serde_json::json!(""),
            serde_json::json!(7),
            serde_json::json!("a\nb"),
        ] {
            let closure = serde_json::json!({"closure": {"outcome": "completed", "model": named}});
            assert!(super::stated_model(&answer(closure)?).is_err());
        }
        Ok(())
    }

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

    // A driver that works turns states the requirements it met.
    #[cfg(any(feature = "agent", feature = "mcp"))]
    #[test]
    fn a_driver_s_answer_states_the_requirements_it_met() -> Result<(), serde_json::Error> {
        let fresh = [TurnRequirement::FreshContext];
        let stated = with_met_requirements(
            serde_json::from_str::<JsonValue>(r#"{"summary": "done"}"#)?,
            &fresh,
        );
        assert_eq!(
            stated.and_then(|stated| check_turn_requirements(
                &fresh,
                &stated,
                RunOutcome::Completed
            )),
            Ok(())
        );
        Ok(())
    }
}
