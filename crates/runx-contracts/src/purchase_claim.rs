//! How a purchase bound to a buyer key is read and claimed (S10). The
//! purchase binds the key; runx holds what the purchase bought. The buyer
//! signs each read with the key, and each claim of a held value with the key
//! over the digest of the purchase's receipt, so neither a purchase id nor a
//! receipt alone reads or claims anything. runx verifies both; no seller does.

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Digest;
use crate::canonical_json::{CanonicalJsonError, canonical_stable_json};
use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};

pub const PURCHASE_CLAIM_SCHEMA: &str = "runx.payment.purchase_claim.v1";
pub const PURCHASE_READ_REQUEST_SCHEMA: &str = "runx.payment.purchase_read_request.v1";

const ED25519_PREFIX: &str = "ed25519:";

/// An Ed25519 public key a buyer holds: `ed25519:` and the 32 key bytes
/// in unpadded base64url, spelled one way only.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct BuyerKey(String);

impl BuyerKey {
    pub fn new(value: String) -> Option<Self> {
        canonical_ed25519_bytes(&value, 32).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for BuyerKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).ok_or_else(|| {
            de::Error::custom("buyer_key is ed25519: and 32 bytes of unpadded base64url")
        })
    }
}

impl RunxSchema for BuyerKey {
    fn json_schema() -> Value {
        json!({ "type": "string", "pattern": "^ed25519:[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$" })
    }
}

/// A buyer's Ed25519 signature: `ed25519:` and the 64 signature bytes in
/// unpadded base64url, spelled one way only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BuyerSignature(String);

impl BuyerSignature {
    pub fn new(value: String) -> Option<Self> {
        canonical_ed25519_bytes(&value, 64).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for BuyerSignature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).ok_or_else(|| {
            de::Error::custom("signature is ed25519: and 64 bytes of unpadded base64url")
        })
    }
}

impl RunxSchema for BuyerSignature {
    fn json_schema() -> Value {
        json!({ "type": "string", "pattern": "^ed25519:[A-Za-z0-9_-]{85}[AQgw]$" })
    }
}

/// What a buyer sends runx's claim route for a held value
/// (`runx.payment.purchase_claim.v1`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.purchase_claim.v1")]
pub struct PurchaseClaim {
    /// The digest of runx's receipt for the fulfilled purchase, as the
    /// purchase gave it: a purchase has none until it is fulfilled.
    pub receipt_digest: Digest,
    /// The endpoint this claim is sent to: a claim is good for one endpoint.
    pub claim_url: NonEmptyString,
    /// When the buyer signed; runx refuses a stale claim.
    pub issued_at: IsoDateTime,
    /// The buyer's signature over the claim schema, the endpoint, the time
    /// and the receipt digest, as stable JSON.
    pub signature: BuyerSignature,
}

/// The bytes a buyer signs for one claim: the claim schema, the
/// endpoint, the time, and the digest of the purchase's receipt, as stable
/// JSON.
pub fn purchase_claim_message(
    claim_url: &str,
    issued_at: &str,
    receipt_digest: &str,
) -> Result<Vec<u8>, CanonicalJsonError> {
    canonical_stable_json(&json!({
        "schema": PURCHASE_CLAIM_SCHEMA,
        "claim_url": claim_url,
        "issued_at": issued_at,
        "receipt_digest": receipt_digest,
    }))
    .map(String::into_bytes)
}

/// The bytes a buyer signs to read its purchase, which may have no
/// receipt yet: the read schema, the method and URL, and the time, as stable
/// JSON. runx refuses a read signed outside a short window around its clock.
pub fn purchase_read_request_message(
    method: &str,
    url: &str,
    issued_at: &str,
) -> Result<Vec<u8>, CanonicalJsonError> {
    canonical_stable_json(&json!({
        "schema": PURCHASE_READ_REQUEST_SCHEMA,
        "method": method,
        "url": url,
        "issued_at": issued_at,
    }))
    .map(String::into_bytes)
}

/// `ed25519:` and exactly `bytes` bytes of unpadded base64url whose unused
/// trailing bits are zero, so each value has one spelling.
fn canonical_ed25519_bytes(value: &str, bytes: usize) -> bool {
    let Some(encoded) = value.strip_prefix(ED25519_PREFIX) else {
        return false;
    };
    let length = (bytes * 8).div_ceil(6);
    let unused_bits = length * 6 - bytes * 8;
    encoded.len() == length
        && encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        && encoded
            .bytes()
            .last()
            .and_then(base64url_value)
            .is_some_and(|last| last & ((1 << unused_bits) - 1) == 0)
}

fn base64url_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buyer_keys_and_signatures_have_one_spelling() {
        let key = format!("ed25519:{}", "A".repeat(43));
        assert!(BuyerKey::new(key).is_some());
        // 32 bytes leave two unused bits; a set unused bit is another spelling.
        assert!(BuyerKey::new(format!("ed25519:{}B", "A".repeat(42))).is_none());
        assert!(BuyerKey::new(format!("ed25519:{}", "A".repeat(44))).is_none());
        assert!(BuyerKey::new(format!("ed25519:{}=", "A".repeat(43))).is_none());
        assert!(BuyerKey::new(format!("x25519:{}", "A".repeat(43))).is_none());
        assert!(BuyerSignature::new(format!("ed25519:{}w", "A".repeat(85))).is_some());
        assert!(BuyerSignature::new(format!("ed25519:{}B", "A".repeat(85))).is_none());
    }

    #[test]
    fn a_claim_signs_its_endpoint_time_and_receipt() -> Result<(), CanonicalJsonError> {
        let message = purchase_claim_message(
            "https://api.runx.ai/v1/purchases/paid_1/claim",
            "2026-10-15T00:00:00Z",
            "sha256:abc",
        )?;
        assert_eq!(
            String::from_utf8_lossy(&message),
            r#"{"claim_url":"https://api.runx.ai/v1/purchases/paid_1/claim","issued_at":"2026-10-15T00:00:00Z","receipt_digest":"sha256:abc","schema":"runx.payment.purchase_claim.v1"}"#
        );
        Ok(())
    }
}
