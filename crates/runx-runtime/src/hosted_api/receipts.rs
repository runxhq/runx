use runx_contracts::JsonValue;
use serde::{Deserialize, Serialize};

use super::HostedApiOperationError;
use super::request::send_json;
use crate::http::{HttpMethod, RuntimeHttpTransport};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReceiptNotarization {
    pub status: String,
    #[serde(default)]
    pub replay_status: Option<String>,
    pub digest: String,
    pub public_digest: String,
    pub mode: String,
    pub published: bool,
    #[serde(default)]
    pub public_url: Option<String>,
    #[serde(default)]
    pub receipt_id: Option<String>,
    #[serde(default)]
    pub verdict: Option<JsonValue>,
}

pub fn notarize_hosted_receipt(
    transport: &impl RuntimeHttpTransport,
    base_url: &str,
    token: &str,
    receipt: &JsonValue,
) -> Result<ReceiptNotarization, HostedApiOperationError> {
    let body = serde_json::json!({
        "publish": true,
        "receipt": receipt,
    })
    .to_string();
    send_json(
        transport,
        base_url,
        "receipt notarize",
        HttpMethod::Post,
        "/v1/receipts/notarize",
        Some(token),
        Some(body),
    )
}
