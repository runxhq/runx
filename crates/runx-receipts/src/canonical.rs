use crate::ReceiptError;
use runx_contracts::receipt::{
    Lineage, ReceiptAct, ReceiptAuthority, ReceiptCanonicalization, ReceiptIssuer, ReceiptSchema,
    Seal, Subject,
};
use runx_contracts::schema::{IsoDateTime, NonEmptyString};
use runx_contracts::{Decision, Receipt, sha256_prefixed, to_canonical_bytes};
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

/// The canonical body that the content-addressed `id` commits: the signed
/// body without the `id` it derives. A receipt names only receipts that exist
/// before it (its children and the receipt it follows), so its lineage is part
/// of its address: two receipts that differ anywhere have different ids.
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
    /// The signed body: every field except `signature` and `digest`.
    Body,
    /// The content address: the body without `id`.
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
    canonicalization: &'a ReceiptCanonicalization,
    issuer: &'a ReceiptIssuer,
    subject: &'a Subject,
    authority: &'a ReceiptAuthority,
    decisions: &'a [Decision],
    acts: &'a [ReceiptAct],
    seal: &'a Seal,
    lineage: &'a Lineage,
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
            subject,
            authority,
            decisions,
            acts,
            seal,
            lineage,
        } = receipt;
        let identity = commitment == Commitment::Identity;
        Self {
            schema,
            id: (!identity).then_some(id),
            created_at,
            canonicalization,
            issuer,
            subject,
            authority,
            decisions,
            acts,
            seal,
            lineage,
        }
    }
}

#[cfg(test)]
mod tests {
    use runx_contracts::Receipt;
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
        full_digest: String,
        body_canonical_json: String,
        body_digest: String,
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
                case.full_digest,
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
                case.body_digest,
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
