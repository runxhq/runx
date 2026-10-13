use runx_contracts::{JsonObject, JsonValue};

use crate::execution::harness::runner::{HarnessReplayError, HarnessReplayOutput};

pub(super) fn assert_caller_answers_replayed(
    output: &HarnessReplayOutput,
) -> Result<(), HarnessReplayError> {
    assert_replayed_answers(&output.fixture.caller.answers, &output.replayed_answers)
}

fn assert_replayed_answers(
    answers: &JsonObject,
    replayed_answers: &JsonObject,
) -> Result<(), HarnessReplayError> {
    for (request_id, answer) in answers {
        let expected = json_text(answer);
        let Some(actual) = replayed_answers.get(request_id) else {
            return Err(HarnessReplayError::Mismatch {
                field: format!("caller.answers.{request_id}.replayed"),
                expected,
                actual: "<answer was not consumed>".to_owned(),
            });
        };
        if json_text(answer) == json_text(actual) {
            continue;
        }
        return Err(HarnessReplayError::Mismatch {
            field: format!("caller.answers.{request_id}.replayed"),
            expected,
            actual: json_text(actual),
        });
    }
    Ok(())
}

fn json_text(value: &JsonValue) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|error| format!("<unserializable JSON value: {error}>"))
}

#[cfg(test)]
mod tests {
    use runx_contracts::{JsonNumber, JsonObject, JsonValue};

    use super::{HarnessReplayError, assert_replayed_answers};

    fn send_plan(decision: &str) -> JsonValue {
        JsonValue::Object(JsonObject::from([(
            "send_plan".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "decision".to_owned(),
                JsonValue::String(decision.to_owned()),
            )])),
        )]))
    }

    #[test]
    fn supplied_caller_answers_are_exact_request_bound_replay_oracles() {
        let answer = send_plan("ready");
        let answers = JsonObject::from([("plan".to_owned(), answer.clone())]);
        let replayed = JsonObject::from([("plan".to_owned(), answer)]);

        assert!(assert_replayed_answers(&answers, &replayed).is_ok());
    }

    #[test]
    fn unused_or_changed_caller_answer_fails_with_request_path() {
        let answers = JsonObject::from([("plan".to_owned(), send_plan("ready"))]);
        let replayed = JsonObject::from([("plan".to_owned(), send_plan("needs_input"))]);

        let result = assert_replayed_answers(&answers, &replayed);

        assert!(matches!(
            result,
            Err(HarnessReplayError::Mismatch { field, .. })
                if field == "caller.answers.plan.replayed"
        ));
    }

    #[test]
    fn fixtures_without_answers_have_no_replay_obligation() {
        assert!(assert_replayed_answers(&JsonObject::new(), &JsonObject::new()).is_ok());
    }

    #[test]
    fn integer_representations_compare_as_json_values() {
        let answers = JsonObject::from([(
            "invoice".to_owned(),
            JsonValue::Number(JsonNumber::I64(1840)),
        )]);
        let replayed = JsonObject::from([(
            "invoice".to_owned(),
            JsonValue::Number(JsonNumber::U64(1840)),
        )]);

        assert!(assert_replayed_answers(&answers, &replayed).is_ok());
    }

    #[test]
    fn answer_replayed_under_another_request_id_does_not_satisfy_the_oracle() {
        let answer = send_plan("ready");
        let answers = JsonObject::from([("plan".to_owned(), answer.clone())]);
        let replayed = JsonObject::from([("other".to_owned(), answer)]);

        let result = assert_replayed_answers(&answers, &replayed);

        assert!(matches!(
            result,
            Err(HarnessReplayError::Mismatch { field, actual, .. })
                if field == "caller.answers.plan.replayed"
                    && actual == "<answer was not consumed>"
        ));
    }
}
