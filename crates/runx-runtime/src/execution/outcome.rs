use runx_contracts::{JsonValue, RunOutcome};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum OutcomeClaimError {
    #[error("agent answer closure must be an object")]
    ClosureNotObject,
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
        .ok_or(OutcomeClaimError::ClosureNotObject)?;
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

    use super::{OutcomeClaimError, agent_answer_outcome_or_completed};

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
}
