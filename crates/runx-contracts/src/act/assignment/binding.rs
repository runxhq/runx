use std::fmt::Write as _;

use crate::canonical_json::ecmascript_number;
use crate::{JsonNumber, JsonValue};

pub(super) fn stable_binding_json(value: &JsonValue) -> String {
    let mut json = String::new();
    append_stable_binding_json(value, &mut json);
    json
}

fn append_stable_binding_json(value: &JsonValue, json: &mut String) {
    match value {
        JsonValue::Null => json.push_str("null"),
        JsonValue::Bool(value) => json.push_str(if *value { "true" } else { "false" }),
        JsonValue::Number(value) => append_json_number(value, json),
        JsonValue::String(value) => append_json_string(value, json),
        JsonValue::Array(values) => {
            json.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    json.push(',');
                }
                append_stable_binding_json(value, json);
            }
            json.push(']');
        }
        JsonValue::Object(values) => {
            json.push('{');
            for (index, (key, value)) in values.iter().enumerate() {
                if index > 0 {
                    json.push(',');
                }
                append_json_string(key, json);
                json.push(':');
                append_stable_binding_json(value, json);
            }
            json.push('}');
        }
    }
}

fn append_json_number(value: &JsonNumber, json: &mut String) {
    match value {
        JsonNumber::I64(value) => {
            let _ = write!(json, "{value}");
        }
        JsonNumber::U64(value) => {
            let _ = write!(json, "{value}");
        }
        // `hashStable` follows TypeScript's JSON.stringify behavior here.
        // Public serde serialization stays strict and rejects non-finite JSON
        // numbers, but idempotency hashing must match the TypeScript oracle.
        JsonNumber::F64(value) => {
            json.push_str(ecmascript_number(*value).as_deref().unwrap_or("null"));
        }
    }
}

fn append_json_string(value: &str, json: &mut String) {
    json.push('"');
    for character in value.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\u{08}' => json.push_str("\\b"),
            '\u{0c}' => json.push_str("\\f"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            character if character <= '\u{1f}' => {
                let _ = write!(json, "\\u{:04x}", u32::from(character));
            }
            // Current Node/V8 JSON.stringify emits U+2028 and U+2029 raw.
            character => json.push(character),
        }
    }
    json.push('"');
}

pub(super) fn sha256_prefixed(value: &str) -> String {
    crate::digest::sha256_prefixed(value.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::stable_binding_json;
    use crate::act::assignment::derive_content_digest;
    use crate::{JsonNumber, JsonValue};

    #[test]
    fn stable_hash_json_matches_json_stringify_for_non_finite_numbers() {
        assert_eq!(
            stable_binding_json(&JsonValue::Number(JsonNumber::F64(f64::NAN))),
            "null",
        );
        assert_eq!(
            derive_content_digest(Some(
                [(
                    "value".to_owned(),
                    JsonValue::Number(JsonNumber::F64(f64::INFINITY)),
                )]
                .into_iter()
                .collect(),
            )),
            "sha256:1c197daef20de3f47eec5e2f735ec6669869d3180cc29f35be4788511e0af0f8",
        );
    }

    #[test]
    fn stable_hash_json_matches_json_stringify_for_special_strings() {
        assert_eq!(
            stable_binding_json(&JsonValue::String("line\u{2028}sep\u{2029}end".to_owned())),
            "\"line\u{2028}sep\u{2029}end\"",
        );
    }

    #[test]
    fn stable_hash_json_matches_json_stringify_for_negative_zero() {
        assert_eq!(
            stable_binding_json(&JsonValue::Number(JsonNumber::F64(-0.0))),
            "0",
        );
    }
}
