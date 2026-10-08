use runx_contracts::JsonValue;
use runx_contracts::policy_proof::{ScopeAdmission, ScopeAdmissionStatus};

use crate::policy::{
    LocalAdmissionGrant, LocalScopeAdmissionOptions, ScopeGrantPolicy,
    credential_grant::{credential_grant_requirement, find_matching_grant},
    scope::unique_strings,
};

use super::util::{non_empty_option, non_empty_vec};

#[must_use]
pub fn build_local_scope_admission(
    auth: Option<&JsonValue>,
    grants: &[LocalAdmissionGrant],
    options: &LocalScopeAdmissionOptions,
) -> ScopeAdmission {
    let requirement = match credential_grant_requirement(auth) {
        Ok(Some(requirement)) => requirement,
        Ok(None) => {
            return scope_admission_allow(
                Vec::new(),
                Vec::new(),
                None,
                "no connected auth requested",
            );
        }
        Err(error) => {
            return scope_admission_deny(
                Vec::new(),
                Vec::new(),
                vec![error.message().to_owned()],
                "invalid connected auth request",
            );
        }
    };

    let requested_scopes = unique_strings(&requirement.scopes);
    if options.denied_before_grant_resolution.unwrap_or(false) {
        return scope_admission_deny(
            requested_scopes,
            Vec::new(),
            vec!["structural policy denied before connected auth grant resolution".to_owned()],
            "structural policy denied before grant resolution",
        );
    }

    match find_matching_grant(
        &requirement,
        grants,
        options.connected_auth_checked_at.as_deref(),
        if options.wildcard_scopes_trusted {
            ScopeGrantPolicy::Trusted
        } else {
            ScopeGrantPolicy::Delegated
        },
    ) {
        Some(grant) => scope_admission_allow(
            requested_scopes,
            unique_strings(&grant.scopes),
            Some(grant.grant_id.clone()),
            "matching active grant admitted",
        ),
        None => scope_admission_deny(
            requested_scopes,
            Vec::new(),
            vec![format!(
                "connected auth grant required for provider '{}'",
                requirement.provider
            )],
            "no matching active grant resolved",
        ),
    }
}

fn scope_admission_allow(
    requested_scopes: Vec<String>,
    granted_scopes: Vec<String>,
    grant_id: Option<String>,
    summary: &str,
) -> ScopeAdmission {
    ScopeAdmission {
        status: ScopeAdmissionStatus::Allow,
        requested_scopes: non_empty_vec(requested_scopes),
        granted_scopes: non_empty_vec(granted_scopes),
        grant_id: non_empty_option(grant_id),
        reasons: None,
        decision_summary: Some(summary.to_owned()),
    }
}

fn scope_admission_deny(
    requested_scopes: Vec<String>,
    granted_scopes: Vec<String>,
    reasons: Vec<String>,
    summary: &str,
) -> ScopeAdmission {
    ScopeAdmission {
        status: ScopeAdmissionStatus::Deny,
        requested_scopes: non_empty_vec(requested_scopes),
        granted_scopes: non_empty_vec(granted_scopes),
        grant_id: None,
        reasons: Some(non_empty_vec(reasons)),
        decision_summary: Some(summary.to_owned()),
    }
}
