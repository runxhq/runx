//! Regenerates the flat `runx.receipt.v1` receipt-tree oracle. Every receipt
//! is sealed under the public fixture key, and every parent names each child
//! by its content address and digest, so a case fails only where it means to.
//! Run with:
//!   cargo run --manifest-path crates/Cargo.toml -p runx-receipts \
//!     --example generate_receipt_tree_oracle

// Fixture/oracle generator tool: failing loud on a construction error and
// printing progress is intended, so the workspace unwrap/print bans are lifted.
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use std::fs;
use std::path::Path;

use runx_contracts::{
    Lineage, Receipt, ReceiptAuthority, ReceiptCanonicalization, ReceiptEnforcement, ReceiptIssuer,
    ReceiptIssuerType, ReceiptSchema, ReceiptSignature, Reference, ReferenceType, RunOutcome, Seal,
    SignatureAlgorithm, Subject, SubjectKind,
};
use serde_json::{Value, json};

#[path = "../tests/fixture_key.rs"]
mod fixture_key;

use fixture_key::{FIXTURE_SEED, FixtureKey};

fn main() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let path = root_dir.join("fixtures/runtime/receipt-tree/oracle.json");
    let key = FixtureKey::local(FIXTURE_SEED);

    let child_a = sealed(&key, "child_a", Vec::new());
    let child_b = sealed(&key, "child_b", Vec::new());
    let child_c = sealed(&key, "child_c", Vec::new());
    let child_a_to_b = sealed(&key, "child_a_to_b", vec![link(&child_b)]);
    let child_a_to_c = sealed(&key, "child_a_to_c", vec![link(&child_c)]);
    let child_b_to_c = sealed(&key, "child_b_to_c", vec![link(&child_c)]);
    let child_cycle = self_cycle(&key);
    let receipts = [
        ("child_a", child_a.clone()),
        ("child_b", child_b.clone()),
        ("child_c", child_c),
        ("child_a_to_b", child_a_to_b.clone()),
        ("child_a_to_c", child_a_to_c.clone()),
        ("child_b_to_c", child_b_to_c.clone()),
        ("child_cycle", child_cycle.clone()),
        ("root_empty", sealed(&key, "root_empty", Vec::new())),
        ("root_to_a", sealed(&key, "root_to_a", vec![link(&child_a)])),
        (
            "root_to_a_b",
            sealed(&key, "root_to_a_b", vec![link(&child_a), link(&child_b)]),
        ),
        (
            "root_to_a_to_b",
            sealed(&key, "root_to_a_to_b", vec![link(&child_a_to_b)]),
        ),
        (
            "root_to_shared",
            sealed(
                &key,
                "root_to_shared",
                vec![link(&child_a_to_c), link(&child_b_to_c)],
            ),
        ),
        (
            "root_to_cycle",
            sealed(&key, "root_to_cycle", vec![link(&child_cycle)]),
        ),
        (
            "root_malformed_uri",
            sealed(
                &key,
                "root_malformed_uri",
                // The child's bare id: a typed receipt ref without the
                // canonical runx:receipt: scheme.
                vec![Reference::with_uri(
                    ReferenceType::Receipt,
                    child_a.id.as_str(),
                )],
            ),
        ),
        (
            "root_wrong_namespace",
            sealed(
                &key,
                "root_wrong_namespace",
                vec![Reference::with_uri(
                    ReferenceType::Receipt,
                    format!("runx:run:{}", child_a.id),
                )],
            ),
        ),
    ];
    let receipts = receipts
        .into_iter()
        .map(|(name, receipt)| (name.to_owned(), serde_json::to_value(receipt).unwrap()))
        .collect::<serde_json::Map<_, _>>();

    let cases = json!([
        case(
            "positive-leaf",
            "root_empty",
            &[],
            &[],
            cfg(64, 1024),
            true,
            &[]
        ),
        case(
            "positive-nested",
            "root_to_a_to_b",
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
            &["child_a", "child_a"],
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
            &[child_a.id.as_str()],
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
            &["child_a", "child_a"],
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
            "root_to_cycle",
            &["child_cycle"],
            &[],
            cfg(64, 1024),
            false,
            &[
                // A receipt naming itself cannot carry its own content
                // address, so the cycle comes with a forged id.
                ("ContentAddressMismatch", "children[0].id"),
                ("ChildReceiptCycle", "children[0].lineage.children[0]"),
            ]
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
            "root_to_a_to_b",
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
            "count-limit",
            "root_to_a_b",
            &["child_a", "child_b"],
            &[],
            cfg(64, 2),
            false,
            &[
                ("ChildReceiptCountLimit", "lineage.children[1]"),
                ("OrphanChildReceipt", "children[1].id"),
            ]
        ),
        case(
            "shared-child",
            "root_to_shared",
            &["child_a_to_c", "child_b_to_c", "child_c"],
            &[],
            cfg(64, 1024),
            false,
            &[("ChildReceiptShared", "children[1].lineage.children[0]"),]
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

fn cfg(max_depth: usize, max_receipts: usize) -> Value {
    json!({
        "max_depth": max_depth,
        "max_receipts": max_receipts,
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

/// The ref a parent signs for `child`: its content address and its digest.
fn link(child: &Receipt) -> Reference {
    Reference {
        locator: Some(child.digest.clone()),
        ..Reference::runx(ReferenceType::Receipt, &child.id)
    }
}

/// A receipt, distinct by `label`, sealed with `children` in its lineage.
fn sealed(key: &FixtureKey, label: &str, children: Vec<Reference>) -> Receipt {
    let mut receipt = base(label);
    receipt.lineage.children = children;
    key.seal(&mut receipt).unwrap();
    receipt
}

/// A receipt whose lineage names its own id. Its id is the content address
/// of the body before the self-reference, re-signed as a forger would.
fn self_cycle(key: &FixtureKey) -> Receipt {
    let mut receipt = sealed(key, "child_cycle", Vec::new());
    receipt.lineage.children = vec![Reference::runx(ReferenceType::Receipt, &receipt.id)];
    key.sign_keeping_id(&mut receipt).unwrap();
    receipt
}

fn base(label: &str) -> Receipt {
    Receipt {
        schema: ReceiptSchema::V1,
        id: label.into(),
        created_at: "2026-05-22T00:00:00Z".into(),
        canonicalization: ReceiptCanonicalization::C14nV1,
        issuer: ReceiptIssuer {
            issuer_type: ReceiptIssuerType::Local,
            kid: "fixture-key".into(),
            public_key_digest: format!("sha256:{}", "0".repeat(64)).into(),
            runx_version: "0.11.0".into(),
        },
        signature: ReceiptSignature {
            alg: SignatureAlgorithm::Ed25519,
            value: "unsealed".into(),
        },
        digest: format!("sha256:{}", "9".repeat(64)).into(),
        subject: Subject {
            kind: SubjectKind::Step,
            reference: Reference::runx(ReferenceType::Run, label),
            outcome_digest: runx_contracts::connector::Digest::of(label.as_bytes()),
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
            outcome: RunOutcome::Completed,
            reason: None,
            criteria: Vec::new(),
        },
        lineage: Lineage::default(),
    }
}
