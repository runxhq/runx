use runx_contracts::{JsonObject, JsonValue};

use super::fixtures::TestJsonExpectation;
use super::replay::TestReplayError;

pub(crate) fn assert_json_expectation(
    expectation: &TestJsonExpectation,
    actual: &JsonValue,
    field: &str,
) -> Result<(), TestReplayError> {
    match expectation {
        TestJsonExpectation::Exact { exact } if exact != actual => {
            Err(json_mismatch(format!("{field}.exact"), exact, actual))
        }
        TestJsonExpectation::Exact { .. } => Ok(()),
        TestJsonExpectation::Subset { subset } => {
            assert_json_subset(subset, actual, &format!("{field}.subset"))
        }
    }
}

fn assert_json_subset(
    expected: &JsonValue,
    actual: &JsonValue,
    field: &str,
) -> Result<(), TestReplayError> {
    match expected {
        JsonValue::Object(expected_object) => {
            let Some(actual_object) = object_value(actual) else {
                return Err(json_mismatch(field.to_owned(), expected, actual));
            };
            for (key, expected_value) in expected_object {
                let path = format!("{field}.{key}");
                let Some(actual_value) = actual_object.get(key) else {
                    return Err(TestReplayError::Mismatch {
                        field: path,
                        expected: json_value_text(expected_value),
                        actual: "<missing>".to_owned(),
                    });
                };
                assert_json_subset(expected_value, actual_value, &path)?;
            }
            Ok(())
        }
        JsonValue::Array(expected_values) => {
            let JsonValue::Array(actual_values) = actual else {
                return Err(json_mismatch(field.to_owned(), expected, actual));
            };
            if expected_values.len() != actual_values.len() {
                return Err(json_mismatch(field.to_owned(), expected, actual));
            }
            for (index, expected_value) in expected_values.iter().enumerate() {
                assert_json_subset(
                    expected_value,
                    &actual_values[index],
                    &format!("{field}[{index}]"),
                )?;
            }
            Ok(())
        }
        _ if expected == actual => Ok(()),
        _ => Err(json_mismatch(field.to_owned(), expected, actual)),
    }
}

fn object_value(value: &JsonValue) -> Option<&JsonObject> {
    match value {
        JsonValue::Object(object) => Some(object),
        _ => None,
    }
}

fn json_mismatch(field: String, expected: &JsonValue, actual: &JsonValue) -> TestReplayError {
    TestReplayError::Mismatch {
        field,
        expected: json_value_text(expected),
        actual: json_value_text(actual),
    }
}

fn json_value_text(value: &JsonValue) -> String {
    match serde_json::to_string(value) {
        Ok(value) => value,
        Err(error) => format!("<unserializable JSON value: {error}>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_null_is_an_assertion_not_an_absent_mode() {
        let expected = TestJsonExpectation::Exact {
            exact: JsonValue::Null,
        };
        assert!(assert_json_expectation(&expected, &JsonValue::Null, "expect.output").is_ok());
        assert!(
            assert_json_expectation(&expected, &JsonValue::Bool(false), "expect.output").is_err()
        );
    }

    #[test]
    fn subset_arrays_match_each_object_without_ignoring_extra_items()
    -> Result<(), Box<dyn std::error::Error>> {
        let expected = serde_json::from_value(serde_json::json!([
            {"id": "first"},
            {"id": "second"}
        ]))?;
        let actual = serde_json::from_value(serde_json::json!([
            {"id": "first", "status": "closed"},
            {"id": "second", "status": "closed"}
        ]))?;

        assert!(assert_json_subset(&expected, &actual, "expect.output.subset").is_ok());
        let prefix = serde_json::from_value(serde_json::json!([{"id": "first"}]))?;
        assert!(assert_json_subset(&prefix, &actual, "expect.output.subset").is_err());
        Ok(())
    }

    #[test]
    fn an_empty_array_asserts_there_are_no_items() -> Result<(), Box<dyn std::error::Error>> {
        let expected = serde_json::from_value(serde_json::json!([]))?;
        let empty = serde_json::from_value(serde_json::json!([]))?;
        let nonempty = serde_json::from_value(serde_json::json!([{"effect": "sent"}]))?;

        assert!(assert_json_subset(&expected, &empty, "expect.output.subset").is_ok());
        assert!(assert_json_subset(&expected, &nonempty, "expect.output.subset").is_err());
        Ok(())
    }

    #[test]
    fn an_explicit_null_requires_the_field_to_exist() -> Result<(), Box<dyn std::error::Error>> {
        let expected = serde_json::from_value(serde_json::json!({"proposal": null}))?;
        let present = serde_json::from_value(serde_json::json!({"proposal": null}))?;
        let absent = serde_json::from_value(serde_json::json!({}))?;

        assert!(assert_json_subset(&expected, &present, "expect.output.subset").is_ok());
        assert!(matches!(
            assert_json_subset(&expected, &absent, "expect.output.subset"),
            Err(TestReplayError::Mismatch { actual, .. }) if actual == "<missing>"
        ));
        Ok(())
    }

    #[test]
    fn subset_arrays_reject_missing_expected_positions() -> Result<(), Box<dyn std::error::Error>> {
        let expected = serde_json::from_value(serde_json::json!([
            {"id": "first"},
            {"id": "second"}
        ]))?;
        let actual = serde_json::from_value(serde_json::json!([{"id": "first"}]))?;

        assert!(matches!(
            assert_json_subset(&expected, &actual, "expect.output.subset"),
            Err(TestReplayError::Mismatch { .. })
        ));
        Ok(())
    }
}
