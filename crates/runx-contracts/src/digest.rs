//! Content digests: the identifiers runx takes over bytes and documents.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Reference;
use crate::schema::{NonEmptyString, RunxSchema};

/// Lowercase hex encoding of raw bytes.
///
/// # Examples
///
/// ```rust,no_run
/// use runx_contracts::digest::hex_lower;
///
/// assert_eq!(hex_lower(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
/// ```
#[must_use]
pub fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

/// SHA-256 of the input bytes as lowercase hex (no prefix).
///
/// # Examples
///
/// ```rust,no_run
/// use runx_contracts::digest::sha256_hex;
///
/// assert_eq!(
///     sha256_hex(b"runx"),
///     "8186b7035bea2f66ebe27c1f5cf7de4e94ef935e259a2f3160352adffc752f28",
/// );
/// ```
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

/// SHA-256 of the input bytes, prefixed with the `sha256:` algorithm tag.
///
/// This is the content-addressed form used for runx identifiers.
///
/// # Examples
///
/// ```rust,no_run
/// use runx_contracts::digest::sha256_prefixed;
///
/// let id = sha256_prefixed(b"runx");
/// assert!(id.starts_with("sha256:"));
/// assert_eq!(
///     id,
///     "sha256:8186b7035bea2f66ebe27c1f5cf7de4e94ef935e259a2f3160352adffc752f28",
/// );
/// ```
#[must_use]
pub fn sha256_prefixed(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

/// The algorithm a content digest was taken with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithm {
    Sha256,
}

/// A content digest with how it was taken and what it was taken over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentDigest {
    pub algorithm: DigestAlgorithm,
    pub canonicalization: NonEmptyString,
    pub value: NonEmptyString,
    pub derived_from: Vec<Reference>,
}

#[cfg(test)]
mod tests {
    use super::{hex_lower, sha256_hex, sha256_prefixed};

    #[test]
    fn hex_lower_encodes_bytes() {
        assert_eq!(hex_lower(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }

    #[test]
    fn sha256_hex_hashes_bytes() {
        assert_eq!(
            sha256_hex(b"runx"),
            "8186b7035bea2f66ebe27c1f5cf7de4e94ef935e259a2f3160352adffc752f28",
        );
    }

    #[test]
    fn sha256_prefixed_includes_algorithm_tag() {
        assert_eq!(
            sha256_prefixed(b"runx"),
            "sha256:8186b7035bea2f66ebe27c1f5cf7de4e94ef935e259a2f3160352adffc752f28",
        );
    }
}
