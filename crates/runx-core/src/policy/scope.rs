use serde::{Deserialize, Serialize};

/// The wildcard authority a scope grant is allowed to exercise.
///
/// Scope names themselves stay open and provider-neutral. This policy is the
/// registry for how those names may be granted:
///
/// - `ExactOnly` interprets every scope except the reserved bare `*` as an
///   opaque exact value.
/// - `Delegated` also admits one concrete `namespace:` segment through
///   `namespace:*`, but never the universal `*` grant.
/// - `Trusted` additionally admits `*` and is reserved for first-party
///   authority propagation and authenticated Runx principals.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeGrantPolicy {
    ExactOnly,
    Delegated,
    Trusted,
}

/// Whether `granted_scope` covers `requested_scope`.
///
/// `namespace:*` matches exactly one non-empty segment. For example,
/// `repo:*` covers `repo:read` but not `repo:admin:keys`. The universal `*`
/// grant is admitted only by [`ScopeGrantPolicy::Trusted`].
#[must_use]
pub fn scope_grant_allows(
    granted_scope: &str,
    requested_scope: &str,
    policy: ScopeGrantPolicy,
) -> bool {
    if granted_scope == "*" {
        return policy == ScopeGrantPolicy::Trusted;
    }
    if granted_scope == requested_scope {
        return true;
    }

    if policy == ScopeGrantPolicy::ExactOnly {
        return false;
    }

    granted_scope
        .strip_suffix('*')
        .filter(|prefix| prefix.len() > 1 && prefix.ends_with(':'))
        .and_then(|prefix| requested_scope.strip_prefix(prefix))
        .is_some_and(|suffix| !suffix.is_empty() && !suffix.contains(':'))
}

/// Return required scopes that are not covered by any grant under `policy`.
#[must_use]
pub fn missing_granted_scopes(
    required_scopes: &[String],
    granted_scopes: &[String],
    policy: ScopeGrantPolicy,
) -> Vec<String> {
    required_scopes
        .iter()
        .filter(|required| {
            !granted_scopes
                .iter()
                .any(|granted| scope_grant_allows(granted, required, policy))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ScopeGrantPolicy, missing_granted_scopes, scope_grant_allows};

    #[test]
    fn universal_wildcard_is_reserved_for_trusted_grants() {
        assert!(scope_grant_allows(
            "*",
            "repo:read",
            ScopeGrantPolicy::Trusted
        ));
        assert!(!scope_grant_allows(
            "*",
            "repo:read",
            ScopeGrantPolicy::Delegated
        ));
        assert!(!scope_grant_allows("*", "*", ScopeGrantPolicy::Delegated));
    }

    #[test]
    fn delegated_wildcard_allows_one_concrete_namespace_segment() {
        assert!(scope_grant_allows(
            "repo:*",
            "repo:read",
            ScopeGrantPolicy::Delegated
        ));
        assert!(!scope_grant_allows(
            "repo:*",
            "repo:admin:keys",
            ScopeGrantPolicy::Delegated
        ));
        assert!(!scope_grant_allows(
            "repo:*",
            "deploy:prod",
            ScopeGrantPolicy::Delegated
        ));
        assert!(!scope_grant_allows(
            "repo:*",
            "repository:read",
            ScopeGrantPolicy::Delegated
        ));
        assert!(!scope_grant_allows(
            ":*",
            "repo:read",
            ScopeGrantPolicy::Delegated
        ));
    }

    #[test]
    fn exact_policy_does_not_interpret_provider_scope_wildcards() {
        assert!(scope_grant_allows(
            "admin:*",
            "admin:*",
            ScopeGrantPolicy::ExactOnly
        ));
        assert!(!scope_grant_allows(
            "admin:*",
            "admin:write",
            ScopeGrantPolicy::ExactOnly
        ));
    }

    #[test]
    fn empty_namespace_cannot_expand_a_wildcard() {
        for policy in [
            ScopeGrantPolicy::ExactOnly,
            ScopeGrantPolicy::Delegated,
            ScopeGrantPolicy::Trusted,
        ] {
            assert!(!scope_grant_allows(":*", ":read", policy));
            assert!(scope_grant_allows(":*", ":*", policy));
        }
    }

    #[test]
    fn missing_scopes_uses_the_selected_grant_policy() {
        let required = vec!["repo:read".to_owned(), "repo:admin:keys".to_owned()];
        let granted = vec!["repo:*".to_owned()];

        assert_eq!(
            missing_granted_scopes(&required, &granted, ScopeGrantPolicy::Delegated),
            ["repo:admin:keys"]
        );
    }
}
