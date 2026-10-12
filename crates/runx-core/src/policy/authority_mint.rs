// Module rationale: the mint primitive keeps its core fn, the
// family-comparator trait, the connector comparator, and their unit tests as one
// cohesive unit; splitting them would fracture the fail-closed minting path.
//! Authority minting: derive a narrowed child authority from a parent term
//! and compute its subset proof, fail-closed.
//!
//! This is the one place that narrows and the one place that proves. The proof
//! is computed here, never trusted from a model or skill input.
//!
//! The core holds zero family knowledge. The only family seam is the
//! [`FamilySubsetComparator`]: the generic verb/approval/expiry checks live in
//! [`authority_algebra`](super::authority_algebra), and a family supplies only
//! its bounds comparison. [`ConnectorBoundsComparator`]
//! is the family admission mints for: operations on one connector instance.

use runx_contracts::schema::{IsoDateTime, NonEmptyString};
use runx_contracts::{
    AuthorityBounds, AuthorityResourceFamily, AuthoritySubsetComparison,
    AuthoritySubsetProof, AuthoritySubsetRelation, AuthoritySubsetResult, AuthorityTerm,
    AuthorityUseLimit, AuthorityVerb, Reference, sha256_hex,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::authority_algebra::{
    items_subset, optional_ref_bound_subset, parent_items_preserved, patterns_subset,
    same_reference_address,
};
use super::rfc3339::parse_rfc3339_moment;

/// The family seam for [`mint_attenuated`]. A comparator owns two
/// responsibilities for one authority family, and nothing else:
///
/// - [`narrow_bounds`](FamilySubsetComparator::narrow_bounds): derive the child
///   term's bounds from the parent and the requested bounds. The default is to
///   take the requested bounds verbatim; a family clamps or normalizes here so
///   the minted ceiling derives from a single source.
/// - [`bounds_subset`](FamilySubsetComparator::bounds_subset): decide whether
///   the child term's family-specific bounds are a subset of the parent's.
///
/// The generic dimensions (verbs, approvals, expiry, resource address) are
/// checked by [`mint_attenuated`] via the shared algebra and are deliberately
/// NOT part of this trait, so a family cannot weaken them. Both methods receive
/// the whole [`AuthorityTerm`] because a family's bounds rule may depend on the
/// term's verbs.
pub trait FamilySubsetComparator {
    /// The `comparison_algorithm` recorded in the emitted proof. A stable,
    /// versioned identifier for the family's comparison vocabulary.
    fn comparison_algorithm(&self) -> &str;

    /// Derive the child term's bounds from the requested bounds, clamped to the
    /// parent where the family demands it. The default takes the request
    /// verbatim; the subsequent [`bounds_subset`](Self::bounds_subset) check is
    /// the fail-closed guarantee, so an over-wide request is rejected, not
    /// silently widened.
    fn narrow_bounds(
        &self,
        parent: &AuthorityTerm,
        requested: &AuthorityBounds,
    ) -> AuthorityBounds {
        let _ = parent;
        requested.clone()
    }

    /// Decide whether the child term's family-specific bounds are a subset of
    /// the parent's. Must be fail-closed: incomparable terms are not subsets.
    fn bounds_subset(&self, child: &AuthorityTerm, parent: &AuthorityTerm) -> bool;
}

/// The comparator for operations on a connector instance: operation and target
/// patterns, grant scopes and use limits.
#[derive(Clone, Debug, Default)]
pub struct ConnectorBoundsComparator;

impl ConnectorBoundsComparator {
    pub const ALGORITHM: &'static str = "runx.connector-bounds-subset.v1";
}

impl FamilySubsetComparator for ConnectorBoundsComparator {
    fn comparison_algorithm(&self) -> &str {
        Self::ALGORITHM
    }

    fn bounds_subset(&self, child: &AuthorityTerm, parent: &AuthorityTerm) -> bool {
        child.resource_family == AuthorityResourceFamily::Connector
            && bounds_subset(&child.bounds, &parent.bounds)
    }
}

fn bounds_subset(child: &AuthorityBounds, parent: &AuthorityBounds) -> bool {
    // Exhaustively destructure the child so growing `AuthorityBounds` breaks
    // this function until the new field is compared: a forgotten field would
    // fail open on the trust boundary.
    let AuthorityBounds {
        operations,
        scopes,
        targets,
        uses,
    } = child;
    patterns_subset(operations, &parent.operations)
        && items_subset(scopes, &parent.scopes)
        && patterns_subset(targets, &parent.targets)
        && uses_subset(uses.as_ref(), parent.uses.as_ref())
}

/// A use limit is no wider than its parent's on each axis the parent bounds.
fn uses_subset(child: Option<&AuthorityUseLimit>, parent: Option<&AuthorityUseLimit>) -> bool {
    let Some(parent) = parent else {
        return true;
    };
    let child = child.cloned().unwrap_or_default();
    optional_ref_bound_subset(child.total.as_ref(), parent.total.as_ref())
        && optional_ref_bound_subset(child.per_day.as_ref(), parent.per_day.as_ref())
}

/// The requested narrowing handed to [`mint_attenuated`].
///
/// It carries the child's identity (`principal_ref`, `resource_ref`,
/// `resource_family`) and the requested ceiling (`verbs`, `bounds`, optional
/// `expires_at`). Approvals are intentionally absent: they are PRESERVED from
/// the parent by [`mint_attenuated`] and cannot be requested away.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttenuationRequest {
    /// The principal the child authority is minted for.
    pub principal_ref: Reference,
    /// The resource the child authority addresses. Must share its address with
    /// the parent's resource (same type and URI); mint fails closed otherwise.
    pub resource_ref: Reference,
    /// The child's resource family. Must equal the parent's family.
    pub resource_family: AuthorityResourceFamily,
    /// The verbs to keep. Must be a subset of the parent's verbs.
    pub verbs: Vec<AuthorityVerb>,
    /// The requested bounds. The family comparator may clamp these; the
    /// result must be a subset of the parent's.
    pub bounds: AuthorityBounds,
    /// An optional expiry, no later than the parent's. Without one the child
    /// expires with its parent: a child never outlives it.
    pub expires_at: Option<IsoDateTime>,
}

/// Why a mint failed. Fail-closed: any failure yields NO proof.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AttenuationError {
    /// A parent or requested child is already expired at the proof time.
    #[error("parent or requested authority is expired or has an invalid proof time")]
    Expired,
    /// The requested resource family does not match the parent's family.
    #[error("requested resource family does not match the parent authority")]
    ResourceFamilyMismatch,
    /// The requested resource address does not match the parent's resource.
    #[error("requested resource address does not match the parent authority")]
    ResourceAddressMismatch,
    /// The requested verbs are not a subset of the parent's verbs.
    #[error("requested verbs are not a subset of the parent authority")]
    VerbsNotSubset,
    /// The requested expiry is later than the parent's.
    #[error("requested expiry outlives the parent authority")]
    ExpiryNotSubset,
    /// The derived child term is not a verified subset of the parent under the
    /// family comparator. The catch-all: the minted term widened some dimension
    /// (most commonly a family bound) the comparator denies.
    #[error("derived child authority is not a subset of the parent authority")]
    ChildNotSubset,
}

/// Derive a narrowed child [`AuthorityTerm`] from `parent` per `request`, then
/// prove it is a subset and emit the [`AuthoritySubsetProof`]. Pure and
/// fail-closed: on any non-subset request it returns an [`AttenuationError`] and
/// emits no proof, so the primitive can never produce a false attestation.
///
/// The child preserves the parent's approvals (they cannot be dropped), narrows
/// verbs to the request, expires no later than the parent (with it, when the
/// request names no expiry), derives bounds through the family `comparator`,
/// and carries a deterministic `term_id` over its own content. The proof records
/// the parent resource ref, the comparator's algorithm, `result: Subset`, the
/// child/parent term ids with their relation, and `checked_at`.
///
/// # Errors
///
/// Returns the matching [`AttenuationError`] when the requested family, resource
/// address, verbs or expiry widen the parent, or when the derived child fails
/// the family subset check.
// Function rationale: minting is one linear derive/compare/prove
// sequence; splitting it would scatter the fail-closed checks across helpers.
pub fn mint_attenuated(
    parent: &AuthorityTerm,
    request: &AttenuationRequest,
    comparator: &dyn FamilySubsetComparator,
    checked_at: IsoDateTime,
) -> Result<(AuthorityTerm, AuthoritySubsetProof), AttenuationError> {
    if !authority_live(parent.expires_at.as_ref(), &checked_at)
        || !authority_live(request.expires_at.as_ref(), &checked_at)
    {
        return Err(AttenuationError::Expired);
    }
    // Cheap generic guards first, so the error is specific. The derived-child
    // subset check below is the authoritative fail-closed gate; these only make
    // the common widening mistakes legible.
    if request.resource_family != parent.resource_family {
        return Err(AttenuationError::ResourceFamilyMismatch);
    }
    if !same_reference_address(&request.resource_ref, &parent.resource_ref) {
        return Err(AttenuationError::ResourceAddressMismatch);
    }
    if !items_subset(&request.verbs, &parent.verbs) {
        return Err(AttenuationError::VerbsNotSubset);
    }
    let expires_at = inherited_expiry(request.expires_at.as_ref(), parent.expires_at.as_ref());
    if !expiry_subset(expires_at.as_ref(), parent.expires_at.as_ref()) {
        return Err(AttenuationError::ExpiryNotSubset);
    }

    // Derive the child. Approvals are PRESERVED verbatim from the parent: a
    // narrowing cannot drop a parent obligation. Bounds derive through the
    // family comparator from the requested bounds, so the minted ceiling has
    // one source and cannot drift from the request.
    let bounds = comparator.narrow_bounds(parent, &request.bounds);
    let term_id = derive_term_id(parent, request, &bounds, expires_at.as_ref());
    let child = AuthorityTerm {
        term_id,
        principal_ref: request.principal_ref.clone(),
        resource_ref: request.resource_ref.clone(),
        resource_family: request.resource_family.clone(),
        verbs: request.verbs.clone(),
        bounds,
        approvals: parent.approvals.clone(),
        expires_at,
        issued_by_ref: parent.issued_by_ref.clone(),
        credential_ref: parent.credential_ref.clone(),
    };

    // Authoritative gate: VERIFY the derived child is a subset of the parent.
    // Generic dimensions via the shared algebra; family bounds via the
    // comparator. No proof is emitted unless this holds.
    if !is_authority_subset(&child, parent, comparator) {
        return Err(AttenuationError::ChildNotSubset);
    }

    let proof = AuthoritySubsetProof {
        parent_authority_ref: parent.resource_ref.clone(),
        comparison_algorithm: NonEmptyString::from(comparator.comparison_algorithm().to_owned()),
        result: AuthoritySubsetResult::Subset,
        compared_terms: vec![AuthoritySubsetComparison {
            child_term_id: child.term_id.clone(),
            parent_term_id: parent.term_id.clone(),
            relation: AuthoritySubsetRelation::Subset,
        }],
        proof_ref: None,
        checked_at,
    };

    Ok((child, proof))
}

pub(super) fn authority_live(expires_at: Option<&IsoDateTime>, checked_at: &IsoDateTime) -> bool {
    let Some(now) = parse_rfc3339_moment(checked_at.as_str()) else {
        return false;
    };
    expires_at
        .is_none_or(|expiry| parse_rfc3339_moment(expiry.as_str()).is_some_and(|end| now < end))
}

/// A child's expiry: the one it names, or its parent's when it names none.
pub(super) fn inherited_expiry(
    child: Option<&IsoDateTime>,
    parent: Option<&IsoDateTime>,
) -> Option<IsoDateTime> {
    child.or(parent).cloned()
}

pub(super) fn expiry_subset(child: Option<&IsoDateTime>, parent: Option<&IsoDateTime>) -> bool {
    match (child, parent) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(child), None) => parse_rfc3339_moment(child.as_str()).is_some(),
        (Some(child), Some(parent)) => {
            match (
                parse_rfc3339_moment(child.as_str()),
                parse_rfc3339_moment(parent.as_str()),
            ) {
                (Some(child), Some(parent)) => child <= parent,
                _ => false,
            }
        }
    }
}

/// The generic subset check: `child` is no broader than `parent` under the
/// shared algebra (resource address, verbs, preserved approvals, expiry) plus
/// the family `comparator`'s bounds rule.
#[must_use]
pub(crate) fn is_authority_subset(
    child: &AuthorityTerm,
    parent: &AuthorityTerm,
    comparator: &dyn FamilySubsetComparator,
) -> bool {
    child.resource_family == parent.resource_family
        && same_reference_address(&child.resource_ref, &parent.resource_ref)
        && items_subset(&child.verbs, &parent.verbs)
        && parent_items_preserved(&child.approvals, &parent.approvals)
        && expiry_subset(child.expires_at.as_ref(), parent.expires_at.as_ref())
        && comparator.bounds_subset(child, parent)
}

/// A deterministic child `term_id`: `mint-` plus the SHA-256 hex of the child's
/// identifying content (parent term, principal, resource, family, verbs,
/// bounds, expiry). Self-syncing by construction; the same parent
/// and request always mint the same id, and any change to the narrowed content
/// changes the id, so there is no hand-maintained parallel identifier.
fn derive_term_id(
    parent: &AuthorityTerm,
    request: &AttenuationRequest,
    bounds: &AuthorityBounds,
    expires_at: Option<&IsoDateTime>,
) -> NonEmptyString {
    let fingerprint = serde_json::json!({
        "parent_term_id": parent.term_id.as_str(),
        "principal_ref": request.principal_ref,
        "resource_ref": request.resource_ref,
        "resource_family": request.resource_family,
        "verbs": request.verbs,
        "bounds": bounds,
        "expires_at": expires_at.map(IsoDateTime::as_str),
    });
    let digest = sha256_hex(fingerprint.to_string().as_bytes());
    NonEmptyString::from(format!("mint-{digest}"))
}

#[cfg(test)]
mod tests {
    use super::{
        AttenuationError, AttenuationRequest, ConnectorBoundsComparator, FamilySubsetComparator,
        is_authority_subset, mint_attenuated,
    };
    use runx_contracts::schema::IsoDateTime;
    use runx_contracts::{
        AuthorityApproval, AuthorityBounds, AuthorityResourceFamily, AuthoritySubsetRelation,
        AuthorityTerm, AuthorityUseLimit, AuthorityVerb, Reference, ReferenceType,
    };

    const CHECKED_AT: &str = "2026-06-23T00:00:00Z";
    const INSTANCE: &str = "runx:connector_instance:acme-slack";

    fn reference(reference_type: ReferenceType, uri: &str) -> Reference {
        Reference::with_uri(reference_type, uri)
    }

    fn approval() -> AuthorityApproval {
        AuthorityApproval {
            approval_ref: reference(ReferenceType::Verification, "runx:approval:approval"),
            approved_by_ref: None,
            approved_at: None,
        }
    }

    /// A connector instance's capability: broad operations, scopes and
    /// targets, three verbs, a use limit, and an approval the child must
    /// preserve.
    fn capability() -> AuthorityTerm {
        AuthorityTerm {
            term_id: "capability".into(),
            principal_ref: reference(ReferenceType::Principal, "runx:principal:operator"),
            resource_ref: reference(ReferenceType::ConnectorInstance, INSTANCE),
            resource_family: AuthorityResourceFamily::Connector,
            verbs: vec![
                AuthorityVerb::Read,
                AuthorityVerb::Write,
                AuthorityVerb::Review,
            ],
            bounds: AuthorityBounds {
                operations: vec!["slack/*".into()],
                scopes: vec!["chat:write".into(), "channels:read".into()],
                targets: vec!["slack://acme/*".into()],
                uses: Some(AuthorityUseLimit {
                    total: Some(10),
                    per_day: None,
                }),
            },
            approvals: vec![approval()],
            expires_at: Some("2026-12-31T00:00:00Z".into()),
            issued_by_ref: reference(ReferenceType::Principal, "runx:principal:operator"),
            credential_ref: None,
        }
    }

    /// A valid delegation: a strict subset on every dimension.
    fn delegation() -> AttenuationRequest {
        AttenuationRequest {
            principal_ref: reference(ReferenceType::Principal, "runx:principal:assistant"),
            resource_ref: reference(ReferenceType::ConnectorInstance, INSTANCE),
            resource_family: AuthorityResourceFamily::Connector,
            verbs: vec![AuthorityVerb::Read],
            bounds: AuthorityBounds {
                operations: vec!["slack/channel.post".into()],
                scopes: vec!["chat:write".into()],
                targets: vec!["slack://acme/C1".into()],
                uses: Some(AuthorityUseLimit {
                    total: Some(2),
                    per_day: None,
                }),
            },
            expires_at: Some("2026-06-30T00:00:00Z".into()),
        }
    }

    fn checked_at() -> IsoDateTime {
        CHECKED_AT.into()
    }

    fn mint(parent: &AuthorityTerm, request: &AttenuationRequest) -> Result<(), AttenuationError> {
        mint_attenuated(parent, request, &ConnectorBoundsComparator, checked_at()).map(|_| ())
    }

    #[test]
    fn valid_narrowing_yields_child_and_proof() -> Result<(), AttenuationError> {
        let parent = capability();
        let (child, proof) = mint_attenuated(
            &parent,
            &delegation(),
            &ConnectorBoundsComparator,
            checked_at(),
        )?;

        // The minted child is the requested narrowing with parent obligations
        // preserved.
        assert_eq!(child.verbs, vec![AuthorityVerb::Read]);
        assert_eq!(child.approvals, parent.approvals);
        assert_eq!(child.resource_family, parent.resource_family);

        // The proof attests this exact pair.
        assert_eq!(proof.parent_authority_ref, parent.resource_ref);
        assert_eq!(
            proof.comparison_algorithm,
            ConnectorBoundsComparator::ALGORITHM
        );
        assert_eq!(proof.compared_terms.len(), 1);
        assert_eq!(proof.compared_terms[0].child_term_id, child.term_id);
        assert_eq!(proof.compared_terms[0].parent_term_id, parent.term_id);
        assert_eq!(
            proof.compared_terms[0].relation,
            AuthoritySubsetRelation::Subset
        );
        assert!(is_authority_subset(
            &child,
            &parent,
            &ConnectorBoundsComparator
        ));
        Ok(())
    }

    #[test]
    fn term_id_is_deterministic() -> Result<(), AttenuationError> {
        let parent = capability();
        let first = mint_attenuated(
            &parent,
            &delegation(),
            &ConnectorBoundsComparator,
            checked_at(),
        )?
        .0;
        let second = mint_attenuated(
            &parent,
            &delegation(),
            &ConnectorBoundsComparator,
            checked_at(),
        )?
        .0;

        assert_eq!(first.term_id, second.term_id);
        assert!(first.term_id.as_str().starts_with("mint-"));
        Ok(())
    }

    #[test]
    fn an_expired_parent_cannot_mint_a_fresh_child() {
        assert_eq!(
            mint_attenuated(
                &capability(),
                &delegation(),
                &ConnectorBoundsComparator,
                "2026-12-31T00:00:00Z".into(),
            ),
            Err(AttenuationError::Expired)
        );
    }

    #[test]
    fn expiry_subset_compares_moments_across_timezones() {
        let mut parent = capability();
        let mut request = delegation();
        parent.expires_at = Some("2026-06-29T23:00:00Z".into());
        request.expires_at = Some("2026-06-30T00:00:00+02:00".into());
        assert!(
            mint_attenuated(
                &parent,
                &request,
                &ConnectorBoundsComparator,
                "2026-06-29T21:00:00Z".into(),
            )
            .is_ok()
        );

        parent.expires_at = Some("2026-06-30T00:00:00+02:00".into());
        request.expires_at = Some("2026-06-29T23:00:00Z".into());
        assert_eq!(
            mint_attenuated(
                &parent,
                &request,
                &ConnectorBoundsComparator,
                "2026-06-29T21:00:00Z".into(),
            ),
            Err(AttenuationError::ExpiryNotSubset)
        );
    }

    #[test]
    fn widening_a_verb_errors_fail_closed() {
        let mut request = delegation();
        request.verbs = vec![AuthorityVerb::Delete];
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::VerbsNotSubset)
        );
    }

    #[test]
    fn widening_an_operation_or_target_errors_fail_closed() {
        let mut request = delegation();
        request.bounds.operations = vec!["github/*".into()];
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ChildNotSubset)
        );

        let mut request = delegation();
        request.bounds.targets = vec!["slack://other/C1".into()];
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ChildNotSubset)
        );
    }

    #[test]
    fn widening_a_scope_errors_fail_closed() {
        let mut request = delegation();
        request.bounds.scopes = vec!["admin".into()];
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ChildNotSubset)
        );
    }

    #[test]
    fn widening_or_dropping_a_use_limit_errors_fail_closed() {
        let mut request = delegation();
        request.bounds.uses = Some(AuthorityUseLimit {
            total: Some(99),
            per_day: None,
        });
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ChildNotSubset)
        );

        let mut request = delegation();
        request.bounds.uses = None;
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ChildNotSubset)
        );
    }

    #[test]
    fn widening_expiry_errors_fail_closed() {
        let mut request = delegation();
        request.expires_at = Some("2027-01-01T00:00:00Z".into());
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ExpiryNotSubset)
        );
    }

    #[test]
    fn a_child_naming_no_expiry_expires_with_its_parent() -> Result<(), AttenuationError> {
        let parent = capability();
        let mut request = delegation();
        request.expires_at = None;
        let (child, _) =
            mint_attenuated(&parent, &request, &ConnectorBoundsComparator, checked_at())?;
        assert_eq!(child.expires_at, parent.expires_at);

        // Re-minted from the term it produced, the same child results: the
        // inherited expiry is part of its identity, not a second form of it.
        request.expires_at = child.expires_at.clone();
        let (again, _) =
            mint_attenuated(&parent, &request, &ConnectorBoundsComparator, checked_at())?;
        assert_eq!(again.term_id, child.term_id);
        Ok(())
    }

    #[test]
    fn mismatched_resource_address_errors_fail_closed() {
        let mut request = delegation();
        request.resource_ref = reference(
            ReferenceType::ConnectorInstance,
            "runx:connector_instance:other",
        );
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ResourceAddressMismatch)
        );
    }

    #[test]
    fn approvals_cannot_be_dropped() -> Result<(), AttenuationError> {
        // The request carries no approvals; the mint must still preserve the
        // parent's. A child with them stripped is not a subset.
        let parent = capability();
        let (mut child, _) = mint_attenuated(
            &parent,
            &delegation(),
            &ConnectorBoundsComparator,
            checked_at(),
        )?;

        assert_eq!(child.approvals, parent.approvals);

        // Independently: a hand-stripped child fails the subset gate, proving
        // the preservation is enforced, not incidental.
        child.approvals = Vec::new();
        assert!(!is_authority_subset(
            &child,
            &parent,
            &ConnectorBoundsComparator
        ));
        Ok(())
    }

    /// A stub family comparator: bounds are a subset only when the child's
    /// scopes are exactly the parent's. Proves the trait drives the bounds
    /// decision and the algorithm name flows into the proof.
    struct ExactScopesComparator;

    impl FamilySubsetComparator for ExactScopesComparator {
        fn comparison_algorithm(&self) -> &str {
            "test.exact-scopes.v1"
        }

        fn bounds_subset(&self, child: &AuthorityTerm, parent: &AuthorityTerm) -> bool {
            child.bounds.scopes == parent.bounds.scopes
        }
    }

    #[test]
    fn stub_family_comparator_drives_the_bounds_decision() -> Result<(), AttenuationError> {
        let parent = capability();
        let mut request = delegation();
        request.bounds.scopes = parent.bounds.scopes.clone();

        let (child, proof) =
            mint_attenuated(&parent, &request, &ExactScopesComparator, checked_at())?;
        assert_eq!(proof.comparison_algorithm, "test.exact-scopes.v1");
        assert!(is_authority_subset(&child, &parent, &ExactScopesComparator));

        // A narrower scope list is not exact, so this family refuses it.
        assert_eq!(
            mint_attenuated(&parent, &delegation(), &ExactScopesComparator, checked_at())
                .map(|_| ()),
            Err(AttenuationError::ChildNotSubset)
        );
        Ok(())
    }
}
