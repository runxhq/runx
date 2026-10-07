use runx_contracts::{
    AuthorityEffectGuardKind, AuthorityTerm, AuthorityVerb, ProofKind, Reference,
};

#[must_use]
pub fn same_reference_address(child: &Reference, parent: &Reference) -> bool {
    child.reference_type == parent.reference_type && child.uri == parent.uri
}

#[must_use]
pub fn authority_term_has_verb(term: &AuthorityTerm, verb: AuthorityVerb) -> bool {
    term.verbs.iter().any(|candidate| candidate == &verb)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorityEffectGuardDecision<'a> {
    pub family: &'a str,
    pub receipt_before_success_required: bool,
    pub non_replay_required: bool,
    pub proof_kinds: Vec<ProofKind>,
}

#[must_use]
pub fn authority_effect_family<'a>(
    parent: &'a AuthorityTerm,
    child: &'a AuthorityTerm,
) -> Option<&'a str> {
    child
        .bounds
        .effects
        .first()
        .or_else(|| parent.bounds.effects.first())
        .map(|guard| guard.family.as_str())
}

#[must_use]
pub fn evaluate_authority_effect_guards<'a>(
    parent: &'a AuthorityTerm,
    child: &'a AuthorityTerm,
    family: &'a str,
) -> AuthorityEffectGuardDecision<'a> {
    AuthorityEffectGuardDecision {
        family,
        receipt_before_success_required: authority_effect_guard_required(
            parent,
            family,
            AuthorityEffectGuardKind::ReceiptBeforeSuccess,
        ) || authority_effect_guard_required(
            child,
            family,
            AuthorityEffectGuardKind::ReceiptBeforeSuccess,
        ),
        non_replay_required: authority_effect_guard_required(
            parent,
            family,
            AuthorityEffectGuardKind::NonReplay,
        ) || authority_effect_guard_required(
            child,
            family,
            AuthorityEffectGuardKind::NonReplay,
        ),
        proof_kinds: authority_effect_proof_kinds(parent, child, family),
    }
}

#[must_use]
pub fn authority_effect_guard_required(
    term: &AuthorityTerm,
    family: &str,
    guard_kind: AuthorityEffectGuardKind,
) -> bool {
    term.bounds
        .effects
        .iter()
        .any(|guard| guard.family.as_str() == family && guard.guard_kinds.contains(&guard_kind))
}

#[must_use]
pub fn authority_effect_proof_kinds(
    parent: &AuthorityTerm,
    child: &AuthorityTerm,
    family: &str,
) -> Vec<ProofKind> {
    let mut proof_kinds = Vec::new();
    for term in [parent, child] {
        for guard in &term.bounds.effects {
            if guard.family.as_str() == family {
                for proof_kind in &guard.proof_kinds {
                    if !proof_kinds.contains(proof_kind) {
                        proof_kinds.push(proof_kind.clone());
                    }
                }
            }
        }
    }
    proof_kinds
}

#[must_use]
pub fn items_subset<T: PartialEq>(child: &[T], parent: &[T]) -> bool {
    child.iter().all(|item| parent.contains(item))
}

#[must_use]
pub fn parent_items_preserved<T: PartialEq>(child: &[T], parent: &[T]) -> bool {
    parent.iter().all(|item| child.contains(item))
}

#[must_use]
pub fn optional_exact_or_narrower<T: Eq>(child: &Option<T>, parent: &Option<T>) -> bool {
    match (child, parent) {
        (_, None) => true,
        (Some(child), Some(parent)) => child == parent,
        (None, Some(_)) => false,
    }
}

#[must_use]
pub fn optional_bound_subset<T: Ord + Copy>(child: Option<T>, parent: Option<T>) -> bool {
    match (child, parent) {
        (Some(child), Some(parent)) => child <= parent,
        (None, Some(_)) => false,
        (Some(_), None) | (None, None) => true,
    }
}

#[must_use]
pub fn optional_ref_bound_subset<T: Ord>(child: Option<&T>, parent: Option<&T>) -> bool {
    match (child, parent) {
        (Some(child), Some(parent)) => child <= parent,
        (None, Some(_)) => false,
        (Some(_), None) | (None, None) => true,
    }
}

/// Whether a parent pattern covers a child value or pattern. `*` covers
/// everything, `<prefix>*` covers anything starting with the prefix (including
/// a narrower pattern), and anything else covers only itself.
#[must_use]
pub fn pattern_covers(parent: &str, child: &str) -> bool {
    match parent.strip_suffix('*') {
        Some(prefix) => child.starts_with(prefix),
        None => parent == child,
    }
}

/// Every child pattern is covered by some parent pattern. An empty parent
/// covers nothing, so a bound is granted only when it is listed.
#[must_use]
pub fn patterns_subset<T: AsRef<str>>(child: &[T], parent: &[T]) -> bool {
    child.iter().all(|child| {
        parent
            .iter()
            .any(|parent| pattern_covers(parent.as_ref(), child.as_ref()))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        authority_effect_family, authority_effect_guard_required, authority_effect_proof_kinds,
        authority_term_has_verb, evaluate_authority_effect_guards, items_subset,
        optional_bound_subset, parent_items_preserved, pattern_covers, patterns_subset,
    };

    #[test]
    fn patterns_cover_exact_prefix_and_wildcard() {
        assert!(pattern_covers("*", "slack/channel.post"));
        assert!(pattern_covers("slack/*", "slack/channel.post"));
        assert!(pattern_covers("slack/*", "slack/channel.*"));
        assert!(!pattern_covers("slack/*", "github/issue.read"));
        assert!(!pattern_covers("slack/channel.*", "slack/*"));
        assert!(pattern_covers("slack://acme/C1", "slack://acme/C1"));
        assert!(!pattern_covers("slack://acme/C1", "slack://acme/C2"));
        assert!(patterns_subset(&["a/x", "a/y"], &["a/*"]));
        assert!(!patterns_subset(&["a/x"], &[] as &[&str]));
        assert!(patterns_subset(&[] as &[&str], &[] as &[&str]));
    }
    use runx_contracts::{
        AuthorityBounds, AuthorityEffectGuard, AuthorityEffectGuardKind, AuthorityResourceFamily,
        AuthorityTerm, AuthorityVerb, ProofKind, Reference, ReferenceType,
    };

    #[test]
    fn item_subset_is_reflexive() {
        let values = ["read", "write", "verify"];

        assert!(items_subset(&values, &values));
    }

    #[test]
    fn authority_term_verb_lookup_is_exact() {
        let term = term("deployment", AuthorityResourceFamily::Deployment, vec![]);

        assert!(authority_term_has_verb(&term, AuthorityVerb::Verify));
        assert!(!authority_term_has_verb(&term, AuthorityVerb::Write));
    }

    #[test]
    fn effect_guard_decision_is_generic_for_deployment_receipt_before_success() -> Result<(), String>
    {
        let parent = term(
            "parent",
            AuthorityResourceFamily::Deployment,
            vec![AuthorityEffectGuard {
                family: "deployment".into(),
                guard_kinds: vec![AuthorityEffectGuardKind::ReceiptBeforeSuccess],
                proof_kinds: vec![ProofKind::CredentialResolution],
            }],
        );
        let child = term("child", AuthorityResourceFamily::Deployment, vec![]);
        let family = authority_effect_family(&parent, &child)
            .ok_or_else(|| "expected an effect family".to_owned())?;
        let decision = evaluate_authority_effect_guards(&parent, &child, family);

        assert_eq!(family, "deployment");
        assert!(decision.receipt_before_success_required);
        assert!(!decision.non_replay_required);
        assert_eq!(decision.proof_kinds, vec![ProofKind::CredentialResolution]);
        Ok(())
    }

    #[test]
    fn effect_guard_decision_is_generic_for_delete_style_non_replay() -> Result<(), String> {
        let parent = term(
            "parent",
            AuthorityResourceFamily::Deployment,
            vec![AuthorityEffectGuard {
                family: "deployment-delete".into(),
                guard_kinds: vec![AuthorityEffectGuardKind::NonReplay],
                proof_kinds: vec![ProofKind::CredentialResolution],
            }],
        );
        let child = term("child", AuthorityResourceFamily::Deployment, vec![]);
        let family = authority_effect_family(&parent, &child)
            .ok_or_else(|| "expected an effect family".to_owned())?;
        let decision = evaluate_authority_effect_guards(&parent, &child, family);

        assert_eq!(family, "deployment-delete");
        assert!(decision.non_replay_required);
        assert!(!decision.receipt_before_success_required);
        assert!(authority_effect_guard_required(
            &parent,
            "deployment-delete",
            AuthorityEffectGuardKind::NonReplay
        ));
        assert!(!authority_effect_guard_required(
            &parent,
            "deployment",
            AuthorityEffectGuardKind::NonReplay
        ));
        Ok(())
    }

    #[test]
    fn effect_proof_kinds_are_deduped_across_parent_and_child() {
        let parent = term(
            "parent",
            AuthorityResourceFamily::Deployment,
            vec![AuthorityEffectGuard {
                family: "deployment".into(),
                guard_kinds: Vec::new(),
                proof_kinds: vec![ProofKind::CredentialResolution],
            }],
        );
        let child = term(
            "child",
            AuthorityResourceFamily::Deployment,
            vec![AuthorityEffectGuard {
                family: "deployment".into(),
                guard_kinds: Vec::new(),
                proof_kinds: vec![ProofKind::CredentialResolution],
            }],
        );

        assert_eq!(
            authority_effect_proof_kinds(&parent, &child, "deployment"),
            vec![ProofKind::CredentialResolution]
        );
    }

    #[test]
    fn parent_items_are_preserved_when_child_keeps_parent_requirements() {
        let parent = ["approval", "mfa"];
        let child = ["approval", "mfa", "reason"];

        assert!(parent_items_preserved(&child, &parent));
        assert!(!parent_items_preserved(&["approval"], &parent));
    }

    #[test]
    fn optional_bounds_deny_missing_or_larger_child_bounds() {
        assert!(optional_bound_subset(Some(5_u64), Some(10_u64)));
        assert!(!optional_bound_subset(Some(11_u64), Some(10_u64)));
        assert!(!optional_bound_subset::<u64>(None, Some(10_u64)));
        assert!(optional_bound_subset(Some(10_u64), None));
    }

    fn term(
        term_id: &str,
        resource_family: AuthorityResourceFamily,
        effects: Vec<AuthorityEffectGuard>,
    ) -> AuthorityTerm {
        AuthorityTerm {
            term_id: term_id.to_owned().into(),
            principal_ref: Reference::with_uri(ReferenceType::Principal, "runx:principal:agent"),
            resource_ref: Reference::with_uri(ReferenceType::Deployment, "runx:deployment:prod"),
            resource_family,
            verbs: vec![
                AuthorityVerb::Read,
                AuthorityVerb::Verify,
                AuthorityVerb::Delete,
            ],
            bounds: AuthorityBounds {
                effects,
                ..Default::default()
            },
            conditions: Vec::new(),
            approvals: Vec::new(),
            capabilities: Vec::new(),
            expires_at: None,
            issued_by_ref: Reference::with_uri(ReferenceType::Principal, "runx:principal:issuer"),
            credential_ref: None,
        }
    }
}
