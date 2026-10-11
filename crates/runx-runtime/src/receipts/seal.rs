// Module rationale: receipt construction, explicit
// signature policy, and local proof sealing stay together until the runtime
// receipt builder is split out.
use std::collections::BTreeMap;

use crate::adapter::InvocationOutput;
use crate::observations::{ContractChecks, Observations};
use crate::{RuntimeError, StepRun};
use runx_contracts::connector::Digest;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ClosureDisposition, Criterion, CriterionBinding, CriterionStatus, Decision, DecisionChoice,
    EXECUTION_LIMITS_METADATA, ExecutionBoundaryObservation, JsonObject, JsonValue, Lineage,
    Receipt, ReceiptAct, ReceiptAuthority, ReceiptCanonicalization, ReceiptEnforcement,
    ReceiptIssuer, ReceiptSchema, Reference, ReferenceType, Seal, SignatureAlgorithm, Subject,
    canonical_stable_json, receipt_subject_kind,
};
use runx_receipts::{
    ReceiptProofContext, ReceiptProofContextProvider, ReceiptSignature, ReceiptTreeConfig,
    SignatureVerifier, canonical_receipt_body_digest, content_addressed_receipt_id,
};

use super::act::{ActOutcome, RuntimeAct, SealedCall};
use super::signing::{RuntimeReceiptSigner, RuntimeReceiptSigningError};

pub fn step_receipt(
    scope: RunScope<'_>,
    step_id: &str,
    attempt: u32,
    output: &InvocationOutput,
    claim: &JsonObject,
    created_at: &str,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Receipt, RuntimeError> {
    step_receipt_with_declared_claim_and_policy(
        StepReceiptWithDisposition::with_default_closure(
            scope, step_id, attempt, output, created_at,
        ),
        claim,
        signature_policy,
    )
}

pub(crate) struct StepReceiptWithDisposition<'a> {
    pub(crate) scope: RunScope<'a>,
    pub(crate) step_id: &'a str,
    pub(crate) attempt: u32,
    pub(crate) output: &'a InvocationOutput,
    pub(crate) created_at: &'a str,
    pub(crate) disposition: ClosureDisposition,
    pub(crate) reason_code: String,
}

struct StepReceiptSeal<'a> {
    params: StepReceiptWithDisposition<'a>,
    claim: &'a JsonObject,
    approval_refs: Vec<Reference>,
    child_receipts: &'a [Receipt],
    descendant_receipts: &'a [Receipt],
    authority: SealAuthority,
    call: Option<SealedCall>,
    receipt_metadata: Option<JsonObject>,
    signature_policy: RuntimeReceiptSignaturePolicy<'a>,
}

impl<'a> StepReceiptWithDisposition<'a> {
    /// A step-receipt request whose closure is derived from the output (the
    /// process-exit default): `Closed`/`Failed` by exit status and the
    /// matching reason code. This is the single source of that derivation for
    /// the process-exit step receipts.
    pub(crate) fn with_default_closure(
        scope: RunScope<'a>,
        step_id: &'a str,
        attempt: u32,
        output: &'a InvocationOutput,
        created_at: &'a str,
    ) -> Self {
        let StepSealClosure {
            disposition,
            reason_code,
        } = StepSealClosure::default_for(output);
        Self {
            scope,
            step_id,
            attempt,
            output,
            created_at,
            disposition,
            reason_code,
        }
    }
}

pub(crate) fn step_receipt_with_declared_claim_and_policy(
    params: StepReceiptWithDisposition<'_>,
    claim: &JsonObject,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Receipt, RuntimeError> {
    step_receipt_with_disposition_projection_authority_and_policy(StepReceiptSeal {
        params,
        claim,
        approval_refs: Vec::new(),
        child_receipts: &[],
        descendant_receipts: &[],
        authority: SealAuthority::default(),
        call: None,
        receipt_metadata: None,
        signature_policy,
    })
}

fn step_receipt_with_disposition_projection_authority_and_policy(
    request: StepReceiptSeal<'_>,
) -> Result<Receipt, RuntimeError> {
    let StepReceiptSeal {
        params,
        claim,
        approval_refs,
        child_receipts,
        descendant_receipts,
        authority,
        call,
        receipt_metadata,
        signature_policy,
    } = request;
    let StepReceiptWithDisposition {
        scope,
        step_id,
        attempt,
        output,
        created_at,
        disposition,
        reason_code,
    } = params;
    let evidence = step_evidence(&output.observations, approval_refs);
    let verification = contract_criteria(&output.observations.contract);
    // Only a governed call seals an act, and only a stated decision seals a
    // decision: a step's own outcome is its seal.
    let act = call.as_ref().map(|call| {
        RuntimeAct::for_call(step_id, call)
            .with_verified_criteria(verification.clone())
            .close(ActOutcome {
                succeeded: output.succeeded(),
                evidence_refs: &evidence.evidence_refs,
                verification_refs: &evidence.verification_refs,
            })
    });
    let decisions = call
        .as_ref()
        .and_then(SealedCall::decision)
        .zip(act.as_ref())
        .map(|(choice, act)| decision(step_id, choice, act))
        .into_iter()
        .collect();
    let mut seal_criteria = vec![step_outcome_criterion(output, &evidence)];
    seal_criteria.extend(verification);
    let seal = seal(disposition, reason_code, created_at, seal_criteria);
    let children = child_receipts
        .iter()
        .map(child_receipt_reference)
        .collect::<Vec<_>>();
    let outcome = outcome_digest(output.succeeded(), claim, &children)?;
    let mut receipt = build_unsealed_receipt(BuildReceipt {
        issuer: signature_policy.signer()?.issuer(),
        scope,
        node: Node::Step { step_id, attempt },
        kind: receipt_subject_kind::SKILL.into(),
        outcome,
        created_at,
        decisions,
        acts: act.into_iter().collect(),
        seal,
        children,
        authority,
        previous: call.as_ref().and_then(SealedCall::previous),
    });
    receipt.as_mut().authority.enforcement.execution_boundary = output
        .observations
        .boundary
        .map(|kind| ExecutionBoundaryObservation { kind });
    receipt.as_mut().metadata =
        receipt_metadata_with_execution_limits(receipt_metadata, &output.observations)?;
    let receipt = receipt.seal(signature_policy)?;
    if !child_receipts.is_empty() {
        validate_receipt_tree_with_policy(
            &receipt,
            child_receipts.iter().chain(descendant_receipts),
            signature_policy,
        )?;
    }
    Ok(receipt)
}

/// The runtime's ceilings for the invocation ride the receipt's unsigned
/// metadata, beside whatever runtime-authored metadata the step carries.
fn receipt_metadata_with_execution_limits(
    receipt_metadata: Option<JsonObject>,
    observations: &Observations,
) -> Result<Option<JsonObject>, RuntimeError> {
    let Some(limits) = &observations.limits else {
        return Ok(receipt_metadata);
    };
    let limits = serde_json::to_value(limits)
        .and_then(serde_json::from_value)
        .map_err(|source| RuntimeError::json("serializing execution limits", source))?;
    let mut metadata = receipt_metadata.unwrap_or_default();
    metadata.insert(EXECUTION_LIMITS_METADATA.to_owned(), limits);
    Ok(Some(metadata))
}

/// The single step-receipt seal. Every runtime step (regular skill, tool,
/// approval, agent act, replay, error) seals through here, so the act,
/// decision, and authority assembly lives in exactly one place. `closure` is
/// `None` for process-exit steps (the disposition is derived from the output)
/// and `Some` for steps that carry their own disposition, e.g. an agent act
/// that closes `Deferred` on a successful turn.
pub(crate) struct StepSeal<'a> {
    pub(crate) scope: RunScope<'a>,
    pub(crate) step_id: &'a str,
    pub(crate) attempt: u32,
    pub(crate) output: &'a InvocationOutput,
    /// The sealed contract claim (the step's declared outputs). Output
    /// identity binds to this claim, never to raw transport values.
    pub(crate) claim: &'a JsonObject,
    pub(crate) created_at: &'a str,
    pub(crate) authority: SealAuthority,
    /// The governed call the step made, for an operation step.
    pub(crate) call: Option<SealedCall>,
    /// Evidence of the policy approvals that let the step run.
    pub(crate) approval_refs: Vec<Reference>,
    /// Direct child receipts for a composed step such as a nested graph. The
    /// step receipt commits their ids and digests; their descendants remain
    /// reachable through the child receipt tree.
    pub(crate) child_receipts: &'a [Receipt],
    /// All receipts below `child_receipts`, supplied only to resolve and
    /// validate the complete tree. They are not direct children of this step.
    pub(crate) descendant_receipts: &'a [Receipt],
    pub(crate) closure: Option<StepSealClosure>,
    /// Runtime-authored, public receipt metadata. Adapter-owned `output.metadata`
    /// is deliberately not copied wholesale across this trust boundary.
    pub(crate) receipt_metadata: Option<JsonObject>,
}

/// A step's own disposition and reason code when it does not derive them
/// from a process exit.
pub(crate) struct StepSealClosure {
    pub(crate) disposition: ClosureDisposition,
    pub(crate) reason_code: String,
}

impl StepSealClosure {
    /// The process-exit default closure derived from the output: `Closed`/`Failed`
    /// by exit status and the matching reason code. The single source of this
    /// derivation.
    pub(crate) fn default_for(output: &InvocationOutput) -> Self {
        let disposition = disposition(output);
        Self {
            reason_code: step_reason_code(&disposition),
            disposition,
        }
    }
}

pub(crate) fn seal_step(
    params: StepSeal<'_>,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Receipt, RuntimeError> {
    let StepSeal {
        scope,
        step_id,
        attempt,
        output,
        claim,
        created_at,
        authority,
        call,
        approval_refs,
        child_receipts,
        descendant_receipts,
        closure,
        receipt_metadata,
    } = params;
    let StepSealClosure {
        disposition,
        reason_code,
    } = closure.unwrap_or_else(|| StepSealClosure::default_for(output));
    step_receipt_with_disposition_projection_authority_and_policy(StepReceiptSeal {
        params: StepReceiptWithDisposition {
            scope,
            step_id,
            attempt,
            output,
            created_at,
            disposition,
            reason_code,
        },
        claim,
        approval_refs,
        child_receipts,
        descendant_receipts,
        authority,
        call,
        receipt_metadata,
        signature_policy,
    })
}

/// The single runtime-outcome criterion a step receipt seals on, independent
/// of whether the producer was a process, native value, approval, or graph.
fn step_outcome_criterion(output: &InvocationOutput, evidence: &StepEvidence) -> CriterionBinding {
    CriterionBinding {
        criterion_id: Criterion::StepOutcome,
        status: if output.succeeded() {
            CriterionStatus::Verified
        } else {
            CriterionStatus::Failed
        },
        evidence_refs: evidence.evidence_refs.clone(),
        verification_refs: evidence.verification_refs.clone(),
    }
}

/// One verified criterion for each contract check the runtime ran.
fn contract_criteria(checks: &ContractChecks) -> Vec<CriterionBinding> {
    let mut result = Vec::new();
    if let Some(digest) = &checks.output_contract_sha256 {
        push_verified_criterion(
            &mut result,
            Criterion::OutputContractVerified,
            vec![verification_ref(Criterion::OutputContractVerified, digest)],
        );
    }
    if let Some(digest) = &checks.voice_profile_sha256 {
        push_verified_criterion(
            &mut result,
            Criterion::VoiceProfileApplied,
            vec![verification_ref(Criterion::VoiceProfileApplied, digest)],
        );
    }
    if !checks.packet_schemas.is_empty() {
        push_verified_criterion(
            &mut result,
            Criterion::PacketSchemasVerified,
            checks
                .packet_schemas
                .values()
                .map(|check| {
                    Reference::with_uri(
                        ReferenceType::Verification,
                        format!(
                            "runx:verification:packet_schema_verified:{}:{}",
                            check.packet, check.schema_sha256
                        ),
                    )
                })
                .collect(),
        );
    }
    result
}

fn verification_ref(criterion: Criterion, digest: &str) -> Reference {
    Reference::with_uri(
        ReferenceType::Verification,
        format!("runx:verification:{}:{digest}", criterion.as_str()),
    )
}

fn push_verified_criterion(
    result: &mut Vec<CriterionBinding>,
    criterion: Criterion,
    verification_refs: Vec<Reference>,
) {
    result.push(CriterionBinding {
        criterion_id: criterion,
        status: CriterionStatus::Verified,
        evidence_refs: Vec::new(),
        verification_refs,
    });
}

pub fn graph_receipt(
    scope: RunScope<'_>,
    steps: &[StepRun],
    created_at: &str,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Receipt, RuntimeError> {
    graph_receipt_with_closure(
        scope,
        steps,
        created_at,
        GraphClosure::completed(),
        signature_policy,
    )
}

pub(crate) struct GraphClosure {
    pub(crate) disposition: ClosureDisposition,
    pub(crate) reason_code: String,
}

impl GraphClosure {
    pub(crate) fn completed() -> Self {
        Self {
            disposition: ClosureDisposition::Closed,
            reason_code: "graph_closed".to_owned(),
        }
    }
}

pub(crate) fn graph_receipt_with_closure(
    scope: RunScope<'_>,
    steps: &[StepRun],
    created_at: &str,
    closure: GraphClosure,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Receipt, RuntimeError> {
    let current_child_indexes = current_step_indexes(steps);
    let child_refs = current_child_indexes
        .iter()
        .map(|index| child_receipt_reference(&steps[*index].receipt))
        .collect::<Vec<_>>();

    // A receipt graph is a DAG: the parent commits each active child's exact
    // id and signed-body digest. Children stay immutable and reusable instead
    // of acquiring one post-hoc parent link.
    let receipt = build_graph_receipt(
        signature_policy.signer()?.issuer(),
        scope,
        child_refs,
        created_at,
        &closure,
    )?
    .seal(signature_policy)?;

    validate_receipt_tree_with_policy(
        &receipt,
        current_child_indexes.iter().flat_map(|index| {
            steps[*index]
                .nested_receipts
                .iter()
                .chain(std::iter::once(&steps[*index].receipt))
        }),
        signature_policy,
    )?;
    Ok(receipt)
}

fn build_graph_receipt(
    issuer: ReceiptIssuer,
    scope: RunScope<'_>,
    children: Vec<Reference>,
    created_at: &str,
    closure: &GraphClosure,
) -> Result<UnsealedReceipt, RuntimeError> {
    let outcome = outcome_digest(
        closure.disposition == ClosureDisposition::Closed,
        &JsonObject::new(),
        &children,
    )?;
    Ok(build_unsealed_receipt(BuildReceipt {
        issuer,
        scope,
        node: Node::Graph,
        kind: receipt_subject_kind::GRAPH.into(),
        outcome,
        created_at,
        decisions: Vec::new(),
        acts: Vec::new(),
        seal: seal(
            closure.disposition.clone(),
            closure.reason_code.clone(),
            created_at,
            Vec::new(),
        ),
        children,
        authority: SealAuthority::default(),
        previous: None,
    }))
}

// A subject's outcome binds whether it succeeded, the claim it sealed (the
// outputs it declared; a graph declares none) and the ids of its direct child
// receipts. Raw transport values, diagnostics, and child proof envelopes
// remain excluded: a replay of the same declared claim and child identities
// keeps its outcome, while a changed nested execution propagates to every
// ancestor.
fn outcome_digest(
    succeeded: bool,
    claim: &JsonObject,
    children: &[Reference],
) -> Result<Digest, RuntimeError> {
    let mut outcome = JsonObject::from([
        (
            "status".to_owned(),
            JsonValue::String(if succeeded { "success" } else { "failure" }.to_owned()),
        ),
        ("claim".to_owned(), JsonValue::Object(claim.clone())),
    ]);
    if !children.is_empty() {
        outcome.insert(
            "children".to_owned(),
            JsonValue::Array(
                children
                    .iter()
                    .map(|reference| JsonValue::String(reference.uri.to_string()))
                    .collect(),
            ),
        );
    }
    let material = canonical_stable_json(&JsonValue::Object(outcome)).map_err(|error| {
        RuntimeError::ReceiptInvalid {
            message: error.to_string(),
        }
    })?;
    Ok(Digest::of(material.as_bytes()))
}

pub(crate) fn validate_step_receipt_claim(
    receipt: &Receipt,
    succeeded: bool,
    claim: &JsonObject,
) -> Result<(), RuntimeError> {
    if receipt.subject.outcome == outcome_digest(succeeded, claim, &receipt.lineage.children)? {
        return Ok(());
    }
    Err(RuntimeError::ReceiptInvalid {
        message: format!(
            "receipt {} does not bind its retained step claim",
            receipt.id
        ),
    })
}

fn validate_receipt_tree_with_policy<'a>(
    root: &Receipt,
    children: impl IntoIterator<Item = &'a Receipt>,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<(), RuntimeError> {
    super::tree::validate_runtime_receipt_tree_refs(
        root,
        children,
        ReceiptTreeConfig::default(),
        signature_policy,
    )
    .map_err(receipt_error)
}

fn step_reason_code(disposition: &ClosureDisposition) -> String {
    let suffix = match disposition {
        ClosureDisposition::Closed => "closed",
        ClosureDisposition::Deferred => "deferred",
        ClosureDisposition::Superseded => "superseded",
        ClosureDisposition::Declined => "declined",
        ClosureDisposition::Blocked => "blocked",
        ClosureDisposition::Failed => "failed",
        ClosureDisposition::Killed => "killed",
        ClosureDisposition::TimedOut => "timed_out",
    };
    format!("step_{suffix}")
}

/// The run a receipt belongs to, and the graph it ran.
#[derive(Clone, Copy, Debug)]
pub struct RunScope<'a> {
    pub run_id: &'a str,
    pub graph_name: &'a str,
}

/// The part of a run a receipt seals.
#[derive(Clone, Copy, Debug)]
enum Node<'a> {
    /// The whole graph.
    Graph,
    /// One step's attempt.
    Step { step_id: &'a str, attempt: u32 },
}

impl Node<'_> {
    /// `runx:run:<id>`, or `runx:run:<id>#<step>` (`#<step>@<attempt>` after
    /// the first attempt).
    fn uri(self, run_id: &str) -> String {
        match self {
            Self::Graph => format!("runx:run:{run_id}"),
            Self::Step { step_id, attempt } if attempt <= 1 => {
                format!("runx:run:{run_id}#{step_id}")
            }
            Self::Step { step_id, attempt } => format!("runx:run:{run_id}#{step_id}@{attempt}"),
        }
    }
}

struct BuildReceipt<'a> {
    /// The issuer the receipt will be sealed under.
    issuer: ReceiptIssuer,
    scope: RunScope<'a>,
    node: Node<'a>,
    kind: NonEmptyString,
    /// The digest of the subject's outcome.
    outcome: Digest,
    created_at: &'a str,
    decisions: Vec<Decision>,
    acts: Vec<ReceiptAct>,
    seal: Seal,
    children: Vec<Reference>,
    authority: SealAuthority,
    /// The receipt this one follows (`lineage.previous`): the act its call's
    /// caller says it follows.
    previous: Option<Reference>,
}

/// A receipt body not yet sealed. Its id, digest and signature hold
/// `UNSEALED` until `seal` computes them; only `seal` yields a `Receipt`.
struct UnsealedReceipt(Receipt);

const UNSEALED: &str = "unsealed";

impl UnsealedReceipt {
    fn as_mut(&mut self) -> &mut Receipt {
        &mut self.0
    }

    fn seal(
        mut self,
        signature_policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<Receipt, RuntimeError> {
        // Content-address the id over the canonical body (id = hash(canonical_body),
        // excluding id/signature/digest/metadata) before the digest commits it.
        self.0.id = content_addressed_receipt_id(&self.0)
            .map_err(|error| RuntimeError::ReceiptInvalid {
                message: error.to_string(),
            })?
            .into();
        let digest = canonical_receipt_body_digest(&self.0).map_err(|error| {
            RuntimeError::ReceiptInvalid {
                message: error.to_string(),
            }
        })?;
        self.0.digest = digest.clone().into();
        signature_policy.sign_receipt(&mut self.0, &digest)?;
        Ok(self.0)
    }
}

fn build_unsealed_receipt(parts: BuildReceipt<'_>) -> UnsealedReceipt {
    let BuildReceipt {
        issuer,
        scope,
        node,
        kind,
        outcome,
        created_at,
        decisions,
        acts,
        seal,
        children,
        authority,
        previous,
    } = parts;
    let lineage = Lineage { previous, children };
    let subject_uri = node.uri(scope.run_id);
    UnsealedReceipt(Receipt {
        schema: ReceiptSchema::V1,
        id: UNSEALED.into(),
        created_at: created_at.into(),
        canonicalization: ReceiptCanonicalization::C14nV1,
        issuer,
        signature: ReceiptSignature {
            alg: SignatureAlgorithm::Ed25519,
            value: UNSEALED.into(),
        },
        digest: UNSEALED.into(),
        subject: subject(scope, subject_uri, kind, outcome),
        authority: local_authority(authority),
        decisions,
        acts,
        seal,
        lineage,
        metadata: None,
    })
}

/// The decision a call's caller states, inline in `decisions[]` and carried
/// out by the call's act, whose intent, target and artifacts it is. The
/// `selected_act_id` integrity property is checked against the inline
/// `acts[]` at verify time.
fn decision(node_id: &str, choice: DecisionChoice, act: &ReceiptAct) -> Decision {
    Decision {
        decision_id: format!("dec_{node_id}").into(),
        choice,
        selected_act_id: act.id.clone(),
    }
}

fn seal(
    disposition: ClosureDisposition,
    reason_code: String,
    closed_at: &str,
    criteria: Vec<CriterionBinding>,
) -> Seal {
    Seal {
        disposition,
        reason_code: reason_code.into(),
        closed_at: closed_at.into(),
        criteria,
    }
}

/// The run node a receipt seals, labelled with the graph that ran.
fn subject(scope: RunScope<'_>, uri: String, kind: NonEmptyString, outcome: Digest) -> Subject {
    let mut reference = Reference::with_uri(ReferenceType::Run, uri);
    reference.label = Some(scope.graph_name.into());
    Subject {
        kind,
        reference,
        outcome,
    }
}

/// The authority a step receipt seals: who acted, the grants and scopes that
/// admitted the step, the mandate a delegated actor acted under, and the
/// execution terms derived for it with their proofs. A step no admission
/// names an actor for is the local runtime's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SealAuthority {
    pub actor_ref: Option<Reference>,
    pub grant_refs: Vec<Reference>,
    pub scope_refs: Vec<Reference>,
    pub mandate_ref: Option<Reference>,
    pub terms: Vec<runx_contracts::AuthorityTerm>,
    pub derivations: Vec<runx_contracts::AuthorityDerivation>,
}

fn local_authority(authority: SealAuthority) -> ReceiptAuthority {
    let SealAuthority {
        actor_ref,
        grant_refs,
        scope_refs,
        mandate_ref,
        terms,
        derivations,
    } = authority;
    ReceiptAuthority {
        actor_ref,
        grant_refs,
        scope_refs,
        terms,
        derivations,
        mandate_ref,
        enforcement: ReceiptEnforcement::default(),
    }
}

/// The evidence of a step's outcome and its verification, from what admission
/// and the runtime recorded: the approvals that let it run, and its effect
/// and credential verification. No output field is read for a reference.
#[derive(Default)]
struct StepEvidence {
    evidence_refs: Vec<Reference>,
    verification_refs: Vec<Reference>,
}

fn step_evidence(observations: &Observations, approval_refs: Vec<Reference>) -> StepEvidence {
    let mut evidence = StepEvidence::default();
    evidence.evidence_refs.extend(approval_refs.iter().cloned());
    evidence.verification_refs.extend(approval_refs);
    evidence
        .verification_refs
        .extend(observations.effect_verification_refs.iter().cloned());
    for reference in observations
        .credentials
        .iter()
        .flat_map(|observation| &observation.credential_refs)
    {
        if !evidence.verification_refs.contains(reference) {
            evidence.verification_refs.push(reference.clone());
        }
    }
    evidence
}

fn disposition(output: &InvocationOutput) -> ClosureDisposition {
    if output.succeeded() {
        ClosureDisposition::Closed
    } else {
        ClosureDisposition::Failed
    }
}

pub(crate) fn child_receipt_reference(receipt: &Receipt) -> Reference {
    Reference {
        locator: Some(receipt.digest.clone()),
        ..Reference::runx(ReferenceType::Receipt, &receipt.id)
    }
}

fn current_step_indexes(steps: &[StepRun]) -> Vec<usize> {
    let mut latest = BTreeMap::<&str, usize>::new();
    for (index, step) in steps.iter().enumerate() {
        latest.insert(step.step_id.as_str(), index);
    }
    steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| {
            latest
                .get(step.step_id.as_str())
                .is_some_and(|latest_index| *latest_index == index)
                .then_some(index)
        })
        .collect()
}

/// The keys a receipt is checked against, and the signer that seals new
/// receipts when this policy may seal. Reading needs only the verifier.
#[derive(Clone, Copy)]
pub struct RuntimeReceiptSignaturePolicy<'a> {
    signer: Option<&'a dyn RuntimeReceiptSigner>,
    verifier: &'a dyn SignatureVerifier,
}

impl std::fmt::Debug for RuntimeReceiptSignaturePolicy<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeReceiptSignaturePolicy")
            .field("issuer", &self.signer.map(|signer| signer.issuer()))
            .finish()
    }
}

impl<'a> RuntimeReceiptSignaturePolicy<'a> {
    /// Seal under `signer`; check every seal and every read against
    /// `verifier`.
    #[must_use]
    pub fn signing(
        signer: &'a dyn RuntimeReceiptSigner,
        verifier: &'a dyn SignatureVerifier,
    ) -> Self {
        Self {
            signer: Some(signer),
            verifier,
        }
    }

    /// Check receipts against `verifier`; seal none.
    #[must_use]
    pub fn verifying(verifier: &'a dyn SignatureVerifier) -> Self {
        Self {
            signer: None,
            verifier,
        }
    }

    #[must_use]
    pub fn verifier(self) -> &'a dyn SignatureVerifier {
        self.verifier
    }

    fn signer(self) -> Result<&'a dyn RuntimeReceiptSigner, RuntimeError> {
        self.signer
            .ok_or_else(|| signing_error(RuntimeReceiptSigningError::MissingSigner))
    }

    /// Sign `body_digest` and check the signature against the verifier, so
    /// no receipt is kept that its own reader would refuse.
    fn sign_receipt(self, receipt: &mut Receipt, body_digest: &str) -> Result<(), RuntimeError> {
        let signer = self.signer()?;
        if receipt.issuer != signer.issuer() {
            return Err(signing_error(RuntimeReceiptSigningError::IssuerMismatch));
        }
        let signature = signer
            .sign_receipt_body(body_digest)
            .map_err(signing_error)?;
        if signature.alg != SignatureAlgorithm::Ed25519 {
            return Err(signing_error(
                RuntimeReceiptSigningError::UnsupportedAlgorithm,
            ));
        }
        receipt.signature = signature;
        self.verifier
            .verify(&receipt.issuer, &receipt.signature, body_digest)
            .map_err(RuntimeReceiptSigningError::SignatureVerification)
            .map_err(signing_error)
    }
}

pub(crate) struct RuntimeReceiptProofContextProvider<'a> {
    verifier: &'a dyn SignatureVerifier,
}

impl<'a> RuntimeReceiptProofContextProvider<'a> {
    pub(crate) fn new(signature_policy: RuntimeReceiptSignaturePolicy<'a>) -> Self {
        Self {
            verifier: signature_policy.verifier,
        }
    }
}

impl ReceiptProofContextProvider for RuntimeReceiptProofContextProvider<'_> {
    fn proof_context<'a>(&'a self, _receipt: &Receipt) -> ReceiptProofContext<'a> {
        ReceiptProofContext {
            signature_verifier: Some(self.verifier),
        }
    }
}

fn signing_error(error: RuntimeReceiptSigningError) -> RuntimeError {
    RuntimeError::ReceiptInvalid {
        message: error.to_string(),
    }
}

fn receipt_error(verification: runx_receipts::ReceiptVerification) -> RuntimeError {
    RuntimeError::ReceiptInvalid {
        message: format!("{:?}", verification.findings),
    }
}

#[cfg(test)]
mod tests {
    fn test_scope(graph_name: &'static str) -> RunScope<'static> {
        RunScope {
            run_id: "run_test",
            graph_name,
        }
    }

    fn fixture() -> Result<crate::receipts::RuntimeReceiptSignatureConfig, TestError> {
        crate::receipts::RuntimeReceiptSignatureConfig::fixture()
            .map_err(|error| TestError(error.to_string()))
    }

    use super::*;
    use crate::adapter::InvocationStatus;
    use runx_contracts::{
        CredentialDeliveryMode, CredentialDeliveryObservation, CredentialDeliveryObservationSchema,
        CredentialDeliveryObservationStatus, CredentialDeliveryPurpose, CredentialMaterialRole,
        JsonValue, ProofKind,
    };

    /// Concrete error type for fallible tests, so `?` propagates the receipt and
    /// serialization errors a test exercises without erasing them behind a trait
    /// object.
    #[derive(Debug)]
    struct TestError(String);

    impl std::fmt::Display for TestError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(&self.0)
        }
    }

    impl std::error::Error for TestError {}

    impl From<RuntimeError> for TestError {
        fn from(error: RuntimeError) -> Self {
            Self(error.to_string())
        }
    }

    impl From<runx_receipts::ReceiptError> for TestError {
        fn from(error: runx_receipts::ReceiptError) -> Self {
            Self(error.to_string())
        }
    }

    impl From<serde_json::Error> for TestError {
        fn from(error: serde_json::Error) -> Self {
            Self(error.to_string())
        }
    }

    #[test]
    fn credential_delivery_refs_are_sealed_as_verification_refs() -> Result<(), TestError> {
        let receipt = step_receipt(
            test_scope("credential_graph"),
            "credential_step",
            1,
            &credential_output()?,
            &JsonObject::new(),
            "2026-05-28T00:00:00Z",
            fixture()?.signature_policy(),
        )?;

        assert!(receipt.acts.is_empty());
        let verification_refs = &receipt.seal.criteria[0].verification_refs;
        assert_eq!(verification_refs.len(), 1);
        assert_eq!(
            verification_refs[0].reference_type,
            ReferenceType::Credential
        );
        assert_eq!(
            verification_refs[0].uri.as_str(),
            "runx:credential:grant_github_main"
        );
        assert_eq!(
            verification_refs[0].proof_kind,
            Some(ProofKind::CredentialResolution)
        );

        let sealed_digest = canonical_receipt_body_digest(&receipt)?;
        let mut without_credential_ref = receipt.clone();
        without_credential_ref.seal.criteria[0]
            .verification_refs
            .clear();
        let unsealed_digest = canonical_receipt_body_digest(&without_credential_ref)?;
        assert_ne!(sealed_digest, unsealed_digest);
        Ok(())
    }

    #[test]
    fn contract_verification_is_bound_into_the_seal_and_a_calls_act() -> Result<(), TestError> {
        let mut output = InvocationOutput::runtime_success(JsonValue::Object(JsonObject::new()), 1);
        output.observations.contract = ContractChecks {
            output_contract_sha256: Some("sha256:output".to_owned()),
            voice_profile_sha256: Some("sha256:voice".to_owned()),
            packet_schemas: BTreeMap::from([(
                "plan".to_owned(),
                crate::observations::PacketCheck {
                    packet: "runx.test.plan.v1".to_owned(),
                    schema_sha256: "sha256:packet".to_owned(),
                },
            )]),
        };

        let receipt = step_receipt(
            test_scope("agent_graph"),
            "agent_step",
            1,
            &output,
            &JsonObject::new(),
            "2026-05-28T00:00:00Z",
            fixture()?.signature_policy(),
        )?;

        // A step without a governed call seals its criteria in its seal alone.
        assert!(receipt.acts.is_empty() && receipt.decisions.is_empty());
        assert_eq!(receipt.seal.criteria.len(), 4);
        // A governed call's act carries the same criteria.
        let call =
            step_receipt_with_disposition_projection_authority_and_policy(StepReceiptSeal {
                params: StepReceiptWithDisposition::with_default_closure(
                    test_scope("agent_graph"),
                    "call_step",
                    1,
                    &output,
                    "2026-05-28T00:00:00Z",
                ),
                claim: &JsonObject::new(),
                approval_refs: Vec::new(),
                child_receipts: &[],
                descendant_receipts: &[],
                authority: SealAuthority::default(),
                call: Some(SealedCall {
                    act: None,
                    description: "Post one message.".into(),
                    term_id: "term_call".to_owned(),
                    target_ref: None,
                    effects: Vec::new(),
                    context_ref: None,
                }),
                receipt_metadata: None,
                signature_policy: fixture()?.signature_policy(),
            })?;
        assert_eq!(call.acts[0].intent.purpose.as_str(), "Post one message.");
        assert_eq!(call.acts[0].criterion_bindings.len(), 4);
        assert!(call.decisions.is_empty(), "an unstated decision seals none");
        assert_eq!(
            receipt.seal.criteria[1].criterion_id.as_str(),
            "output_contract_verified"
        );
        assert_eq!(
            receipt.seal.criteria[1].verification_refs[0].reference_type,
            ReferenceType::Verification
        );
        assert!(
            receipt.seal.criteria[1].verification_refs[0]
                .uri
                .as_str()
                .ends_with("sha256:output")
        );
        Ok(())
    }

    #[test]
    fn an_answer_that_claims_checks_a_boundary_or_evidence_seals_none() -> Result<(), TestError> {
        let claims = serde_json::json!({
            "contract_verification": { "output_contract_sha256": "sha256:forged" },
            "execution_boundary": { "kind": "native_capability" },
            "credential_delivery_observations": [],
            "runx_effect_verification_refs": { "refs": [] },
            "execution_limits": {}
        });
        let output = InvocationOutput::runtime_success(serde_json::from_value(claims)?, 1);

        let receipt = step_receipt(
            test_scope("claims_graph"),
            "claims_step",
            1,
            &output,
            &JsonObject::new(),
            "2026-05-28T00:00:00Z",
            fixture()?.signature_policy(),
        )?;

        assert_eq!(
            receipt.seal.criteria.len(),
            1,
            "only the step outcome is sealed"
        );
        assert!(receipt.seal.criteria[0].verification_refs.is_empty());
        assert_eq!(receipt.authority.enforcement.execution_boundary, None);
        assert_eq!(receipt.metadata, None);
        Ok(())
    }

    #[test]
    fn a_supervisors_verification_refs_are_sealed_and_stdout_claims_are_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let reference = Reference::runx(ReferenceType::Verification, "supervised-proof");
        let mut output = InvocationOutput::process(
            crate::InvocationStatus::Success,
            r#"{"verification":{"verification_id":"stdout-verification"}}"#.to_owned(),
            String::new(),
            Some(0),
            10,
        );
        output
            .observations
            .effect_verification_refs
            .push(reference.clone());

        let receipt = step_receipt(
            test_scope("verified"),
            "fulfill",
            1,
            &output,
            &JsonObject::new(),
            "2026-05-18T00:00:00Z",
            fixture()?.signature_policy(),
        )?;
        let verification_refs = receipt
            .seal
            .criteria
            .iter()
            .flat_map(|criterion| criterion.verification_refs.iter())
            .collect::<Vec<_>>();

        assert!(verification_refs.contains(&&reference));
        assert!(
            verification_refs
                .iter()
                .all(|reference| reference.uri != "runx:verification:stdout-verification")
        );
        Ok(())
    }

    #[test]
    fn execution_limit_hit_is_copied_into_receipt_read_metadata() -> Result<(), TestError> {
        let limit = runx_contracts::ExecutionLimit {
            configured: 7_000,
            maximum: 30_000,
            unit: runx_contracts::ExecutionLimitUnit::Milliseconds,
            manifest_field: None,
        };
        let limits = runx_contracts::ExecutionLimits {
            configured: BTreeMap::from([(
                "javascript.wall_milliseconds".to_owned(),
                limit.clone(),
            )]),
            hit: Some(runx_contracts::ExecutionLimitHit {
                id: "javascript.wall_milliseconds".to_owned(),
                limit,
            }),
        };
        let mut output =
            InvocationOutput::runtime_failure(JsonValue::Null, "wall limit reached", 7_000);
        output.observations.limits = Some(limits.clone());

        let receipt = step_receipt(
            test_scope("limit_graph"),
            "limit_step",
            1,
            &output,
            &JsonObject::new(),
            "2026-05-28T00:00:00Z",
            fixture()?.signature_policy(),
        )?;
        assert_eq!(
            receipt
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get(EXECUTION_LIMITS_METADATA))
                .map(serde_json::to_value)
                .transpose()?,
            Some(serde_json::to_value(&limits)?)
        );
        Ok(())
    }

    #[test]
    fn observed_execution_boundary_is_bound_into_signed_authority() -> Result<(), TestError> {
        let mut output = InvocationOutput::runtime_success(JsonValue::Object(JsonObject::new()), 1);
        output.observations.boundary =
            Some(runx_contracts::ExecutionBoundaryKind::TrustedHostProcess);

        let receipt = step_receipt(
            test_scope("boundary_graph"),
            "boundary_step",
            1,
            &output,
            &JsonObject::new(),
            "2026-05-28T00:00:00Z",
            fixture()?.signature_policy(),
        )?;

        assert_eq!(
            receipt.authority.enforcement.execution_boundary,
            Some(runx_contracts::ExecutionBoundaryObservation {
                kind: runx_contracts::ExecutionBoundaryKind::TrustedHostProcess,
            })
        );
        Ok(())
    }

    fn identity_receipt(stdout: &str, claim: JsonObject) -> Result<Receipt, TestError> {
        let output = successful_output(stdout);
        Ok(
            step_receipt_with_disposition_projection_authority_and_policy(StepReceiptSeal {
                params: StepReceiptWithDisposition::with_default_closure(
                    test_scope("identity_graph"),
                    "identity_step",
                    1,
                    &output,
                    "2026-05-28T00:00:00Z",
                ),
                claim: &claim,
                approval_refs: Vec::new(),
                child_receipts: &[],
                descendant_receipts: &[],
                authority: SealAuthority::default(),
                call: None,
                receipt_metadata: None,
                signature_policy: fixture()?.signature_policy(),
            })?,
        )
    }

    #[test]
    fn step_receipt_identity_commits_sealed_claim_not_transport_value() -> Result<(), TestError> {
        let claim = |a: &str, b: &str| {
            JsonObject::from([
                ("a".to_owned(), JsonValue::String(a.to_owned())),
                ("b".to_owned(), JsonValue::String(b.to_owned())),
            ])
        };

        let first = identity_receipt("{\"a\":\"1\",\"b\":\"2\"}", claim("1", "2"))?;
        // Same sealed claim, entirely different transport bytes: identity is
        // unchanged, because a receipt binds its declared claim, never the raw
        // transport value or its diagnostics.
        let other_transport = identity_receipt("unrelated transport bytes", claim("1", "2"))?;
        // A reordered claim canonicalises to the same identity.
        let reordered = identity_receipt(
            "{\"a\":\"1\",\"b\":\"2\"}",
            JsonObject::from([
                ("b".to_owned(), JsonValue::String("2".to_owned())),
                ("a".to_owned(), JsonValue::String("1".to_owned())),
            ]),
        )?;
        // A changed claim value moves the identity.
        let changed = identity_receipt("{\"a\":\"1\",\"b\":\"2\"}", claim("1", "3"))?;

        assert_eq!(first.id, other_transport.id);
        assert_eq!(first.subject.outcome, other_transport.subject.outcome);
        assert_eq!(first.id, reordered.id);
        assert_eq!(first.subject.outcome, reordered.subject.outcome);
        assert_ne!(first.id, changed.id);
        assert_ne!(first.subject.outcome, changed.subject.outcome);
        Ok(())
    }

    #[test]
    fn step_receipt_identity_commits_direct_child_identities() -> Result<(), TestError> {
        let signature = fixture()?;
        let child = |value: &str| {
            identity_receipt(
                "transport",
                JsonObject::from([("value".to_owned(), JsonValue::String(value.to_owned()))]),
            )
        };
        let outer = |child: &Receipt| {
            let output = successful_output("outer transport");
            step_receipt_with_disposition_projection_authority_and_policy(StepReceiptSeal {
                params: StepReceiptWithDisposition::with_default_closure(
                    test_scope("outer_graph"),
                    "outer_step",
                    1,
                    &output,
                    "2026-05-28T00:00:00Z",
                ),
                claim: &JsonObject::new(),
                approval_refs: Vec::new(),
                child_receipts: std::slice::from_ref(child),
                descendant_receipts: &[],
                authority: SealAuthority::default(),
                call: None,
                receipt_metadata: None,
                signature_policy: signature.signature_policy(),
            })
        };

        let first = outer(&child("first")?)?;
        let replay = outer(&child("first")?)?;
        let changed = outer(&child("changed")?)?;

        assert_eq!(first.id, replay.id);
        assert_ne!(first.id, changed.id);
        Ok(())
    }

    #[test]
    fn an_outcome_commits_child_receipt_ids_not_their_digests() -> Result<(), RuntimeError> {
        let mut first = Reference::runx(ReferenceType::Receipt, "sha256:first");
        first.locator = Some("sha256:first-digest".into());
        let mut same_id_new_digest = first.clone();
        same_id_new_digest.locator = Some("sha256:updated-digest".into());
        let second = Reference::runx(ReferenceType::Receipt, "sha256:second");
        let claim = JsonObject::new();

        assert_eq!(
            outcome_digest(true, &claim, &[first.clone()])?,
            outcome_digest(true, &claim, &[same_id_new_digest])?
        );
        assert_ne!(
            outcome_digest(true, &claim, &[first.clone()])?,
            outcome_digest(true, &claim, &[second])?
        );
        assert_ne!(
            outcome_digest(true, &claim, &[first.clone()])?,
            outcome_digest(false, &claim, &[first])?
        );
        Ok(())
    }

    fn successful_output(stdout: &str) -> InvocationOutput {
        InvocationOutput::process(
            InvocationStatus::Success,
            stdout.to_owned(),
            String::new(),
            Some(0),
            1,
        )
    }

    fn credential_output() -> Result<InvocationOutput, TestError> {
        let observation = CredentialDeliveryObservation {
            schema: CredentialDeliveryObservationSchema::V1,
            observation_id: "credential_delivery_observation_1".into(),
            request_id: "credential_delivery_request_1".into(),
            response_id: Some("credential_delivery_response_1".into()),
            status: CredentialDeliveryObservationStatus::Delivered,
            harness_ref: Reference::with_uri(
                ReferenceType::Harness,
                "runx:harness:local-credential-provision",
            ),
            host_ref: Some(Reference::with_uri(
                ReferenceType::Host,
                "runx:host:local-cli",
            )),
            profile_id: "github-api-key-env".into(),
            provider: "github".into(),
            purpose: CredentialDeliveryPurpose::ProviderApi,
            delivery_mode: Some(CredentialDeliveryMode::ProcessEnv),
            credential_refs: vec![Reference {
                reference_type: ReferenceType::Credential,
                uri: "runx:credential:grant_github_main".into(),
                provider: Some("github".into()),
                locator: None,
                label: None,
                observed_at: None,
                proof_kind: Some(ProofKind::CredentialResolution),
            }],
            material_ref_hash: Some("sha256:material-ref-hash".into()),
            delivered_roles: vec![CredentialMaterialRole::ApiKey],
            redaction_refs: None,
            observed_at: "2026-05-28T00:00:00Z".into(),
        };
        let mut output = InvocationOutput::process(
            InvocationStatus::Success,
            "ok".to_owned(),
            String::new(),
            Some(0),
            1,
        );
        output.observations.record_credential(&observation)?;
        Ok(output)
    }
}
