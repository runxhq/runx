use runx_contracts::Purchase;

use super::HostedApiOperationError;
use super::request::send_json;
use crate::http::{HttpMethod, RuntimeHttpTransport};

/// A purchase as its buyer reads it (`GET /v1/purchases/:id`): the paid
/// invocation, runx's signed purchase receipt once sealed, the runner's public
/// result once fulfilled, and where to claim. Never the seller's run.
pub fn read_hosted_purchase(
    transport: &(impl RuntimeHttpTransport + ?Sized),
    base_url: &str,
    token: &str,
    purchase_id: &str,
) -> Result<Purchase, HostedApiOperationError> {
    send_json(
        transport,
        base_url,
        "purchase read",
        HttpMethod::Get,
        &format!("/v1/purchases/{}", url_segment(purchase_id)),
        Some(token),
        None,
    )
}

/// A purchase id as one path segment: only unreserved characters pass.
fn url_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
