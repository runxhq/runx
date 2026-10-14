//! What a placement returns, and the packet a step sees.
//!
//! Every placement returns one [`Readback`]: the provider's result and an
//! opaque reference to the independent evidence behind it. It is checked
//! against the admitted intent, then projected into the
//! `runx.provider.operation.v1` packet the step emits.

use runx_contracts::{JsonObject, JsonValue, ProviderOperationPacket};
use serde::{Deserialize, Serialize};

use super::call::ResultShape;

/// A placement's evidence for one call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Readback {
    pub provider: String,
    pub operation: String,
    pub target: String,
    pub result: JsonValue,
    pub readback_ref: String,
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
    /// The provider key, for mutations.
    pub idempotency_key: Option<&'a str>,
}

/// The facts the packet carries beside the readback.
pub(crate) struct Seal<'a> {
    pub access: &'static str,
    /// `confirmed`, or `acknowledged` for a write the provider cannot read
    /// back.
    pub finality: &'static str,
    pub transport: &'static str,
    pub principal_ref: &'a str,
    /// The runx Connect grant the call ran under; a local instance has none.
    pub grant_id: Option<&'a str>,
    pub account_ref: Option<&'a str>,
    pub intent_digest: &'a str,
    pub operation_digest: &'a str,
    pub implementation_digest: Option<&'a str>,
    /// What a priced call cost.
    pub cost: Option<runx_contracts::connector::CallCost>,
}

impl Readback {
    /// Check the readback against the admitted call.
    pub(crate) fn verify(&self, expected: &Expected<'_>) -> Result<(), String> {
        if self.provider != expected.connector {
            return Err(format!("the readback names provider {:?}", self.provider));
        }
        if self.operation != expected.operation || self.target != expected.target {
            return Err("the readback does not match the admitted operation and target".to_owned());
        }
        if self.readback_ref.trim().is_empty() {
            return Err("the readback carries no evidence reference".to_owned());
        }
        if let Some(key) = expected.idempotency_key {
            if self.operation_id.as_deref().is_none_or(str::is_empty) {
                return Err("the mutation was not acknowledged with an operation id".to_owned());
            }
            if self.idempotency_key.as_deref() != Some(key) {
                return Err(
                    "the acknowledgement does not carry the admitted idempotency key".to_owned(),
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
            for (field, value) in expected_result.into_iter().flatten() {
                if result.get(field) != Some(value) {
                    return Err(format!(
                        "result field {field:?} did not match the expected value"
                    ));
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
            readback_ref: self.readback_ref,
            access: Some(seal.access.to_owned()),
            principal_ref: Some(seal.principal_ref.to_owned()),
            grant_ref: seal
                .grant_id
                .map(|grant_id| format!("runx:grant:{grant_id}")),
            finality: Some(seal.finality.to_owned()),
            plan_digest: Some(seal.intent_digest.to_owned()),
            result_digest: Some(result_digest.as_str().to_owned()),
            operation_id: self.operation_id,
            idempotency_key: self.idempotency_key,
            host: None,
            account_ref: seal.account_ref.map(str::to_owned),
            operation_digest: Some(seal.operation_digest.to_owned()),
            implementation_digest: seal.implementation_digest.map(str::to_owned),
            cost: seal.cost.clone(),
        })
    }
}

/// Move the caller-only result paths out of a packet's result. Their values
/// reach the immediate caller and never graph state or receipts; the result
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
            readback_ref: "runx:provider-readback:1".to_owned(),
            operation_id: Some("op-1".to_owned()),
            idempotency_key: Some("runx:expected".to_owned()),
        }
    }

    fn expected(key: Option<&'static str>) -> Expected<'static> {
        Expected {
            connector: "slack",
            operation: "thread.reply",
            target: "slack://T/C/2",
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
}
