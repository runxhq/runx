//! Regenerates the flat `runx.receipt.v1` harness-spine fixtures and the
//! canonical-json oracle. Run with:
//!   cargo run --manifest-path crates/Cargo.toml -p runx-receipts \
//!     --example generate_harness_spine_fixtures

// Fixture/oracle generator tool: failing loud on a construction error and
// printing progress is intended, so the workspace unwrap/print bans are lifted.
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use std::fs;
use std::path::Path;

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ActForm, AuthorityAttenuation, AuthorityBounds, AuthorityResourceFamily, AuthorityTerm,
    AuthorityVerb, Closure, ClosureDisposition, CriterionBinding, CriterionStatus, Decision,
    DecisionChoice, DecisionInputs, DecisionJustification, HashAlgorithm, Intent, JsonValue,
    Lineage, RECEIPT_CANONICALIZATION, Receipt, ReceiptAct, ReceiptAuthority, ReceiptCommitment,
    ReceiptCommitmentScope, ReceiptEnforcement, ReceiptIdempotency, ReceiptInputContext,
    ReceiptIssuer, ReceiptIssuerType, ReceiptSchema, ReceiptSignature, Reference, ReferenceType,
    Seal, SignatureAlgorithm, Subject, SuccessCriterion, authority_term_legitimacy,
    canonical_stable_json, receipt_subject_kind,
};
use runx_receipts::{
    canonical_receipt_body_digest, canonical_receipt_digest, canonical_receipt_json,
    content_addressed_receipt_id,
};
use serde_json::{Value, json};

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

fn sealed(mut receipt: Receipt) -> Receipt {
    receipt.id = content_addressed_receipt_id(&receipt).unwrap().into();
    let digest = canonical_receipt_body_digest(&receipt).unwrap();
    receipt.digest = digest.clone().into();
    receipt.signature.value = format!("sig:{digest}").into();
    receipt
}

fn base(id: &str, kind: NonEmptyString, subject_id: &str) -> Receipt {
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
        digest: "sha256:pending".into(),
        idempotency: ReceiptIdempotency {
            intent_key: format!("sha256:{}", "1".repeat(64)).into(),
            trigger_fingerprint: format!("sha256:{}", "2".repeat(64)).into(),
            content_hash: format!("sha256:{}", "3".repeat(64)).into(),
        },
        class: runx_contracts::ReceiptClass::Executed,
        subject: Subject {
            kind,
            reference: Reference::runx(ReferenceType::Run, subject_id),
            input_context: Some(ReceiptInputContext {
                source: format!("runx:signal:{subject_id}").into(),
                preview: format!("Run {subject_id}"),
                value_hash: format!("sha256:{}", "6".repeat(64)).into(),
            }),
            commitments: vec![ReceiptCommitment {
                scope: ReceiptCommitmentScope::Output,
                algorithm: HashAlgorithm::Sha256,
                value: format!("sha256:{}", "4".repeat(64)).into(),
                canonicalization: "runx.stable-json.v1".into(),
            }],
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
            reason_code: "step_closed".into(),
            summary: "step completed successfully".into(),
            closed_at: "2026-05-22T00:00:00Z".into(),
            last_observed_at: "2026-05-22T00:00:00Z".into(),
            criteria: Vec::new(),
        },
        lineage: Some(Lineage::default()),
        metadata: None,
    }
}

const CREATED_AT: &str = "2026-05-22T00:00:00Z";

/// The criterion every governed call's outcome is judged by.
const OUTCOME_CRITERION: &str = "step_outcome";

/// The execution term admission derived for the call.
const TERM_ID: &str = "term_echo";

/// The authority a governed call is admitted under: its grant, and the
/// execution term its act's legitimacy names.
fn call_authority(receipt: &mut Receipt) {
    let principal = Reference::runx(ReferenceType::Principal, "operator");
    receipt.authority.actor_ref = principal.clone();
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

/// A governed call's intent: its purpose, the execution term admission
/// derived for it, and the outcome criterion.
fn call_intent(purpose: &str) -> Intent {
    Intent {
        purpose: purpose.into(),
        legitimacy: authority_term_legitimacy(TERM_ID).into(),
        success_criteria: vec![SuccessCriterion {
            criterion_id: OUTCOME_CRITERION.into(),
            statement: "runtime step completes successfully".into(),
            required: true,
        }],
        constraints: Vec::new(),
        derived_from: Vec::new(),
    }
}

fn outcome_binding(status: CriterionStatus, outcome: &str) -> CriterionBinding {
    CriterionBinding {
        criterion_id: OUTCOME_CRITERION.into(),
        status,
        evidence_refs: Vec::new(),
        verification_refs: Vec::new(),
        summary: Some(outcome.into()),
    }
}

/// The act a governed call seals, closed with its outcome.
struct CallOutcome<'a> {
    form: ActForm,
    purpose: &'a str,
    summary: &'a str,
    target_refs: Vec<Reference>,
    artifact_refs: Vec<Reference>,
    context_ref: Option<Reference>,
    status: CriterionStatus,
    disposition: ClosureDisposition,
    outcome: &'a str,
}

fn call_act(call: CallOutcome<'_>) -> ReceiptAct {
    ReceiptAct {
        id: "act_echo".into(),
        form: call.form,
        intent: call_intent(call.purpose),
        summary: call.summary.into(),
        criterion_bindings: vec![outcome_binding(call.status, call.outcome)],
        by: None,
        source_refs: Vec::new(),
        target_refs: call.target_refs,
        artifact_refs: call.artifact_refs,
        context_ref: call.context_ref,
        closure: Closure {
            disposition: call.disposition,
            reason_code: OUTCOME_CRITERION.into(),
            summary: call.outcome.into(),
            closed_at: CREATED_AT.into(),
        },
        revision: None,
        verification: None,
    }
}

/// The decision a call's caller states, carried out by its act.
fn stated_decision(choice: DecisionChoice, act: &ReceiptAct) -> Decision {
    Decision {
        decision_id: "dec_echo".into(),
        choice,
        inputs: DecisionInputs {
            target_ref: act.target_refs.first().cloned(),
            ..DecisionInputs::default()
        },
        proposed_intent: act.intent.clone(),
        selected_act_id: Some(act.id.clone()),
        selected_harness_ref: None,
        justification: DecisionJustification {
            summary: act.summary.clone(),
            evidence_refs: Vec::new(),
        },
        closure: None,
        artifact_refs: act.artifact_refs.clone(),
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
        summary: "The reply names the release that fixes the crash.",
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
        disposition: ClosureDisposition::Closed,
        outcome: "step completed successfully",
    });
    call_authority(&mut receipt);
    receipt.decisions = vec![stated_decision(DecisionChoice::Continue, &act)];
    receipt.acts = vec![act];
    receipt.seal.criteria = vec![outcome_binding(
        CriterionStatus::Verified,
        "step completed successfully",
    )];
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
        summary: "Echo the message back.",
        target_refs: Vec::new(),
        artifact_refs: Vec::new(),
        context_ref: None,
        status: CriterionStatus::Failed,
        disposition: ClosureDisposition::Failed,
        outcome: "echo exited with status 1",
    })];
    receipt.seal.disposition = ClosureDisposition::Failed;
    receipt.seal.reason_code = "step_failed".into();
    receipt.seal.summary = "echo exited with status 1".into();
    receipt.seal.criteria = vec![outcome_binding(
        CriterionStatus::Failed,
        "echo exited with status 1",
    )];
    receipt
}
