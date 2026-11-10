use runx_contracts::{JsonValue, RunOutcome};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum OutcomeClaimError {
    #[error("agent answer ending must be a string")]
    EndingNotString,
    #[error("agent answer ending {0:?} is not supported")]
    UnsupportedEnding(String),
    #[error(
        "agent answer ending {0:?} is the runtime's to observe; an agent ends its turn completed, deferred, superseded, declined, blocked or failed"
    )]
    RuntimeObserved(String),
    #[error("agent answer gives an ending_reason and no ending")]
    ReasonWithoutEnding,
    #[error(
        "agent answer holds a closure: an answer says how its turn ended in ending, and why in ending_reason, at its top level"
    )]
    Closure,
}

/// How an agent's answer says its turn ended: its `ending`, or completed when
/// it states none.
pub(crate) fn agent_answer_outcome_or_completed(
    answer: &JsonValue,
) -> Result<RunOutcome, OutcomeClaimError> {
    let Some(answer) = answer.as_object() else {
        return Ok(RunOutcome::Completed);
    };
    if answer.contains_key("closure") {
        return Err(OutcomeClaimError::Closure);
    }
    match answer.get("ending") {
        Some(JsonValue::String(ending)) => agent_outcome(ending),
        Some(_) => Err(OutcomeClaimError::EndingNotString),
        None if answer.contains_key("ending_reason") => Err(OutcomeClaimError::ReasonWithoutEnding),
        None => Ok(RunOutcome::Completed),
    }
}

/// Whether an agent's answer meets its turn's requirements. An agent harness that
/// meets one says so in the answer's `requirements`; one that can't ends the turn
/// failed with `unsupported_requirement` naming it. Any other ending that
/// leaves a requirement unstated is refused, so a turn's requirement is never
/// assumed met.
pub(crate) fn check_turn_requirements(
    required: &[runx_contracts::TurnRequirement],
    answer: &JsonValue,
    outcome: RunOutcome,
) -> Result<(), String> {
    let answer = answer.as_object();
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
    let stated = match answer.and_then(|answer| answer.get("requirements")) {
        None => Vec::new(),
        Some(JsonValue::Array(values)) => values
            .iter()
            .map(|value| named(value, "requirements"))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("agent answer requirements must be a list".to_owned()),
    };
    if let Some(value) = answer.and_then(|answer| answer.get("unsupported_requirement")) {
        let unsupported = named(value, "unsupported_requirement")?;
        if !required.contains(&unsupported) {
            return Err(format!(
                "agent answer says its agent harness can't meet {}, which the turn doesn't require",
                unsupported.as_str()
            ));
        }
        if outcome != RunOutcome::Failed {
            return Err(format!(
                "agent answer says its agent harness can't meet {} and ends the turn {}; an unmet requirement ends it failed",
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
            "the turn requires {}, and its answer doesn't say its agent harness met it: list it in requirements, or end the turn failed with unsupported_requirement",
            missing.as_str()
        )),
        None => Ok(()),
    }
}

/// The answer of a turn that failed, saying why, and naming the requirement
/// its agent harness couldn't meet, when that's why.
pub(crate) fn failed_answer(
    reason: String,
    unsupported: Option<runx_contracts::TurnRequirement>,
) -> JsonValue {
    let mut answer = runx_contracts::JsonObject::from([
        ("ending".to_owned(), JsonValue::String("failed".to_owned())),
        ("ending_reason".to_owned(), JsonValue::String(reason)),
    ]);
    if let Some(requirement) = unsupported {
        answer.insert(
            "unsupported_requirement".to_owned(),
            JsonValue::String(requirement.as_str().to_owned()),
        );
    }
    JsonValue::Object(answer)
}

/// `answer`, saying its agent harness met `met`, in its `requirements`.
///
/// # Errors
///
/// Returns an error when the answer isn't an object.
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

/// `answer`, saying `model` answered it, in its `model`.
///
/// # Errors
///
/// Returns an error when the answer isn't an object.
#[cfg(feature = "agent")]
pub(crate) fn with_stated_model(answer: JsonValue, model: &str) -> Result<JsonValue, String> {
    stating(answer, "model", JsonValue::String(model.to_owned()))
}

/// `answer`, recording the host that gave it in its `host`.
///
/// # Errors
///
/// Returns an error when the answer isn't an object.
#[cfg(any(feature = "agent", feature = "mcp"))]
pub(crate) fn with_stated_host(
    answer: JsonValue,
    host: &runx_contracts::TurnHost,
) -> Result<JsonValue, String> {
    let host = JsonValue::from_serialize(host).map_err(|error| error.to_string())?;
    stating(answer, "host", host)
}

/// `answer` with `value` at its `key`.
#[cfg(any(feature = "agent", feature = "mcp"))]
fn stating(answer: JsonValue, key: &str, value: JsonValue) -> Result<JsonValue, String> {
    let JsonValue::Object(mut answer) = answer else {
        return Err(format!("an answer that states its {key} is an object"));
    };
    answer.insert(key.to_owned(), value);
    Ok(JsonValue::Object(answer))
}

/// Whether an agent turn's answer states its agent harness met `requirement`,
/// in its `requirements`.
pub(crate) fn states_met(answer: &JsonValue, requirement: runx_contracts::TurnRequirement) -> bool {
    answer
        .as_object()
        .and_then(|answer| answer.get("requirements"))
        .and_then(JsonValue::as_array)
        .is_some_and(|stated| {
            stated
                .iter()
                .any(|stated| stated.as_str() == Some(requirement.as_str()))
        })
}

/// The host an agent turn's answer records as the one that gave it, in its
/// `host`; none when it records none.
///
/// # Errors
///
/// Returns an error when the answer's host isn't a host.
pub(crate) fn stated_host(answer: &JsonValue) -> Result<Option<runx_contracts::TurnHost>, String> {
    answer
        .as_object()
        .and_then(|answer| answer.get("host"))
        .map(|host| {
            host.clone()
                .deserialize_into::<runx_contracts::TurnHost>()
                .map_err(|error| format!("agent answer host names no host: {error}"))
        })
        .transpose()
}

/// The longest model name an answer may state.
const MAX_MODEL: usize = 200;

/// The model an agent turn's answer says answered it, in its `model`: its
/// agent harness or runx's driver states it, and a turn whose answer states
/// none has no model of record.
///
/// # Errors
///
/// Returns an error when the answer states a model that isn't a name.
pub(crate) fn stated_model(answer: &JsonValue) -> Result<Option<&str>, String> {
    let stated = answer.as_object().and_then(|answer| answer.get("model"));
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
            "agent answer model names the model that answered, in 1 to {MAX_MODEL} characters"
        )),
    }
}

/// How an agent says its turn ended. A kill or a timeout is observed by the
/// runtime that ran the act, never claimed by the agent's answer.
fn agent_outcome(ending: &str) -> Result<RunOutcome, OutcomeClaimError> {
    match ending {
        "completed" => Ok(RunOutcome::Completed),
        "deferred" => Ok(RunOutcome::Deferred),
        "superseded" => Ok(RunOutcome::Superseded),
        "declined" => Ok(RunOutcome::Declined),
        "blocked" => Ok(RunOutcome::Blocked),
        "failed" => Ok(RunOutcome::Failed),
        "killed" | "timed_out" => Err(OutcomeClaimError::RuntimeObserved(ending.to_owned())),
        other => Err(OutcomeClaimError::UnsupportedEnding(other.to_owned())),
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
            let stated = serde_json::json!({"draft": "Hello.", "model": named});
            assert!(super::stated_model(&answer(stated)?).is_err());
        }
        Ok(())
    }

    /// An agent says how its turn ended, and why, at its answer's top level,
    /// beside the outputs its step declares; an answer holding a closure is
    /// refused, since closure names a package's closure.
    #[test]
    fn an_answer_states_its_ending_and_why_at_its_top_level() -> Result<(), serde_json::Error> {
        let answer = |text: &str| serde_json::from_str::<JsonValue>(text);
        let declined = answer(
            r#"{"verdict": "skip", "ending": "declined", "ending_reason": "no source to review"}"#,
        )?;
        assert_eq!(
            agent_answer_outcome_or_completed(&declined),
            Ok(RunOutcome::Declined)
        );
        assert_eq!(
            failed_answer("no subagent".to_owned(), None),
            answer(r#"{"ending": "failed", "ending_reason": "no subagent"}"#)?
        );
        assert_eq!(
            agent_answer_outcome_or_completed(&answer(r#"{"closure": {"outcome": "completed"}}"#)?),
            Err(OutcomeClaimError::Closure)
        );
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
                outcome(&format!(r#"{{"ending": "{claimed}"}}"#))?,
                Err(OutcomeClaimError::RuntimeObserved(claimed.to_owned()))
            );
        }
        assert_eq!(
            outcome(r#"{"ending": "declined"}"#)?,
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
        let misnamed = answer(r#"{"summary": "done", "requirements": ["telepathy"]}"#)?;
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
