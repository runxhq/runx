//! What an operation step passes: the call's arguments and how the caller
//! wants the result shaped. Everything else (verb, scopes, target, floors)
//! comes from the connector, and the instance from resolution.

use std::collections::BTreeSet;

use runx_contracts::connector::{ConnectorId, OperationName};
use runx_contracts::{CallAct, JsonObject, OperationAmount};
use serde::Deserialize;

const MAX_PROJECTED: usize = 50;
const MAX_EPHEMERAL: usize = 16;
const MAX_TEXT: usize = 512;

/// One operation call, decoded from the step's materialized inputs.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperationCall {
    /// The connector a run-time requirement chose. Bound by the parser from
    /// the requirement's `connector_from`; never written by an author.
    #[serde(default)]
    pub connector: Option<ConnectorId>,
    /// The operation chosen at run time, when the step names only its
    /// requirement.
    #[serde(default)]
    pub operation_name: Option<OperationName>,
    /// The operation's arguments: the payload the intent digests.
    #[serde(default)]
    pub input: JsonObject,
    /// The caller's request key. Required for mutations.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// What a person is shown when the call is gated.
    #[serde(default)]
    pub approval: Option<ApprovalText>,
    #[serde(default)]
    pub amount: Option<OperationAmount>,
    /// Result fields that must hold these values before the result is trusted.
    #[serde(default)]
    pub expected_result: Option<JsonObject>,
    #[serde(default)]
    pub result_fields: Option<Vec<String>>,
    #[serde(default)]
    pub optional_result_fields: Option<Vec<String>>,
    /// Nested result paths delivered only to the immediate caller: their
    /// values stay out of graph state and receipts.
    #[serde(default)]
    pub ephemeral_result_paths: Option<Vec<String>>,
    /// The confirmed mutation this read verifies. A failed readback holds the
    /// mutation for reconciliation instead of failing it.
    #[serde(default)]
    pub readback_of: Option<JsonObject>,
    /// The act the call performs, as its caller states it. Bound into the
    /// admitted intent and sealed in the call's receipt; never sent to the
    /// provider.
    #[serde(default)]
    pub act: Option<CallAct>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApprovalText {
    pub reason: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub gate_type: Option<String>,
}

/// The validated result shape a call asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ResultShape {
    pub required: Option<Vec<String>>,
    pub optional: Option<Vec<String>>,
    pub ephemeral: Vec<Vec<String>>,
}

impl OperationCall {
    pub(crate) fn decode(inputs: &JsonObject) -> Result<Self, String> {
        let call: Self = runx_contracts::JsonValue::Object(inputs.clone())
            .deserialize_into()
            .map_err(|error| format!("operation step inputs are invalid: {error}"))?;
        if let Some(approval) = &call.approval {
            bounded_text(&approval.reason, "approval.reason")?;
            if let Some(gate_type) = &approval.gate_type {
                bounded_text(gate_type, "approval.type")?;
            }
        }
        if let Some(amount) = &call.amount {
            bounded_text(&amount.unit, "amount.unit")?;
        }
        if let Some(key) = &call.idempotency_key {
            bounded_text(key, "idempotency_key")?;
        }
        if let Some(act) = &call.act {
            act.check()?;
        }
        if call.input.contains_key("idempotency_key") {
            return Err(
                "input must not carry idempotency_key; runx derives the provider key".to_owned(),
            );
        }
        Ok(call)
    }

    pub(crate) fn shape(&self) -> Result<ResultShape, String> {
        let required = projected(self.result_fields.as_deref(), "result_fields")?;
        let optional = projected(
            self.optional_result_fields.as_deref(),
            "optional_result_fields",
        )?;
        if let (Some(required), Some(optional)) = (&required, &optional)
            && optional.iter().any(|field| required.contains(field))
        {
            return Err("result_fields and optional_result_fields must not overlap".to_owned());
        }
        let projected_fields = required
            .iter()
            .chain(optional.iter())
            .flatten()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let ephemeral = ephemeral_paths(self.ephemeral_result_paths.as_deref(), &projected_fields)?;
        let total = required.as_ref().map_or(0, Vec::len)
            + optional.as_ref().map_or(0, Vec::len)
            + ephemeral.len();
        if total > MAX_PROJECTED {
            return Err(format!(
                "result fields and ephemeral result paths must contain at most {MAX_PROJECTED} entries in total"
            ));
        }
        if let Some(expected) = &self.expected_result
            && (expected.is_empty()
                || expected.len() > MAX_PROJECTED
                || expected.keys().any(|field| !valid_field(field)))
        {
            return Err("expected_result must hold 1 to 50 safe top-level fields".to_owned());
        }
        Ok(ResultShape {
            required,
            optional,
            ephemeral,
        })
    }
}

fn projected(fields: Option<&[String]>, name: &str) -> Result<Option<Vec<String>>, String> {
    let Some(fields) = fields else {
        return Ok(None);
    };
    if fields.is_empty() || fields.len() > MAX_PROJECTED {
        return Err(format!("{name} must contain 1 to {MAX_PROJECTED} entries"));
    }
    let mut seen = BTreeSet::new();
    let mut projected = Vec::with_capacity(fields.len());
    for field in fields {
        let field = field.trim();
        if !valid_field(field) {
            return Err(format!("{name} entries must be safe top-level field names"));
        }
        if seen.insert(field) {
            projected.push(field.to_owned());
        }
    }
    Ok(Some(projected))
}

fn ephemeral_paths(
    paths: Option<&[String]>,
    projected: &BTreeSet<&str>,
) -> Result<Vec<Vec<String>>, String> {
    let Some(paths) = paths else {
        return Ok(Vec::new());
    };
    if paths.is_empty() || paths.len() > MAX_EPHEMERAL {
        return Err(format!(
            "ephemeral_result_paths must contain 1 to {MAX_EPHEMERAL} entries"
        ));
    }
    let mut parsed: Vec<Vec<String>> = Vec::with_capacity(paths.len());
    for raw in paths {
        let segments = raw
            .trim()
            .strip_prefix('/')
            .filter(|path| !path.is_empty())
            .map(|path| path.split('/').map(str::to_owned).collect::<Vec<_>>())
            .filter(|segments| {
                (2..=8).contains(&segments.len())
                    && segments.iter().all(|segment| valid_field(segment))
            })
            .ok_or("ephemeral_result_paths entries must name 2 to 8 safe object fields")?;
        if !projected.contains(segments[0].as_str()) {
            return Err(
                "each ephemeral result path must descend from a projected result field".to_owned(),
            );
        }
        if parsed
            .iter()
            .any(|existing| is_prefix(existing, &segments) || is_prefix(&segments, existing))
        {
            return Err("ephemeral_result_paths must be unique and non-overlapping".to_owned());
        }
        parsed.push(segments);
    }
    Ok(parsed)
}

fn is_prefix(left: &[String], right: &[String]) -> bool {
    left.len() <= right.len() && left.iter().zip(right).all(|(left, right)| left == right)
}

pub(crate) fn valid_field(field: &str) -> bool {
    !field.is_empty()
        && field.len() <= 100
        && field
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn bounded_text(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > MAX_TEXT || value.chars().any(char::is_control) {
        Err(format!("{field} must be a safe non-empty value"))
    } else {
        Ok(())
    }
}
