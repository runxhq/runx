use crate::ReceiptError;
use runx_contracts::receipt::{
    Lineage, ReceiptAct, ReceiptAuthority, ReceiptClass, ReceiptEvidence, ReceiptIdempotency,
    ReceiptIssuer, ReceiptSchema, Seal, Subject,
};
use runx_contracts::schema::{IsoDateTime, NonEmptyString};
use runx_contracts::{Decision, Receipt, Reference, sha256_prefixed, to_canonical_bytes};
use serde::Serialize;

pub fn canonical_receipt_json(receipt: &Receipt) -> Result<String, ReceiptError> {
    canonical_string(receipt)
}

pub fn canonical_receipt_digest(receipt: &Receipt) -> Result<String, ReceiptError> {
    canonical_bytes(receipt).map(|bytes| sha256_prefixed(&bytes))
}

pub fn canonical_receipt_body_json(receipt: &Receipt) -> Result<String, ReceiptError> {
    canonical_string(&ReceiptView::of(receipt, Commitment::Body))
}

pub fn canonical_receipt_body_digest(receipt: &Receipt) -> Result<String, ReceiptError> {
    canonical_bytes(&ReceiptView::of(receipt, Commitment::Body))
        .map(|bytes| sha256_prefixed(&bytes))
}

/// The canonical body that the content-addressed `id` commits: every intrinsic
/// run field except the envelope's `id` (which it derives), `signature`,
/// `digest`, the runtime-local `metadata` read aid, and `lineage`. `lineage` is
/// post-hoc graph wiring (parent/children refs) attached after the children's
/// own ids are known; excluding it breaks the parent<->child id circularity
/// while keeping the address stable. The full `digest` still commits `lineage`.
pub fn canonical_receipt_identity_json(receipt: &Receipt) -> Result<String, ReceiptError> {
    canonical_string(&ReceiptView::of(receipt, Commitment::Identity))
}

/// `id = hash(canonical_body)` under `runx.receipt.c14n.v1`: the content address
/// of this receipt. References to a receipt use this id.
pub fn content_addressed_receipt_id(receipt: &Receipt) -> Result<String, ReceiptError> {
    canonical_bytes(&ReceiptView::of(receipt, Commitment::Identity))
        .map(|bytes| sha256_prefixed(&bytes))
}

fn canonical_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, ReceiptError> {
    to_canonical_bytes(value).map_err(|source| ReceiptError::Serialization {
        message: source.to_string(),
    })
}

fn canonical_string<T: Serialize + ?Sized>(value: &T) -> Result<String, ReceiptError> {
    String::from_utf8(canonical_bytes(value)?).map_err(|source| ReceiptError::Serialization {
        message: source.to_string(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Commitment {
    /// The signed body: every field except `signature`, `digest` and the
    /// runtime-local `metadata` read aid.
    Body,
    /// The content address: the body without `id` and `lineage`.
    Identity,
}

/// A borrowed projection of a receipt for one commitment. Serialized straight
/// into canonical bytes; nothing is copied or converted.
#[derive(Serialize)]
struct ReceiptView<'a> {
    schema: &'a ReceiptSchema,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a NonEmptyString>,
    created_at: &'a IsoDateTime,
    canonicalization: &'a NonEmptyString,
    issuer: &'a ReceiptIssuer,
    idempotency: &'a ReceiptIdempotency,
    class: &'a ReceiptClass,
    subject: &'a Subject,
    authority: &'a ReceiptAuthority,
    signals: &'a [Reference],
    decisions: &'a [Decision],
    acts: &'a [ReceiptAct],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    evidence: &'a [ReceiptEvidence],
    seal: &'a Seal,
    #[serde(skip_serializing_if = "Option::is_none")]
    lineage: Option<&'a Lineage>,
}

impl<'a> ReceiptView<'a> {
    fn of(receipt: &'a Receipt, commitment: Commitment) -> Self {
        // Exhaustive: a new receipt field fails to compile here until it is
        // classified as committed or excluded.
        let Receipt {
            schema,
            id,
            created_at,
            canonicalization,
            issuer,
            signature: _,
            digest: _,
            idempotency,
            class,
            subject,
            authority,
            signals,
            decisions,
            acts,
            evidence,
            seal,
            lineage,
            metadata: _,
        } = receipt;
        let identity = commitment == Commitment::Identity;
        Self {
            schema,
            id: (!identity).then_some(id),
            created_at,
            canonicalization,
            issuer,
            idempotency,
            class,
            subject,
            authority,
            signals,
            decisions,
            acts,
            evidence,
            seal,
            lineage: lineage.as_ref().filter(|_| !identity),
        }
    }
}

#[cfg(test)]
mod tests {
    use runx_contracts::{JsonValue, Receipt};
    use serde::Deserialize;

    use super::{
        ReceiptError, canonical_receipt_body_digest, canonical_receipt_body_json,
        canonical_receipt_digest, canonical_receipt_json,
    };

    const SUCCESS_RECEIPT: &str =
        include_str!("../../../fixtures/contracts/harness-spine/receipt-success.json");
    const ABNORMAL_RECEIPT: &str =
        include_str!("../../../fixtures/contracts/harness-spine/receipt-abnormal.json");
    const RECEIPT_ORACLE: &str = include_str!(
        "../../../fixtures/contracts/canonical-json/runx-receipt-c14n-v1.oracles.json"
    );
    #[derive(Debug, Deserialize)]
    struct Fixture {
        expected: Receipt,
    }

    #[derive(Debug, Deserialize)]
    struct ReceiptOracleFixture {
        canonicalization: String,
        cases: Vec<ReceiptOracleCase>,
    }

    #[derive(Debug, Deserialize)]
    struct ReceiptOracleCase {
        name: String,
        fixture: String,
        full_canonical_json: String,
        full_sha256: String,
        body_canonical_json: String,
        body_sha256: String,
    }

    #[test]
    fn canonical_receipt_json_is_stable_and_sorted() -> Result<(), ReceiptError> {
        let receipt = fixture()?;
        let first = canonical_receipt_json(&receipt)?;
        let second = canonical_receipt_json(&receipt)?;

        assert_eq!(first, second);
        assert!(first.contains("\"created_at\":\""));
        assert!(canonical_receipt_digest(&receipt)?.starts_with("sha256:"));
        Ok(())
    }

    #[test]
    fn body_commitment_excludes_signature_and_seal_derived_fields() -> Result<(), ReceiptError> {
        let mut receipt = fixture()?;
        let baseline_json = canonical_receipt_body_json(&receipt)?;
        let baseline_digest = canonical_receipt_body_digest(&receipt)?;

        receipt.signature.value = "base64:changed".into();
        receipt.digest = "sha256:changed".into();

        assert_eq!(canonical_receipt_body_json(&receipt)?, baseline_json);
        assert_eq!(canonical_receipt_body_digest(&receipt)?, baseline_digest);
        Ok(())
    }

    #[test]
    fn body_commitment_excludes_metadata_read_aid() -> Result<(), ReceiptError> {
        let mut receipt = fixture()?;
        let baseline_digest = canonical_receipt_body_digest(&receipt)?;

        receipt.metadata.get_or_insert_default().insert(
            "skill_name".to_owned(),
            JsonValue::String("changed-read-aid".to_owned()),
        );

        assert_eq!(canonical_receipt_body_digest(&receipt)?, baseline_digest);
        Ok(())
    }

    #[test]
    fn receipt_oracle_matches_rust_canonical_json() -> Result<(), ReceiptError> {
        let oracle: ReceiptOracleFixture =
            serde_json::from_str(RECEIPT_ORACLE).map_err(|source| ReceiptError::Serialization {
                message: source.to_string(),
            })?;
        assert_eq!(oracle.canonicalization, "runx.receipt.c14n.v1");

        for case in oracle.cases {
            let receipt = fixture_by_path(&case.fixture)?;
            assert_eq!(
                canonical_receipt_json(&receipt)?,
                case.full_canonical_json,
                "{} full canonical JSON drifted",
                case.name
            );
            assert_eq!(
                canonical_receipt_digest(&receipt)?,
                case.full_sha256,
                "{} full digest drifted",
                case.name
            );
            assert_eq!(
                canonical_receipt_body_json(&receipt)?,
                case.body_canonical_json,
                "{} body canonical JSON drifted",
                case.name
            );
            assert_eq!(
                canonical_receipt_body_digest(&receipt)?,
                case.body_sha256,
                "{} body digest drifted",
                case.name
            );
        }
        Ok(())
    }

    fn fixture() -> Result<Receipt, ReceiptError> {
        serde_json::from_str::<Fixture>(SUCCESS_RECEIPT)
            .map(|fixture| fixture.expected)
            .map_err(|source| ReceiptError::Serialization {
                message: source.to_string(),
            })
    }

    fn fixture_by_path(path: &str) -> Result<Receipt, ReceiptError> {
        let json = match path {
            "harness-spine/receipt-abnormal.json" => ABNORMAL_RECEIPT,
            "harness-spine/receipt-success.json" => SUCCESS_RECEIPT,
            _ => {
                return Err(ReceiptError::Serialization {
                    message: format!("unknown receipt oracle fixture: {path}"),
                });
            }
        };
        serde_json::from_str::<Fixture>(json)
            .map(|fixture| fixture.expected)
            .map_err(|source| ReceiptError::Serialization {
                message: source.to_string(),
            })
    }
}
