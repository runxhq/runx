use runx_contracts::policy_proof::{
    CredentialEnvelope, CredentialGrantReference, ScopeAdmission, ScopeAdmissionStatus,
};

use crate::policy::{
    CredentialBindingDecision, CredentialBindingRequest, LocalAdmissionGrant,
    credential_grant::{
        CredentialGrantRequirement, credential_grant_requirement, has_grant_reference,
    },
    scope::{ScopeGrantPolicy, scope_grant_allows},
};

use super::util::non_empty_option;

#[must_use]
pub fn validate_credential_binding(
    request: &CredentialBindingRequest,
) -> CredentialBindingDecision {
    let requirement = match credential_grant_requirement(request.auth.as_ref()) {
        Ok(requirement) => requirement,
        Err(error) => return deny(vec![error.message().to_owned()]),
    };
    match request.credential.as_ref() {
        None => validate_missing_credential(requirement.as_ref(), &request.scope_admission),
        Some(credential) => validate_resolved_credential(request, requirement.as_ref(), credential),
    }
}

fn validate_missing_credential(
    requirement: Option<&CredentialGrantRequirement>,
    scope_admission: &ScopeAdmission,
) -> CredentialBindingDecision {
    if requirement.is_some()
        && scope_admission.status == ScopeAdmissionStatus::Allow
        && scope_admission.grant_id.is_some()
    {
        return deny(vec![
            "credential material was not resolved for admitted connected auth grant".to_owned(),
        ]);
    }
    allow(vec!["no credential material resolved".to_owned()])
}

fn validate_resolved_credential(
    request: &CredentialBindingRequest,
    requirement: Option<&CredentialGrantRequirement>,
    credential: &CredentialEnvelope,
) -> CredentialBindingDecision {
    let Some(requirement) = requirement else {
        return deny(vec![
            "credential material resolved for a skill with no connected auth requirement"
                .to_owned(),
        ]);
    };
    let Some(admitted_grant_id) = admitted_grant_id(&request.scope_admission) else {
        return deny(vec![
            "credential material resolved without an admitted connected auth grant".to_owned(),
        ]);
    };
    let Some(admitted_grant) = request
        .grants
        .iter()
        .find(|grant| grant.grant_id == admitted_grant_id)
    else {
        return deny(vec![format!(
            "credential admission references grant '{admitted_grant_id}' that was not resolved",
        )]);
    };

    let reasons = credential_binding_reasons(
        credential,
        requirement,
        admitted_grant,
        &request.scope_admission,
    );
    if reasons.is_empty() {
        allow(vec![
            "credential material matches admitted grant".to_owned(),
        ])
    } else {
        deny(reasons)
    }
}

fn credential_binding_reasons(
    credential: &CredentialEnvelope,
    requirement: &CredentialGrantRequirement,
    admitted_grant: &LocalAdmissionGrant,
    scope_admission: &ScopeAdmission,
) -> Vec<String> {
    let mut reasons = Vec::new();
    collect_credential_identity_reasons(credential, requirement, admitted_grant, &mut reasons);
    collect_credential_scope_reasons(credential, admitted_grant, scope_admission, &mut reasons);
    collect_credential_reference_reasons(credential, admitted_grant, &mut reasons);
    reasons
}

fn collect_credential_identity_reasons(
    credential: &CredentialEnvelope,
    requirement: &CredentialGrantRequirement,
    admitted_grant: &LocalAdmissionGrant,
    reasons: &mut Vec<String>,
) {
    if credential.grant_id != admitted_grant.grant_id {
        reasons.push(format!(
            "credential grant_id '{}' does not match admitted grant '{}'",
            credential.grant_id, admitted_grant.grant_id
        ));
    }
    if credential.provider != requirement.provider || credential.provider != admitted_grant.provider
    {
        reasons.push(format!(
            "credential provider '{}' does not match admitted provider '{}'",
            credential.provider, admitted_grant.provider
        ));
    }
}

fn collect_credential_scope_reasons(
    credential: &CredentialEnvelope,
    admitted_grant: &LocalAdmissionGrant,
    scope_admission: &ScopeAdmission,
    reasons: &mut Vec<String>,
) {
    let missing_requested_scopes = scope_admission
        .requested_scopes
        .iter()
        .filter(|scope| {
            !credential.scopes.iter().any(|credential_scope| {
                scope_grant_allows(credential_scope, scope, ScopeGrantPolicy::Delegated)
            })
        })
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !missing_requested_scopes.is_empty() {
        reasons.push(format!(
            "credential scopes do not include admitted request scope(s): {}",
            missing_requested_scopes.join(", ")
        ));
    }

    let out_of_grant_scopes = credential
        .scopes
        .iter()
        .filter(|scope| {
            !admitted_grant.scopes.iter().any(|granted_scope| {
                scope_grant_allows(granted_scope, scope, ScopeGrantPolicy::Delegated)
            })
        })
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !out_of_grant_scopes.is_empty() {
        reasons.push(format!(
            "credential scopes exceed admitted grant scope(s): {}",
            out_of_grant_scopes.join(", ")
        ));
    }
}

fn collect_credential_reference_reasons(
    credential: &CredentialEnvelope,
    admitted_grant: &LocalAdmissionGrant,
    reasons: &mut Vec<String>,
) {
    let expected_reference = credential_grant_reference(admitted_grant);
    match (
        expected_reference.as_ref(),
        credential.grant_reference.as_ref(),
    ) {
        (Some(_), None) => {
            reasons.push(
                "credential grant_reference is missing for a targeted admitted grant".to_owned(),
            );
        }
        (Some(expected), Some(actual)) => {
            reasons.extend(grant_reference_mismatches(expected, actual));
        }
        (None, Some(_)) => {
            reasons.push(
                "credential grant_reference is present but the admitted grant is not targeted"
                    .to_owned(),
            );
        }
        (None, None) => {}
    }
}

fn credential_grant_reference(grant: &LocalAdmissionGrant) -> Option<CredentialGrantReference> {
    if !has_grant_reference(grant) {
        return None;
    }
    let scope_family = non_empty_option(grant.scope_family.clone())?;
    let authority_kind = grant.authority_kind.clone()?;
    Some(CredentialGrantReference {
        grant_id: grant.grant_id.clone().into(),
        scope_family,
        authority_kind,
        target_repo: non_empty_option(grant.target_repo.clone()),
        target_locator: non_empty_option(grant.target_locator.clone()),
    })
}

fn grant_reference_mismatches(
    expected: &CredentialGrantReference,
    actual: &CredentialGrantReference,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if actual.grant_id != expected.grant_id {
        reasons
            .push("credential grant_reference.grant_id does not match admitted grant".to_owned());
    }
    if actual.scope_family != expected.scope_family {
        reasons.push(
            "credential grant_reference.scope_family does not match admitted grant".to_owned(),
        );
    }
    if actual.authority_kind != expected.authority_kind {
        reasons.push(
            "credential grant_reference.authority_kind does not match admitted grant".to_owned(),
        );
    }
    if actual.target_repo != expected.target_repo {
        reasons.push(
            "credential grant_reference.target_repo does not match admitted grant".to_owned(),
        );
    }
    if actual.target_locator != expected.target_locator {
        reasons.push(
            "credential grant_reference.target_locator does not match admitted grant".to_owned(),
        );
    }
    reasons
}

fn admitted_grant_id(scope_admission: &ScopeAdmission) -> Option<&str> {
    if scope_admission.status != ScopeAdmissionStatus::Allow {
        return None;
    }
    scope_admission.grant_id.as_deref()
}

fn allow(reasons: Vec<String>) -> CredentialBindingDecision {
    CredentialBindingDecision::Allow { reasons }
}

fn deny(reasons: Vec<String>) -> CredentialBindingDecision {
    CredentialBindingDecision::Deny { reasons }
}
