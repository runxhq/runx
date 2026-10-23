use runx_contracts::{Receipt, ReceiptIssuerType};
use serde::Serialize;

use crate::{canonical_receipt_body_digest, content_addressed_receipt_id};

use super::{ReceiptFinding, ReceiptFindingCode, ReceiptProofContext, verify_receipt_proof};

pub const VERIFY_VERDICT_SCHEMA: &str = "runx.verify_verdict.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReceiptVerifyVerdict {
    pub schema: &'static str,
    pub receipt_id: Option<String>,
    pub valid: bool,
    pub digest: ReceiptVerifyCheck,
    pub content_address: ReceiptVerifyCheck,
    pub signature: ReceiptVerifySignatureCheck,
    pub lineage: ReceiptVerifyLineageCheck,
    pub findings: Vec<ReceiptVerifyFinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReceiptVerifyCheck {
    pub status: &'static str,
    pub expected: Option<String>,
    pub actual: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReceiptVerifySignatureCheck {
    pub status: &'static str,
    /// The kind of identity the receipt names as its signer.
    pub issuer_type: Option<ReceiptIssuerType>,
    pub kid: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReceiptVerifyLineageCheck {
    pub status: &'static str,
    pub message: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReceiptVerifyFinding {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[must_use]
pub fn verify_receipt_document_verdict(
    document: &[u8],
    context: &ReceiptProofContext<'_>,
) -> ReceiptVerifyVerdict {
    match crate::decode_receipt(document) {
        Ok(receipt) => verify_receipt_verdict(&receipt, context),
        Err(error) => parse_error_verdict(error.to_string()),
    }
}

#[must_use]
pub fn verify_receipt_verdict(
    receipt: &Receipt,
    context: &ReceiptProofContext<'_>,
) -> ReceiptVerifyVerdict {
    let digest = digest_check(receipt);
    let content_address = content_address_check(receipt);
    let verification = verify_receipt_proof(receipt, context);
    let findings = verification
        .findings
        .iter()
        .map(verdict_finding)
        .collect::<Vec<_>>();
    let signature = signature_check(receipt, &verification.findings);
    let valid = verification.valid
        && digest.status == "valid"
        && content_address.status == "valid"
        && signature.status == "valid";

    ReceiptVerifyVerdict {
        schema: VERIFY_VERDICT_SCHEMA,
        receipt_id: Some(receipt.id.to_string()),
        valid,
        digest,
        content_address,
        signature,
        lineage: lineage_check(),
        findings,
    }
}

fn parse_error_verdict(message: String) -> ReceiptVerifyVerdict {
    ReceiptVerifyVerdict {
        schema: VERIFY_VERDICT_SCHEMA,
        receipt_id: None,
        valid: false,
        digest: not_evaluated_check(),
        content_address: not_evaluated_check(),
        signature: ReceiptVerifySignatureCheck {
            status: "not_evaluated",
            issuer_type: None,
            kid: None,
        },
        lineage: lineage_check(),
        findings: vec![ReceiptVerifyFinding {
            code: "receipt_parse_error".to_owned(),
            path: "$".to_owned(),
            message,
        }],
    }
}

fn digest_check(receipt: &Receipt) -> ReceiptVerifyCheck {
    match canonical_receipt_body_digest(receipt) {
        Ok(expected) => ReceiptVerifyCheck {
            status: if receipt.digest == expected {
                "valid"
            } else {
                "invalid"
            },
            expected: Some(expected),
            actual: Some(receipt.digest.to_string()),
        },
        Err(_error) => ReceiptVerifyCheck {
            status: "not_evaluated",
            expected: None,
            actual: Some(receipt.digest.to_string()),
        },
    }
}

fn content_address_check(receipt: &Receipt) -> ReceiptVerifyCheck {
    match content_addressed_receipt_id(receipt) {
        Ok(expected) => ReceiptVerifyCheck {
            status: if receipt.id == expected {
                "valid"
            } else {
                "invalid"
            },
            expected: Some(expected),
            actual: Some(receipt.id.to_string()),
        },
        Err(_error) => ReceiptVerifyCheck {
            status: "not_evaluated",
            expected: None,
            actual: Some(receipt.id.to_string()),
        },
    }
}

fn not_evaluated_check() -> ReceiptVerifyCheck {
    ReceiptVerifyCheck {
        status: "not_evaluated",
        expected: None,
        actual: None,
    }
}

fn signature_check(receipt: &Receipt, findings: &[ReceiptFinding]) -> ReceiptVerifySignatureCheck {
    ReceiptVerifySignatureCheck {
        status: if findings
            .iter()
            .any(|finding| is_signature_finding(finding.code))
        {
            "invalid"
        } else {
            "valid"
        },
        issuer_type: Some(receipt.issuer.issuer_type.clone()),
        kid: if receipt.issuer.kid.is_empty() {
            None
        } else {
            Some(receipt.issuer.kid.to_string())
        },
    }
}

fn lineage_check() -> ReceiptVerifyLineageCheck {
    ReceiptVerifyLineageCheck {
        status: "unverified",
        message: "single receipt verification cannot prove receipt-tree lineage",
    }
}

fn verdict_finding(finding: &ReceiptFinding) -> ReceiptVerifyFinding {
    ReceiptVerifyFinding {
        code: finding.code.as_str().to_owned(),
        path: finding.path.clone(),
        message: finding.message.clone(),
    }
}

fn is_signature_finding(code: ReceiptFindingCode) -> bool {
    matches!(
        code,
        ReceiptFindingCode::SignatureVerifierMissing
            | ReceiptFindingCode::SignatureInvalid
            | ReceiptFindingCode::SignatureKeyMissing
            | ReceiptFindingCode::SignatureKeyMalformed
            | ReceiptFindingCode::SignatureKeyDigestMismatch
            | ReceiptFindingCode::SignatureUnsupportedIssuer
            | ReceiptFindingCode::SignatureUnsupportedAlgorithm
            | ReceiptFindingCode::SignatureMalformed
    )
}
