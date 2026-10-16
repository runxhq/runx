use runx_contracts::{JsonObject, JsonValue};

pub(super) fn append_text(output: &mut String, inspection: &JsonObject) {
    let Some(environment) = inspection.get("environment").and_then(JsonValue::as_object) else {
        return;
    };
    let Some(variables) = environment.get("variables").and_then(JsonValue::as_array) else {
        return;
    };
    for variable in variables {
        let Some(variable) = variable.as_object() else {
            continue;
        };
        let Some(name) = variable.get("name").and_then(JsonValue::as_str) else {
            continue;
        };
        let required = variable
            .get("required")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let available = variable
            .get("available")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        output.push_str(&format!(
            "environment: {name} ({}, {})\n",
            if required { "required" } else { "optional" },
            if available { "available" } else { "missing" }
        ));
    }
}
