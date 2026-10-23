use runx_contracts::digest::DigestAlgorithm;
use runx_contracts::{
    DataComparison, DataDigest, DataValidation, DataValidationIssue, DataValueValidation, Digest,
    HarnessContext, JsonObject, JsonValue,
};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

use super::capability::{NativeCapability, TypedNativeCapability};
use super::{NativeInvocation, invalid_input};
use crate::RuntimeError;

const DIGEST_TOOL: &str = "data.digest";

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct DigestInput {
    value: JsonValue,
    encoding: String,
}

impl CapabilityInput for DigestInput {
    fn defaults() -> JsonObject {
        JsonObject::from([(
            "encoding".to_owned(),
            JsonValue::String("canonical_json".to_owned()),
        )])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct CompareInput {
    actual: JsonValue,
    expected: JsonValue,
}

impl CapabilityInput for CompareInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct ValidateInput {
    schema: JsonValue,
    values: Vec<JsonValue>,
}

impl CapabilityInput for ValidateInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct ValidateOutput {
    validation: DataValidation,
}

impl CapabilityOutput for ValidateOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct HarnessContextInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    harness: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signal: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<JsonObject>,
}

impl CapabilityInput for HarnessContextInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct DigestOutput {
    digest_result: DataDigest,
}

impl CapabilityOutput for DigestOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct CompareOutput {
    comparison: DataComparison,
}

impl CapabilityOutput for CompareOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct HarnessContextOutput {
    present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    harness: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signal: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<JsonObject>,
    harness_context: HarnessContext,
}

impl CapabilityOutput for HarnessContextOutput {}

const DIGEST_FIELDS: &[CapabilityField] = &[
    field("value", "Exact JSON value or UTF-8 string to hash."),
    field("encoding", "Digest encoding: canonical_json or utf8_text."),
];

const COMPARE_FIELDS: &[CapabilityField] = &[
    field("actual", "Resolved value observed at execution time."),
    field("expected", "Resolved value the operation is bound to."),
];

const CONTEXT_FIELDS: &[CapabilityField] = &[
    field("harness", "Optional current runx.harness.v1 packet."),
    field(
        "signal",
        "Optional runx.signal.v1 packet that informed the run.",
    ),
    field(
        "decision",
        "Optional runx.decision.v1 packet selecting the next action.",
    ),
];

const fn field(name: &'static str, description: &'static str) -> CapabilityField {
    CapabilityField { name, description }
}

static DIGEST: TypedNativeCapability<DigestInput, DigestOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "data.digest",
        source: "runx-runtime/data",
        summary: "Compute a stable SHA-256 digest over canonical JSON or exact UTF-8 text.",
        scopes: &[],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "digest_result",
            packet: "runx.data.digest.v1",
        },
        fields: DIGEST_FIELDS,
    },
    digest_value,
);

static COMPARE: TypedNativeCapability<CompareInput, CompareOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "data.compare",
        source: "runx-runtime/data",
        summary: "Compare two resolved JSON values without copying them into output.",
        scopes: &[],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "comparison",
            packet: "runx.data.comparison.v1",
        },
        fields: COMPARE_FIELDS,
    },
    compare_values,
);

static HARNESS_CONTEXT: TypedNativeCapability<HarnessContextInput, HarnessContextOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "control.capture_harness_context",
            source: "runx-runtime/control",
            summary: "Capture supplied harness, signal, and decision packets as explicit context.",
            scopes: &["runx:control:read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Named {
                output: "harness_context",
                packet: "runx.control.harness_context.v1",
            },
            fields: CONTEXT_FIELDS,
        },
        capture_harness_context,
    );

static VALIDATE: TypedNativeCapability<ValidateInput, ValidateOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "data.validate",
        source: "runx-runtime/data",
        summary: "Validate up to 128 JSON values against a self-contained JSON Schema 2020-12; formats are annotations, external retrieval is disabled.",
        scopes: &[],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "validation",
            packet: "runx.data.validation.v1",
        },
        fields: &[
            field(
                "schema",
                "Self-contained JSON Schema 2020-12, at most 256 KiB.",
            ),
            field(
                "values",
                "Up to 128 values, at most 2 MiB total; error output contains paths, not sample contents.",
            ),
        ],
    },
    validate_values,
);

pub(super) const CAPABILITIES: &[&dyn NativeCapability] =
    &[&DIGEST, &COMPARE, &VALIDATE, &HARNESS_CONTEXT];

fn validate_values(
    invocation: &NativeInvocation<'_, ValidateInput>,
) -> Result<ValidateOutput, RuntimeError> {
    validate_batch(invocation.inputs).map(|validation| ValidateOutput { validation })
}

fn validate_batch(input: &ValidateInput) -> Result<DataValidation, RuntimeError> {
    let schema_bytes = digest_bytes(&input.schema, "canonical_json")?;
    let values = JsonValue::Array(input.values.clone());
    let values_bytes = digest_bytes(&values, "canonical_json")?;
    if input.values.len() > 128
        || schema_bytes.len() > 256 * 1024
        || values_bytes.len() > 2 * 1024 * 1024
    {
        return Err(invalid_input(
            "data.validate",
            "validation exceeds the schema, value-count or payload byte bound",
        ));
    }
    let schema = serde_json::to_value(&input.schema)
        .map_err(|source| RuntimeError::json("serializing validation schema", source))?;
    let validator = jsonschema::draft202012::options()
        .offline()
        .should_validate_formats(false)
        .build(&schema)
        .map_err(|error| {
            invalid_input(
                "data.validate",
                format!(
                    "invalid or unresolved JSON Schema at {}",
                    error.schema_path()
                ),
            )
        })?;
    let mut results = Vec::with_capacity(input.values.len());
    for (index, value) in input.values.iter().enumerate() {
        let value = serde_json::to_value(value)
            .map_err(|source| RuntimeError::json("serializing validation value", source))?;
        let mut errors = validator.iter_errors(&value);
        let issues: Vec<_> = errors
            .by_ref()
            .take(32)
            .map(|error| DataValidationIssue {
                instance_path: error.instance_path().to_string(),
                schema_path: error.schema_path().to_string(),
            })
            .collect();
        let truncated = errors.next().is_some();
        results.push(DataValueValidation {
            index,
            valid: issues.is_empty() && !truncated,
            errors: issues,
            truncated,
        });
    }
    Ok(DataValidation {
        valid: results.iter().all(|result| result.valid),
        schema_digest: runx_contracts::sha256_prefixed(&schema_bytes),
        values_digest: runx_contracts::sha256_prefixed(&values_bytes),
        results,
    })
}

fn digest_value(
    invocation: &NativeInvocation<'_, DigestInput>,
) -> Result<DigestOutput, RuntimeError> {
    let bytes = digest_bytes(&invocation.inputs.value, &invocation.inputs.encoding)?;
    Ok(DigestOutput {
        digest_result: DataDigest {
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(&bytes),
        },
    })
}

fn compare_values(
    invocation: &NativeInvocation<'_, CompareInput>,
) -> Result<CompareOutput, RuntimeError> {
    let actual = &invocation.inputs.actual;
    let expected = &invocation.inputs.expected;
    let actual_bytes = digest_bytes(actual, "canonical_json")?;
    let expected_bytes = digest_bytes(expected, "canonical_json")?;
    Ok(CompareOutput {
        comparison: DataComparison {
            equal: actual == expected,
            actual_digest: Digest::of(&actual_bytes),
            expected_digest: Digest::of(&expected_bytes),
        },
    })
}

fn capture_harness_context(
    invocation: &NativeInvocation<'_, HarnessContextInput>,
) -> Result<HarnessContextOutput, RuntimeError> {
    let harness = invocation.inputs.harness.as_ref();
    let signal = invocation.inputs.signal.as_ref();
    let decision = invocation.inputs.decision.as_ref();
    let present = harness.is_some() || signal.is_some() || decision.is_some();
    Ok(HarnessContextOutput {
        present,
        harness: harness.cloned(),
        signal: signal.cloned(),
        decision: decision.cloned(),
        harness_context: HarnessContext {
            captured: present,
            harness: harness.cloned(),
            signal: signal.cloned(),
            decision: decision.cloned(),
        },
    })
}

fn digest_bytes(value: &JsonValue, encoding: &str) -> Result<Vec<u8>, RuntimeError> {
    match encoding {
        "canonical_json" => serde_json::to_vec(value)
            .map_err(|source| RuntimeError::json("serializing native digest input", source)),
        "utf8_text" => value
            .as_str()
            .map(|value| value.as_bytes().to_vec())
            .ok_or_else(|| {
                invalid_input(DIGEST_TOOL, "utf8_text encoding requires a string value")
            }),
        other => Err(invalid_input(
            DIGEST_TOOL,
            format!("encoding {other:?} is unsupported"),
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use runx_contracts::JsonValue;

    use super::digest_bytes;

    #[test]
    fn digest_preserves_json_compatibility_and_exact_text() {
        let text = JsonValue::String("hello".to_owned());
        assert_eq!(
            digest_bytes(&text, "canonical_json").expect("canonical JSON bytes"),
            b"\"hello\""
        );
        assert_eq!(
            digest_bytes(&text, "utf8_text").expect("UTF-8 bytes"),
            b"hello"
        );
    }

    #[test]
    fn validate_enforces_nested_constraints_and_number_semantics() {
        let input = serde_json::from_value(serde_json::json!({
            "schema": {"type":"object", "required":["items"], "properties":{
                "items":{"type":"array", "minItems":1, "items":{"$ref":"#/$defs/item"}}
            }, "$defs":{"item":{"type":"object", "required":["name","amount"], "properties":{
                "name":{"type":"string","minLength":10}, "amount":{"type":"number","minimum":1}
            }, "additionalProperties":false}}},
            "values":[{"items":[{"name":"long enough", "amount":2}]}, {"items":[{"name":"x","amount":0}]}, {"items":[]}]
        })).expect("input");
        let result = super::validate_batch(&input).expect("validation");
        assert!(!result.valid);
        assert!(result.results[0].valid);
        assert_eq!(result.results[1].errors.len(), 2);
        assert!(!result.results[2].valid);
    }

    #[test]
    fn validate_refuses_remote_and_malformed_schemas() {
        for schema in [
            serde_json::json!({"$ref":"https://example.invalid/schema"}),
            serde_json::json!({"type":42}),
        ] {
            let input = serde_json::from_value(serde_json::json!({"schema":schema, "values":[{}]}))
                .expect("input");
            assert!(super::validate_batch(&input).is_err());
        }
    }

    #[test]
    fn validate_bounds_errors_without_exposing_values() {
        let input = serde_json::from_value(serde_json::json!({"schema":{"type":"array","items":{"const":false}}, "values":[vec!["private sample"; 40]]})).expect("input");
        let result = super::validate_batch(&input).expect("validation");
        assert_eq!(result.results[0].errors.len(), 32);
        assert!(result.results[0].truncated);
        assert!(
            !serde_json::to_string(&result)
                .expect("output")
                .contains("private sample")
        );
    }
}
