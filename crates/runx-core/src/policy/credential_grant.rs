use runx_contracts::{JsonObject, JsonValue, json_string_field as string_field};
use serde::{Deserialize, Serialize};

use runx_contracts::policy_proof::AuthorityKind;

use super::rfc3339::parse_rfc3339_moment;
use super::{
    LocalAdmissionGrant,
    scope::{ScopeGrantPolicy, scope_grant_allows},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CredentialGrantRequirementError {
    message: String,
}

impl CredentialGrantRequirementError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct CredentialGrantRequirement {
    pub provider: String,
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_kind: Option<AuthorityKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_locator: Option<String>,
}

pub(crate) fn credential_grant_requirement(
    auth: Option<&JsonValue>,
) -> Result<Option<CredentialGrantRequirement>, CredentialGrantRequirementError> {
    match auth {
        None | Some(JsonValue::Null) | Some(JsonValue::Bool(false)) => Ok(None),
        Some(JsonValue::Object(object)) => requirement_from_object(object),
        Some(_) => Err(CredentialGrantRequirementError::new(
            "connected auth must be an object, null, or false",
        )),
    }
}

pub(crate) fn find_matching_grant<'a>(
    requirement: &CredentialGrantRequirement,
    grants: &'a [LocalAdmissionGrant],
    connected_auth_checked_at: Option<&str>,
    scope_policy: ScopeGrantPolicy,
) -> Option<&'a LocalAdmissionGrant> {
    grants.iter().find(|grant| {
        grant.provider == requirement.provider
            // Fail closed: only an explicitly active grant admits. A missing
            // status (omitted JSON deserializes to `None`) must not be treated
            // as live.
            && grant.status == Some(super::LocalAdmissionGrantStatus::Active)
            && grant_lifetime_allows(grant, connected_auth_checked_at)
            && requirement.scopes.iter().all(|scope| {
                grant
                    .scopes
                    .iter()
                    .any(|granted_scope| scope_grant_allows(granted_scope, scope, scope_policy))
            })
            && grant_reference_matches(requirement, grant)
    })
}

fn grant_lifetime_allows(grant: &LocalAdmissionGrant, checked_at: Option<&str>) -> bool {
    let Some(expires_at) = grant.expires_at.as_deref() else {
        return false;
    };
    let Some(checked_at) = checked_at.and_then(parse_rfc3339_moment) else {
        return false;
    };
    let Some(expires_at) = parse_rfc3339_moment(expires_at) else {
        return false;
    };
    if checked_at >= expires_at {
        return false;
    }

    match grant.not_before.as_deref().map(parse_rfc3339_moment) {
        Some(Some(not_before)) => checked_at >= not_before,
        Some(None) => false,
        None => true,
    }
}

pub(crate) fn grant_reference_matches(
    requirement: &CredentialGrantRequirement,
    grant: &LocalAdmissionGrant,
) -> bool {
    if !has_requirement_reference(requirement) {
        return !has_grant_reference(grant);
    }

    grant.scope_family == requirement.scope_family
        && grant.authority_kind == requirement.authority_kind
        && grant.target_repo == requirement.target_repo
        && grant.target_locator == requirement.target_locator
}

pub(crate) fn has_grant_reference(grant: &LocalAdmissionGrant) -> bool {
    truthy_string(&grant.scope_family)
        || grant.authority_kind.is_some()
        || truthy_string(&grant.target_repo)
        || truthy_string(&grant.target_locator)
}

fn has_requirement_reference(requirement: &CredentialGrantRequirement) -> bool {
    truthy_string(&requirement.scope_family)
        || requirement.authority_kind.is_some()
        || truthy_string(&requirement.target_repo)
        || truthy_string(&requirement.target_locator)
}

fn requirement_from_object(
    object: &JsonObject,
) -> Result<Option<CredentialGrantRequirement>, CredentialGrantRequirementError> {
    let auth_type = string_field(object, "type");
    if matches!(auth_type, Some("env" | "none" | "local")) {
        return Ok(None);
    }

    let provider = string_field(object, "provider")
        .or(auth_type)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CredentialGrantRequirementError::new("connected auth provider is required"))?
        .to_owned();
    let scopes = string_array_field(object, "scopes")?;
    if scopes.is_empty() {
        return Err(CredentialGrantRequirementError::new(
            "connected auth scopes must include at least one non-empty string",
        ));
    }

    Ok(Some(CredentialGrantRequirement {
        provider,
        scopes,
        scope_family: owned_string_field(object, "scope_family"),
        authority_kind: authority_kind_field(object, "authority_kind"),
        target_repo: owned_string_field(object, "target_repo"),
        target_locator: owned_string_field(object, "target_locator"),
    }))
}

fn owned_string_field(object: &JsonObject, field: &str) -> Option<String> {
    string_field(object, field).map(ToOwned::to_owned)
}

fn string_array_field(
    object: &JsonObject,
    field: &str,
) -> Result<Vec<String>, CredentialGrantRequirementError> {
    match object.get(field) {
        Some(JsonValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                JsonValue::String(scope) if !scope.trim().is_empty() => Ok(scope.clone()),
                JsonValue::String(_) => Err(CredentialGrantRequirementError::new(format!(
                    "connected auth {field} cannot include empty strings"
                ))),
                _ => Err(CredentialGrantRequirementError::new(format!(
                    "connected auth {field} must contain only strings"
                ))),
            })
            .collect(),
        Some(_) => Err(CredentialGrantRequirementError::new(format!(
            "connected auth {field} must be an array"
        ))),
        None => Ok(Vec::new()),
    }
}

fn authority_kind_field(object: &JsonObject, field: &str) -> Option<AuthorityKind> {
    match string_field(object, field) {
        Some("read_only") => Some(AuthorityKind::ReadOnly),
        Some("constructive") => Some(AuthorityKind::Constructive),
        Some("destructive") => Some(AuthorityKind::Destructive),
        _ => None,
    }
}

fn truthy_string(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use runx_contracts::{JsonObject, JsonValue};

    use super::{CredentialGrantRequirement, credential_grant_requirement, find_matching_grant};
    use crate::policy::{LocalAdmissionGrant, LocalAdmissionGrantStatus, ScopeGrantPolicy};

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn first_matching_grant_wins(first_id in grant_id(), second_id in grant_id()) {
            prop_assume!(first_id != second_id);
            let requirement = CredentialGrantRequirement {
                provider: "github".to_owned(),
                scopes: vec!["repo:read".to_owned()],
                scope_family: None,
                authority_kind: None,
                target_repo: None,
                target_locator: None,
            };
            let first = matching_grant(first_id.clone(), "repo:*");
            let second = matching_grant(second_id, "*");
            let grants = vec![first, second];

            let matched = find_matching_grant(
                &requirement,
                &grants,
                Some("2026-05-22T00:00:00Z"),
                ScopeGrantPolicy::Delegated,
            );

            prop_assert_eq!(
                matched.map(|grant| grant.grant_id.as_str()),
                Some(first_id.as_str()),
            );
        }
    }

    #[test]
    fn missing_status_denies_even_when_lifetime_is_valid() {
        let requirement = github_repo_read_requirement();
        let mut grant = matching_grant("grant_a".to_owned(), "repo:*");
        grant.status = None;
        let grants = vec![grant];

        let matched = find_matching_grant(
            &requirement,
            &grants,
            Some("2026-05-22T00:00:00Z"),
            ScopeGrantPolicy::Delegated,
        );

        assert!(matched.is_none());
    }

    #[test]
    fn active_grant_without_expiry_denies() {
        let requirement = github_repo_read_requirement();
        let mut grant = matching_grant("grant_a".to_owned(), "repo:*");
        grant.expires_at = None;
        let grants = vec![grant];

        let matched = find_matching_grant(
            &requirement,
            &grants,
            Some("2026-05-22T00:00:00Z"),
            ScopeGrantPolicy::Delegated,
        );

        assert!(matched.is_none());
    }

    #[test]
    fn active_grant_without_checked_at_denies() {
        let requirement = github_repo_read_requirement();
        let grants = vec![matching_grant("grant_a".to_owned(), "repo:*")];

        let matched = find_matching_grant(&requirement, &grants, None, ScopeGrantPolicy::Delegated);

        assert!(matched.is_none());
    }

    #[test]
    fn expired_grant_denies() {
        let requirement = github_repo_read_requirement();
        let grants = vec![matching_grant("grant_a".to_owned(), "repo:*")];

        let matched = find_matching_grant(
            &requirement,
            &grants,
            Some("2026-05-23T00:00:00Z"),
            ScopeGrantPolicy::Delegated,
        );

        assert!(matched.is_none());
    }

    #[test]
    fn malformed_lifetime_denies() {
        let requirement = github_repo_read_requirement();
        let mut grant = matching_grant("grant_a".to_owned(), "repo:*");
        grant.expires_at = Some("2026-5-23T00:00:00Z".to_owned());
        let grants = vec![grant];

        let matched = find_matching_grant(
            &requirement,
            &grants,
            Some("2026-05-22T00:00:00Z"),
            ScopeGrantPolicy::Delegated,
        );

        assert!(matched.is_none());
    }

    #[test]
    fn not_before_future_grant_denies() {
        let requirement = github_repo_read_requirement();
        let mut grant = matching_grant("grant_a".to_owned(), "repo:*");
        grant.not_before = Some("2026-05-23T00:00:00Z".to_owned());
        let grants = vec![grant];

        let matched = find_matching_grant(
            &requirement,
            &grants,
            Some("2026-05-22T00:00:00Z"),
            ScopeGrantPolicy::Delegated,
        );

        assert!(matched.is_none());
    }

    #[test]
    fn non_object_connected_auth_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let auth = JsonValue::String("github".to_owned());

        let Err(error) = credential_grant_requirement(Some(&auth)) else {
            return Err("non-object connected auth must be rejected".into());
        };

        assert_eq!(
            error.message(),
            "connected auth must be an object, null, or false"
        );
        Ok(())
    }

    #[test]
    fn connected_auth_requires_provider() -> Result<(), Box<dyn std::error::Error>> {
        let auth = JsonValue::Object(JsonObject::from_iter([(
            "scopes".to_owned(),
            JsonValue::Array(vec![JsonValue::String("repo:read".to_owned())]),
        )]));

        let Err(error) = credential_grant_requirement(Some(&auth)) else {
            return Err("connected auth without provider must be rejected".into());
        };

        assert_eq!(error.message(), "connected auth provider is required");
        Ok(())
    }

    #[test]
    fn connected_auth_requires_non_empty_scopes() -> Result<(), Box<dyn std::error::Error>> {
        let auth = JsonValue::Object(JsonObject::from_iter([(
            "provider".to_owned(),
            JsonValue::String("github".to_owned()),
        )]));

        let Err(error) = credential_grant_requirement(Some(&auth)) else {
            return Err("connected auth without scopes must be rejected".into());
        };

        assert_eq!(
            error.message(),
            "connected auth scopes must include at least one non-empty string"
        );
        Ok(())
    }

    #[test]
    fn connected_auth_scope_values_must_be_non_empty_strings()
    -> Result<(), Box<dyn std::error::Error>> {
        let auth = JsonValue::Object(JsonObject::from_iter([
            (
                "provider".to_owned(),
                JsonValue::String("github".to_owned()),
            ),
            (
                "scopes".to_owned(),
                JsonValue::Array(vec![JsonValue::String(String::new())]),
            ),
        ]));

        let Err(error) = credential_grant_requirement(Some(&auth)) else {
            return Err("connected auth with empty scope strings must be rejected".into());
        };

        assert_eq!(
            error.message(),
            "connected auth scopes cannot include empty strings"
        );
        Ok(())
    }

    fn github_repo_read_requirement() -> CredentialGrantRequirement {
        CredentialGrantRequirement {
            provider: "github".to_owned(),
            scopes: vec!["repo:read".to_owned()],
            scope_family: None,
            authority_kind: None,
            target_repo: None,
            target_locator: None,
        }
    }

    fn matching_grant(grant_id: String, scope: &str) -> LocalAdmissionGrant {
        LocalAdmissionGrant {
            grant_id,
            provider: "github".to_owned(),
            scopes: vec![scope.to_owned()],
            status: Some(LocalAdmissionGrantStatus::Active),
            not_before: Some("2026-05-21T00:00:00Z".to_owned()),
            expires_at: Some("2026-05-23T00:00:00Z".to_owned()),
            scope_family: None,
            authority_kind: None,
            target_repo: None,
            target_locator: None,
        }
    }

    fn grant_id() -> impl Strategy<Value = String> {
        prop::sample::select(&["grant_a", "grant_b", "grant_c", "grant_d"]).prop_map(str::to_owned)
    }
}
