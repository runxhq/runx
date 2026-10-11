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
//! [`FamilySubsetComparator`]: the generic verb/capability/condition/approval/
//! expiry checks live in [`authority_algebra`](super::authority_algebra), and a
//! family supplies only its bounds comparison. [`ConnectorBoundsComparator`]
//! is the family admission mints for: operations on one connector instance.

use runx_contracts::schema::{IsoDateTime, NonEmptyString};
use runx_contracts::{
    AuthorityBounds, AuthorityCapability, AuthorityResourceFamily, AuthoritySubsetComparison,
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
/// The generic dimensions (verbs, capabilities, conditions, approvals, expiry,
/// resource address) are checked by [`mint_attenuated`] via the shared algebra
/// and are deliberately NOT part of this trait, so a family cannot weaken them.
/// Both methods receive the whole [`AuthorityTerm`] because a family's bounds
/// rule may depend on the term's verbs (for example, a cost-class authority
/// requiring an aggregate cap), exactly as a domain comparator does.
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

/// The comparator for operations on a connector instance: the generic bounds
/// plus operation and target patterns, grant scopes and use limits.
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

// Function rationale: the exhaustive destructure of both bounds sides plus the
// single subset conjunction is one trust-boundary comparison; splitting it would
// let a forgotten field fail open.
fn bounds_subset(child: &AuthorityBounds, parent: &AuthorityBounds) -> bool {
    // Exhaustively destructure both sides so growing `AuthorityBounds` breaks
    // this function until the new field is classified as generic-narrowable
    // (compared below) or family-owned (denied on the child). Silent fail-open
    // on a forgotten field is what the no-brittleness rule forbids, and this
    // sits on the trust boundary.
    let AuthorityBounds {
        repo_path_globs: child_repo_path_globs,
        branch_patterns: child_branch_patterns,
        filesystem_roots: child_filesystem_roots,
        network_destinations: child_network_destinations,
        deployment_environments: child_deployment_environments,
        token_audiences: child_token_audiences,
        max_cost_units: _, // compared via max_cost_units_subset (non-Ord projection)
        // Effect bounds are not narrowable by minting: denied on the child.
        effect_limits: child_effect_limits,
        effects: child_effects,
        max_runtime_ms: child_max_runtime_ms,
        max_fanout: child_max_fanout,
        max_child_depth: child_max_child_depth,
        operations: child_operations,
        scopes: child_scopes,
        targets: child_targets,
        uses: child_uses,
    } = child;
    let AuthorityBounds {
        repo_path_globs: parent_repo_path_globs,
        branch_patterns: parent_branch_patterns,
        filesystem_roots: parent_filesystem_roots,
        network_destinations: parent_network_destinations,
        deployment_environments: parent_deployment_environments,
        token_audiences: parent_token_audiences,
        max_cost_units: _, // compared via max_cost_units_subset
        effect_limits: _,  // effect bounds, see child denial
        effects: _,        // effect bounds, see child denial
        max_runtime_ms: parent_max_runtime_ms,
        max_fanout: parent_max_fanout,
        max_child_depth: parent_max_child_depth,
        operations: parent_operations,
        scopes: parent_scopes,
        targets: parent_targets,
        uses: parent_uses,
    } = parent;

    child_effect_limits.is_empty()
        && child_effects.is_empty()
        && patterns_subset(child_operations, parent_operations)
        && items_subset(child_scopes, parent_scopes)
        && patterns_subset(child_targets, parent_targets)
        && uses_subset(child_uses.as_ref(), parent_uses.as_ref())
        && items_subset(child_repo_path_globs, parent_repo_path_globs)
        && items_subset(child_branch_patterns, parent_branch_patterns)
        && items_subset(child_filesystem_roots, parent_filesystem_roots)
        && items_subset(child_network_destinations, parent_network_destinations)
        && items_subset(
            child_deployment_environments,
            parent_deployment_environments,
        )
        && items_subset(child_token_audiences, parent_token_audiences)
        && max_cost_units_subset(child, parent)
        && optional_ref_bound_subset(
            child_max_runtime_ms.as_ref(),
            parent_max_runtime_ms.as_ref(),
        )
        && optional_ref_bound_subset(child_max_fanout.as_ref(), parent_max_fanout.as_ref())
        && optional_ref_bound_subset(
            child_max_child_depth.as_ref(),
            parent_max_child_depth.as_ref(),
        )
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

/// `max_cost_units` is an optional [`JsonNumber`](runx_contracts::JsonNumber),
/// which is not totally ordered (it carries non-finite `f64`). Compare on the
/// finite `f64` projection and fail closed when either side is absent or
/// non-finite while the parent constrains it.
fn max_cost_units_subset(child: &AuthorityBounds, parent: &AuthorityBounds) -> bool {
    match (
        child.max_cost_units.as_ref().and_then(|n| n.as_f64()),
        parent.max_cost_units.as_ref().and_then(|n| n.as_f64()),
    ) {
        // Both finite: the child bound must be no larger.
        (Some(child), Some(parent)) => child <= parent,
        // Parent caps cost (finite projection) but the child declares no usable
        // bound: deny. The child's projection is None because its cap is absent
        // OR present-but-non-finite; either way it cannot be proven no larger.
        (None, Some(_)) => false,
        // Parent's projection is None. Allow only when the parent genuinely has
        // no cap; a present-but-non-finite parent cap is uncomparable, so deny.
        (_, None) => parent.max_cost_units.is_none(),
    }
}

/// The requested narrowing handed to [`mint_attenuated`].
///
/// It carries the child's identity (`principal_ref`, `resource_ref`,
/// `resource_family`) and the requested ceiling (`verbs`, `capabilities`,
/// `bounds`, optional `expires_at`). `bounds` is the typed [`AuthorityBounds`],
/// which already expresses BOTH an agency narrowing (the string-scope lists: a
/// member's needed repos, roots, destinations) AND a cost narrowing (the
/// `effect_limits` caps and rails). There is no stringly map and no
/// hand-maintained parallel list: the request is exactly the contract's own
/// narrowable surface.
///
/// Conditions and approvals are intentionally absent: they are PRESERVED from
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
    /// The capabilities to keep. Must be a subset of the parent's capabilities.
    pub capabilities: Vec<AuthorityCapability>,
    /// The requested bounds (string scopes and/or family limits). The family
    /// comparator may clamp these; the result must be a subset of the parent's.
    pub bounds: AuthorityBounds,
    /// An optional expiry. Must be no later than the parent's expiry; a child
    /// may not outlive its parent.
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
    /// The requested capabilities are not a subset of the parent's.
    #[error("requested capabilities are not a subset of the parent authority")]
    CapabilitiesNotSubset,
    /// The requested expiry is later than the parent's, or absent while the
    /// parent bounds it.
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
/// The child preserves the parent's conditions and approvals (they cannot be
/// dropped), narrows verbs/capabilities to the request, clamps expiry to no
/// later than the parent, derives bounds through the family `comparator`, and
/// carries a deterministic `term_id` over its own content. The proof records
/// the parent resource ref, the comparator's algorithm, `result: Subset`, the
/// child/parent term ids with their relation, and `checked_at`.
///
/// # Errors
///
/// Returns the matching [`AttenuationError`] when the requested family, resource
/// address, verbs, capabilities, or expiry widen the parent, or when the derived
/// child fails the family subset check.
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
    if !items_subset(&request.capabilities, &parent.capabilities) {
        return Err(AttenuationError::CapabilitiesNotSubset);
    }
    if !expiry_subset(request.expires_at.as_ref(), parent.expires_at.as_ref()) {
        return Err(AttenuationError::ExpiryNotSubset);
    }

    // Derive the child. Conditions and approvals are PRESERVED verbatim from the
    // parent: a narrowing cannot drop a parent obligation. Bounds derive through
    // the family comparator from the requested bounds, so the minted ceiling has
    // one source and cannot drift from the request.
    let bounds = comparator.narrow_bounds(parent, &request.bounds);
    let term_id = derive_term_id(parent, request, &bounds);
    let child = AuthorityTerm {
        term_id,
        principal_ref: request.principal_ref.clone(),
        resource_ref: request.resource_ref.clone(),
        resource_family: request.resource_family.clone(),
        verbs: request.verbs.clone(),
        bounds,
        conditions: parent.conditions.clone(),
        approvals: parent.approvals.clone(),
        capabilities: request.capabilities.clone(),
        expires_at: request.expires_at.clone(),
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
/// shared algebra (resource address, verbs, capabilities, preserved conditions
/// and approvals, expiry) plus the family `comparator`'s bounds rule.
#[must_use]
pub(crate) fn is_authority_subset(
    child: &AuthorityTerm,
    parent: &AuthorityTerm,
    comparator: &dyn FamilySubsetComparator,
) -> bool {
    child.resource_family == parent.resource_family
        && same_reference_address(&child.resource_ref, &parent.resource_ref)
        && items_subset(&child.verbs, &parent.verbs)
        && items_subset(&child.capabilities, &parent.capabilities)
        && parent_items_preserved(&child.conditions, &parent.conditions)
        && parent_items_preserved(&child.approvals, &parent.approvals)
        && expiry_subset(child.expires_at.as_ref(), parent.expires_at.as_ref())
        && comparator.bounds_subset(child, parent)
}

/// A deterministic child `term_id`: `mint-` plus the SHA-256 hex of the child's
/// identifying content (parent term, principal, resource, family, verbs,
/// capabilities, bounds, expiry). Self-syncing by construction; the same parent
/// and request always mint the same id, and any change to the narrowed content
/// changes the id, so there is no hand-maintained parallel identifier.
fn derive_term_id(
    parent: &AuthorityTerm,
    request: &AttenuationRequest,
    bounds: &AuthorityBounds,
) -> NonEmptyString {
    let fingerprint = serde_json::json!({
        "parent_term_id": parent.term_id.as_str(),
        "principal_ref": request.principal_ref,
        "resource_ref": request.resource_ref,
        "resource_family": request.resource_family,
        "verbs": request.verbs,
        "capabilities": request.capabilities,
        "bounds": bounds,
        "expires_at": request.expires_at.as_ref().map(IsoDateTime::as_str),
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
        AuthorityApproval, AuthorityBounds, AuthorityCapability, AuthorityCondition,
        AuthorityConditionPredicate, AuthorityEffectGuard, AuthorityEffectLimit,
        AuthorityResourceFamily, AuthoritySubsetRelation, AuthorityTerm, AuthorityVerb, JsonNumber,
        Reference, ReferenceType,
    };

    const CHECKED_AT: &str = "2026-06-23T00:00:00Z";
    const INSTANCE: &str = "runx:connector_instance:acme-slack";

    fn reference(reference_type: ReferenceType, uri: &str) -> Reference {
        Reference::with_uri(reference_type, uri)
    }

    fn condition() -> AuthorityCondition {
        AuthorityCondition {
            condition_id: "condition_approval".into(),
            predicate: AuthorityConditionPredicate::ApprovalPresent,
            refs: Vec::new(),
            parameters: None,
        }
    }

    fn approval() -> AuthorityApproval {
        AuthorityApproval {
            approval_ref: reference(ReferenceType::Verification, "runx:approval:approval"),
            approved_by_ref: None,
            approved_at: None,
        }
    }

    /// A connector instance's capability: broad operations and targets, two
    /// verbs, conditions and approvals the child must preserve.
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
                targets: vec!["slack://acme/*".into()],
                network_destinations: vec!["slack.com".into(), "files.slack.com".into()],
                max_fanout: Some(8),
                max_child_depth: Some(3),
                ..AuthorityBounds::default()
            },
            conditions: vec![condition()],
            approvals: vec![approval()],
            capabilities: vec![
                AuthorityCapability::FilesystemRead,
                AuthorityCapability::NetworkEgress,
            ],
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
            capabilities: vec![AuthorityCapability::NetworkEgress],
            bounds: AuthorityBounds {
                operations: vec!["slack/channel.post".into()],
                targets: vec!["slack://acme/C1".into()],
                network_destinations: vec!["slack.com".into()],
                max_fanout: Some(2),
                max_child_depth: Some(1),
                ..AuthorityBounds::default()
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
        assert_eq!(child.capabilities, vec![AuthorityCapability::NetworkEgress]);
        assert_eq!(child.conditions, parent.conditions);
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
    fn widening_a_capability_errors_fail_closed() {
        let mut request = delegation();
        request.capabilities = vec![AuthorityCapability::SecretRead];
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::CapabilitiesNotSubset)
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
    fn widening_a_string_bound_errors_fail_closed() {
        let mut request = delegation();
        request.bounds.network_destinations = vec!["evil.example".into()];
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ChildNotSubset)
        );
    }

    #[test]
    fn widening_a_numeric_bound_errors_fail_closed() {
        let mut request = delegation();
        request.bounds.max_fanout = Some(99);
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ChildNotSubset)
        );
    }

    #[test]
    fn widening_max_cost_units_errors_fail_closed() {
        let mut parent = capability();
        parent.bounds.max_cost_units = Some(JsonNumber::U64(100));
        let mut request = delegation();
        request.bounds.max_cost_units = Some(JsonNumber::U64(500));
        assert_eq!(
            mint(&parent, &request),
            Err(AttenuationError::ChildNotSubset)
        );
    }

    #[test]
    fn dropping_max_cost_units_when_parent_caps_it_errors_fail_closed() {
        let mut parent = capability();
        parent.bounds.max_cost_units = Some(JsonNumber::U64(100));
        let mut request = delegation();
        request.bounds.max_cost_units = None;
        assert_eq!(
            mint(&parent, &request),
            Err(AttenuationError::ChildNotSubset)
        );
    }

    #[test]
    fn narrowing_max_cost_units_succeeds() -> Result<(), AttenuationError> {
        let mut parent = capability();
        parent.bounds.max_cost_units = Some(JsonNumber::U64(500));
        let mut request = delegation();
        request.bounds.max_cost_units = Some(JsonNumber::U64(100));

        let (child, _) =
            mint_attenuated(&parent, &request, &ConnectorBoundsComparator, checked_at())?;
        assert!(is_authority_subset(
            &child,
            &parent,
            &ConnectorBoundsComparator
        ));
        Ok(())
    }

    #[test]
    fn an_effect_bearing_child_errors_fail_closed() {
        // Minting does not narrow effect ceilings or guards. A child carrying an
        // effect limit or guard is denied, not waved through with a proof.
        let mut limit_request = delegation();
        limit_request.bounds.effect_limits = vec![AuthorityEffectLimit {
            family: "deployment".into(),
            unit: "USD".into(),
            max_per_call_units: Some(1_000),
            max_per_run_units: Some(5_000),
            max_per_period_units: None,
            period: None,
            channels: vec!["stripe".into()],
            realm: None,
            peer: None,
            operation: None,
            preflight_ttl_ms: None,
            approval_threshold_units: None,
            authorization_form: None,
            preflight_required: false,
            commitment_required: false,
            idempotency_required: false,
            recovery_required: false,
            receipt_before_success: false,
            single_use_capability: false,
        }];
        assert_eq!(
            mint(&capability(), &limit_request),
            Err(AttenuationError::ChildNotSubset)
        );

        let mut guard_request = delegation();
        guard_request.bounds.effects = vec![AuthorityEffectGuard {
            family: "deployment".into(),
            guard_kinds: Vec::new(),
            proof_kinds: Vec::new(),
        }];
        assert_eq!(
            mint(&capability(), &guard_request),
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
    fn dropping_expiry_when_parent_bounds_it_errors_fail_closed() {
        let mut request = delegation();
        request.expires_at = None;
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ExpiryNotSubset)
        );
    }

    #[test]
    fn mismatched_resource_family_errors_fail_closed() {
        let mut request = delegation();
        request.resource_family = AuthorityResourceFamily::Effect;
        assert_eq!(
            mint(&capability(), &request),
            Err(AttenuationError::ResourceFamilyMismatch)
        );
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
    fn conditions_and_approvals_cannot_be_dropped() -> Result<(), AttenuationError> {
        // The request carries no conditions or approvals; the mint must still
        // preserve the parent's. A child with them stripped is not a subset.
        let parent = capability();
        let (mut child, _) = mint_attenuated(
            &parent,
            &delegation(),
            &ConnectorBoundsComparator,
            checked_at(),
        )?;

        assert_eq!(child.conditions, parent.conditions);
        assert_eq!(child.approvals, parent.approvals);

        // Independently: a hand-stripped child fails the subset gate, proving
        // the preservation is enforced, not incidental.
        child.conditions = Vec::new();
        child.approvals = Vec::new();
        assert!(!is_authority_subset(
            &child,
            &parent,
            &ConnectorBoundsComparator
        ));
        Ok(())
    }

    /// A stub family comparator: bounds are a subset only when the child's
    /// `token_audiences` is exactly the parent's. Proves the trait drives the
    /// bounds decision and the algorithm name flows into the proof.
    struct ExactAudienceComparator;

    impl FamilySubsetComparator for ExactAudienceComparator {
        fn comparison_algorithm(&self) -> &str {
            "test.exact-audience.v1"
        }

        fn bounds_subset(&self, child: &AuthorityTerm, parent: &AuthorityTerm) -> bool {
            child.bounds.token_audiences == parent.bounds.token_audiences
        }
    }

    #[test]
    fn stub_family_comparator_drives_the_bounds_decision() -> Result<(), AttenuationError> {
        let mut parent = capability();
        parent.bounds.token_audiences = vec!["aud:a".into()];
        let mut request = delegation();
        request.bounds.token_audiences = vec!["aud:a".into()];

        let (child, proof) =
            mint_attenuated(&parent, &request, &ExactAudienceComparator, checked_at())?;
        assert_eq!(proof.comparison_algorithm, "test.exact-audience.v1");
        assert!(is_authority_subset(
            &child,
            &parent,
            &ExactAudienceComparator
        ));

        // A divergent audience fails the family bounds check.
        let mut widened = delegation();
        widened.bounds.token_audiences = vec!["aud:b".into()];
        assert_eq!(
            mint_attenuated(&parent, &widened, &ExactAudienceComparator, checked_at()).map(|_| ()),
            Err(AttenuationError::ChildNotSubset)
        );
        Ok(())
    }
}
