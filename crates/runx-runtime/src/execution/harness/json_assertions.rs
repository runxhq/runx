use runx_contracts::{JsonObject, JsonValue};

use super::fixtures::TestJsonExpectation;
use super::runner::TestReplayError;

pub(crate) fn assert_json_expectation(
    expectation: &TestJsonExpectation,
    actual: &JsonValue,
    field: &str,
) -> Result<(), TestReplayError> {
    if let Some(expected) = &expectation.exact
        && expected != actual
    {
        return Err(json_mismatch(format!("{field}.exact"), expected, actual));
    }
    if let Some(expected) = &expectation.subset {
        assert_json_subset(expected, actual, &format!("{field}.subset"))?;
    }
    Ok(())
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
                let actual_value = actual_object.get(key).unwrap_or(&JsonValue::Null);
                assert_json_subset(expected_value, actual_value, &path)?;
            }
            Ok(())
        }
        JsonValue::Array(expected_values) => {
            let JsonValue::Array(actual_values) = actual else {
                return Err(json_mismatch(field.to_owned(), expected, actual));
            };
            if expected_values.len() > actual_values.len() {
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
    fn subset_arrays_match_ordered_object_prefixes() -> Result<(), Box<dyn std::error::Error>> {
        let expected = serde_json::from_value(serde_json::json!([{"id": "first"}]))?;
        let actual = serde_json::from_value(serde_json::json!([
            {"id": "first", "status": "closed"},
            {"id": "second", "status": "closed"}
        ]))?;

        assert!(assert_json_subset(&expected, &actual, "expect.output.subset").is_ok());
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
