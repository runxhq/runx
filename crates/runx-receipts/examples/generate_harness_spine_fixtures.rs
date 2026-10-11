//! Regenerates the flat `runx.receipt.v1` harness-spine fixtures and the
//! canonical-json oracle. Run with:
//!   cargo run --manifest-path crates/Cargo.toml -p runx-receipts \
//!     --example generate_harness_spine_fixtures

// Fixture/oracle generator tool: failing loud on a construction error and
// printing progress is intended, so the workspace unwrap/print bans are lifted.
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use std::fs;
use std::path::Path;

use runx_contracts::connector::Digest;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ActForm, AuthorityBounds, AuthorityResourceFamily, AuthorityTerm, AuthorityVerb,
    ClosureDisposition, Criterion, CriterionBinding, CriterionStatus, Decision, DecisionChoice,
    ExecutionBoundaryKind, ExecutionBoundaryObservation, Intent, JsonValue, Lineage,
    RECEIPT_CANONICALIZATION, Receipt, ReceiptAct, ReceiptAuthority, ReceiptCanonicalization,
    ReceiptEnforcement, ReceiptIssuer, ReceiptIssuerType, ReceiptSchema, ReceiptSignature,
    Reference, ReferenceType, Seal, SignatureAlgorithm, Subject, authority_term_legitimacy,
    canonical_stable_json, receipt_subject_kind,
};
use runx_receipts::{
    canonical_receipt_body_digest, canonical_receipt_digest, canonical_receipt_json,
};
use serde_json::{Value, json};

#[path = "../tests/fixture_key.rs"]
mod fixture_key;

use fixture_key::{FIXTURE_SEED, FixtureKey};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let spine = root.join("fixtures/contracts/harness-spine");
    let canonical = root.join("fixtures/contracts/canonical-json");

    let success = sealed(success_receipt());
    let abnormal = sealed(abnormal_receipt());

    write_fixture(
        &spine.join("receipt-success.json"),
        "receipt_success",
        "Sealed runx.receipt.v1 for a successful skill run.",
        "receipt",
        &success,
    );
    write_fixture(
        &spine.join("receipt-abnormal.json"),
        "receipt_abnormal",
        "Sealed runx.receipt.v1 for a failed skill run.",
        "receipt",
        &abnormal,
    );
    let oracle = json!({
        "schema": "runx.canonical_json_oracle.v1",
        "canonicalization": RECEIPT_CANONICALIZATION,
        "cases": [
            oracle_case("receipt-success", "harness-spine/receipt-success.json", &success),
            oracle_case("receipt-abnormal", "harness-spine/receipt-abnormal.json", &abnormal),
        ],
    });
    write_canonical_json(&canonical.join("runx-receipt-c14n-v1.oracles.json"), oracle);

    println!("regenerated harness-spine fixtures + receipt c14n oracle");
}

fn write_fixture(path: &Path, name: &str, description: &str, kind: &str, receipt: &Receipt) {
    let wrapper = json!({
        "fixture_kind": kind,
        "name": name,
        "description": description,
        "scope": "harness-spine",
        "expected": serde_json::to_value(receipt).unwrap(),
    });
    write_canonical_json(path, wrapper);
}

fn write_canonical_json(path: &Path, value: Value) {
    let value = serde_json::from_value::<JsonValue>(value).unwrap();
    fs::write(
        path,
        format!("{}\n", canonical_stable_json(&value).unwrap()),
    )
    .unwrap();
}

fn oracle_case(name: &str, fixture: &str, receipt: &Receipt) -> Value {
    json!({
        "name": name,
        "fixture": fixture,
        "full_canonical_json": canonical_receipt_json(receipt).unwrap(),
        "full_sha256": canonical_receipt_digest(receipt).unwrap(),
        "body_canonical_json": runx_receipts::canonical_receipt_body_json(receipt).unwrap(),
        "body_sha256": canonical_receipt_body_digest(receipt).unwrap(),
    })
}

/// Sealed under the harness's fixture identity, as a replayed run is.
fn sealed(mut receipt: Receipt) -> Receipt {
    FixtureKey::local(FIXTURE_SEED).seal(&mut receipt).unwrap();
    receipt
}

fn base(id: &str, kind: NonEmptyString, subject_id: &str) -> Receipt {
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
        digest: "sha256:pending".into(),
        subject: Subject {
            kind,
            reference: Reference::runx(ReferenceType::Run, subject_id),
            // The digest of the subject's outcome: its status, declared
            // outputs and direct children.
            outcome: Digest::of(subject_id.as_bytes()),
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
            reason_code: "step_closed".into(),
            criteria: Vec::new(),
        },
        lineage: Lineage::default(),
        metadata: None,
    }
}

/// The execution term admission derived for the call.
const TERM_ID: &str = "term_echo";

/// The authority a governed call is admitted under: its grant, the
/// execution term its act's legitimacy names, and the provider boundary the
/// call ran across.
fn call_authority(receipt: &mut Receipt) {
    let principal = Reference::runx(ReferenceType::Principal, "operator");
    receipt.authority.enforcement.execution_boundary = Some(ExecutionBoundaryObservation {
        kind: ExecutionBoundaryKind::RemoteProvider,
    });
    receipt.authority.actor_ref = Some(principal.clone());
    receipt.authority.grant_refs = vec![Reference::runx(ReferenceType::Grant, "grant_echo")];
    receipt.authority.terms = vec![AuthorityTerm {
        term_id: TERM_ID.into(),
        principal_ref: principal,
        resource_ref: Reference::runx(ReferenceType::ConnectorInstance, "github.default"),
        resource_family: AuthorityResourceFamily::Connector,
        verbs: vec![AuthorityVerb::Write],
        bounds: AuthorityBounds {
            operations: vec!["github/comments.create".into()],
            scopes: vec!["issues:write".into()],
            targets: vec!["github://acme/app".into()],
            ..AuthorityBounds::default()
        },
        conditions: Vec::new(),
        approvals: Vec::new(),
        capabilities: Vec::new(),
        expires_at: None,
        issued_by_ref: Reference::runx(ReferenceType::Host, "runx-kernel"),
        credential_ref: None,
    }];
}

/// A governed call's intent: its purpose and the execution term admission
/// derived for it.
fn call_intent(purpose: &str) -> Intent {
    Intent {
        purpose: purpose.into(),
        legitimacy: authority_term_legitimacy(TERM_ID).into(),
    }
}

fn outcome_binding(status: CriterionStatus) -> CriterionBinding {
    CriterionBinding {
        criterion_id: Criterion::StepOutcome,
        status,
        evidence_refs: Vec::new(),
        verification_refs: Vec::new(),
    }
}

/// The act a governed call seals, with the outcome check bound to it.
struct CallOutcome<'a> {
    form: ActForm,
    purpose: &'a str,
    reason: Option<&'a str>,
    target_refs: Vec<Reference>,
    artifact_refs: Vec<Reference>,
    context_ref: Option<Reference>,
    status: CriterionStatus,
}

fn call_act(call: CallOutcome<'_>) -> ReceiptAct {
    ReceiptAct {
        id: "act_echo".into(),
        form: call.form,
        intent: call_intent(call.purpose),
        reason: call.reason.map(Into::into),
        criterion_bindings: vec![outcome_binding(call.status)],
        target_refs: call.target_refs,
        artifact_refs: call.artifact_refs,
        context_ref: call.context_ref,
    }
}

/// The decision a call's caller states, carried out by its act.
fn stated_decision(choice: DecisionChoice, act: &ReceiptAct) -> Decision {
    Decision {
        decision_id: "dec_echo".into(),
        choice,
        selected_act_id: act.id.clone(),
    }
}

/// The operator context a run was prepared with, as the runtime references it.
fn operator_context_ref() -> Reference {
    let digest = format!("sha256:{}", "7".repeat(64));
    Reference {
        provider: Some("runx".into()),
        locator: Some(digest.clone().into()),
        label: Some("prepared operator context".into()),
        ..Reference::with_uri(
            ReferenceType::Artifact,
            format!("runx:artifact:operator_context:{digest}"),
        )
    }
}

/// A governed call whose caller stated a reply that continues the thread.
fn success_receipt() -> Receipt {
    let mut receipt = base(
        "rcpt_echo_success",
        receipt_subject_kind::SKILL.into(),
        "echo_success",
    );
    let act = call_act(CallOutcome {
        form: ActForm::Reply,
        purpose: "Answer the question on the thread.",
        reason: Some("The reply names the release that fixes the crash."),
        target_refs: vec![Reference::with_uri(
            ReferenceType::Thread,
            "github://acme/app/issues/7".to_owned(),
        )],
        artifact_refs: vec![Reference {
            provider: Some("github".into()),
            ..Reference::with_uri(
                ReferenceType::Comment,
                "github://acme/app/issues/comments/1".to_owned(),
            )
        }],
        context_ref: Some(operator_context_ref()),
        status: CriterionStatus::Verified,
    });
    call_authority(&mut receipt);
    receipt.decisions = vec![stated_decision(DecisionChoice::Continue, &act)];
    receipt.acts = vec![act];
    receipt.seal.criteria = vec![outcome_binding(CriterionStatus::Verified)];
    receipt
}

/// A governed call whose caller stated no act: an observation whose purpose
/// is its operation's declared description.
fn abnormal_receipt() -> Receipt {
    let mut receipt = base(
        "rcpt_echo_abnormal",
        receipt_subject_kind::SKILL.into(),
        "echo_abnormal",
    );
    call_authority(&mut receipt);
    receipt.acts = vec![call_act(CallOutcome {
        form: ActForm::Observation,
        purpose: "Echo the message back.",
        reason: None,
        target_refs: Vec::new(),
        artifact_refs: Vec::new(),
        context_ref: None,
        status: CriterionStatus::Failed,
    })];
    receipt.seal.disposition = ClosureDisposition::Failed;
    receipt.seal.reason_code = "step_failed".into();
    receipt.seal.criteria = vec![outcome_binding(CriterionStatus::Failed)];
    receipt
}
