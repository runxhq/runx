use std::collections::BTreeMap;

use runx_contracts::{Receipt, Reference, ReferenceType};

use super::ResolvedReceipt;
use crate::{ReceiptFinding, ReceiptFindingCode, validate_receipt};

pub(super) fn duplicate_child_findings(children: &[ResolvedReceipt<'_>]) -> Vec<ReceiptFinding> {
    let mut seen = BTreeMap::new();
    children
        .iter()
        .filter_map(|child| {
            if seen
                .insert(child.receipt.id.as_str(), child.path.as_str())
                .is_some()
            {
                Some(ReceiptFinding {
                    code: ReceiptFindingCode::DuplicateChildReceipt,
                    path: format!("{}.id", child.path),
                    message: "child receipt ids must be unique".to_owned(),
                })
            } else {
                None
            }
        })
        .collect()
}

pub(super) fn child_receipt_findings(children: &[ResolvedReceipt<'_>]) -> Vec<ReceiptFinding> {
    children
        .iter()
        .flat_map(|child| {
            validate_receipt(child.receipt)
                .err()
                .map_or_else(Vec::new, |verification| {
                    verification
                        .findings
                        .into_iter()
                        .map(|finding| child_finding(&child.path, finding))
                        .collect()
                })
        })
        .collect()
}

pub(super) fn orphan_child_findings(
    children: &[ResolvedReceipt<'_>],
    reached: &std::collections::BTreeSet<String>,
) -> Vec<ReceiptFinding> {
    children
        .iter()
        .filter(|child| !reached.contains(child.receipt.id.as_str()))
        .map(|child| ReceiptFinding {
            code: ReceiptFindingCode::OrphanChildReceipt,
            path: format!("{}.id", child.path),
            message: "supplied child receipts must be reachable from the root receipt".to_owned(),
        })
        .collect()
}

pub(super) fn missing_child(path: &str) -> ReceiptFinding {
    ReceiptFinding {
        code: ReceiptFindingCode::ChildReceiptMissing,
        path: path.to_owned(),
        message: "child receipt ref must resolve to a supplied child receipt".to_owned(),
    }
}

pub(super) fn malformed_child_ref(path: &str) -> ReceiptFinding {
    ReceiptFinding {
        code: ReceiptFindingCode::ChildReceiptRefMalformed,
        path: path.to_owned(),
        message: "child receipt ref must be a typed runx receipt URI".to_owned(),
    }
}

pub(super) fn ambiguous_child(path: &str) -> ReceiptFinding {
    ReceiptFinding {
        code: ReceiptFindingCode::ChildReceiptAmbiguous,
        path: path.to_owned(),
        message: "child receipt ref resolved to multiple supplied receipts".to_owned(),
    }
}

pub(super) fn resolver_error(path: &str) -> ReceiptFinding {
    ReceiptFinding {
        code: ReceiptFindingCode::ChildReceiptResolverError,
        path: path.to_owned(),
        message: "child receipt ref resolver failed before proof verification".to_owned(),
    }
}

pub(super) fn child_digest_link_findings(
    path: &str,
    reference: &Reference,
    child: &Receipt,
) -> Vec<ReceiptFinding> {
    if reference.locator.as_deref() == Some(child.digest.as_str()) {
        return Vec::new();
    }
    vec![ReceiptFinding {
        code: ReceiptFindingCode::ChildReceiptDigestMismatch,
        path: format!("{path}.locator"),
        message:
            "strict tree proof requires child receipt refs to carry the exact child receipt digest"
                .to_owned(),
    }]
}

pub(super) fn child_finding(path: &str, finding: ReceiptFinding) -> ReceiptFinding {
    ReceiptFinding {
        path: format!("{path}.{}", finding.path),
        ..finding
    }
}

pub(super) fn join(path: &str, segment: &str) -> String {
    if path.is_empty() {
        segment.to_owned()
    } else {
        format!("{path}.{segment}")
    }
}

pub(super) fn referenced_receipt_id(reference: &Reference) -> Option<&str> {
    if reference.reference_type != ReferenceType::Receipt {
        return None;
    }
    reference
        .uri
        .strip_prefix("runx:receipt:")
        .filter(|id| !id.is_empty())
}
