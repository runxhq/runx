use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::KeyPair;
use runx_contracts::{sha256_hex, sha256_prefixed};
use runx_runtime::registry::{
    InstallCandidate, InstallError, InstallLocalSkillOptions, RegistryManifestSignature,
    RegistryManifestSigner, RegistryManifestSourceAuthority, RegistryManifestTrustEnvError,
    RegistryPackageFile, RegistrySignedManifest, RegistrySubject, RegistrySubjectKind, TrustTier,
    TrustedRegistryManifestKey, install_local_skill, skill_content_digest,
    trusted_registry_manifest_keys_from_env,
};
use tempfile::tempdir;

const TEST_MANIFEST_KEY_ID: &str = "runx-registry-test-key";
const TEST_MANIFEST_SIGNER_ID: &str = "runx-registry-test-signer";
const DYNAMIC_MANIFEST_SEED: [u8; 32] = [7; 32];

#[test]
fn trusted_signed_manifest_installs() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let candidate = install_candidate()?;

    let install = install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            expected_digest: None,
            trusted_manifest_keys: trusted_manifest_keys()?,
        },
    )?;

    assert_eq!(install.skill_id.as_deref(), Some("acme/echo"));
    assert_eq!(install.digest, skill_digest());
    assert!(install.destination.exists());
    Ok(())
}

#[test]
fn package_files_install_and_digest_mismatch_fails() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.package_files = package_files_fixture();
    candidate.package_digest = Some(package_digest(&candidate.package_files)?);
    candidate.signed_manifest = Some(signed_for(&candidate, "acme/echo")?);

    let install = install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            expected_digest: None,
            trusted_manifest_keys: trusted_manifest_keys()?,
        },
    )?;

    let package_root = install.destination.parent().ok_or("missing package root")?;
    assert_eq!(
        std::fs::read_to_string(package_root.join("graph/stage/run.mjs"))?,
        "console.log('installed');\n"
    );
    assert_eq!(
        std::fs::read_to_string(package_root.join("context/review-rubric/SKILL.md"))?,
        "---\nname: review-rubric\ndescription: review-rubric fixture.\n---\n# Review\n"
    );
    assert_eq!(
        std::fs::read_to_string(package_root.join("references/operator.md"))?,
        "# Operator\n"
    );
    assert_eq!(
        std::fs::read_to_string(package_root.join("graph/stage/run.yaml"))?,
        stage_profile()
    );
    assert_eq!(
        std::fs::read_to_string(package_root.join("push-outbox/manifest.json"))?,
        "{}\n"
    );

    let mut tampered = candidate;
    tampered.package_digest = Some("sha256:not-the-package".to_owned());
    let error = install_error(&tampered, temp.path())?;
    assert!(matches!(error, InstallError::PackageDigestMismatch { .. }));
    Ok(())
}

#[test]
fn package_files_must_be_bound_by_signed_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.package_files = package_files_fixture();
    candidate.package_digest = Some(package_digest(&candidate.package_files)?);

    let error = install_error(&candidate, temp.path())?;

    // The envelope signed the version without these files: a content mismatch.
    assert!(
        matches!(error, InstallError::DigestMismatch { .. }),
        "{error}"
    );
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn tampered_content_fails_against_signed_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.markdown = candidate.markdown.replace("Echo", "Tampered");

    let error = install_error(&candidate, temp.path())?;

    assert!(matches!(error, InstallError::DigestMismatch { .. }));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn unsigned_candidate_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.signed_manifest = None;

    let error = install_error(&candidate, temp.path())?;

    assert!(matches!(error, InstallError::UnsignedManifest(_)));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn unsigned_local_registry_candidate_installs() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.signed_manifest = None;
    candidate.profile_digest = Some(profile_digest());
    candidate.package_files = package_files_fixture();
    candidate.package_digest = Some(package_digest(&candidate.package_files)?);
    candidate.manifest_source_authority = Some(RegistryManifestSourceAuthority::RegistrySource(
        format!("local:{}", temp.path().join("registry").display()),
    ));

    let install = install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            expected_digest: None,
            trusted_manifest_keys: Vec::new(),
        },
    )?;

    let package_root = install.destination.parent().ok_or("missing package root")?;
    assert_eq!(
        std::fs::read_to_string(package_root.join("graph/stage/run.mjs"))?,
        "console.log('installed');\n"
    );
    assert_eq!(install.digest, skill_digest());
    assert_eq!(
        install.profile_digest.as_deref(),
        Some(profile_digest().as_str())
    );
    Ok(())
}

#[test]
fn mismatched_manifest_identity_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.signed_manifest = Some(signed_for(&candidate, "acme/other")?);

    let error = install_error(&candidate, temp.path())?;

    assert!(matches!(
        error,
        InstallError::ManifestIdentityMismatch { .. }
    ));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn a_connector_envelope_never_installs_as_a_skill() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    let skill_id = candidate.skill_id.clone().ok_or("candidate skill id")?;
    // Trusted, over these exact files, for the same ref: only the kind differs.
    candidate.signed_manifest = Some(signed_as(
        &candidate,
        RegistrySubject {
            kind: RegistrySubjectKind::Connector,
            subject_ref: skill_id.clone(),
        },
    )?);

    let error = install_error(&candidate, temp.path())?;

    let InstallError::ManifestIdentityMismatch { expected, actual, .. } = error else {
        return Err(format!("expected an identity mismatch, got {error}").into());
    };
    assert_eq!(expected, format!("skill {skill_id}@1.0.0"));
    assert_eq!(actual, format!("connector {skill_id}@1.0.0"));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn missing_manifest_identity_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.skill_id = None;

    let error = install_error(&candidate, temp.path())?;

    assert!(matches!(
        error,
        InstallError::ManifestIdentityMissing {
            field: "skill_id",
            ..
        }
    ));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn unknown_manifest_key_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    let manifest = candidate
        .signed_manifest
        .as_mut()
        .ok_or("signed manifest missing from fixture")?;
    manifest.signer.key_id = "unknown-key".to_owned();

    let error = install_error(&candidate, temp.path())?;

    assert!(matches!(error, InstallError::UnknownManifestKey { .. }));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn invalid_manifest_signature_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    let manifest = candidate
        .signed_manifest
        .as_mut()
        .ok_or("signed manifest missing from fixture")?;
    manifest.signature = RegistryManifestSignature {
        alg: "ed25519".to_owned(),
        value: "base64:invalid".to_owned(),
    };

    let error = install_error(&candidate, temp.path())?;

    assert!(matches!(
        error,
        InstallError::InvalidManifestSignature { .. }
    ));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn malformed_signed_manifest_payload_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    let manifest = candidate
        .signed_manifest
        .as_mut()
        .ok_or("signed manifest missing from fixture")?;
    manifest.subject.subject_ref = "acme/echo\nversion=1.0.0".to_owned();

    let error = install_error(&candidate, temp.path())?;

    assert!(matches!(
        error,
        InstallError::InvalidManifestSignature { reason, .. } if reason == "malformed payload"
    ));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn registry_install_rejects_out_of_scope_manifest_key() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let candidate = install_candidate()?;
    let public_key = manifest_public_key_base64()?;
    let error = match install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            expected_digest: None,
            trusted_manifest_keys: vec![TrustedRegistryManifestKey::official_from_base64(
                TEST_MANIFEST_KEY_ID.to_owned(),
                &public_key,
            )?],
        },
    ) {
        Ok(_) => return Err("install should fail".into()),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        InstallError::ManifestTrustScopeViolation { .. }
    ));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn registry_install_rejects_official_key_from_non_official_source()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.r#ref = "runx/echo@1.0.0".to_owned();
    candidate.skill_id = Some("runx/echo".to_owned());
    candidate.trust_tier = Some(TrustTier::FirstParty);
    candidate.signed_manifest = Some(signed_for(&candidate, "runx/echo")?);
    let key_pair = dynamic_manifest_key_pair()?;
    let error = match install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            expected_digest: None,
            trusted_manifest_keys: vec![TrustedRegistryManifestKey::official_from_base64(
                TEST_MANIFEST_KEY_ID.to_owned(),
                &STANDARD.encode(key_pair.public_key().as_ref()),
            )?],
        },
    ) {
        Ok(_) => return Err("install should fail".into()),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        InstallError::ManifestTrustScopeViolation { reason, .. }
            if reason.contains("official runx registry source")
    ));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn registry_install_accepts_official_source_signature_for_community_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.manifest_source_authority = Some(RegistryManifestSourceAuthority::OfficialRunx);
    candidate.signed_manifest = Some(signed_for(&candidate, "acme/echo")?);
    let key_pair = dynamic_manifest_key_pair()?;

    let install = install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            expected_digest: None,
            trusted_manifest_keys: vec![TrustedRegistryManifestKey::official_from_base64(
                TEST_MANIFEST_KEY_ID.to_owned(),
                &STANDARD.encode(key_pair.public_key().as_ref()),
            )?],
        },
    )?;

    assert_eq!(install.skill_id.as_deref(), Some("acme/echo"));
    assert!(install.destination.exists());
    Ok(())
}

#[test]
fn registry_install_rejects_third_party_key_outside_owner_namespace()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let candidate = install_candidate()?;
    let public_key = manifest_public_key_base64()?;
    let error = match install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            expected_digest: None,
            trusted_manifest_keys: vec![TrustedRegistryManifestKey::from_base64(
                TEST_MANIFEST_KEY_ID.to_owned(),
                &public_key,
                "other".to_owned(),
                "test-registry".to_owned(),
            )?],
        },
    ) {
        Ok(_) => return Err("install should fail".into()),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        InstallError::ManifestTrustScopeViolation { reason, .. }
            if reason.contains("other/*")
    ));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

#[test]
fn registry_manifest_env_key_cannot_self_promote_to_official()
-> Result<(), Box<dyn std::error::Error>> {
    let key_pair = dynamic_manifest_key_pair()?;
    let env = [
        (
            runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID_ENV.to_owned(),
            TEST_MANIFEST_KEY_ID.to_owned(),
        ),
        (
            runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_KEY_ENV.to_owned(),
            STANDARD.encode(key_pair.public_key().as_ref()),
        ),
        (
            runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_OWNER_ENV.to_owned(),
            "runx".to_owned(),
        ),
        (
            runx_runtime::registry::RUNX_REGISTRY_SOURCE_AUTHORITY_ENV.to_owned(),
            "official_runx".to_owned(),
        ),
    ]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    assert!(matches!(
        trusted_registry_manifest_keys_from_env(&env),
        Err(RegistryManifestTrustEnvError::InvalidKey)
    ));
    Ok(())
}

#[test]
fn registry_install_rejects_unsigned_or_mismatched_trust_tier()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut unsigned = install_candidate()?;
    unsigned.signed_manifest = None;
    let unsigned_error = install_error(&unsigned, temp.path())?;
    assert!(matches!(unsigned_error, InstallError::UnsignedManifest(_)));

    let temp = tempdir()?;
    let mut first_party = install_candidate()?;
    first_party.trust_tier = Some(TrustTier::FirstParty);
    let tier_error = install_error(&first_party, temp.path())?;
    assert!(matches!(
        tier_error,
        InstallError::ManifestTrustScopeViolation { .. }
    ));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

fn install_error(
    candidate: &InstallCandidate,
    temp_path: &std::path::Path,
) -> Result<InstallError, Box<dyn std::error::Error>> {
    match install_local_skill(
        candidate,
        &InstallLocalSkillOptions {
            destination_root: temp_path.join("skills"),
            expected_digest: None,
            trusted_manifest_keys: trusted_manifest_keys()?,
        },
    ) {
        Ok(_) => Err("install should fail".into()),
        Err(error) => Ok(error),
    }
}

fn install_candidate() -> Result<InstallCandidate, Box<dyn std::error::Error>> {
    let mut candidate = InstallCandidate {
        markdown: include_str!("../../../fixtures/registry/install/echo-SKILL.md").to_owned(),
        profile_document: Some(
            include_str!("../../../fixtures/registry/install/echo-run.yaml").to_owned(),
        ),
        package_files: Vec::new(),
        package_digest: None,
        source: "runx-registry".to_owned(),
        source_label: "runx registry".to_owned(),
        r#ref: "acme/echo@1.0.0".to_owned(),
        skill_id: Some("acme/echo".to_owned()),
        version: Some("1.0.0".to_owned()),
        signed_manifest: None,
        profile_digest: None,
        runner_names: vec!["default".to_owned()],
        trust_tier: Some(TrustTier::Community),
        manifest_source_authority: Some(RegistryManifestSourceAuthority::RegistrySource(
            "test-registry".to_owned(),
        )),
    };
    candidate.signed_manifest = Some(signed_for(&candidate, "acme/echo")?);
    Ok(candidate)
}

fn skill_digest() -> String {
    sha256_prefixed(include_str!("../../../fixtures/registry/install/echo-SKILL.md").as_bytes())
}

fn profile_digest() -> String {
    sha256_prefixed(include_str!("../../../fixtures/registry/install/echo-run.yaml").as_bytes())
}

fn package_files_fixture() -> Vec<RegistryPackageFile> {
    vec![
        RegistryPackageFile {
            path: "graph/stage/run.mjs".to_owned(),
            content: "console.log('installed');\n".to_owned(),
        },
        RegistryPackageFile {
            path: "context/review-rubric/SKILL.md".to_owned(),
            content:
                "---\nname: review-rubric\ndescription: review-rubric fixture.\n---\n# Review\n"
                    .to_owned(),
        },
        RegistryPackageFile {
            path: "references/operator.md".to_owned(),
            content: "# Operator\n".to_owned(),
        },
        RegistryPackageFile {
            path: "graph/stage/run.yaml".to_owned(),
            content: stage_profile().to_owned(),
        },
        RegistryPackageFile {
            path: "push-outbox/manifest.json".to_owned(),
            content: "{}\n".to_owned(),
        },
    ]
}

fn stage_profile() -> &'static str {
    "skill: stage\nrunners:\n  operate:\n    default: true\n    steps:\n      - id: operate\n        agent: { role: operator, task: operate }\n        outputs:\n          result: object\n  script:\n    steps:\n      - id: script\n        tool: { module: run.mjs }\n"
}

fn package_digest(files: &[RegistryPackageFile]) -> Result<String, serde_json::Error> {
    let mut sorted = files.to_vec();
    sorted.sort_by(|left, right| left.path.cmp(&right.path));
    let mut canonical = String::from("{\"files\":[");
    for (index, file) in sorted.iter().enumerate() {
        if index > 0 {
            canonical.push(',');
        }
        canonical.push_str("{\"content\":");
        canonical.push_str(&serde_json::to_string(&file.content)?);
        canonical.push_str(",\"path\":");
        canonical.push_str(&serde_json::to_string(&file.path)?);
        canonical.push('}');
    }
    canonical.push_str("]}");
    Ok(sha256_hex(canonical.as_bytes()))
}

fn trusted_manifest_keys() -> Result<Vec<TrustedRegistryManifestKey>, Box<dyn std::error::Error>> {
    let public_key = manifest_public_key_base64()?;
    Ok(vec![TrustedRegistryManifestKey::from_base64(
        TEST_MANIFEST_KEY_ID.to_owned(),
        &public_key,
        "acme".to_owned(),
        "test-registry".to_owned(),
    )?])
}

fn manifest_public_key_base64() -> Result<String, Box<dyn std::error::Error>> {
    Ok(STANDARD.encode(dynamic_manifest_key_pair()?.public_key().as_ref()))
}

/// The envelope the registry serves for a candidate's exact files, under `skill_id`.
fn signed_for(
    candidate: &InstallCandidate,
    skill_id: &str,
) -> Result<RegistrySignedManifest, Box<dyn std::error::Error>> {
    signed_as(
        candidate,
        RegistrySubject {
            kind: RegistrySubjectKind::Skill,
            subject_ref: skill_id.to_owned(),
        },
    )
}

/// A trusted envelope over a candidate's exact files, for any subject.
fn signed_as(
    candidate: &InstallCandidate,
    subject: RegistrySubject,
) -> Result<RegistrySignedManifest, Box<dyn std::error::Error>> {
    let content_digest = skill_content_digest(
        &candidate.markdown,
        candidate.profile_document.as_deref(),
        &candidate.package_files,
    )?;
    let mut manifest = RegistrySignedManifest {
        schema: runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA.to_owned(),
        subject,
        version: "1.0.0".to_owned(),
        content_digest,
        signer: RegistryManifestSigner {
            id: TEST_MANIFEST_SIGNER_ID.to_owned(),
            key_id: TEST_MANIFEST_KEY_ID.to_owned(),
        },
        signature: RegistryManifestSignature {
            alg: "ed25519".to_owned(),
            value: "base64:".to_owned(),
        },
    };
    let signature = dynamic_manifest_key_pair()?.sign(&manifest.signing_payload()?);
    manifest.signature.value = format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
    Ok(manifest)
}

fn dynamic_manifest_key_pair() -> Result<ring::signature::Ed25519KeyPair, std::io::Error> {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(&DYNAMIC_MANIFEST_SEED).map_err(|error| {
        std::io::Error::other(format!(
            "dynamic registry manifest seed rejected: {error:?}"
        ))
    })
}
