use runx_contracts::digest::DigestAlgorithm;
use runx_contracts::{
    DataComparison, DataDigest, DataValidation, DataValidationIssue, DataValueValidation, Digest,
    JsonObject, JsonValue, RunContext,
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
const DEFAULT_VALIDATION_VALUE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_VALIDATION_VALUE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct ObservedAtInput {}
impl CapabilityInput for ObservedAtInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct ObservedAtOutput {
    observed_at: String,
}
impl CapabilityOutput for ObservedAtOutput {}

static OBSERVED_AT: TypedNativeCapability<ObservedAtInput, ObservedAtOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "control.observed_at",
            source: "runx-runtime/control",
            summary: "Return this invocation's already-admitted observation time, without reading a clock.",
            scopes: &[],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::None,
            fields: &[],
        },
        |invocation| {
            Ok(ObservedAtOutput {
                observed_at: invocation.observed_at.to_owned(),
            })
        },
    );

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
    #[serde(skip_serializing_if = "Option::is_none")]
    max_value_bytes: Option<runx_contracts::schema::BoundedU64<1, MAX_VALIDATION_VALUE_BYTES>>,
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
struct MinorUnitsInput {
    decimal: String,
    scale: runx_contracts::schema::BoundedU64<0, 18>,
}

impl CapabilityInput for MinorUnitsInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct MinorUnitsOutput {
    units: u64,
}

impl CapabilityOutput for MinorUnitsOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct RunContextInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    harness: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signal: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<JsonObject>,
}

impl CapabilityInput for RunContextInput {}

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
struct RunContextOutput {
    present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    harness: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signal: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<JsonObject>,
    run_context: RunContext,
}

impl CapabilityOutput for RunContextOutput {}

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

static RUN_CONTEXT: TypedNativeCapability<RunContextInput, RunContextOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "control.capture_run_context",
            source: "runx-runtime/control",
            summary: "Capture supplied harness, signal, and decision packets as explicit context.",
            scopes: &["runx:control:read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Named {
                output: "run_context",
                packet: "runx.control.run_context.v1",
            },
            fields: CONTEXT_FIELDS,
        },
        capture_run_context,
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
                "Up to 128 values; 2 MiB total by default. Error output contains paths, not sample contents.",
            ),
            field(
                "max_value_bytes",
                "Explicit serialized-value budget, from 1 byte to 32 MiB. Omit to retain the 2 MiB default; this does not change schema, value-count or error-output bounds.",
            ),
        ],
    },
    validate_values,
);

static MINOR_UNITS: TypedNativeCapability<MinorUnitsInput, MinorUnitsOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "data.minor_units",
            source: "runx-runtime/data",
            summary: "Convert an exact decimal amount to whole smallest units at the caller's bound scale, with no rounding.",
            scopes: &[],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::None,
            fields: &[
                field(
                    "decimal",
                    "Canonical decimal with exactly `scale` fractional digits.",
                ),
                field("scale", "Fractional digits of the currency unit, 0 to 18."),
            ],
        },
        minor_units,
    );

pub(super) const CAPABILITIES: &[&dyn NativeCapability] = &[
    &DIGEST,
    &COMPARE,
    &VALIDATE,
    &MINOR_UNITS,
    &RUN_CONTEXT,
    &OBSERVED_AT,
];

fn minor_units(
    invocation: &NativeInvocation<'_, MinorUnitsInput>,
) -> Result<MinorUnitsOutput, RuntimeError> {
    let MinorUnitsInput { decimal, scale } = invocation.inputs;
    runx_contracts::connector::decimal_units(decimal, scale.get())
        .map(|units| MinorUnitsOutput { units })
        .ok_or_else(|| {
            invalid_input(
                "data.minor_units",
                format!(
                    "the amount needs a canonical decimal with exactly {} fractional digits in the portable integer range",
                    scale.get()
                ),
            )
        })
}

fn validate_values(
    invocation: &NativeInvocation<'_, ValidateInput>,
) -> Result<ValidateOutput, RuntimeError> {
    validate_batch(invocation.inputs).map(|validation| ValidateOutput { validation })
}

fn validate_batch(input: &ValidateInput) -> Result<DataValidation, RuntimeError> {
    if input.values.len() > 128 {
        return Err(invalid_input(
            "data.validate",
            "validation exceeds the value-count bound",
        ));
    }
    let value_limit = input
        .max_value_bytes
        .map_or(DEFAULT_VALIDATION_VALUE_BYTES, |limit| limit.get());
    let schema_bytes = validation_bytes(&input.schema, 256 * 1024)?;
    let values_bytes = validation_bytes(&input.values, value_limit)?;
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

fn validation_bytes(value: &impl Serialize, limit: u64) -> Result<Vec<u8>, RuntimeError> {
    crate::services::bounded_json_bytes(value, limit)
        .map_err(|_| invalid_input("data.validate", "validation byte budget exceeded"))
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

fn capture_run_context(
    invocation: &NativeInvocation<'_, RunContextInput>,
) -> Result<RunContextOutput, RuntimeError> {
    let harness = invocation.inputs.harness.as_ref();
    let signal = invocation.inputs.signal.as_ref();
    let decision = invocation.inputs.decision.as_ref();
    let present = harness.is_some() || signal.is_some() || decision.is_some();
    Ok(RunContextOutput {
        present,
        harness: harness.cloned(),
        signal: signal.cloned(),
        decision: decision.cloned(),
        run_context: RunContext {
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
    fn observation_time_is_admitted_metadata_without_ambient_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::credentials::CredentialDelivery;
        use crate::effects::RuntimeEffectRegistry;
        use crate::services::LocalArtifactService;
        use runx_contracts::Reach;
        use std::collections::BTreeMap;

        let directory = tempfile::tempdir()?;
        let effects = RuntimeEffectRegistry::default();
        let artifacts = LocalArtifactService::default();
        let delivery = CredentialDelivery::none();
        let result = crate::tool_catalogs::native::invoke(
            crate::tool_catalogs::native::NativeToolInvocation {
                tool: "control.observed_at",
                observed_at: "2026-11-03T06:00:00.123Z",
                inputs: runx_contracts::JsonObject::new(),
                scopes: &[],
                env: &BTreeMap::new(),
                skill_directory: directory.path(),
                credential_delivery: &delivery,
                local_artifacts: &artifacts,
                effects: &effects,
                reach: Reach::Workspace,
                call: None,
                effect_admission: None,
            },
        )
        .ok_or("missing observation capability")?
        .result?;
        assert_eq!(
            result
                .as_object()
                .and_then(|output| output.get("observed_at"))
                .and_then(JsonValue::as_str),
            Some("2026-11-03T06:00:00.123Z")
        );
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
        Ok(())
    }

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

    #[test]
    fn validate_retains_its_default_budget_and_admits_an_explicit_larger_read() {
        let mut document = serde_json::json!({
            "schema": {"type": "object", "properties": {"complete": {"const": true}}},
            "values": [{"complete": true, "contents": "x".repeat(2 * 1024 * 1024)}]
        });
        let default = serde_json::from_value(document.clone()).expect("default input");
        assert!(super::validate_batch(&default).is_err());
        document["max_value_bytes"] = serde_json::json!(4 * 1024 * 1024);
        let explicit = serde_json::from_value(document.clone()).expect("explicit input");
        let result = super::validate_batch(&explicit).expect("bounded larger validation");
        assert!(result.valid);
        assert_eq!(
            result.values_digest,
            runx_contracts::sha256_prefixed(
                &serde_json::to_vec(&explicit.values).expect("original canonical array")
            )
        );
        for limit in [0, super::MAX_VALIDATION_VALUE_BYTES + 1] {
            document["max_value_bytes"] = serde_json::json!(limit);
            assert!(serde_json::from_value::<super::ValidateInput>(document.clone()).is_err());
        }
    }
}
