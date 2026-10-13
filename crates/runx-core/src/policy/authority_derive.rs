//! Execution-term derivation: where admission proves a conjunction.
//!
//! A provider call runs only when the instance's capability and every
//! delegation link from principal to actor cover it. Derivation builds the
//! narrowest term for exactly one intent and proves it a subset of every
//! parent, emitting one proof per parent. Approval evidence is recorded on the
//! term; it is never a parent and never covers anything by itself.
//!
//! This is the multi-parent form of [`mint_attenuated`](super::mint_attenuated),
//! which derives a child from one parent. Both share the algebra in
//! [`authority_algebra`](super::authority_algebra) and the family comparators.

use runx_contracts::schema::{IsoDateTime, NonEmptyString};
use runx_contracts::{
    AuthorityApproval, AuthoritySubsetComparison, AuthoritySubsetProof, AuthoritySubsetRelation,
    AuthoritySubsetResult, AuthorityTerm, Reference, sha256_hex,
};
use thiserror::Error;

use super::authority_algebra::{items_subset, same_reference_address};
use super::authority_mint::{
    AttenuationRequest, FamilySubsetComparator, authority_live, expiry_subset, inherited_expiry,
    is_authority_subset, mint_attenuated,
};
use super::rfc3339::parse_rfc3339_moment;

/// The evidence and the intent to derive a term for.
pub struct ExecutionTermRequest<'a> {
    /// What the principal can do at the provider.
    pub capability: &'a AuthorityTerm,
    /// What each actor between principal and caller may do, in order. Empty
    /// when the principal acts directly.
    pub delegations: &'a [AuthorityTerm],
    /// The narrowest ceiling for this call: the actor, the instance, one verb,
    /// the operation, its scopes, the exact target and a single use.
    pub intent: &'a AttenuationRequest,
    /// The digest of the exact intent the term is bound to.
    pub intent_digest: &'a str,
    /// Approval evidence for this intent, when the operation has a floor.
    pub approvals: Vec<AuthorityApproval>,
    /// The kernel that admits the call.
    pub issued_by_ref: Reference,
}

/// Which parent refused the intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivationParent {
    Capability,
    /// The zero-based delegation link.
    Delegation(usize),
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DerivationError {
    #[error("{parent:?} or its requested call is expired")]
    Expired { parent: DerivationParent },
    #[error("{parent:?} is not a delegation minted from the preceding authority")]
    InvalidDelegation { parent: DerivationParent },
    #[error("{parent:?} belongs to a different actor than the call")]
    ActorMismatch { parent: DerivationParent },
    #[error("{parent:?} addresses a different resource than the intent")]
    ResourceMismatch { parent: DerivationParent },
    #[error("{parent:?} does not grant the intent's verbs")]
    VerbsNotCovered { parent: DerivationParent },
    #[error("{parent:?} expires before the intent")]
    ExpiryNotCovered { parent: DerivationParent },
    #[error("{parent:?} does not cover the intent's bounds")]
    BoundsNotCovered { parent: DerivationParent },
}

impl DerivationError {
    #[must_use]
    pub fn parent(&self) -> DerivationParent {
        match self {
            Self::Expired { parent }
            | Self::InvalidDelegation { parent }
            | Self::ActorMismatch { parent }
            | Self::ResourceMismatch { parent }
            | Self::VerbsNotCovered { parent }
            | Self::ExpiryNotCovered { parent }
            | Self::BoundsNotCovered { parent } => *parent,
        }
    }
}

/// Derive the execution term for one intent and prove it against every
/// parent. Pure and fail-closed: any parent that does not cover the intent
/// refuses the whole derivation, and no proof is emitted.
///
/// # Errors
///
/// Returns the first [`DerivationError`], naming the parent that refused.
pub fn derive_execution_term(
    request: &ExecutionTermRequest<'_>,
    comparator: &dyn FamilySubsetComparator,
    checked_at: IsoDateTime,
) -> Result<(AuthorityTerm, Vec<AuthoritySubsetProof>), DerivationError> {
    // A caller cannot supply a plausible-looking term as a delegation. Each
    // link must be exactly what the core mint would derive from the preceding
    // one, including its deterministic identity and preserved obligations.
    let mut preceding = request.capability;
    for (index, delegation) in request.delegations.iter().enumerate() {
        let narrowing = AttenuationRequest {
            principal_ref: delegation.principal_ref.clone(),
            resource_ref: delegation.resource_ref.clone(),
            resource_family: delegation.resource_family.clone(),
            verbs: delegation.verbs.clone(),
            bounds: delegation.bounds.clone(),
            expires_at: delegation.expires_at.clone(),
        };
        let minted = mint_attenuated(preceding, &narrowing, comparator, checked_at.clone())
            .map_err(|_| DerivationError::InvalidDelegation {
                parent: DerivationParent::Delegation(index),
            })?
            .0;
        if &minted != delegation {
            return Err(DerivationError::InvalidDelegation {
                parent: DerivationParent::Delegation(index),
            });
        }
        preceding = delegation;
    }
    if !same_reference_address(&request.intent.principal_ref, &preceding.principal_ref) {
        return Err(DerivationError::ActorMismatch {
            parent: request
                .delegations
                .len()
                .checked_sub(1)
                .map_or(DerivationParent::Capability, DerivationParent::Delegation),
        });
    }
    let parents = parents(request);
    for (parent, term) in &parents {
        if !authority_live(term.expires_at.as_ref(), &checked_at) {
            return Err(DerivationError::Expired { parent: *parent });
        }
    }
    if !authority_live(request.intent.expires_at.as_ref(), &checked_at) {
        return Err(DerivationError::Expired {
            parent: parents
                .last()
                .map_or(DerivationParent::Capability, |(parent, _)| *parent),
        });
    }
    for (parent, term) in &parents {
        precheck(request.intent, term, *parent)?;
    }

    let mut approvals: Vec<AuthorityApproval> = Vec::new();
    for (_, term) in &parents {
        extend_unique(&mut approvals, &term.approvals);
    }
    extend_unique(&mut approvals, &request.approvals);

    let intent = request.intent;
    let term = AuthorityTerm {
        term_id: derive_term_id(request, &parents),
        principal_ref: intent.principal_ref.clone(),
        resource_ref: intent.resource_ref.clone(),
        resource_family: intent.resource_family.clone(),
        verbs: intent.verbs.clone(),
        bounds: intent.bounds.clone(),
        approvals,
        expires_at: inherited_expiry(
            intent.expires_at.as_ref(),
            earliest_expiry(&parents).as_ref(),
        ),
        issued_by_ref: request.issued_by_ref.clone(),
        credential_ref: request.capability.credential_ref.clone(),
    };

    let mut proofs = Vec::with_capacity(parents.len());
    for (parent, parent_term) in &parents {
        if !is_authority_subset(&term, parent_term, comparator) {
            return Err(DerivationError::BoundsNotCovered { parent: *parent });
        }
        proofs.push(AuthoritySubsetProof {
            parent_authority_ref: parent_term.resource_ref.clone(),
            comparison_algorithm: NonEmptyString::from(
                comparator.comparison_algorithm().to_owned(),
            ),
            result: AuthoritySubsetResult::Subset,
            compared_terms: vec![AuthoritySubsetComparison {
                child_term_id: term.term_id.clone(),
                parent_term_id: parent_term.term_id.clone(),
                relation: AuthoritySubsetRelation::Subset,
            }],
            proof_ref: None,
            checked_at: checked_at.clone(),
        });
    }
    Ok((term, proofs))
}

fn parents<'a>(request: &ExecutionTermRequest<'a>) -> Vec<(DerivationParent, &'a AuthorityTerm)> {
    std::iter::once((DerivationParent::Capability, request.capability))
        .chain(
            request
                .delegations
                .iter()
                .enumerate()
                .map(|(index, term)| (DerivationParent::Delegation(index), term)),
        )
        .collect()
}

/// The common widening mistakes, checked first so a refusal names its cause.
/// The subset proof after derivation remains the authoritative gate.
fn precheck(
    intent: &AttenuationRequest,
    parent_term: &AuthorityTerm,
    parent: DerivationParent,
) -> Result<(), DerivationError> {
    if intent.resource_family != parent_term.resource_family
        || !same_reference_address(&intent.resource_ref, &parent_term.resource_ref)
    {
        return Err(DerivationError::ResourceMismatch { parent });
    }
    if !items_subset(&intent.verbs, &parent_term.verbs) {
        return Err(DerivationError::VerbsNotCovered { parent });
    }
    let expires_at = inherited_expiry(intent.expires_at.as_ref(), parent_term.expires_at.as_ref());
    if !expiry_subset(expires_at.as_ref(), parent_term.expires_at.as_ref()) {
        return Err(DerivationError::ExpiryNotCovered { parent });
    }
    Ok(())
}

/// The earliest expiry among the parents: an execution term never outlives
/// any authority it derives from.
fn earliest_expiry(parents: &[(DerivationParent, &AuthorityTerm)]) -> Option<IsoDateTime> {
    parents
        .iter()
        .filter_map(|(_, term)| term.expires_at.as_ref())
        .min_by_key(|expiry| parse_rfc3339_moment(expiry.as_str()))
        .cloned()
}

fn extend_unique<T: Clone + PartialEq>(into: &mut Vec<T>, items: &[T]) {
    for item in items {
        if !into.contains(item) {
            into.push(item.clone());
        }
    }
}

/// `exec-` plus a digest of every parent term id, the intent and its approval
/// evidence: the same evidence and intent always derive the same id.
fn derive_term_id(
    request: &ExecutionTermRequest<'_>,
    parents: &[(DerivationParent, &AuthorityTerm)],
) -> NonEmptyString {
    let fingerprint = serde_json::json!({
        "parent_term_ids": parents.iter().map(|(_, term)| term.term_id.as_str()).collect::<Vec<_>>(),
        "intent": request.intent,
        "intent_digest": request.intent_digest,
        "approvals": request.approvals,
    });
    NonEmptyString::from(format!(
        "exec-{}",
        sha256_hex(fingerprint.to_string().as_bytes())
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use runx_contracts::{
        AuthorityApproval, AuthorityBounds, AuthorityResourceFamily, AuthorityTerm,
        AuthorityUseLimit, AuthorityVerb, Reference, ReferenceType,
    };

    use super::{DerivationError, DerivationParent, ExecutionTermRequest, derive_execution_term};
    use crate::policy::{AttenuationRequest, ConnectorBoundsComparator, mint_attenuated};

    const INSTANCE: &str = "runx:connector_instance:slack-acme";

    fn instance() -> Reference {
        Reference::with_uri(ReferenceType::ConnectorInstance, INSTANCE)
    }

    fn term(id: &str, bounds: AuthorityBounds) -> AuthorityTerm {
        AuthorityTerm {
            term_id: id.into(),
            principal_ref: Reference::with_uri(ReferenceType::Principal, "runx:principal:kam"),
            resource_ref: instance(),
            resource_family: AuthorityResourceFamily::Connector,
            verbs: vec![AuthorityVerb::Read, AuthorityVerb::Write],
            bounds,
            approvals: Vec::new(),
            expires_at: Some("2026-12-31T00:00:00Z".into()),
            issued_by_ref: Reference::with_uri(ReferenceType::Principal, "runx:principal:connect"),
            credential_ref: None,
        }
    }

    fn capability() -> AuthorityTerm {
        term(
            "grant-slack",
            AuthorityBounds {
                operations: vec!["slack/*".into()],
                scopes: vec!["channel.post".into(), "channel.post.read".into()],
                targets: vec!["*".into()],
                ..AuthorityBounds::default()
            },
        )
    }

    fn mandate(capability: &AuthorityTerm, target: &str) -> AuthorityTerm {
        mint_attenuated(
            capability,
            &AttenuationRequest {
                principal_ref: Reference::with_uri(
                    ReferenceType::Principal,
                    "runx:principal:assistant",
                ),
                resource_ref: instance(),
                resource_family: AuthorityResourceFamily::Connector,
                verbs: vec![AuthorityVerb::Write],
                bounds: AuthorityBounds {
                    operations: vec!["slack/channel.post".into()],
                    scopes: vec!["channel.post".into()],
                    targets: vec![target.into()],
                    uses: Some(AuthorityUseLimit {
                        total: Some(100),
                        per_day: Some(5),
                    }),
                },
                expires_at: capability.expires_at.clone(),
            },
            &ConnectorBoundsComparator,
            "2026-10-06T00:00:00Z".into(),
        )
        .expect("mandate is bounded by capability")
        .0
    }

    fn intent(target: &str) -> AttenuationRequest {
        AttenuationRequest {
            principal_ref: Reference::with_uri(
                ReferenceType::Principal,
                "runx:principal:assistant",
            ),
            resource_ref: instance(),
            resource_family: AuthorityResourceFamily::Connector,
            verbs: vec![AuthorityVerb::Write],
            bounds: AuthorityBounds {
                operations: vec!["slack/channel.post".into()],
                scopes: vec!["channel.post".into()],
                targets: vec![target.into()],
                uses: Some(AuthorityUseLimit {
                    total: Some(1),
                    per_day: Some(1),
                }),
            },
            expires_at: Some("2026-10-06T01:00:00Z".into()),
        }
    }

    fn request<'a>(
        capability: &'a AuthorityTerm,
        delegations: &'a [AuthorityTerm],
        intent: &'a AttenuationRequest,
    ) -> ExecutionTermRequest<'a> {
        ExecutionTermRequest {
            capability,
            delegations,
            intent,
            intent_digest: "sha256:intent",
            approvals: Vec::new(),
            issued_by_ref: Reference::with_uri(ReferenceType::Principal, "runx:principal:kernel"),
        }
    }

    #[test]
    fn derives_a_term_proven_against_every_parent() {
        let capability = capability();
        let delegations = [mandate(&capability, "slack://acme/C1")];
        let intent = intent("slack://acme/C1");
        let (term, proofs) = derive_execution_term(
            &request(&capability, &delegations, &intent),
            &ConnectorBoundsComparator,
            "2026-10-06T00:00:00Z".into(),
        )
        .expect("derivation");
        assert_eq!(proofs.len(), 2);
        assert!(term.term_id.as_str().starts_with("exec-"));
        for (proof, parent) in proofs.iter().zip([&capability, &delegations[0]]) {
            assert_eq!(proof.parent_authority_ref, parent.resource_ref);
            assert_eq!(proof.compared_terms[0].child_term_id, term.term_id);
            assert_eq!(proof.compared_terms[0].parent_term_id, parent.term_id);
        }
    }

    #[test]
    fn an_intent_naming_no_expiry_expires_with_its_earliest_parent() {
        let capability = capability();
        let mut delegation_request = intent("slack://acme/*");
        delegation_request.bounds.uses = Some(AuthorityUseLimit {
            total: Some(100),
            per_day: Some(5),
        });
        delegation_request.expires_at = Some("2026-10-06T00:30:00Z".into());
        let delegations = [mint_attenuated(
            &capability,
            &delegation_request,
            &ConnectorBoundsComparator,
            "2026-10-06T00:00:00Z".into(),
        )
        .expect("delegation")
        .0];
        let mut intent = intent("slack://acme/C1");
        intent.expires_at = None;
        let (term, _) = derive_execution_term(
            &request(&capability, &delegations, &intent),
            &ConnectorBoundsComparator,
            "2026-10-06T00:00:00Z".into(),
        )
        .expect("an expiring delegation still authorizes a call that names no expiry");
        assert_eq!(term.expires_at, Some("2026-10-06T00:30:00Z".into()));
    }

    #[test]
    fn a_delegation_that_does_not_cover_the_target_refuses() {
        let capability = capability();
        let delegations = [mandate(&capability, "slack://acme/C1")];
        let intent = intent("slack://acme/C2");
        let error = derive_execution_term(
            &request(&capability, &delegations, &intent),
            &ConnectorBoundsComparator,
            "2026-10-06T00:00:00Z".into(),
        )
        .expect_err("refused");
        assert_eq!(
            error,
            DerivationError::BoundsNotCovered {
                parent: DerivationParent::Delegation(0)
            }
        );
    }

    #[test]
    fn approval_never_substitutes_for_capability() {
        let mut capability = capability();
        capability.bounds.scopes = vec!["channel.post.read".into()];
        let mut intent = intent("slack://acme/C1");
        intent.principal_ref = capability.principal_ref.clone();
        let mut request = request(&capability, &[], &intent);
        request.approvals = vec![AuthorityApproval {
            approval_ref: Reference::with_uri(ReferenceType::Verification, "runx:approval:yes"),
            approved_by_ref: None,
            approved_at: None,
        }];
        let error = derive_execution_term(
            &request,
            &ConnectorBoundsComparator,
            "2026-10-06T00:00:00Z".into(),
        )
        .expect_err("approval alone does not authorize");
        assert_eq!(error.parent(), DerivationParent::Capability);
    }

    #[test]
    fn an_unbounded_intent_under_a_use_limited_delegation_refuses() {
        let capability = capability();
        let delegations = [mandate(&capability, "slack://acme/*")];
        let mut intent = intent("slack://acme/C9");
        intent.bounds.uses = None;
        assert!(
            derive_execution_term(
                &request(&capability, &delegations, &intent),
                &ConnectorBoundsComparator,
                "2026-10-06T00:00:00Z".into(),
            )
            .is_err()
        );
    }

    #[test]
    fn direct_call_cannot_act_as_another_principal() {
        let capability = capability();
        let intent = intent("slack://acme/C1");
        let error = derive_execution_term(
            &request(&capability, &[], &intent),
            &ConnectorBoundsComparator,
            "2026-10-06T00:00:00Z".into(),
        )
        .expect_err("the assistant is not the provider principal");
        assert_eq!(
            error,
            DerivationError::ActorMismatch {
                parent: DerivationParent::Capability
            }
        );
    }

    #[test]
    fn supplied_delegation_must_be_minted_from_the_capability() {
        let capability = capability();
        let mut forged = mandate(&capability, "slack://acme/C1");
        forged.term_id = "self-declared-mandate".into();
        let intent = intent("slack://acme/C1");
        let error = derive_execution_term(
            &request(&capability, &[forged], &intent),
            &ConnectorBoundsComparator,
            "2026-10-06T00:00:00Z".into(),
        )
        .expect_err("a term label is not delegation proof");
        assert_eq!(
            error,
            DerivationError::InvalidDelegation {
                parent: DerivationParent::Delegation(0)
            }
        );
    }

    #[test]
    fn an_expired_intent_cannot_authorize_a_call() {
        let capability = capability();
        let delegations = [mandate(&capability, "slack://acme/C1")];
        let intent = intent("slack://acme/C1");
        let error = derive_execution_term(
            &request(&capability, &delegations, &intent),
            &ConnectorBoundsComparator,
            "2026-10-06T01:00:00Z".into(),
        )
        .expect_err("a term expiring at the proof time is no longer live");
        assert_eq!(
            error,
            DerivationError::Expired {
                parent: DerivationParent::Delegation(0)
            }
        );
    }
}
