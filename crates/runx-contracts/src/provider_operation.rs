//! Provider operation readback packet contract.

use serde::{Deserialize, Serialize};

use crate::JsonValue;
use crate::schema::RunxSchema;

/// The provider-neutral packet emitted after a provider operation has been
/// projected and, for mutations, independently read back. Provider adapters
/// own the shape of `result`; Runx owns this envelope and its identity fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.provider.operation.v1",
    url = "https://schemas.runx.ai/runx/provider/operation/v1.json"
)]
pub struct ProviderOperationPacket {
    pub schema: String,
    pub status: String,
    pub provider: String,
    pub operation: String,
    pub target: String,
    pub result: JsonValue,
    pub transport: String,
    pub readback_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_ref: Option<String>,
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
    pub account_ref: Option<String>,
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
