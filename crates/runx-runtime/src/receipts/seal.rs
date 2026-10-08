// Module rationale: receipt construction, explicit
// signature policy, and local proof sealing stay together until the runtime
// receipt builder is split out.
use std::collections::BTreeMap;

use crate::adapter::{
    CONTRACT_VERIFICATION_METADATA, CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA,
    EXECUTION_LIMITS_METADATA, InvocationOutput,
};
use crate::effects::effect_verification_refs;
use crate::execution::output_projection::{StepOutputRefs, claim_refs};
use crate::{RuntimeError, StepRun};
use runx_contracts::fingerprint::sha256_hex;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ActForm, AuthorityAttenuation, AuthoritySubsetResult, AuthorityTerm, Closure,
    ClosureDisposition, CredentialDeliveryObservation, CriterionBinding, CriterionStatus, Decision,
    DecisionChoice, DecisionInputs, DecisionJustification, Intent, JsonObject, JsonValue, Lineage,
    RECEIPT_CANONICALIZATION, Receipt, ReceiptAct, ReceiptAuthority, ReceiptClass,
    ReceiptEnforcement, ReceiptIdempotency, ReceiptIssuer, ReceiptSchema, Reference, ReferenceType,
    Seal, SignatureAlgorithm, Subject, SuccessCriterion, canonical_stable_json, json_string_field,
    receipt_subject_kind,
};
use runx_receipts::{
    ReceiptProofContext, ReceiptProofContextProvider, ReceiptSignature, ReceiptTreeConfig,
    SignatureVerificationFailure, SignatureVerifier, canonical_receipt_body_digest,
    content_addressed_receipt_id,
};

use super::act::{ActOutcome, RuntimeAct};
use super::local_runtime_issuer;
use super::signing::{
    RuntimeReceiptSigner, RuntimeReceiptSigningError, is_local_pseudo_signature,
    validate_production_issuer,
};
pub fn step_receipt(
    scope: RunScope<'_>,
    step_id: &str,
    attempt: u32,
    output: &InvocationOutput,
    claim: &JsonObject,
    created_at: &str,
) -> Result<Receipt, RuntimeError> {
    step_receipt_with_declared_claim_and_policy(
        StepReceiptWithDisposition::with_default_closure(
            scope, step_id, attempt, output, created_at,
        ),
        claim,
        claim_refs(claim),
        RuntimeReceiptSignaturePolicy::local_development(),
    )
}

pub fn step_receipt_with_signature_policy(
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
        claim_refs(claim),
        signature_policy,
    )
}

pub fn step_receipt_with_authority_grant_refs(
    scope: RunScope<'_>,
    step_id: &str,
    attempt: u32,
    output: &InvocationOutput,
    claim: &JsonObject,
    authority_grant_refs: Vec<Reference>,
    created_at: &str,
) -> Result<Receipt, RuntimeError> {
    let refs = claim_refs(claim);
    step_receipt_with_disposition_projection_authority_and_policy(StepReceiptSeal {
        params: StepReceiptWithDisposition::with_default_closure(
            scope, step_id, attempt, output, created_at,
        ),
        claim,
        projection_refs: refs,
        child_receipts: &[],
        descendant_receipts: &[],
        authority: SealAuthority {
            grant_refs: authority_grant_refs,
            ..SealAuthority::default()
        },
        receipt_metadata: None,
        signature_policy: RuntimeReceiptSignaturePolicy::local_development(),
    })
}

pub(crate) struct StepReceiptWithDisposition<'a> {
    pub(crate) scope: RunScope<'a>,
    pub(crate) step_id: &'a str,
    pub(crate) attempt: u32,
    pub(crate) output: &'a InvocationOutput,
    pub(crate) created_at: &'a str,
    pub(crate) disposition: ClosureDisposition,
    pub(crate) reason_code: String,
    pub(crate) summary: String,
}

struct StepReceiptSeal<'a> {
    params: StepReceiptWithDisposition<'a>,
    claim: &'a JsonObject,
    projection_refs: StepOutputRefs,
    child_receipts: &'a [Receipt],
    descendant_receipts: &'a [Receipt],
    authority: SealAuthority,
    receipt_metadata: Option<JsonObject>,
    signature_policy: RuntimeReceiptSignaturePolicy<'a>,
}

impl<'a> StepReceiptWithDisposition<'a> {
    /// A step-receipt request whose closure is derived from the output (the
    /// process-exit default): `Closed`/`Failed` by exit status, the matching
    /// `process_*` reason code, and a generic completion summary. This is the
    /// single source of that derivation for the process-exit step receipts.
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
            summary,
        } = StepSealClosure::default_for(output, step_id);
        Self {
            scope,
            step_id,
            attempt,
            output,
            created_at,
            disposition,
            reason_code,
            summary,
        }
    }
}

pub(crate) fn step_receipt_with_declared_claim_and_policy(
    params: StepReceiptWithDisposition<'_>,
    claim: &JsonObject,
    projection_refs: StepOutputRefs,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Receipt, RuntimeError> {
    step_receipt_with_disposition_projection_authority_and_policy(StepReceiptSeal {
        params,
        claim,
        projection_refs,
        child_receipts: &[],
        descendant_receipts: &[],
        authority: SealAuthority::default(),
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
        projection_refs,
        child_receipts,
        descendant_receipts,
        authority,
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
        summary,
    } = params;
    let output_refs = output_refs(output, projection_refs)?;
    let verification = contract_verification_criteria(&output.metadata)?;
    let act = RuntimeAct::observation(step_id)
        .with_verified_criteria(verification.criteria.clone(), verification.bindings.clone())
        .close(ActOutcome {
            disposition: disposition.clone(),
            succeeded: output.succeeded(),
            summary: output_summary(output),
            performed_at: created_at,
            refs: &output_refs,
        });
    let seal_criterion = step_outcome_criterion(output, &output_refs);
    let mut seal_criteria = vec![seal_criterion];
    seal_criteria.extend(verification.bindings);
    let seal = seal(disposition, reason_code, summary, created_at, seal_criteria);
    let decisions = decisions(
        step_id,
        &act,
        &output_refs.signal_refs,
        &output_refs.artifact_refs,
    );
    let mut receipt = build_unsealed_receipt(BuildReceipt {
        class: ReceiptClass::Executed,
        scope,
        node: Node::Step { step_id, attempt },
        kind: receipt_subject_kind::SKILL.into(),
        created_at,
        decisions,
        acts: vec![act],
        seal,
        children: child_receipts.iter().map(child_receipt_reference).collect(),
        signals: output_refs.signal_refs,
        authority,
        authority_override: None,
        previous: None,
    });
    bind_step_identity(receipt.as_mut(), output, claim)?;
    bind_execution_boundary(receipt.as_mut(), output)?;
    receipt.as_mut().metadata = receipt_metadata_with_execution_limits(receipt_metadata, output)?;
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

fn bind_execution_boundary(
    receipt: &mut Receipt,
    output: &InvocationOutput,
) -> Result<(), RuntimeError> {
    let Some(value) = output
        .metadata
        .get(runx_contracts::EXECUTION_BOUNDARY_METADATA)
    else {
        return Ok(());
    };
    let observation = serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|source| RuntimeError::ReceiptInvalid {
            message: format!("invalid execution boundary metadata: {source}"),
        })?;
    receipt.authority.enforcement.execution_boundary = Some(observation);
    Ok(())
}

fn receipt_metadata_with_execution_limits(
    receipt_metadata: Option<JsonObject>,
    output: &InvocationOutput,
) -> Result<Option<JsonObject>, RuntimeError> {
    let Some(limits) = output.metadata.get(EXECUTION_LIMITS_METADATA) else {
        return Ok(receipt_metadata);
    };
    if !matches!(limits, JsonValue::Object(_)) {
        return Err(RuntimeError::ReceiptInvalid {
            message: "execution_limits metadata must be an object".to_owned(),
        });
    }
    let mut metadata = receipt_metadata.unwrap_or_default();
    metadata.insert(EXECUTION_LIMITS_METADATA.to_owned(), limits.clone());
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
    pub(crate) projection_refs: StepOutputRefs,
    pub(crate) created_at: &'a str,
    pub(crate) authority: SealAuthority,
    pub(crate) operator_refs: Vec<Reference>,
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

/// A step's own disposition, reason, and summary when it does not derive them
/// from a process exit.
pub(crate) struct StepSealClosure {
    pub(crate) disposition: ClosureDisposition,
    pub(crate) reason_code: String,
    pub(crate) summary: String,
}

impl StepSealClosure {
    /// The process-exit default closure derived from the output: `Closed`/`Failed`
    /// by exit status, the matching `process_*` reason code, and a generic
    /// completion summary. The single source of this derivation.
    pub(crate) fn default_for(output: &InvocationOutput, step_id: &str) -> Self {
        let disposition = disposition(output);
        Self {
            reason_code: step_reason_code(&disposition),
            summary: format!("step {step_id} completed"),
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
        mut projection_refs,
        created_at,
        authority,
        operator_refs,
        child_receipts,
        descendant_receipts,
        closure,
        receipt_metadata,
    } = params;
    let StepSealClosure {
        disposition,
        reason_code,
        summary,
    } = closure.unwrap_or_else(|| StepSealClosure::default_for(output, step_id));
    for reference in operator_refs {
        if reference.reference_type == ReferenceType::Artifact {
            projection_refs.artifact_refs.push(reference.clone());
        } else {
            projection_refs.artifact_refs.push(reference.clone());
            projection_refs.verification_refs.push(reference.clone());
        }
        projection_refs.evidence_refs.push(reference);
    }
    step_receipt_with_disposition_projection_authority_and_policy(StepReceiptSeal {
        params: StepReceiptWithDisposition {
            scope,
            step_id,
            attempt,
            output,
            created_at,
            disposition,
            reason_code,
            summary,
        },
        claim,
        projection_refs,
        child_receipts,
        descendant_receipts,
        authority,
        receipt_metadata,
        signature_policy,
    })
}

/// The single runtime-outcome criterion a step receipt seals on, independent
/// of whether the producer was a process, native value, approval, or graph.
fn step_outcome_criterion(
    output: &InvocationOutput,
    output_refs: &StepOutputRefs,
) -> CriterionBinding {
    CriterionBinding {
        criterion_id: "step_outcome".into(),
        status: if output.succeeded() {
            CriterionStatus::Verified
        } else {
            CriterionStatus::Failed
        },
        evidence_refs: output_refs.evidence_refs.clone(),
        verification_refs: output_refs.verification_refs.clone(),
        summary: Some(output_summary(output).into()),
    }
}

#[derive(Clone, Debug, Default)]
struct ContractVerificationCriteria {
    criteria: Vec<SuccessCriterion>,
    bindings: Vec<CriterionBinding>,
}

fn contract_verification_criteria(
    metadata: &JsonObject,
) -> Result<ContractVerificationCriteria, RuntimeError> {
    let Some(value) = metadata.get(CONTRACT_VERIFICATION_METADATA) else {
        return Ok(ContractVerificationCriteria::default());
    };
    let JsonValue::Object(verification) = value else {
        return Err(invalid_contract_verification(
            "contract_verification must be an object",
        ));
    };
    const ALLOWED_FIELDS: &[&str] = &[
        "output_contract_sha256",
        "voice_profile_sha256",
        "packet_schemas",
    ];
    if let Some(field) = verification
        .keys()
        .find(|field| !ALLOWED_FIELDS.contains(&field.as_str()))
    {
        return Err(invalid_contract_verification(format!(
            "contract_verification contains undeclared field '{field}'"
        )));
    }

    let mut result = ContractVerificationCriteria::default();
    if let Some(output_digest) =
        optional_verification_digest(verification, "output_contract_sha256")?
    {
        push_verified_criterion(
            &mut result,
            "output_contract_verified",
            "Runner result satisfies its declared output contract",
            output_digest,
        );
    }
    if let Some(digest) = optional_verification_digest(verification, "voice_profile_sha256")? {
        push_verified_criterion(
            &mut result,
            "voice_profile_applied",
            "Agent invocation used the resolved voice profile",
            digest,
        );
    }
    append_packet_schema_criterion(&mut result, verification)?;
    if result.criteria.is_empty() {
        return Err(invalid_contract_verification(
            "contract_verification must contain a verified output, voice, or packet contract",
        ));
    }
    Ok(result)
}

fn append_packet_schema_criterion(
    result: &mut ContractVerificationCriteria,
    verification: &JsonObject,
) -> Result<(), RuntimeError> {
    let Some(value) = verification.get("packet_schemas") else {
        return Ok(());
    };
    let JsonValue::Object(entries) = value else {
        return Err(invalid_contract_verification(
            "contract_verification.packet_schemas must be an object",
        ));
    };
    if entries.is_empty() {
        return Err(invalid_contract_verification(
            "contract_verification.packet_schemas must not be empty",
        ));
    }
    let mut references = Vec::with_capacity(entries.len());
    for (output, value) in entries {
        let (packet, digest) = packet_schema_verification(output, value)?;
        references.push(Reference::with_uri(
            ReferenceType::Verification,
            format!("runx:verification:packet_schema_verified:{packet}:{digest}"),
        ));
    }
    let statement = "Packet outputs satisfy their declared packet schemas";
    result.criteria.push(SuccessCriterion {
        criterion_id: "packet_schemas_verified".into(),
        statement: statement.into(),
        required: true,
    });
    result.bindings.push(CriterionBinding {
        criterion_id: "packet_schemas_verified".into(),
        status: CriterionStatus::Verified,
        evidence_refs: Vec::new(),
        verification_refs: references,
        summary: Some(statement.into()),
    });
    Ok(())
}

fn packet_schema_verification<'a>(
    output: &str,
    value: &'a JsonValue,
) -> Result<(&'a str, &'a str), RuntimeError> {
    let JsonValue::Object(entry) = value else {
        return Err(invalid_contract_verification(format!(
            "contract_verification.packet_schemas.{output} must be an object"
        )));
    };
    const ALLOWED_FIELDS: &[&str] = &["packet", "schema_sha256"];
    if let Some(field) = entry
        .keys()
        .find(|field| !ALLOWED_FIELDS.contains(&field.as_str()))
    {
        return Err(invalid_contract_verification(format!(
            "contract_verification.packet_schemas.{output} contains undeclared field '{field}'"
        )));
    }
    let packet = required_verification_string(entry, output, "packet")?;
    let digest = required_verification_string(entry, output, "schema_sha256")?;
    if !digest.starts_with("sha256:") {
        return Err(invalid_contract_verification(format!(
            "contract_verification.packet_schemas.{output}.schema_sha256 must be a sha256 digest"
        )));
    }
    Ok((packet, digest))
}

fn required_verification_string<'a>(
    entry: &'a JsonObject,
    output: &str,
    field: &str,
) -> Result<&'a str, RuntimeError> {
    entry
        .get(field)
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            invalid_contract_verification(format!(
                "contract_verification.packet_schemas.{output}.{field} must be a non-empty string"
            ))
        })
}

fn optional_verification_digest<'a>(
    verification: &'a JsonObject,
    field: &str,
) -> Result<Option<&'a str>, RuntimeError> {
    let Some(value) = verification.get(field) else {
        return Ok(None);
    };
    let Some(digest) = value.as_str().filter(|value| value.starts_with("sha256:")) else {
        return Err(invalid_contract_verification(format!(
            "contract_verification.{field} must be a sha256 digest"
        )));
    };
    Ok(Some(digest))
}

fn push_verified_criterion(
    result: &mut ContractVerificationCriteria,
    criterion_id: &str,
    statement: &str,
    digest: &str,
) {
    let reference = Reference::with_uri(
        ReferenceType::Verification,
        format!("runx:verification:{criterion_id}:{digest}"),
    );
    result.criteria.push(SuccessCriterion {
        criterion_id: criterion_id.into(),
        statement: statement.into(),
        required: true,
    });
    result.bindings.push(CriterionBinding {
        criterion_id: criterion_id.into(),
        status: CriterionStatus::Verified,
        evidence_refs: Vec::new(),
        verification_refs: vec![reference],
        summary: Some(statement.into()),
    });
}

fn invalid_contract_verification(message: impl Into<String>) -> RuntimeError {
    RuntimeError::ReceiptInvalid {
        message: message.into(),
    }
}

pub fn graph_receipt(
    scope: RunScope<'_>,
    steps: &[StepRun],
    created_at: &str,
) -> Result<Receipt, RuntimeError> {
    graph_receipt_with_signature_policy(
        scope,
        steps,
        created_at,
        RuntimeReceiptSignaturePolicy::local_development(),
    )
}

pub fn graph_receipt_with_signature_policy(
    scope: RunScope<'_>,
    steps: &[StepRun],
    created_at: &str,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Receipt, RuntimeError> {
    graph_receipt_with_closure(
        scope,
        steps,
        created_at,
        GraphClosure::completed(scope.graph_name),
        signature_policy,
    )
}

pub(crate) struct GraphClosure {
    pub(crate) disposition: ClosureDisposition,
    pub(crate) reason_code: String,
    pub(crate) summary: String,
}

impl GraphClosure {
    pub(crate) fn completed(graph_name: &str) -> Self {
        Self {
            disposition: ClosureDisposition::Closed,
            reason_code: "graph_closed".to_owned(),
            summary: format!("graph {graph_name} completed"),
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
    let mut receipt = build_graph_receipt(scope, child_refs, created_at, &closure);
    bind_graph_operator_refs(receipt.as_mut(), steps);
    let receipt = receipt.seal(signature_policy)?;

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

fn bind_graph_operator_refs(receipt: &mut Receipt, steps: &[StepRun]) {
    let mut refs = Vec::<Reference>::new();
    for reference in steps
        .iter()
        .flat_map(|step| step.receipt.acts.iter())
        .flat_map(|act| act.artifact_refs.iter())
        .filter(|reference| reference.uri.as_str().contains("operator_context"))
    {
        if !refs.iter().any(|existing| existing.uri == reference.uri) {
            refs.push(reference.clone());
        }
    }
    let artifacts = refs
        .iter()
        .filter(|reference| reference.reference_type == ReferenceType::Artifact)
        .cloned()
        .collect::<Vec<_>>();
    let decisions = refs
        .iter()
        .filter(|reference| reference.reference_type == ReferenceType::Decision)
        .cloned()
        .collect::<Vec<_>>();
    for act in &mut receipt.acts {
        act.artifact_refs.extend(artifacts.iter().cloned());
        act.artifact_refs.extend(decisions.iter().cloned());
        for criterion in &mut act.criterion_bindings {
            criterion.evidence_refs.extend(refs.iter().cloned());
            criterion
                .verification_refs
                .extend(decisions.iter().cloned());
        }
    }
    for decision in &mut receipt.decisions {
        decision.artifact_refs.extend(artifacts.iter().cloned());
        decision
            .justification
            .evidence_refs
            .extend(refs.iter().cloned());
    }
    for criterion in &mut receipt.seal.criteria {
        criterion.evidence_refs.extend(refs.iter().cloned());
        criterion
            .verification_refs
            .extend(decisions.iter().cloned());
    }
}

fn build_graph_receipt(
    scope: RunScope<'_>,
    children: Vec<Reference>,
    created_at: &str,
    closure: &GraphClosure,
) -> UnsealedReceipt {
    let child_identity = graph_child_identity(&children);
    let mut receipt = build_unsealed_receipt(BuildReceipt {
        class: ReceiptClass::Executed,
        scope,
        node: Node::Graph,
        kind: receipt_subject_kind::GRAPH.into(),
        created_at,
        decisions: Vec::new(),
        acts: Vec::new(),
        seal: seal(
            closure.disposition.clone(),
            closure.reason_code.clone(),
            closure.summary.clone(),
            created_at,
            Vec::new(),
        ),
        children,
        signals: Vec::new(),
        authority: SealAuthority::default(),
        authority_override: None,
        previous: None,
    });
    let receipt_body = receipt.as_mut();
    receipt_body.subject.reference.locator = Some(child_identity.clone().into());
    receipt_body.idempotency.content_hash = child_identity.into();
    receipt
}

// Step identity binds the sealed contract claim and the semantic identities of
// direct child receipts. Raw transport values, diagnostics, and child proof
// envelopes remain excluded: a replay of the same declared claim and child
// identities must retain its address, while a changed nested execution must
// propagate to every ancestor.
fn bind_step_identity(
    receipt: &mut Receipt,
    output: &InvocationOutput,
    claim: &JsonObject,
) -> Result<(), RuntimeError> {
    let identity = step_identity_locator(receipt, output.succeeded(), claim)?;
    receipt.subject.reference.locator = Some(identity.clone().into());
    receipt.idempotency.content_hash = identity.into();
    Ok(())
}

fn step_identity_locator(
    receipt: &Receipt,
    succeeded: bool,
    claim: &JsonObject,
) -> Result<String, RuntimeError> {
    let mut identity = JsonObject::from([
        (
            "status".to_owned(),
            JsonValue::String(if succeeded { "success" } else { "failure" }.to_owned()),
        ),
        ("claim".to_owned(), JsonValue::Object(claim.clone())),
    ]);
    let child_ids = receipt
        .lineage
        .as_ref()
        .into_iter()
        .flat_map(|lineage| &lineage.children)
        .map(|reference| JsonValue::String(reference.uri.to_string()))
        .collect::<Vec<_>>();
    if !child_ids.is_empty() {
        identity.insert("children".to_owned(), JsonValue::Array(child_ids));
    }
    let material = canonical_stable_json(&JsonValue::Object(identity)).map_err(|error| {
        RuntimeError::ReceiptInvalid {
            message: error.to_string(),
        }
    })?;
    Ok(format!("sha256:{}", sha256_hex(material.as_bytes())))
}

pub(crate) fn validate_step_receipt_claim(
    receipt: &Receipt,
    succeeded: bool,
    claim: &JsonObject,
) -> Result<(), RuntimeError> {
    let expected = step_identity_locator(receipt, succeeded, claim)?;
    if receipt
        .subject
        .reference
        .locator
        .as_ref()
        .is_some_and(|locator| locator.as_str() == expected)
    {
        return Ok(());
    }
    Err(RuntimeError::ReceiptInvalid {
        message: format!(
            "receipt {} does not bind its retained step claim",
            receipt.id
        ),
    })
}

fn graph_child_identity(children: &[Reference]) -> String {
    let child_ids = children
        .iter()
        .map(|reference| reference.uri.as_str())
        .collect::<Vec<_>>();
    let canonical = serde_json::to_vec(&child_ids).unwrap_or_default();
    format!("sha256:{}", sha256_hex(&canonical))
}

fn validate_receipt_tree_with_policy<'a>(
    root: &Receipt,
    children: impl IntoIterator<Item = &'a Receipt>,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<(), RuntimeError> {
    super::tree::validate_runtime_receipt_tree_refs_with_policy(
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
    /// The run's domain act.
    Act,
}

impl Node<'_> {
    /// `runx:run:<id>`, `runx:run:<id>#<step>` (`#<step>@<attempt>` after the
    /// first attempt) or `runx:run:<id>#act`.
    fn uri(self, run_id: &str) -> String {
        match self {
            Self::Graph => format!("runx:run:{run_id}"),
            Self::Step { step_id, attempt } if attempt <= 1 => {
                format!("runx:run:{run_id}#{step_id}")
            }
            Self::Step { step_id, attempt } => format!("runx:run:{run_id}#{step_id}@{attempt}"),
            Self::Act => format!("runx:run:{run_id}#act"),
        }
    }
}

struct BuildReceipt<'a> {
    class: ReceiptClass,
    scope: RunScope<'a>,
    node: Node<'a>,
    kind: NonEmptyString,
    created_at: &'a str,
    decisions: Vec<Decision>,
    acts: Vec<ReceiptAct>,
    seal: Seal,
    children: Vec<Reference>,
    signals: Vec<Reference>,
    authority: SealAuthority,
    /// Fully-built authority for a domain act seal. When `None`, the generic
    /// `local_runtime` authority is used.
    authority_override: Option<ReceiptAuthority>,
    /// The predecessor receipt this one chains from (`lineage.previous`), e.g. a
    /// judgment chaining from the delivery it judged. `None` for generic seals.
    previous: Option<Reference>,
}

struct UnsealedReceipt(Receipt);

impl UnsealedReceipt {
    fn as_mut(&mut self) -> &mut Receipt {
        &mut self.0
    }

    fn seal(
        mut self,
        signature_policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<Receipt, RuntimeError> {
        // Content-address the id over the canonical body (id = hash(canonical_body),
        // excluding id/signature/digest/metadata/lineage) before the digest commits
        // it. Lineage is excluded so parent<->child wiring does not perturb the id.
        content_address_receipt(&mut self.0, signature_policy)?;
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
        class,
        scope,
        node,
        kind,
        created_at,
        decisions,
        acts,
        seal,
        children,
        signals,
        authority,
        authority_override,
        previous,
    } = parts;
    let lineage = Lineage {
        parent: None,
        previous,
        children,
        resume_ref: None,
    };
    let subject_uri = node.uri(scope.run_id);
    UnsealedReceipt(Receipt {
        schema: ReceiptSchema::V1,
        // Replaced by the content address when the receipt is sealed.
        id: "rcpt_unsealed".into(),
        created_at: created_at.into(),
        canonicalization: RECEIPT_CANONICALIZATION.into(),
        issuer: local_runtime_issuer(),
        signature: placeholder_signature(),
        digest: "sha256:runtime-skeleton".into(),
        idempotency: idempotency(&subject_uri, &signals),
        class,
        subject: subject(scope, subject_uri, kind),
        authority: authority_override.unwrap_or_else(|| local_authority(authority)),
        signals,
        decisions,
        acts,
        evidence: Vec::new(),
        seal,
        lineage: Some(lineage),
        metadata: None,
    })
}

/// The planner deliberation, inline in `decisions[]`. The `selected_act_id`
/// integrity property is checked against the inline `acts[]` at verify time.
fn decisions(
    node_id: &str,
    act: &ReceiptAct,
    signal_refs: &[Reference],
    artifact_refs: &[Reference],
) -> Vec<Decision> {
    vec![Decision {
        decision_id: format!("dec_{node_id}").into(),
        choice: DecisionChoice::Open,
        inputs: DecisionInputs {
            signal_refs: signal_refs.to_vec(),
            ..DecisionInputs::default()
        },
        proposed_intent: Intent {
            purpose: format!("Open runtime node {node_id}").into(),
            legitimacy: "Local graph execution requested this node".into(),
            success_criteria: Vec::new(),
            constraints: Vec::new(),
            derived_from: Vec::new(),
        },
        selected_act_id: Some(act.id.clone()),
        selected_harness_ref: None,
        justification: DecisionJustification {
            summary: "runtime graph planner selected this node".into(),
            evidence_refs: signal_refs.to_vec(),
        },
        closure: None,
        artifact_refs: artifact_refs.to_vec(),
    }]
}

fn seal(
    disposition: ClosureDisposition,
    reason_code: String,
    summary: String,
    closed_at: &str,
    criteria: Vec<CriterionBinding>,
) -> Seal {
    Seal {
        disposition,
        reason_code: reason_code.into(),
        summary: summary.into(),
        closed_at: closed_at.into(),
        last_observed_at: closed_at.into(),
        criteria,
    }
}

/// The run node a receipt seals, labelled with the graph that ran.
fn subject(scope: RunScope<'_>, uri: String, kind: NonEmptyString) -> Subject {
    let mut reference = Reference::with_uri(ReferenceType::Run, uri);
    reference.label = Some(scope.graph_name.into());
    Subject {
        kind,
        reference,
        input_context: None,
        commitments: Vec::new(),
        paid_invocation: None,
    }
}

/// The stable identity of the local runtime's enforcement profile. The hash is
/// derived over this id plus the profile's redaction/setup/teardown refs, never
/// over [`ReceiptEnforcement`] itself (which carries the resulting hash).
const RUNTIME_ENFORCEMENT_PROFILE_ID: &str = "runx.runtime.enforcement.profile.v1";

/// Content-address an enforcement profile as `sha256:<digest>` over its stable
/// id plus its redaction/setup/teardown ref inputs in deterministic order. Both
/// runtime `ReceiptEnforcement` build sites (the generic `authority()` helper and
/// the domain-act seal) call this so the profile hash has one source of truth.
fn enforcement_profile_hash(
    redaction_refs: &[Reference],
    setup_refs: &[Reference],
    teardown_refs: &[Reference],
) -> NonEmptyString {
    let identity = serde_json::json!({
        "profile_id": RUNTIME_ENFORCEMENT_PROFILE_ID,
        "redaction_refs": redaction_refs,
        "setup_refs": setup_refs,
        "teardown_refs": teardown_refs,
    });
    let canonical = serde_json::to_vec(&identity).unwrap_or_default();
    format!("sha256:{}", sha256_hex(&canonical)).into()
}

/// The authority a step receipt seals: the grants and scopes that admitted
/// the step, and the execution terms derived for it with their proofs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SealAuthority {
    pub grant_refs: Vec<Reference>,
    pub scope_refs: Vec<Reference>,
    pub terms: Vec<runx_contracts::AuthorityTerm>,
    pub derivations: Vec<runx_contracts::AuthorityDerivation>,
}

fn local_authority(authority: SealAuthority) -> ReceiptAuthority {
    let SealAuthority {
        grant_refs,
        scope_refs,
        terms,
        derivations,
    } = authority;
    let redaction_refs = Vec::new();
    let setup_refs = Vec::new();
    let teardown_refs = Vec::new();
    ReceiptAuthority {
        actor_ref: Reference::runx(ReferenceType::Principal, "local_runtime"),
        authority_proof_refs: Vec::new(),
        grant_refs,
        scope_refs,
        terms,
        derivations,
        attenuation: AuthorityAttenuation {
            parent_authority_ref: None,
            subset_proof: None,
        },
        mandate_ref: None,
        enforcement: ReceiptEnforcement {
            profile_hash: enforcement_profile_hash(&redaction_refs, &setup_refs, &teardown_refs),
            execution_boundary: None,
            redaction_refs,
            setup_refs,
            teardown_refs,
        },
    }
}

/// A governed turn's domain act, assembled from trusted sources (the skill's act
/// declaration, the driver's pinned beat inputs, the delivered credential) plus
/// the model's reason text. This is what makes a receipt read as "operator judged
/// claim c-4417, rejected" instead of "a turn ran". The model never sets the
/// form, target, choice, or authority; it supplies only the reason prose.
pub(crate) struct DomainActFrame {
    pub form: ActForm,
    pub purpose: NonEmptyString,
    pub legitimacy: NonEmptyString,
    pub summary: NonEmptyString,
    pub target_refs: Vec<Reference>,
    pub artifact_refs: Vec<Reference>,
    pub decision_choice: DecisionChoice,
    pub decision_summary: NonEmptyString,
    pub actor_ref: Reference,
    pub authority_grant_refs: Vec<Reference>,
    pub authority_scope_refs: Vec<Reference>,
    /// The member's own authority term (the child grant minted from the charter).
    /// Empty for an unattenuated turn.
    pub authority_terms: Vec<AuthorityTerm>,
    /// The charter attenuation: the parent authority and the subset proof that the
    /// child term is no broader. `None` for a root (unattenuated) turn.
    pub authority_attenuation: Option<AuthorityAttenuation>,
    pub previous: Option<Reference>,
}

pub(crate) struct DomainActReceiptRequest<'a> {
    pub scope: RunScope<'a>,
    pub succeeded: bool,
    pub created_at: &'a str,
    pub disposition: ClosureDisposition,
    pub reason_code: String,
    pub seal_summary: String,
    pub frame: DomainActFrame,
    pub verification_metadata: JsonObject,
    pub signature_policy: RuntimeReceiptSignaturePolicy<'a>,
}

/// Seal a governed turn as its domain act. Reuses the generic receipt assembly
/// (`build_unsealed_receipt`/`UnsealedReceipt::seal`) but fills the act,
/// decision, and authority from the
/// trusted `DomainActFrame`. Transport (tool names, urls, status codes, tokens)
/// never enters the receipt.
// Function rationale: domain-act receipt assembly binds act,
// decision, authority, and signature policy in one auditable receipt boundary.
pub(crate) fn domain_act_receipt(
    request: DomainActReceiptRequest<'_>,
) -> Result<Receipt, RuntimeError> {
    let DomainActReceiptRequest {
        scope,
        succeeded,
        created_at,
        disposition,
        reason_code,
        seal_summary,
        frame,
        verification_metadata,
        signature_policy,
    } = request;
    let verification = contract_verification_criteria(&verification_metadata)?;
    let status = if succeeded {
        CriterionStatus::Verified
    } else {
        CriterionStatus::Failed
    };
    let closure = Closure {
        disposition: disposition.clone(),
        reason_code: reason_code.clone().into(),
        summary: frame.summary.clone(),
        closed_at: created_at.into(),
    };
    let criterion = CriterionBinding {
        criterion_id: "act_closed".into(),
        status,
        evidence_refs: Vec::new(),
        verification_refs: Vec::new(),
        summary: Some(frame.summary.clone()),
    };
    let intent = Intent {
        purpose: frame.purpose.clone(),
        legitimacy: frame.legitimacy.clone(),
        success_criteria: verification.criteria,
        constraints: Vec::new(),
        derived_from: Vec::new(),
    };
    let mut criterion_bindings = vec![criterion.clone()];
    criterion_bindings.extend(verification.bindings.iter().cloned());
    let act = ReceiptAct {
        id: "act_run".into(),
        form: frame.form,
        intent: intent.clone(),
        summary: frame.summary.clone(),
        criterion_bindings,
        by: None,
        source_refs: Vec::new(),
        target_refs: frame.target_refs.clone(),
        artifact_refs: frame.artifact_refs.clone(),
        context_ref: None,
        closure: closure.clone(),
        revision: None,
        verification: None,
    };
    let decision = Decision {
        decision_id: "dec_run".into(),
        choice: frame.decision_choice,
        inputs: DecisionInputs {
            target_ref: frame.target_refs.first().cloned(),
            ..DecisionInputs::default()
        },
        proposed_intent: intent,
        selected_act_id: Some(act.id.clone()),
        selected_harness_ref: None,
        justification: DecisionJustification {
            summary: frame.decision_summary,
            evidence_refs: Vec::new(),
        },
        closure: Some(closure),
        artifact_refs: frame.artifact_refs.clone(),
    };
    let redaction_refs = Vec::new();
    let setup_refs = Vec::new();
    let teardown_refs = Vec::new();
    let authority = ReceiptAuthority {
        actor_ref: frame.actor_ref,
        authority_proof_refs: Vec::new(),
        grant_refs: frame.authority_grant_refs,
        scope_refs: frame.authority_scope_refs,
        terms: frame.authority_terms,
        derivations: Vec::new(),
        attenuation: frame.authority_attenuation.unwrap_or(AuthorityAttenuation {
            parent_authority_ref: None,
            subset_proof: None,
        }),
        mandate_ref: None,
        enforcement: ReceiptEnforcement {
            profile_hash: enforcement_profile_hash(&redaction_refs, &setup_refs, &teardown_refs),
            execution_boundary: None,
            redaction_refs,
            setup_refs,
            teardown_refs,
        },
    };
    let mut seal_criteria = vec![criterion];
    seal_criteria.extend(verification.bindings);
    let seal = seal(
        disposition,
        reason_code,
        seal_summary,
        created_at,
        seal_criteria,
    );
    let mut receipt = build_unsealed_receipt(BuildReceipt {
        class: ReceiptClass::Executed,
        scope,
        node: Node::Act,
        kind: receipt_subject_kind::SKILL.into(),
        created_at,
        decisions: vec![decision],
        acts: vec![act],
        seal,
        children: Vec::new(),
        signals: Vec::new(),
        authority: SealAuthority::default(),
        authority_override: Some(authority),
        previous: frame.previous,
    });
    let acts = canonical_stable_json(&JsonValue::from_serialize(&receipt.as_mut().acts).map_err(
        |error| RuntimeError::ReceiptInvalid {
            message: error.to_string(),
        },
    )?)
    .map_err(|error| RuntimeError::ReceiptInvalid {
        message: error.to_string(),
    })?;
    receipt.as_mut().idempotency.content_hash =
        format!("sha256:{}", sha256_hex(acts.as_bytes())).into();
    receipt.seal(signature_policy)
}

/// What the receipt is for (its run and node), what triggered it (its signal
/// references) and what it produced. The content hash starts as the digest
/// of nothing and is bound to the sealed claim by the node's builder.
fn idempotency(subject_uri: &str, signals: &[Reference]) -> ReceiptIdempotency {
    let digest = |bytes: &[u8]| format!("sha256:{}", sha256_hex(bytes));
    let signal_uris = signals
        .iter()
        .map(|reference| reference.uri.as_str())
        .collect::<Vec<_>>();
    ReceiptIdempotency {
        intent_key: digest(subject_uri.as_bytes()).into(),
        trigger_fingerprint: digest(&serde_json::to_vec(&signal_uris).unwrap_or_default()).into(),
        content_hash: digest(b"").into(),
    }
}

fn output_refs(
    output: &InvocationOutput,
    mut refs: StepOutputRefs,
) -> Result<StepOutputRefs, RuntimeError> {
    if let Some(request_id) = json_string_field(&output.metadata, "agent_request_id") {
        let reference = Reference {
            uri: format!("runx:agent_act:{request_id}").into(),
            reference_type: ReferenceType::Act,
            provider: None,
            locator: Some(request_id.to_owned().into()),
            label: Some("agent act request".to_owned().into()),
            observed_at: None,
            proof_kind: None,
        };
        refs.source_refs.insert(0, reference.clone());
        refs.evidence_refs.insert(0, reference);
    }
    collect_supervisor_metadata_refs(&output.metadata, &mut refs)?;
    collect_credential_delivery_refs(&output.metadata, &mut refs)?;
    Ok(refs)
}

fn collect_supervisor_metadata_refs(
    metadata: &JsonObject,
    refs: &mut StepOutputRefs,
) -> Result<(), RuntimeError> {
    let mut verification_refs =
        effect_verification_refs(metadata).map_err(|error| RuntimeError::ReceiptInvalid {
            message: format!("invalid effect verification metadata: {error}"),
        })?;
    refs.verification_refs.append(&mut verification_refs);
    Ok(())
}

fn collect_credential_delivery_refs(
    metadata: &JsonObject,
    refs: &mut StepOutputRefs,
) -> Result<(), RuntimeError> {
    let Some(value) = metadata.get(CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA) else {
        return Ok(());
    };
    let observations = serde_json::to_value(value)
        .and_then(serde_json::from_value::<Vec<CredentialDeliveryObservation>>)
        .map_err(|error| RuntimeError::ReceiptInvalid {
            message: format!("invalid credential delivery observation metadata: {error}"),
        })?;

    for reference in observations
        .into_iter()
        .flat_map(|observation| observation.credential_refs)
    {
        if !refs
            .verification_refs
            .iter()
            .any(|existing| existing == &reference)
        {
            refs.verification_refs.push(reference);
        }
    }
    Ok(())
}

fn disposition(output: &InvocationOutput) -> ClosureDisposition {
    if output.succeeded() {
        ClosureDisposition::Closed
    } else {
        ClosureDisposition::Failed
    }
}

fn output_summary(output: &InvocationOutput) -> String {
    if output.succeeded() {
        "step completed successfully".to_owned()
    } else {
        output
            .failure_message()
            .unwrap_or_else(|| "step failed without diagnostic output".to_owned())
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

fn placeholder_signature() -> ReceiptSignature {
    ReceiptSignature {
        alg: SignatureAlgorithm::Ed25519,
        value: "sig:pending".into(),
    }
}

fn content_address_receipt(
    receipt: &mut Receipt,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<(), RuntimeError> {
    signature_policy.prepare_receipt(receipt)?;
    receipt.id = content_addressed_receipt_id(receipt)
        .map_err(|error| RuntimeError::ReceiptInvalid {
            message: error.to_string(),
        })?
        .into();
    Ok(())
}

pub(crate) fn proof_context<'a>(
    signature_verifier: Option<&'a dyn SignatureVerifier>,
    receipt: &Receipt,
) -> ReceiptProofContext<'a> {
    ReceiptProofContext {
        signature_verifier,
        authority_verified: authority_attenuation_verified(&receipt.authority.attenuation),
        external_attestations_verified: true,
        verified_redaction_refs: std::collections::BTreeSet::new(),
        verified_hash_commitments: std::collections::BTreeSet::new(),
    }
}

#[derive(Clone, Copy)]
pub struct RuntimeReceiptSignaturePolicy<'a> {
    mode: RuntimeReceiptSignatureMode,
    production_signer: Option<&'a dyn RuntimeReceiptSigner>,
    production_verifier: Option<&'a dyn SignatureVerifier>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeReceiptSignatureMode {
    LocalDevelopment,
    Production,
}

impl std::fmt::Debug for RuntimeReceiptSignaturePolicy<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeReceiptSignaturePolicy")
            .field("mode", &self.mode)
            .field(
                "production_signer_supplied",
                &self.production_signer.is_some(),
            )
            .field(
                "production_verifier_supplied",
                &self.production_verifier.is_some(),
            )
            .finish()
    }
}

impl<'a> RuntimeReceiptSignaturePolicy<'a> {
    #[must_use]
    pub fn local_development() -> Self {
        Self {
            mode: RuntimeReceiptSignatureMode::LocalDevelopment,
            production_signer: None,
            production_verifier: None,
        }
    }

    #[must_use]
    pub fn production(verifier: &'a dyn SignatureVerifier) -> Self {
        Self {
            mode: RuntimeReceiptSignatureMode::Production,
            production_signer: None,
            production_verifier: Some(verifier),
        }
    }

    #[must_use]
    pub fn production_signing(
        signer: &'a dyn RuntimeReceiptSigner,
        verifier: &'a dyn SignatureVerifier,
    ) -> Self {
        Self {
            mode: RuntimeReceiptSignatureMode::Production,
            production_signer: Some(signer),
            production_verifier: Some(verifier),
        }
    }

    #[must_use]
    pub fn production_signing_without_verifier(signer: &'a dyn RuntimeReceiptSigner) -> Self {
        Self {
            mode: RuntimeReceiptSignatureMode::Production,
            production_signer: Some(signer),
            production_verifier: None,
        }
    }

    #[must_use]
    pub fn production_without_verifier() -> Self {
        Self {
            mode: RuntimeReceiptSignatureMode::Production,
            production_signer: None,
            production_verifier: None,
        }
    }

    #[must_use]
    pub fn allows_local_pseudo_signatures(&self) -> bool {
        self.mode == RuntimeReceiptSignatureMode::LocalDevelopment
    }

    #[must_use]
    pub fn can_report_production_verified(&self) -> bool {
        self.mode == RuntimeReceiptSignatureMode::Production && self.production_verifier.is_some()
    }

    fn prepare_receipt(self, receipt: &mut Receipt) -> Result<(), RuntimeError> {
        if self.allows_local_pseudo_signatures() {
            receipt.issuer = local_runtime_issuer();
            return Ok(());
        }
        let Some(signer) = self.production_signer else {
            return Err(signing_error(RuntimeReceiptSigningError::MissingSigner));
        };
        if self.production_verifier.is_none() {
            return Err(signing_error(RuntimeReceiptSigningError::MissingVerifier));
        }
        let issuer = signer.issuer();
        validate_production_issuer(&issuer).map_err(signing_error)?;
        receipt.issuer = issuer;
        Ok(())
    }

    fn sign_receipt(self, receipt: &mut Receipt, body_digest: &str) -> Result<(), RuntimeError> {
        if self.allows_local_pseudo_signatures() {
            receipt.signature.value = format!("sig:{body_digest}").into();
            return Ok(());
        }
        let Some(signer) = self.production_signer else {
            return Err(signing_error(RuntimeReceiptSigningError::MissingSigner));
        };
        let Some(verifier) = self.production_verifier else {
            return Err(signing_error(RuntimeReceiptSigningError::MissingVerifier));
        };
        let signature = signer
            .sign_receipt_body(body_digest)
            .map_err(signing_error)?;
        if signature.alg != SignatureAlgorithm::Ed25519 {
            return Err(signing_error(
                RuntimeReceiptSigningError::UnsupportedAlgorithm,
            ));
        }
        if is_local_pseudo_signature(&signature.value) {
            return Err(signing_error(RuntimeReceiptSigningError::PseudoSignature));
        }
        receipt.signature = signature;
        verifier
            .verify(&receipt.issuer, &receipt.signature, body_digest)
            .map_err(RuntimeReceiptSigningError::SignatureVerification)
            .map_err(signing_error)
    }

    fn verifier(self) -> Option<RuntimeReceiptSignatureVerifier<'a>> {
        if self.mode == RuntimeReceiptSignatureMode::Production
            && self.production_verifier.is_none()
        {
            return None;
        }
        Some(RuntimeReceiptSignatureVerifier { policy: self })
    }
}

pub(crate) struct RuntimeReceiptProofContextProvider<'a> {
    signature_verifier: Option<RuntimeReceiptSignatureVerifier<'a>>,
}

impl<'a> RuntimeReceiptProofContextProvider<'a> {
    pub(crate) fn new(signature_policy: RuntimeReceiptSignaturePolicy<'a>) -> Self {
        Self {
            signature_verifier: signature_policy.verifier(),
        }
    }
}

impl ReceiptProofContextProvider for RuntimeReceiptProofContextProvider<'_> {
    fn proof_context<'a>(&'a self, receipt: &Receipt) -> ReceiptProofContext<'a> {
        proof_context(
            self.signature_verifier
                .as_ref()
                .map(|verifier| verifier as &dyn SignatureVerifier),
            receipt,
        )
    }
}

struct RuntimeReceiptSignatureVerifier<'a> {
    policy: RuntimeReceiptSignaturePolicy<'a>,
}

impl SignatureVerifier for RuntimeReceiptSignatureVerifier<'_> {
    fn verify(
        &self,
        issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        if is_local_pseudo_signature(&signature.value) {
            if !self.policy.allows_local_pseudo_signatures() {
                return Err(SignatureVerificationFailure::MalformedSignature);
            }
            return if signature.value.strip_prefix("sig:") == Some(body_digest) {
                Ok(())
            } else {
                Err(SignatureVerificationFailure::SignatureMismatch)
            };
        }
        let Some(verifier) = self.policy.production_verifier else {
            return Err(SignatureVerificationFailure::MissingKey);
        };
        verifier.verify(issuer, signature, body_digest)
    }
}

fn signing_error(error: RuntimeReceiptSigningError) -> RuntimeError {
    RuntimeError::ReceiptInvalid {
        message: error.to_string(),
    }
}

fn authority_attenuation_verified(attenuation: &AuthorityAttenuation) -> bool {
    match (&attenuation.parent_authority_ref, &attenuation.subset_proof) {
        (Some(parent), Some(proof)) => {
            proof.parent_authority_ref == *parent
                && matches!(proof.result, AuthoritySubsetResult::Subset)
        }
        _ => false,
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

    use super::*;
    use crate::adapter::InvocationStatus;
    use runx_contracts::{
        CredentialDeliveryMode, CredentialDeliveryObservationSchema,
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
        )?;

        let verification_refs = &receipt.acts[0].criterion_bindings[0].verification_refs;
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
        assert_eq!(
            receipt.seal.criteria[0].verification_refs,
            *verification_refs
        );

        let sealed_digest = canonical_receipt_body_digest(&receipt)?;
        let mut without_credential_ref = receipt.clone();
        without_credential_ref.acts[0].criterion_bindings[0]
            .verification_refs
            .clear();
        without_credential_ref.seal.criteria[0]
            .verification_refs
            .clear();
        let unsealed_digest = canonical_receipt_body_digest(&without_credential_ref)?;
        assert_ne!(sealed_digest, unsealed_digest);
        Ok(())
    }

    #[test]
    fn contract_verification_is_bound_into_signed_act_and_seal() -> Result<(), TestError> {
        let mut verification = JsonObject::new();
        verification.insert(
            "output_contract_sha256".to_owned(),
            JsonValue::String("sha256:output".to_owned()),
        );
        verification.insert(
            "voice_profile_sha256".to_owned(),
            JsonValue::String("sha256:voice".to_owned()),
        );
        verification.insert(
            "packet_schemas".to_owned(),
            JsonValue::Object(BTreeMap::from([(
                "plan".to_owned(),
                JsonValue::Object(BTreeMap::from([
                    (
                        "packet".to_owned(),
                        JsonValue::String("runx.test.plan.v1".to_owned()),
                    ),
                    (
                        "schema_sha256".to_owned(),
                        JsonValue::String("sha256:packet".to_owned()),
                    ),
                ])),
            )])),
        );
        let mut metadata = JsonObject::new();
        metadata.insert(
            CONTRACT_VERIFICATION_METADATA.to_owned(),
            JsonValue::Object(verification),
        );
        let output =
            InvocationOutput::runtime_success(JsonValue::Object(JsonObject::new()), 1, metadata);

        let receipt = step_receipt(
            test_scope("agent_graph"),
            "agent_step",
            1,
            &output,
            &JsonObject::new(),
            "2026-05-28T00:00:00Z",
        )?;

        assert_eq!(receipt.acts[0].intent.success_criteria.len(), 4);
        assert_eq!(receipt.acts[0].criterion_bindings.len(), 4);
        assert_eq!(receipt.seal.criteria.len(), 4);
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
    fn malformed_contract_verification_fails_closed() {
        let mut metadata = JsonObject::new();
        metadata.insert(
            CONTRACT_VERIFICATION_METADATA.to_owned(),
            JsonValue::String("untrusted".to_owned()),
        );
        let output =
            InvocationOutput::runtime_success(JsonValue::Object(JsonObject::new()), 1, metadata);

        assert!(
            step_receipt(
                test_scope("agent_graph"),
                "agent_step",
                1,
                &output,
                &JsonObject::new(),
                "2026-05-28T00:00:00Z"
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_runtime_evidence_metadata_fails_closed() {
        for key in [
            crate::effects::EFFECT_VERIFICATION_REFS_METADATA,
            CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA,
        ] {
            let output = InvocationOutput::runtime_success(
                JsonValue::Object(JsonObject::new()),
                1,
                JsonObject::from([(key.to_owned(), JsonValue::String("untrusted".to_owned()))]),
            );

            assert!(
                step_receipt(
                    test_scope("evidence_graph"),
                    "evidence_step",
                    1,
                    &output,
                    &JsonObject::new(),
                    "2026-05-28T00:00:00Z"
                )
                .is_err(),
                "{key} must not be silently discarded"
            );
        }

        let output = InvocationOutput::runtime_success(
            JsonValue::Object(JsonObject::new()),
            1,
            JsonObject::from([(
                runx_contracts::EXECUTION_BOUNDARY_METADATA.to_owned(),
                JsonValue::String("untrusted".to_owned()),
            )]),
        );
        assert!(
            step_receipt(
                test_scope("evidence_graph"),
                "boundary_step",
                1,
                &output,
                &JsonObject::new(),
                "2026-05-28T00:00:00Z"
            )
            .is_err(),
            "malformed execution boundary must not be silently discarded"
        );
    }

    #[test]
    fn execution_limit_hit_is_copied_into_receipt_read_metadata() -> Result<(), TestError> {
        let limits = JsonObject::from([(
            "hit".to_owned(),
            JsonValue::Object(JsonObject::from([
                (
                    "id".to_owned(),
                    JsonValue::String("javascript.wall_milliseconds".to_owned()),
                ),
                (
                    "configured".to_owned(),
                    JsonValue::Number(runx_contracts::JsonNumber::U64(7_000)),
                ),
            ])),
        )]);
        let output = InvocationOutput::runtime_failure(
            JsonValue::Null,
            "wall limit reached",
            7_000,
            JsonObject::from([(
                EXECUTION_LIMITS_METADATA.to_owned(),
                JsonValue::Object(limits.clone()),
            )]),
        );

        let receipt = step_receipt(
            test_scope("limit_graph"),
            "limit_step",
            1,
            &output,
            &JsonObject::new(),
            "2026-05-28T00:00:00Z",
        )?;
        assert_eq!(
            receipt
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get(EXECUTION_LIMITS_METADATA)),
            Some(&JsonValue::Object(limits))
        );
        Ok(())
    }

    #[test]
    fn observed_execution_boundary_is_bound_into_signed_authority() -> Result<(), TestError> {
        let output = InvocationOutput::runtime_success(
            JsonValue::Object(JsonObject::new()),
            1,
            crate::process_invocation::boundary_metadata(
                runx_contracts::ExecutionBoundaryKind::TrustedHostProcess,
            )?,
        );

        let receipt = step_receipt(
            test_scope("boundary_graph"),
            "boundary_step",
            1,
            &output,
            &JsonObject::new(),
            "2026-05-28T00:00:00Z",
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
                projection_refs: StepOutputRefs::default(),
                child_receipts: &[],
                descendant_receipts: &[],
                authority: SealAuthority::default(),
                receipt_metadata: None,
                signature_policy: RuntimeReceiptSignaturePolicy::local_development(),
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
        assert_eq!(
            first.subject.reference.locator,
            other_transport.subject.reference.locator
        );
        assert_eq!(first.id, reordered.id);
        assert_eq!(
            first.subject.reference.locator,
            reordered.subject.reference.locator
        );
        assert_ne!(first.id, changed.id);
        assert_ne!(
            first.subject.reference.locator,
            changed.subject.reference.locator
        );
        Ok(())
    }

    #[test]
    fn step_receipt_identity_commits_direct_child_identities() -> Result<(), TestError> {
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
                projection_refs: StepOutputRefs::default(),
                child_receipts: std::slice::from_ref(child),
                descendant_receipts: &[],
                authority: SealAuthority::default(),
                receipt_metadata: None,
                signature_policy: RuntimeReceiptSignaturePolicy::local_development(),
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
    fn graph_child_identity_commits_child_receipt_ids_not_lineage_digests() {
        let mut first = Reference::runx(ReferenceType::Receipt, "sha256:first");
        first.locator = Some("sha256:first-digest".into());
        let mut same_id_new_digest = first.clone();
        same_id_new_digest.locator = Some("sha256:updated-digest".into());
        let second = Reference::runx(ReferenceType::Receipt, "sha256:second");

        assert_eq!(
            graph_child_identity(&[first.clone()]),
            graph_child_identity(&[same_id_new_digest])
        );
        assert_ne!(
            graph_child_identity(&[first]),
            graph_child_identity(&[second])
        );
    }

    fn successful_output(stdout: &str) -> InvocationOutput {
        InvocationOutput::process(
            InvocationStatus::Success,
            stdout.to_owned(),
            String::new(),
            Some(0),
            1,
            JsonObject::new(),
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
        let mut metadata = JsonObject::new();
        let observation_json = serde_json::to_string(&vec![observation])?;
        metadata.insert(
            CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA.to_owned(),
            serde_json::from_str::<JsonValue>(&observation_json)?,
        );
        Ok(InvocationOutput::process(
            InvocationStatus::Success,
            "ok".to_owned(),
            String::new(),
            Some(0),
            1,
            metadata,
        ))
    }
}
