//! How a purchase owned by a purchaser key is read and claimed (S10). The
//! purchase binds the key; runx holds what the purchase bought. The owner
//! signs each read with the key, and each claim of a held value with the key
//! over the purchase receipt, so neither a purchase id nor a receipt alone
//! reads or claims anything.

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::JsonObject;
use crate::canonical_json::{CanonicalJsonError, canonical_stable_json};
use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};

pub const PURCHASE_CLAIM_SCHEMA: &str = "runx.payment.purchase_claim.v1";
pub const PURCHASE_READ_SCHEMA: &str = "runx.payment.purchase_read.v1";
/// The receipt runx seals when a purchase's fulfilment wins.
pub const PURCHASE_RECEIPT_SCHEMA: &str = "runx.hosted_paid_invocation_terminal_receipt.v1";

const ED25519_PREFIX: &str = "ed25519:";

/// An Ed25519 public key a purchaser holds: `ed25519:` and the 32 key bytes
/// in unpadded base64url, spelled one way only.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PurchaserKey(String);

impl PurchaserKey {
    pub fn new(value: String) -> Option<Self> {
        canonical_ed25519_bytes(&value, 32).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PurchaserKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).ok_or_else(|| {
            de::Error::custom("purchaser_key is ed25519: and 32 bytes of unpadded base64url")
        })
    }
}

impl RunxSchema for PurchaserKey {
    fn json_schema() -> Value {
        json!({ "type": "string", "pattern": "^ed25519:[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$" })
    }
}

/// A purchaser's Ed25519 signature: `ed25519:` and the 64 signature bytes in
/// unpadded base64url, spelled one way only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PurchaserSignature(String);

impl PurchaserSignature {
    pub fn new(value: String) -> Option<Self> {
        canonical_ed25519_bytes(&value, 64).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PurchaserSignature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).ok_or_else(|| {
            de::Error::custom("signature is ed25519: and 64 bytes of unpadded base64url")
        })
    }
}

impl RunxSchema for PurchaserSignature {
    fn json_schema() -> Value {
        json!({ "type": "string", "pattern": "^ed25519:[A-Za-z0-9_-]{85}[AQgw]$" })
    }
}

/// What a purchaser sends runx's claim route for a held value
/// (`runx.payment.purchase_claim.v1`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.purchase_claim.v1")]
pub struct PurchaseClaim {
    /// runx's signed purchase receipt, exactly as the purchase gave it.
    pub receipt: JsonObject,
    /// The endpoint this claim is sent to: a claim is good for one endpoint.
    pub claim_url: NonEmptyString,
    /// When the purchaser signed; a seller refuses a stale claim.
    pub issued_at: IsoDateTime,
    /// The purchaser's signature over [`purchase_claim_message`].
    pub signature: PurchaserSignature,
}

/// The bytes a purchaser signs for one claim: the claim schema, the
/// endpoint, the time, and the digest of the receipt it presents, as stable
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

/// The bytes a purchaser signs to read its purchase, which may have no
/// receipt yet: the read schema, the method and URL, and the time, as stable
/// JSON. runx refuses a read signed outside a short window around its clock.
pub fn purchase_read_message(
    method: &str,
    url: &str,
    issued_at: &str,
) -> Result<Vec<u8>, CanonicalJsonError> {
    canonical_stable_json(&json!({
        "schema": PURCHASE_READ_SCHEMA,
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
    fn purchaser_keys_and_signatures_have_one_spelling() {
        let key = format!("ed25519:{}", "A".repeat(43));
        assert!(PurchaserKey::new(key).is_some());
        // 32 bytes leave two unused bits; a set unused bit is another spelling.
        assert!(PurchaserKey::new(format!("ed25519:{}B", "A".repeat(42))).is_none());
        assert!(PurchaserKey::new(format!("ed25519:{}", "A".repeat(44))).is_none());
        assert!(PurchaserKey::new(format!("ed25519:{}=", "A".repeat(43))).is_none());
        assert!(PurchaserKey::new(format!("x25519:{}", "A".repeat(43))).is_none());
        assert!(PurchaserSignature::new(format!("ed25519:{}w", "A".repeat(85))).is_some());
        assert!(PurchaserSignature::new(format!("ed25519:{}B", "A".repeat(85))).is_none());
    }

    #[test]
    fn a_claim_signs_its_endpoint_time_and_receipt() -> Result<(), CanonicalJsonError> {
        let message = purchase_claim_message(
            "https://inbox.ausca.com/v1/agent-inboxes/inb_1/claim",
            "2026-10-15T00:00:00Z",
            "sha256:abc",
        )?;
        assert_eq!(
            String::from_utf8_lossy(&message),
            r#"{"claim_url":"https://inbox.ausca.com/v1/agent-inboxes/inb_1/claim","issued_at":"2026-10-15T00:00:00Z","receipt_digest":"sha256:abc","schema":"runx.payment.purchase_claim.v1"}"#
        );
        Ok(())
    }
}
