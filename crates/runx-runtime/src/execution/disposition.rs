use runx_contracts::{ClosureDisposition, JsonValue};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum ClosureDispositionParseError {
    #[error("agent answer closure must be an object")]
    ClosureNotObject,
    #[error("agent answer closure.disposition is required")]
    MissingDisposition,
    #[error("agent answer closure.disposition must be a string")]
    DispositionNotString,
    #[error("agent answer closure.disposition {0:?} is not supported")]
    UnsupportedDisposition(String),
    #[error(
        "agent answer closure.disposition {0:?} is the runtime's to observe; an agent closes its act closed, deferred, superseded, declined, blocked or failed"
    )]
    RuntimeObserved(String),
}

pub(crate) fn parse_agent_answer_disposition(
    answer: &JsonValue,
) -> Result<ClosureDisposition, ClosureDispositionParseError> {
    let closure = answer
        .as_object()
        .and_then(|object| object.get("closure"))
        .ok_or(ClosureDispositionParseError::MissingDisposition)?;
    let closure = closure
        .as_object()
        .ok_or(ClosureDispositionParseError::ClosureNotObject)?;
    let disposition = closure
        .get("disposition")
        .ok_or(ClosureDispositionParseError::MissingDisposition)?;
    let disposition = disposition
        .as_str()
        .ok_or(ClosureDispositionParseError::DispositionNotString)?;
    agent_disposition(disposition)
}

pub(crate) fn agent_answer_disposition_or_closed(
    answer: &JsonValue,
) -> Result<ClosureDisposition, ClosureDispositionParseError> {
    match answer.as_object() {
        Some(object) if object.contains_key("closure") => parse_agent_answer_disposition(answer),
        _ => Ok(ClosureDisposition::Closed),
    }
}

/// How an agent says its act closed. A kill or a timeout is observed by the
/// runtime that ran the act, never claimed by the agent's answer.
fn agent_disposition(disposition: &str) -> Result<ClosureDisposition, ClosureDispositionParseError> {
    match disposition {
        "closed" => Ok(ClosureDisposition::Closed),
        "deferred" => Ok(ClosureDisposition::Deferred),
        "superseded" => Ok(ClosureDisposition::Superseded),
        "declined" => Ok(ClosureDisposition::Declined),
        "blocked" => Ok(ClosureDisposition::Blocked),
        "failed" => Ok(ClosureDisposition::Failed),
        "killed" | "timed_out" => Err(ClosureDispositionParseError::RuntimeObserved(
            disposition.to_owned(),
        )),
        other => Err(ClosureDispositionParseError::UnsupportedDisposition(
            other.to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use runx_contracts::{ClosureDisposition, JsonValue};

    use super::{ClosureDispositionParseError, agent_answer_disposition_or_closed};

    #[test]
    fn an_agent_cannot_claim_a_kill_or_a_timeout() -> Result<(), serde_json::Error> {
        let closed = |answer: &str| {
            serde_json::from_str::<JsonValue>(answer)
                .map(|answer| agent_answer_disposition_or_closed(&answer))
        };
        for claimed in ["killed", "timed_out"] {
            assert_eq!(
                closed(&format!(r#"{{"closure": {{"disposition": "{claimed}"}}}}"#))?,
                Err(ClosureDispositionParseError::RuntimeObserved(
                    claimed.to_owned()
                ))
            );
        }
        assert_eq!(
            closed(r#"{"closure": {"disposition": "declined"}}"#)?,
            Ok(ClosureDisposition::Declined)
        );
        assert_eq!(
            closed(r#"{"answer": "done"}"#)?,
            Ok(ClosureDisposition::Closed)
        );
        Ok(())
    }
}
