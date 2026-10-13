use runx_contracts::Receipt;

use super::ReceiptVerification;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptProofStatusKind {
    Verified,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptProofFindingSummary {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptProofStatus {
    pub receipt_id: String,
    pub status: ReceiptProofStatusKind,
    pub finding_summaries: Vec<ReceiptProofFindingSummary>,
}

#[must_use]
pub fn receipt_proof_status(
    receipt: &Receipt,
    verification: &ReceiptVerification,
) -> ReceiptProofStatus {
    ReceiptProofStatus {
        receipt_id: receipt.id.to_string(),
        status: if verification.valid {
            ReceiptProofStatusKind::Verified
        } else {
            ReceiptProofStatusKind::Failed
        },
        finding_summaries: verification
            .findings
            .iter()
            .map(|finding| ReceiptProofFindingSummary {
                code: finding.code.as_str().to_owned(),
                path: public_text(&finding.path),
                message: public_text(&finding.message),
            })
            .collect(),
    }
}

fn public_text(value: &str) -> String {
    value
        .split_whitespace()
        .map(redact_public_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_public_token(token: &str) -> &str {
    let core = token.trim_matches(|character: char| {
        matches!(
            character,
            '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | ',' | ';'
        )
    });
    if looks_like_local_path(core) {
        "[local-path]"
    } else if looks_like_secret_value(core) {
        "[secret]"
    } else {
        token
    }
}

fn looks_like_local_path(token: &str) -> bool {
    token.starts_with('/') || token.as_bytes().get(1..3) == Some(b":\\")
}

fn looks_like_secret_value(token: &str) -> bool {
    token.len() >= 32
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
        && token.bytes().any(|byte| matches!(byte, b'+' | b'/' | b'='))
}
