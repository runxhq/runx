use std::collections::BTreeMap;

use runx_contracts::sha256_prefixed;
use runx_runtime::registry::resolve::{RegistryTarget, install_skill_ref};
use runx_runtime::registry::{
    InstallCandidate, InstallError, InstallLocalSkillOptions, RUNX_REGISTRY_KEYS_ENV, RegistryKey,
    RegistryKeyScope, RegistryKeysError, RegistryManifestSignature,
    RegistryManifestSourceAuthority, RegistryPackageFile, RegistrySignedManifest, RegistrySubject,
    RegistrySubjectKind, TrustTier, install_local_skill, registry_keys as key_set,
    skill_content_digest,
};
use tempfile::tempdir;

use crate::registry_fixture::{LoopbackRegistry, Publication};

#[cfg(unix)]
#[test]
fn installed_scripts_run_from_signed_bytes_and_data_stays_non_executable()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    let script = "#!/bin/sh\nprintf 'installed-provider'\n";
    candidate.package_files.extend([
        RegistryPackageFile {
            path: "fixtures/bin/provider".into(),
            content: script.into(),
        },
        RegistryPackageFile {
            path: "fixtures/provider.txt".into(),
            content: "data".into(),
        },
    ]);
    candidate.bundle_digest = Some(bundle_digest(&candidate.package_files)?);
    candidate.signed_manifest = Some(signed_for(&candidate, "acme/echo")?);
    let installed = install_local_skill(&candidate, &options(temp.path())?)?;
    let root = installed
        .destination
        .parent()
        .ok_or("installed package root missing")?;
    let output = std::process::Command::new(root.join("fixtures/bin/provider")).output()?;
    assert!(output.status.success());
    assert_eq!(output.stdout, b"installed-provider");
    assert_eq!(
        std::fs::read_to_string(root.join("fixtures/bin/provider"))?,
        script
    );
    assert_eq!(
        std::fs::metadata(root.join("fixtures/provider.txt"))?
            .permissions()
            .mode()
            & 0o111,
        0
    );
    Ok(())
}

#[test]
fn trusted_signed_manifest_installs() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let candidate = install_candidate()?;

    let install = install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            connectors_root: temp.path().join("connectors"),
            install_records: temp.path().join(".runx").join("installs"),
            expected_digest: None,
            registry_keys: registry_keys()?,
        },
    )?;

    assert_eq!(install.skill_id.as_deref(), Some("acme/echo"));
    assert_eq!(install.digest, skill_digest());
    assert!(install.destination.exists());
    Ok(())
}

/// A published package this runx does not read is refused as unreadable,
/// never as tampering, and leaves nothing in the project.
#[test]
fn a_package_this_runx_does_not_read_leaves_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.profile_document = Some(
        include_str!("../../../fixtures/registry/install/echo/run.yaml")
            .replace("  default:\n", "  default:\n    colour: red\n"),
    );
    candidate.signed_manifest = Some(signed_for(&candidate, "acme/echo")?);
    let skills = temp.path().join("skills");

    let error = install_local_skill(&candidate, &options(temp.path())?)
        .err()
        .ok_or("an unreadable package installed")?;

    assert!(
        matches!(&error, InstallError::Unreadable { reason, .. } if reason.contains("colour")),
        "{error}"
    );
    assert!(!skills.exists() || std::fs::read_dir(&skills)?.next().is_none());
    Ok(())
}

/// A connector the package carries is read before the package is placed: a
/// version whose connector this runx does not read leaves no files, no
/// staging directory and no connector behind.
#[test]
fn a_carried_connector_this_runx_does_not_read_leaves_nothing()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.package_files = unreadable_connector_files();
    candidate.bundle_digest = Some(bundle_digest(&candidate.package_files)?);
    candidate.signed_manifest = Some(signed_for(&candidate, "acme/echo")?);

    let error = install_local_skill(&candidate, &options(temp.path())?)
        .err()
        .ok_or("a package carrying an unreadable connector installed")?;

    assert!(
        matches!(
            &error,
            InstallError::Unreadable { reason, .. }
                if reason.starts_with("connectors/acme/records/connector.yaml: ")
        ),
        "{error}"
    );
    let versions = temp.path().join("skills/acme/echo");
    assert!(!versions.exists() || std::fs::read_dir(&versions)?.next().is_none());
    assert!(!temp.path().join("connectors").exists());
    Ok(())
}

/// A name without a version installs the newest version this runx reads and
/// names the newer one it does not; that version, asked for by name, refuses.
/// The registry lists one version a page, so the older one is found on the
/// next.
#[test]
fn a_bare_name_installs_the_newest_version_this_runx_reads()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry = LoopbackRegistry::serve()?;
    registry.page_size(1);
    let markdown = include_str!("../../../fixtures/registry/install/echo/SKILL.md");
    let profile = include_str!("../../../fixtures/registry/install/echo/run.yaml");
    registry.publish(Publication::new("acme", "1.0.0", markdown, Some(profile)))?;
    registry.publish(Publication {
        files: unreadable_connector_files(),
        ..Publication::new("acme", "2.0.0", markdown, Some(profile))
    })?;
    let target = RegistryTarget {
        registry_url: registry.url.clone(),
    };
    let env = registry_env(&registry, temp.path())?;
    let options = InstallLocalSkillOptions {
        registry_keys: key_set(&env, &runx_runtime::RunxPaths::resolve(&env, temp.path()))?,
        ..options(temp.path())?
    };

    let installed = install_skill_ref(
        "acme/echo",
        None,
        target.clone(),
        &env,
        temp.path(),
        |candidate| install_local_skill(candidate, &options),
    )?
    .ok_or("the registry publishes acme/echo")?;

    assert_eq!(installed.installed.version.as_deref(), Some("1.0.0"));
    let unread = installed
        .newest_unread
        .ok_or("the newest version was read")?;
    assert_eq!(unread.version, "2.0.0");
    assert!(
        unread
            .reason
            .starts_with("connectors/acme/records/connector.yaml: "),
        "{}",
        unread.reason
    );
    assert!(!temp.path().join("skills/acme/echo/2.0.0").exists());

    let named = install_skill_ref(
        "acme/echo",
        Some("2.0.0"),
        target,
        &env,
        temp.path(),
        |candidate| install_local_skill(candidate, &options),
    )
    .err()
    .ok_or("a version this runx does not read installed when named")?;
    assert!(
        named
            .into_message()
            .contains("does not read acme/echo@2.0.0")
    );
    Ok(())
}

/// A paid version's listing reaches the ref it resolves to, as the registry
/// served it with the acquisition: the registry derives a listing, and a
/// client only carries it.
#[test]
fn an_acquired_paid_listing_reaches_the_resolved_ref() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry = LoopbackRegistry::serve()?;
    registry.publish(Publication::new(
        "acme",
        "1.0.0",
        include_str!("../../../fixtures/registry/install/echo/SKILL.md"),
        Some(include_str!(
            "../../../fixtures/registry/install/echo/run.yaml"
        )),
    ))?;
    let listing = serde_json::json!({
        "skill_id": "acme/echo",
        "version": "1.0.0",
        "skill_digest": skill_digest(),
        "profile_digest": profile_digest(),
        "bundle_digest": format!("sha256:{}", "c".repeat(64)),
        "seller_ref": {"type": "principal", "uri": "runx:principal:acme"},
        "offers": {"default": {
            "offer_version": {
                "offer_id": "acme/echo#default",
                "version": "1.0.0",
                "profile_digest": profile_digest(),
                "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
                "output_schema_digest": format!("sha256:{}", "e".repeat(64)),
            },
            "price": {"basis": "fixed", "amount_minor": 125},
            "currency": "USD",
            "accepts": ["x402"],
        }},
    });
    registry.tamper("acme/echo", "1.0.0", |record| {
        record["paid_listing"] = listing.clone();
    })?;
    let env = registry_env(&registry, temp.path())?;

    let resolved = runx_runtime::registry::resolve::resolve_skill_ref_details(
        std::path::Path::new("acme/echo@1.0.0"),
        temp.path(),
        runx_runtime::registry::resolve::SkillResolverOptions {
            env: &env,
            registry: Some(&registry.url),
            expected_digest: None,
            absent_pins: runx_runtime::registry::resolve::AbsentPins::Fail,
            holder: runx_runtime::registry::resolve::Holder::Project,
        },
    )?;

    assert_eq!(
        serde_json::to_value(resolved.paid_listing.ok_or("the listing was dropped")?)?,
        listing
    );
    Ok(())
}

/// The environment of a project at `root` reaching `registry` and trusting
/// its key.
fn registry_env(
    registry: &LoopbackRegistry,
    root: &std::path::Path,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut env = registry.env()?;
    env.insert(
        "RUNX_HOME".to_owned(),
        root.join("home").to_string_lossy().into_owned(),
    );
    env.insert(
        runx_runtime::RUNX_CWD_ENV.to_owned(),
        root.to_string_lossy().into_owned(),
    );
    Ok(env)
}

/// A bundle carrying a connector whose manifest has a field this runx does
/// not read; the digests are placeholders, since the manifest never loads.
fn unreadable_connector_files() -> Vec<RegistryPackageFile> {
    let placeholder = format!("sha256:{}", "0".repeat(64));
    vec![
        RegistryPackageFile {
            path: "connectors/acme/records/connector.yaml".to_owned(),
            content: "schema: runx.connector.v1\nname: acme/records\ntitle: Records\ndescription: Records.\ncolour: red\nauth: {none: {}}\nplacements: {local: {auth: [none]}}\noperations: {}\n".to_owned(),
        },
        RegistryPackageFile {
            path: "runx.package.json".to_owned(),
            content: serde_json::json!({
                "schema": "runx.registry.package_bundle.v1",
                "dependencies": [],
                "connectors": [{
                    "from": ".",
                    "connector": "acme/records",
                    "manifest_digest": placeholder,
                    "connector_digest": placeholder,
                }],
            })
            .to_string(),
        },
    ]
}

fn options(root: &std::path::Path) -> Result<InstallLocalSkillOptions, Box<dyn std::error::Error>> {
    Ok(InstallLocalSkillOptions {
        destination_root: root.join("skills"),
        connectors_root: root.join("connectors"),
        install_records: root.join(".runx").join("installs"),
        expected_digest: None,
        registry_keys: registry_keys()?,
    })
}

#[test]
fn package_files_install_and_digest_mismatch_fails() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.package_files = package_files_fixture();
    candidate.bundle_digest = Some(bundle_digest(&candidate.package_files)?);
    candidate.signed_manifest = Some(signed_for(&candidate, "acme/echo")?);

    let install = install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            connectors_root: temp.path().join("connectors"),
            install_records: temp.path().join(".runx").join("installs"),
            expected_digest: None,
            registry_keys: registry_keys()?,
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
    tampered.bundle_digest = Some("sha256:not-the-package".to_owned());
    let error = install_error(&tampered, temp.path())?;
    assert!(matches!(error, InstallError::BundleDigestMismatch { .. }));
    Ok(())
}

#[test]
fn package_files_must_be_bound_by_signed_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.package_files = package_files_fixture();
    candidate.bundle_digest = Some(bundle_digest(&candidate.package_files)?);

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
            name: skill_id.clone(),
        },
    )?);

    let error = install_error(&candidate, temp.path())?;

    let InstallError::ManifestIdentityMismatch {
        expected, actual, ..
    } = error
    else {
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
    manifest.subject.name = "acme/echo\nversion=1.0.0".to_owned();

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
    let public_key = crate::registry_fixture::public_key_base64()?;
    let error = match install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            connectors_root: temp.path().join("connectors"),
            install_records: temp.path().join(".runx").join("installs"),
            expected_digest: None,
            registry_keys: vec![RegistryKey::from_base64(
                crate::registry_fixture::KEY_ID,
                &public_key,
                RegistryKeyScope::Official,
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
    candidate.package = "runx/echo@1.0.0".to_owned();
    candidate.skill_id = Some("runx/echo".to_owned());
    candidate.trust_tier = Some(TrustTier::FirstParty);
    candidate.signed_manifest = Some(signed_for(&candidate, "runx/echo")?);
    let error = match install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            connectors_root: temp.path().join("connectors"),
            install_records: temp.path().join(".runx").join("installs"),
            expected_digest: None,
            registry_keys: vec![RegistryKey::from_base64(
                crate::registry_fixture::KEY_ID,
                &crate::registry_fixture::public_key_base64()?,
                RegistryKeyScope::Official,
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

    let install = install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            connectors_root: temp.path().join("connectors"),
            install_records: temp.path().join(".runx").join("installs"),
            expected_digest: None,
            registry_keys: vec![RegistryKey::from_base64(
                crate::registry_fixture::KEY_ID,
                &crate::registry_fixture::public_key_base64()?,
                RegistryKeyScope::Official,
            )?],
        },
    )?;

    assert_eq!(install.skill_id.as_deref(), Some("acme/echo"));
    assert!(install.destination.exists());
    Ok(())
}

#[test]
fn registry_install_rejects_third_party_key_outside_publisher_namespace()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let candidate = install_candidate()?;
    let public_key = crate::registry_fixture::public_key_base64()?;
    let error = match install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: temp.path().join("skills"),
            connectors_root: temp.path().join("connectors"),
            install_records: temp.path().join(".runx").join("installs"),
            expected_digest: None,
            registry_keys: vec![RegistryKey::from_base64(
                crate::registry_fixture::KEY_ID,
                &public_key,
                RegistryKeyScope::ThirdParty {
                    publisher: "other".to_owned(),
                    source: RegistryManifestSourceAuthority::RegistrySource(
                        "test-registry".to_owned(),
                    ),
                },
            )?],
        },
    ) {
        Ok(_) => return Err("install should fail".into()),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        InstallError::ManifestTrustScopeViolation { reason, .. }
            if reason.contains("in the other namespace")
    ));
    assert!(!temp.path().join("skills").exists());
    Ok(())
}

/// An operator key never signs for the official registry, so it cannot pass
/// for one of the official keys.
#[test]
fn an_operator_key_cannot_name_the_official_registry() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = runx_runtime::RunxPaths::resolve(
        &BTreeMap::from([("RUNX_HOME".to_owned(), temp.path().display().to_string())]),
        temp.path(),
    );
    for official in ["https://runx.ai", "https://api.runx.ai/"] {
        let env = BTreeMap::from([(
            RUNX_REGISTRY_KEYS_ENV.to_owned(),
            serde_json::json!([{
                "key_id": crate::registry_fixture::KEY_ID,
                "public_key": crate::registry_fixture::public_key_base64()?,
                "publisher": "acme",
                "source": official,
            }])
            .to_string(),
        )]);
        assert!(matches!(
            key_set(&env, &paths),
            Err(RegistryKeysError::Env(message)) if message.contains("official registry")
        ));
    }
    Ok(())
}

#[test]
fn registry_install_rejects_an_unsigned_version() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut unsigned = install_candidate()?;
    unsigned.signed_manifest = None;
    let unsigned_error = install_error(&unsigned, temp.path())?;
    assert!(matches!(unsigned_error, InstallError::UnsignedManifest(_)));
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
            connectors_root: temp_path.join("connectors"),
            install_records: temp_path.join(".runx").join("installs"),
            expected_digest: None,
            registry_keys: registry_keys()?,
        },
    ) {
        Ok(_) => Err("install should fail".into()),
        Err(error) => Ok(error),
    }
}

fn install_candidate() -> Result<InstallCandidate, Box<dyn std::error::Error>> {
    let mut candidate = InstallCandidate {
        markdown: include_str!("../../../fixtures/registry/install/echo/SKILL.md").to_owned(),
        profile_document: Some(
            include_str!("../../../fixtures/registry/install/echo/run.yaml").to_owned(),
        ),
        package_files: Vec::new(),
        bundle_digest: None,
        source: "runx-registry".to_owned(),
        source_label: "runx registry".to_owned(),
        package: "acme/echo@1.0.0".to_owned(),
        skill_id: Some("acme/echo".to_owned()),
        version: Some("1.0.0".to_owned()),
        signed_manifest: None,
        profile_digest: None,
        method_names: vec!["default".to_owned()],
        trust_tier: Some(TrustTier::Community),
        manifest_source_authority: Some(RegistryManifestSourceAuthority::RegistrySource(
            "test-registry".to_owned(),
        )),
    };
    candidate.signed_manifest = Some(signed_for(&candidate, "acme/echo")?);
    Ok(candidate)
}

fn skill_digest() -> String {
    sha256_prefixed(include_str!("../../../fixtures/registry/install/echo/SKILL.md").as_bytes())
}

fn profile_digest() -> String {
    sha256_prefixed(include_str!("../../../fixtures/registry/install/echo/run.yaml").as_bytes())
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
            path: "graph/stage/SKILL.md".to_owned(),
            content: stage_manual().to_owned(),
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

fn stage_manual() -> &'static str {
    "---\nname: stage\ndescription: stage fixture.\n---\n# Stage\n\n## Agent task: operate\n\nOperate.\n"
}

fn stage_profile() -> &'static str {
    "minimum_runx: 0.12.0\ndefault_method: operate\nmethods:\n  operate:\n    steps:\n      - id: operate\n        agent: operate\n        outputs:\n          result: {type: object}\n  script:\n    steps:\n      - id: script\n        tool: {module: run.mjs}\n"
}

fn bundle_digest(files: &[RegistryPackageFile]) -> Result<String, Box<dyn std::error::Error>> {
    Ok(crate::registry_fixture::bundle_digest(files)?.ok_or("a bundle holds files")?)
}

fn registry_keys() -> Result<Vec<RegistryKey>, Box<dyn std::error::Error>> {
    let public_key = crate::registry_fixture::public_key_base64()?;
    Ok(vec![RegistryKey::from_base64(
        crate::registry_fixture::KEY_ID,
        &public_key,
        RegistryKeyScope::ThirdParty {
            publisher: "acme".to_owned(),
            source: RegistryManifestSourceAuthority::RegistrySource("test-registry".to_owned()),
        },
    )?])
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
            name: skill_id.to_owned(),
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
    crate::registry_fixture::sign(subject.kind, &subject.name, "1.0.0", content_digest)
}
