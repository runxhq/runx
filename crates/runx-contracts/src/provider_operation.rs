//! Provider operation readback packet contract.

use serde::{Deserialize, Serialize};

use crate::JsonValue;
use crate::schema::RunxSchema;

/// The provider-neutral packet emitted after a provider operation has been
/// projected and, for mutations, independently read back. Provider adapters
/// own the shape of `result`; Runx owns this envelope and its identity fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.provider.operation.v1")]
pub struct ProviderOperationPacket {
    pub schema: String,
    pub status: String,
    pub provider: String,
    pub operation: String,
    pub target: String,
    pub result: JsonValue,
    pub transport: String,
    pub readback_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// What the operation meant when the call was admitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_digest: Option<String>,
    /// The local implementation that ran; absent for hosted calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implementation_digest: Option<String>,
    /// What a priced call cost; absent when the operation declares no cost.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<crate::connector::CallCost>,
}

impl ProviderOperationPacket {
    /// The packet an operation step's artifact is checked against.
    #[must_use]
    pub fn packet_id() -> crate::PacketId {
        crate::PacketId::core("runx.provider.operation.v1")
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderOperationPacket;
    use crate::schema::RunxSchema;

    #[test]
    fn the_packet_id_is_the_contract_schema_id() {
        assert_eq!(
            ProviderOperationPacket::json_schema()
                .get("x-runx-schema")
                .and_then(serde_json::Value::as_str),
            Some(ProviderOperationPacket::packet_id().as_str())
        );
    }
}
