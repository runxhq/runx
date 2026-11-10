//! What a placement returns, and the packet a step sees.
//!
//! Every placement returns one [`Readback`]: the provider's result and an
//! opaque reference to the independent evidence behind it. It is checked
//! against the admitted intent, then projected into the
//! `runx.provider.operation.v1` packet the step emits.

use runx_contracts::{JsonObject, JsonValue, ProviderOperationPacket};
use serde::{Deserialize, Serialize};

use super::call::ResultShape;
use super::confirm::Unconfirmed;
use crate::error::{Contradiction, Mismatch};

/// A placement's evidence for one call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Readback {
    pub provider: String,
    pub operation: String,
    pub target: String,
    pub result: JsonValue,
    pub readback_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// The admitted call a readback must match.
pub(crate) struct Expected<'a> {
    pub connector: &'a str,
    pub operation: &'a str,
    pub target: &'a str,
    /// A mutation requires an acknowledgement; a keyed read does not.
    pub mutation: bool,
    /// The provider key, for mutations or deduplicated metered reads.
    pub idempotency_key: Option<&'a str>,
}

/// The facts the packet carries beside the readback.
pub(crate) struct Seal<'a> {
    pub access: &'static str,
    /// `confirmed`, or `acknowledged` for a write the provider cannot read
    /// back.
    pub finality: &'static str,
    pub transport: &'static str,
    pub principal_uri: &'a str,
    /// The runx Connect grant the call ran under; a local instance has none.
    pub grant_id: Option<&'a str>,
    pub account_id: Option<&'a str>,
    pub intent_digest: &'a str,
    pub operation_digest: &'a str,
    pub implementation_digest: Option<&'a str>,
    /// What a priced call cost.
    pub cost: Option<runx_contracts::connector::CallCost>,
}

impl Readback {
    /// Check the readback against the admitted call. A provider, operation,
    /// target or key it states other than the call's contradicts the call;
    /// one it leaves out doesn't answer it.
    pub(crate) fn verify(&self, expected: &Expected<'_>) -> Result<(), Unconfirmed> {
        let key = expected.idempotency_key;
        let mismatches = [
            (
                "provider",
                Some(expected.connector),
                Some(self.provider.as_str()),
            ),
            (
                "operation",
                Some(expected.operation),
                Some(self.operation.as_str()),
            ),
            ("target", Some(expected.target), Some(self.target.as_str())),
            ("idempotency_key", key, self.idempotency_key.as_deref()),
        ]
        .into_iter()
        // The packet's own fields, never a result's, so none is withheld.
        .filter_map(|(field, expected, actual)| match (expected, actual) {
            (Some(expected), Some(actual)) if expected != actual => Some(Mismatch::new(
                field,
                JsonValue::String(expected.to_owned()),
                Some(JsonValue::String(actual.to_owned())),
                &[],
            )),
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?;
        if !mismatches.is_empty() {
            return Err(Unconfirmed::Contradicted(Contradiction { mismatches }));
        }
        if self.readback_uri.trim().is_empty() {
            return Err("the readback carries no evidence reference".into());
        }
        if key.is_some() {
            if expected.mutation && self.operation_id.as_deref().is_none_or(str::is_empty) {
                return Err("the mutation was not acknowledged with an operation id".into());
            }
            if self.idempotency_key.is_none() {
                return Err(
                    "the acknowledgement does not carry the admitted idempotency key".into(),
                );
            }
        }
        Ok(())
    }

    /// Check the caller's expectations and project the packet.
    pub(crate) fn into_packet(
        self,
        shape: &ResultShape,
        expected_result: Option<&JsonObject>,
        seal: &Seal<'_>,
    ) -> Result<ProviderOperationPacket, String> {
        let result_digest = runx_contracts::connector::stable_digest(&self.result)
            .map_err(|error| error.to_string())?;
        let projected = shape.required.is_some() || shape.optional.is_some();
        let result = if projected || expected_result.is_some() {
            let JsonValue::Object(mut result) = self.result else {
                return Err(
                    "the provider result must be an object to verify or project it".to_owned(),
                );
            };
            for (field, expected) in expected_result.into_iter().flatten() {
                let actual = result.get(field);
                if actual != Some(expected) {
                    return Err(unequal(field, expected, actual, &shape.ephemeral)?);
                }
            }
            if projected {
                let mut kept = JsonObject::new();
                for field in shape.required.iter().flatten() {
                    let value = result
                        .remove(field)
                        .ok_or_else(|| format!("the result is missing required field {field:?}"))?;
                    kept.insert(field.clone(), value);
                }
                for field in shape.optional.iter().flatten() {
                    if let Some(value) = result.remove(field) {
                        kept.insert(field.clone(), value);
                    }
                }
                result = kept;
            }
            JsonValue::Object(result)
        } else {
            self.result
        };
        Ok(ProviderOperationPacket {
            schema: "runx.provider.operation.v1".to_owned(),
            status: "success".to_owned(),
            provider: self.provider,
            operation: self.operation,
            target: self.target,
            result,
            transport: seal.transport.to_owned(),
            readback_uri: self.readback_uri,
            access: Some(seal.access.to_owned()),
            principal_uri: Some(seal.principal_uri.to_owned()),
            grant_uri: seal
                .grant_id
                .map(|grant_id| format!("runx:grant:{grant_id}")),
            finality: Some(seal.finality.to_owned()),
            plan_digest: Some(seal.intent_digest.to_owned()),
            result_digest: Some(result_digest.as_str().to_owned()),
            operation_id: self.operation_id,
            idempotency_key: self.idempotency_key,
            host: None,
            account_id: seal.account_id.map(str::to_owned),
            operation_digest: Some(seal.operation_digest.to_owned()),
            implementation_digest: seal.implementation_digest.map(str::to_owned),
            cost: seal.cost.clone(),
        })
    }
}

/// Why a result field fails the value the call requires of it, naming what
/// the result has there, a value from a path the call marks never to
/// persist by its digest.
fn unequal(
    field: &str,
    expected: &JsonValue,
    actual: Option<&JsonValue>,
    withheld: &[Vec<String>],
) -> Result<String, String> {
    let actual = actual.cloned().unwrap_or(JsonValue::Null);
    let mismatch = Mismatch::new(field, expected.clone(), Some(actual.clone()), withheld)?;
    Ok(match actual {
        JsonValue::Null => format!(
            "the result states no value at {field}, where {} was expected",
            mismatch.expected
        ),
        _ => format!(
            "the result contradicts the call: {}",
            Contradiction {
                mismatches: vec![mismatch],
            }
        ),
    })
}

/// Move the caller-only result paths out of a packet's result. Their values
/// reach the immediate caller and never workflow state or receipts; the result
/// digest still covers them.
pub(crate) fn split_ephemeral(
    packet: &mut ProviderOperationPacket,
    paths: &[Vec<String>],
) -> Result<Option<JsonObject>, String> {
    if paths.is_empty() {
        return Ok(None);
    }
    let JsonValue::Object(result) = &mut packet.result else {
        return Err("the provider result must be an object to keep paths caller-only".to_owned());
    };
    let mut ephemeral = JsonObject::new();
    for path in paths {
        if let Some(value) = remove_path(result, path) {
            insert_path(&mut ephemeral, path, value)?;
        }
    }
    Ok((!ephemeral.is_empty()).then_some(ephemeral))
}

fn remove_path(object: &mut JsonObject, path: &[String]) -> Option<JsonValue> {
    let (field, tail) = path.split_first()?;
    if tail.is_empty() {
        return object.remove(field);
    }
    match object.get_mut(field) {
        Some(JsonValue::Object(child)) => remove_path(child, tail),
        _ => None,
    }
}

fn insert_path(object: &mut JsonObject, path: &[String], value: JsonValue) -> Result<(), String> {
    let Some((field, tail)) = path.split_first() else {
        return Err("an ephemeral path is empty".to_owned());
    };
    if tail.is_empty() {
        object.insert(field.clone(), value);
        return Ok(());
    }
    match object
        .entry(field.clone())
        .or_insert_with(|| JsonValue::Object(JsonObject::new()))
    {
        JsonValue::Object(child) => insert_path(child, tail, value),
        _ => Err("an ephemeral path crosses a non-object value".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readback() -> Readback {
        Readback {
            provider: "slack".to_owned(),
            operation: "thread.reply".to_owned(),
            target: "slack://T/C/2".to_owned(),
            result: JsonValue::Null,
            readback_uri: "runx:provider-readback:1".to_owned(),
            operation_id: Some("op-1".to_owned()),
            idempotency_key: Some("runx:expected".to_owned()),
        }
    }

    fn expected(key: Option<&'static str>) -> Expected<'static> {
        Expected {
            connector: "slack",
            operation: "thread.reply",
            target: "slack://T/C/2",
            mutation: true,
            idempotency_key: key,
        }
    }

    #[test]
    fn a_readback_must_answer_the_admitted_call() {
        assert!(readback().verify(&expected(Some("runx:expected"))).is_ok());
        let wrong_operation = Readback {
            operation: "thread.read".to_owned(),
            ..readback()
        };
        assert!(wrong_operation.verify(&expected(None)).is_err());
        let caller_key = Readback {
            idempotency_key: Some("caller-controlled".to_owned()),
            ..readback()
        };
        assert!(caller_key.verify(&expected(Some("runx:expected"))).is_err());
        let unacknowledged = Readback {
            operation_id: None,
            ..readback()
        };
        assert!(
            unacknowledged
                .verify(&expected(Some("runx:expected")))
                .is_err()
        );
    }

    #[test]
    fn a_deduplicated_read_binds_its_key_without_inventing_a_mutation_acknowledgement() {
        let mut read = readback();
        read.operation_id = None;
        let mut selected = expected(Some("runx:expected"));
        selected.mutation = false;
        assert!(read.verify(&selected).is_ok());
        read.idempotency_key = Some("another-call".to_owned());
        assert!(read.verify(&selected).is_err());
    }

    #[test]
    fn an_unequal_result_field_names_its_actual_value_by_the_rule_for_values() {
        let json = |value: serde_json::Value| {
            serde_json::from_value::<JsonValue>(value).unwrap_or(JsonValue::Null)
        };
        let answered = |status: &str| Readback {
            result: json(
                serde_json::json!({"status": status, "token": {"value": "actual-secret"}}),
            ),
            ..readback()
        };
        let shape = ResultShape {
            required: None,
            optional: None,
            ephemeral: vec![vec!["token".to_owned(), "value".to_owned()]],
        };
        let seal = Seal {
            access: "mutate",
            finality: "confirmed",
            transport: "runx_connect",
            principal_uri: "runx:principal:operator-1",
            grant_id: None,
            account_id: None,
            intent_digest: "sha256:intent",
            operation_digest: "sha256:operation",
            implementation_digest: None,
            cost: None,
        };
        let equals = |field: &str, value: serde_json::Value| {
            JsonObject::from([(field.to_owned(), json(value))])
        };
        let refused = |status: &str, field: &str, value: serde_json::Value| {
            answered(status)
                .into_packet(&shape, Some(&equals(field, value)), &seal)
                .err()
                .unwrap_or_default()
        };
        assert_eq!(
            refused("pending", "status", serde_json::json!("succeeded")),
            r#"the result contradicts the call: status is "pending" where "succeeded" was expected"#
        );
        assert_eq!(
            refused("pending", "missing", serde_json::json!("x")),
            r#"the result states no value at missing, where "x" was expected"#
        );
        let withheld = refused(
            "succeeded",
            "token",
            serde_json::json!({"value": "expected-secret"}),
        );
        assert!(
            withheld.starts_with(
                "the result contradicts the call: token is a value with digest sha256:"
            ),
            "{withheld}"
        );
        assert!(!withheld.contains("secret"), "{withheld}");
    }
}
