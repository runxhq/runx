//! Regenerates the flat `runx.receipt.v1` receipt-tree oracle. Run with:
//!   cargo run --manifest-path crates/Cargo.toml -p runx-receipts \
//!     --example generate_receipt_tree_oracle

// Fixture/oracle generator tool: failing loud on a construction error and
// printing progress is intended, so the workspace unwrap/print bans are lifted.
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use std::fs;
use std::path::Path;

use runx_contracts::{
    ClosureDisposition, Lineage, Receipt, ReceiptAuthority, ReceiptCanonicalization,
    ReceiptEnforcement, ReceiptIssuer, ReceiptIssuerType, ReceiptSchema, ReceiptSignature,
    Reference, ReferenceType, Seal, SignatureAlgorithm, Subject, SubjectKind,
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
        "child_a": rec("rcpt_child_a", &[]),
        "child_b": rec("rcpt_child_b", &[]),
        "child_a_to_b": rec("rcpt_child_a", &["rcpt_child_b"]),
        "child_a_duplicate": rec("rcpt_child_a", &[]),
        "child_a_self_cycle": rec("rcpt_child_a", &["rcpt_child_a"]),
        "root_empty": rec("rcpt_root", &[]),
        "root_to_a": rec("rcpt_root", &["rcpt_child_a"]),
        "root_to_a_b": rec("rcpt_root", &["rcpt_child_a", "rcpt_child_b"]),
        "root_malformed_uri": rec_raw_child("rcpt_root", "rcpt_child_a"),
        "root_wrong_namespace": rec_wrong_ns_child("rcpt_root", "rcpt_child_a"),
    });

    let cases = json!([
        case(
            "positive-nested",
            "root_to_a",
            &["child_a_to_b", "child_b"],
            &[],
            cfg(64, 1024),
            true,
            &[]
        ),
        case(
            "positive-fanout",
            "root_to_a_b",
            &["child_a", "child_b"],
            &[],
            cfg(64, 1024),
            true,
            &[]
        ),
        case(
            "duplicate-id",
            "root_empty",
            &["child_a", "child_a_duplicate"],
            &[],
            cfg(64, 1024),
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
            cfg(64, 1024),
            false,
            &[("ChildReceiptMissing", "lineage.children[0]"),]
        ),
        case(
            "resolver-error",
            "root_to_a",
            &[],
            &["rcpt_child_a"],
            cfg(64, 1024),
            false,
            &[("ChildReceiptResolverError", "lineage.children[0]"),]
        ),
        case(
            "malformed-uri",
            "root_malformed_uri",
            &["child_a"],
            &[],
            cfg(64, 1024),
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
            cfg(64, 1024),
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
            cfg(64, 1024),
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
            cfg(64, 1024),
            false,
            &[("ChildReceiptCycle", "children[0].lineage.children[0]"),]
        ),
        case(
            "orphan",
            "root_empty",
            &["child_a"],
            &[],
            cfg(64, 1024),
            false,
            &[("OrphanChildReceipt", "children[0].id"),]
        ),
        case(
            "depth-limit",
            "root_to_a",
            &["child_a_to_b", "child_b"],
            &[],
            cfg(1, 1024),
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
            cfg(64, 1),
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

fn cfg(max_depth: usize, max_breadth: usize) -> Value {
    json!({
        "max_depth": max_depth,
        "max_breadth": max_breadth,
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

fn rec(id: &str, child_ids: &[&str]) -> Value {
    let mut receipt = base(id);
    receipt.lineage.children = child_ids
        .iter()
        .map(|cid| Reference::runx(ReferenceType::Receipt, cid))
        .collect();
    serde_json::to_value(receipt).unwrap()
}

fn rec_raw_child(id: &str, child_id: &str) -> Value {
    let mut receipt = base(id);
    receipt.lineage.children = vec![Reference {
        // Suffix-only uri: typed receipt ref but not the canonical runx:receipt: scheme.
        ..Reference::with_uri(ReferenceType::Receipt, child_id)
    }];
    serde_json::to_value(receipt).unwrap()
}

fn rec_wrong_ns_child(id: &str, child_id: &str) -> Value {
    let mut receipt = base(id);
    receipt.lineage.children = vec![Reference::with_uri(
        ReferenceType::Receipt,
        format!("runx:run:{child_id}"),
    )];
    serde_json::to_value(receipt).unwrap()
}

fn base(id: &str) -> Receipt {
    Receipt {
        schema: ReceiptSchema::V1,
        id: id.into(),
        created_at: "2026-05-22T00:00:00Z".into(),
        canonicalization: ReceiptCanonicalization::C14nV1,
        issuer: ReceiptIssuer {
            issuer_type: ReceiptIssuerType::Local,
            kid: "fixture-key".into(),
            public_key_sha256: format!("sha256:{}", "0".repeat(64)).into(),
        },
        signature: ReceiptSignature {
            alg: SignatureAlgorithm::Ed25519,
            value: "unsealed".into(),
        },
        digest: format!("sha256:{}", "9".repeat(64)).into(),
        subject: Subject {
            kind: SubjectKind::Step,
            reference: Reference::runx(ReferenceType::Run, id),
            outcome: runx_contracts::connector::Digest::of(id.as_bytes()),
        },
        authority: ReceiptAuthority {
            actor_ref: None,
            grant_refs: Vec::new(),
            scope_refs: Vec::new(),
            terms: Vec::new(),
            derivations: Vec::new(),
            mandate_ref: None,
            enforcement: ReceiptEnforcement::default(),
        },
        decisions: Vec::new(),
        acts: Vec::new(),
        seal: Seal {
            disposition: ClosureDisposition::Closed,
            reason: None,
            criteria: Vec::new(),
        },
        lineage: Lineage::default(),
    }
}
