//! Regenerates the flat `runx.receipt.v1` receipt-tree oracle. Run with:
//!   cargo run --manifest-path crates/Cargo.toml -p runx-receipts \
//!     --example generate_receipt_tree_oracle

// Fixture/oracle generator tool: failing loud on a construction error and
// printing progress is intended, so the workspace unwrap/print bans are lifted.
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use std::fs;
use std::path::Path;

use runx_contracts::{
    AuthorityAttenuation, ClosureDisposition, Lineage, RECEIPT_CANONICALIZATION, Receipt,
    ReceiptAuthority, ReceiptEnforcement, ReceiptIdempotency, ReceiptIssuer, ReceiptIssuerType,
    ReceiptSchema, ReceiptSignature, Reference, ReferenceType, Seal, SignatureAlgorithm, Subject,
    receipt_subject_kind,
};
use serde_json::{Value, json};

fn main() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let path = root_dir.join("fixtures/runtime/receipt-tree/oracle.json");

    let receipts = json!({
        "child_a": rec("hrn_rcpt_child_a", &[], None),
        "child_b": rec("hrn_rcpt_child_b", &[], None),
        "child_a_to_b": rec("hrn_rcpt_child_a", &["hrn_rcpt_child_b"], None),
        "child_a_duplicate": rec("hrn_rcpt_child_a", &[], None),
        "child_a_self_cycle": rec("hrn_rcpt_child_a", &["hrn_rcpt_child_a"], None),
        "child_a_wrong_parent": rec("hrn_rcpt_child_a", &[], Some("runx:receipt:other")),
        "root_empty": rec("hrn_rcpt_root", &[], None),
        "root_to_a": rec("hrn_rcpt_root", &["hrn_rcpt_child_a"], None),
        "root_to_a_b": rec("hrn_rcpt_root", &["hrn_rcpt_child_a", "hrn_rcpt_child_b"], None),
        "root_malformed_uri": rec_raw_child("hrn_rcpt_root", "hrn_rcpt_child_a"),
        "root_wrong_namespace": rec_wrong_ns_child("hrn_rcpt_root", "hrn_rcpt_child_a"),
    });

    let cases = json!([
        case(
            "positive-nested",
            "root_to_a",
            &["child_a_to_b", "child_b"],
            &[],
            cfg(64, 1024, false),
            true,
            &[]
        ),
        case(
            "positive-fanout",
            "root_to_a_b",
            &["child_a", "child_b"],
            &[],
            cfg(64, 1024, false),
            true,
            &[]
        ),
        case(
            "duplicate-id",
            "root_empty",
            &["child_a", "child_a_duplicate"],
            &[],
            cfg(64, 1024, false),
            false,
            &[
                ("DuplicateChildReceipt", "children[1].id"),
                ("OrphanChildReceipt", "children[0].id"),
                ("OrphanChildReceipt", "children[1].id"),
            ]
        ),
        case(
            "missing-child",
            "root_to_a",
            &[],
            &[],
            cfg(64, 1024, false),
            false,
            &[("ChildReceiptMissing", "lineage.children[0]"),]
        ),
        case(
            "resolver-error",
            "root_to_a",
            &[],
            &["hrn_rcpt_child_a"],
            cfg(64, 1024, false),
            false,
            &[("ChildReceiptResolverError", "lineage.children[0]"),]
        ),
        case(
            "malformed-uri",
            "root_malformed_uri",
            &["child_a"],
            &[],
            cfg(64, 1024, false),
            false,
            &[
                ("ChildReceiptRefMalformed", "lineage.children[0]"),
                ("OrphanChildReceipt", "children[0].id"),
            ]
        ),
        case(
            "wrong-namespace",
            "root_wrong_namespace",
            &["child_a"],
            &[],
            cfg(64, 1024, false),
            false,
            &[
                ("ChildReceiptRefMalformed", "lineage.children[0]"),
                ("OrphanChildReceipt", "children[0].id"),
            ]
        ),
        case(
            "ambiguous-id",
            "root_to_a",
            &["child_a", "child_a_duplicate"],
            &[],
            cfg(64, 1024, false),
            false,
            &[
                ("DuplicateChildReceipt", "children[1].id"),
                ("ChildReceiptAmbiguous", "lineage.children[0]"),
                ("OrphanChildReceipt", "children[0].id"),
                ("OrphanChildReceipt", "children[1].id"),
            ]
        ),
        case(
            "cycle",
            "root_to_a",
            &["child_a_self_cycle"],
            &[],
            cfg(64, 1024, false),
            false,
            &[("ChildReceiptCycle", "children[0].lineage.children[0]"),]
        ),
        case(
            "orphan",
            "root_empty",
            &["child_a"],
            &[],
            cfg(64, 1024, false),
            false,
            &[("OrphanChildReceipt", "children[0].id"),]
        ),
        case(
            "wrong-parent",
            "root_to_a",
            &["child_a_wrong_parent"],
            &[],
            cfg(64, 1024, true),
            false,
            &[(
                "ChildReceiptParentMismatch",
                "lineage.children[0].lineage.parent"
            ),]
        ),
        case(
            "depth-limit",
            "root_to_a",
            &["child_a_to_b", "child_b"],
            &[],
            cfg(1, 1024, false),
            false,
            &[
                ("ChildReceiptDepthLimit", "children[0].lineage.children[0]"),
                ("OrphanChildReceipt", "children[1].id"),
            ]
        ),
        case(
            "breadth-limit",
            "root_to_a_b",
            &["child_a", "child_b"],
            &[],
            cfg(64, 1, false),
            false,
            &[
                ("ChildReceiptBreadthLimit", "lineage.children"),
                ("OrphanChildReceipt", "children[1].id"),
            ]
        ),
    ]);

    let oracle = json!({
        "schema": "runx.receipt_tree_oracle.v1",
        "receipts": receipts,
        "cases": cases,
    });
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&oracle).unwrap()),
    )
    .unwrap();
    println!("regenerated receipt-tree oracle");
}

fn cfg(max_depth: usize, max_breadth: usize, require_parent_links: bool) -> Value {
    json!({
        "max_depth": max_depth,
        "max_breadth": max_breadth,
        "require_parent_links": require_parent_links,
    })
}

#[allow(clippy::too_many_arguments)]
fn case(
    name: &str,
    root_receipt: &str,
    children: &[&str],
    resolver_error_receipt_ids: &[&str],
    config: Value,
    valid: bool,
    findings: &[(&str, &str)],
) -> Value {
    json!({
        "name": name,
        "root_receipt": root_receipt,
        "supplied_child_receipts": children,
        "resolver_error_receipt_ids": resolver_error_receipt_ids,
        "config": config,
        "expected": {
            "valid": valid,
            "findings": findings
                .iter()
                .map(|(code, path)| json!({"code": code, "path": path}))
                .collect::<Vec<_>>(),
        },
    })
}

fn rec(id: &str, child_ids: &[&str], parent_uri: Option<&str>) -> Value {
    let mut receipt = base(id);
    let lineage = receipt.lineage.get_or_insert_with(Default::default);
    lineage.children = child_ids
        .iter()
        .map(|cid| Reference::runx(ReferenceType::Receipt, cid))
        .collect();
    if let Some(parent) = parent_uri {
        lineage.parent = Some(Reference::with_uri(ReferenceType::Receipt, parent));
    }
    serde_json::to_value(receipt).unwrap()
}

fn rec_raw_child(id: &str, child_id: &str) -> Value {
    let mut receipt = base(id);
    receipt
        .lineage
        .get_or_insert_with(Default::default)
        .children = vec![Reference {
        // Suffix-only uri: typed receipt ref but not the canonical runx:receipt: scheme.
        ..Reference::with_uri(ReferenceType::Receipt, child_id)
    }];
    serde_json::to_value(receipt).unwrap()
}

fn rec_wrong_ns_child(id: &str, child_id: &str) -> Value {
    let mut receipt = base(id);
    receipt
        .lineage
        .get_or_insert_with(Default::default)
        .children = vec![Reference::with_uri(
        ReferenceType::Receipt,
        format!("runx:graph_receipt:{child_id}"),
    )];
    serde_json::to_value(receipt).unwrap()
}

fn base(id: &str) -> Receipt {
    Receipt {
        schema: ReceiptSchema::V1,
        id: id.into(),
        created_at: "2026-05-22T00:00:00Z".into(),
        canonicalization: RECEIPT_CANONICALIZATION.into(),
        issuer: ReceiptIssuer {
            issuer_type: ReceiptIssuerType::Local,
            kid: "fixture-key".into(),
            public_key_sha256: format!("sha256:{}", "0".repeat(64)).into(),
        },
        signature: ReceiptSignature {
            alg: SignatureAlgorithm::Ed25519,
            value: "sig:pending".into(),
        },
        digest: format!("sha256:{}", "9".repeat(64)).into(),
        idempotency: ReceiptIdempotency {
            intent_key: format!("sha256:{}", "1".repeat(64)).into(),
            trigger_fingerprint: format!("sha256:{}", "2".repeat(64)).into(),
            content_hash: format!("sha256:{}", "3".repeat(64)).into(),
        },
        class: runx_contracts::ReceiptClass::Executed,
        subject: Subject {
            kind: receipt_subject_kind::SKILL.into(),
            reference: Reference::runx(ReferenceType::Harness, id),
            input_context: None,
            commitments: Vec::new(),
            paid_invocation: None,
        },
        authority: ReceiptAuthority {
            actor_ref: Reference::runx(ReferenceType::Principal, "local_runtime"),
            authority_proof_refs: Vec::new(),
            grant_refs: Vec::new(),
            scope_refs: Vec::new(),
            terms: Vec::new(),
            derivations: Vec::new(),
            attenuation: AuthorityAttenuation {
                parent_authority_ref: None,
                subset_proof: None,
            },
            mandate_ref: None,
            enforcement: ReceiptEnforcement {
                profile_hash: format!("sha256:{}", "5".repeat(64)).into(),
                execution_boundary: None,
                redaction_refs: Vec::new(),
                setup_refs: Vec::new(),
                teardown_refs: Vec::new(),
            },
        },
        signals: Vec::new(),
        decisions: Vec::new(),
        acts: Vec::new(),
        evidence: Vec::new(),
        seal: Seal {
            disposition: ClosureDisposition::Closed,
            reason_code: "closed".into(),
            summary: "closed".into(),
            closed_at: "2026-05-22T00:00:00Z".into(),
            last_observed_at: "2026-05-22T00:00:00Z".into(),
            criteria: Vec::new(),
        },
        lineage: Some(Lineage::default()),
        metadata: None,
    }
}
