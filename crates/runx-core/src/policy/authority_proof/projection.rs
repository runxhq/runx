use runx_contracts::policy_proof::{
    AuthorityProof, AuthorityProofApprovalDecision, AuthorityProofApprovalDecisionValue,
    AuthorityProofCredentialMaterial, AuthorityProofCredentialMaterialStatus,
    AuthorityProofRedaction, AuthorityProofRedactionSecretMaterial, AuthorityProofRedactionStatus,
    AuthorityProofRedactionStream, AuthorityProofRequested, AuthorityProofSchemaVersion,
    CredentialEnvelope, ScopeAdmission, ScopeAdmissionStatus,
};
use runx_contracts::sha256_hex;

use crate::policy::{
    AuthorityProofMetadata, BuildAuthorityProofOptions, LocalScopeAdmissionOptions,
    credential_grant::{CredentialGrantRequirement, credential_grant_requirement},
    scope::unique_strings,
};

use super::{
    admission::build_local_scope_admission,
    util::{non_empty_option, non_empty_vec},
};

#[must_use]
pub fn build_authority_proof(options: &BuildAuthorityProofOptions) -> AuthorityProof {
    let requirement = credential_grant_requirement(options.auth.as_ref())
        .ok()
        .flatten();
    let scope_admission = options.scope_admission.clone().unwrap_or_else(|| {
        build_local_scope_admission(
            options.auth.as_ref(),
            &options.grants,
            &LocalScopeAdmissionOptions {
                connected_auth_checked_at: options.connected_auth_checked_at.clone(),
                ..LocalScopeAdmissionOptions::default()
            },
        )
    });
    AuthorityProof {
        schema_version: AuthorityProofSchemaVersion::V1,
        run_id: non_empty_option(options.run_id.clone()),
        skill_name: options.skill_name.clone().into(),
        source_type: options.source_type.clone().into(),
        requested: authority_proof_requested(&requirement, options),
        scope_admission: scope_admission.clone(),
        credential_material: credential_material_proof(
            options.credential.as_ref(),
            requirement.as_ref(),
            &scope_admission,
        ),
        execution_boundary: options.execution_boundary,
        approval_gate: options.approval.as_ref().map(approval_decision),
        redaction: authority_redaction(),
    }
}

#[must_use]
pub fn build_authority_proof_metadata(
    options: &BuildAuthorityProofOptions,
) -> AuthorityProofMetadata {
    AuthorityProofMetadata {
        authority_proof: build_authority_proof(options),
    }
}

fn authority_proof_requested(
    requirement: &Option<CredentialGrantRequirement>,
    options: &BuildAuthorityProofOptions,
) -> AuthorityProofRequested {
    let mut requested_scopes = options.scopes.clone();
    if let Some(requirement) = requirement {
        requested_scopes.extend(requirement.scopes.iter().cloned());
    }
    AuthorityProofRequested {
        connected_auth: requirement.is_some(),
        scopes: non_empty_vec(unique_strings(&requested_scopes)),
        scope_family: requirement
            .as_ref()
            .and_then(|value| non_empty_option(value.scope_family.clone())),
        authority_kind: requirement
            .as_ref()
            .and_then(|value| value.authority_kind.clone()),
        target_repo: requirement
            .as_ref()
            .and_then(|value| non_empty_option(value.target_repo.clone())),
        target_locator: requirement
            .as_ref()
            .and_then(|value| non_empty_option(value.target_locator.clone())),
    }
}

fn credential_material_proof(
    credential: Option<&CredentialEnvelope>,
    requirement: Option<&CredentialGrantRequirement>,
    scope_admission: &ScopeAdmission,
) -> AuthorityProofCredentialMaterial {
    if let Some(credential) = credential {
        return resolved_credential_material(credential);
    }
    match requirement {
        None => AuthorityProofCredentialMaterial {
            status: AuthorityProofCredentialMaterialStatus::NotRequested,
            ..empty_credential_material()
        },
        Some(requirement) => unresolved_credential_material(requirement, scope_admission),
    }
}

fn resolved_credential_material(
    credential: &CredentialEnvelope,
) -> AuthorityProofCredentialMaterial {
    AuthorityProofCredentialMaterial {
        status: AuthorityProofCredentialMaterialStatus::Resolved,
        grant_id: Some(credential.grant_id.clone()),
        provider: Some(credential.provider.clone()),
        provider_reference: Some(credential.provider_reference.clone()),
        scopes: Some(credential.scopes.clone()),
        grant_reference: credential.grant_reference.clone(),
        material_ref_hash: Some(sha256_hex(credential.material_ref.as_bytes()).into()),
        ..empty_credential_material()
    }
}

fn unresolved_credential_material(
    requirement: &CredentialGrantRequirement,
    scope_admission: &ScopeAdmission,
) -> AuthorityProofCredentialMaterial {
    AuthorityProofCredentialMaterial {
        status: if scope_admission.status == ScopeAdmissionStatus::Deny {
            AuthorityProofCredentialMaterialStatus::Denied
        } else {
            AuthorityProofCredentialMaterialStatus::NotResolved
        },
        grant_id: scope_admission.grant_id.clone(),
        provider: Some(requirement.provider.clone().into()),
        scopes: Some(non_empty_vec(unique_strings(&requirement.scopes))),
        scope_family: non_empty_option(requirement.scope_family.clone()),
        authority_kind: requirement.authority_kind.clone(),
        target_repo: non_empty_option(requirement.target_repo.clone()),
        target_locator: non_empty_option(requirement.target_locator.clone()),
        ..empty_credential_material()
    }
}

fn approval_decision(
    approval: &crate::policy::AuthorityProofApproval,
) -> AuthorityProofApprovalDecision {
    AuthorityProofApprovalDecision {
        gate_id: approval.gate.id.clone().into(),
        gate_type: approval
            .gate
            .gate_type
            .clone()
            .unwrap_or_else(|| "unspecified".to_owned())
            .into(),
        decision: if approval.approved {
            AuthorityProofApprovalDecisionValue::Approved
        } else {
            AuthorityProofApprovalDecisionValue::Denied
        },
        reason: non_empty_option(approval.gate.reason.clone()),
    }
}

fn authority_redaction() -> AuthorityProofRedaction {
    AuthorityProofRedaction {
        status: AuthorityProofRedactionStatus::Applied,
        secret_material: AuthorityProofRedactionSecretMaterial::Omitted,
        stdout: AuthorityProofRedactionStream::Hashed,
        stderr: AuthorityProofRedactionStream::Hashed,
        metadata_secret_keys: vec![
            "token-like metadata keys".into(),
            "api-key-like metadata keys".into(),
            "password-like metadata keys".into(),
            "client-secret-like metadata keys".into(),
            "raw-secret-like metadata keys".into(),
        ],
    }
}

fn empty_credential_material() -> AuthorityProofCredentialMaterial {
    AuthorityProofCredentialMaterial {
        status: AuthorityProofCredentialMaterialStatus::NotRequested,
        grant_id: None,
        provider: None,
        provider_reference: None,
        scopes: None,
        scope_family: None,
        authority_kind: None,
        target_repo: None,
        target_locator: None,
        grant_reference: None,
        material_ref_hash: None,
    }
}
