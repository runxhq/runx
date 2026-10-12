use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::KeyPair;
use runx_contracts::{RegistryContentFile, registry_content_digest};
use runx_runtime::registry::{
    ConnectorInstallError, InstallRegistryConnectorOptions, InstallStatus,
    RegistryConnectorVersion, RegistryManifestSignature, RegistryManifestSigner,
    RegistryManifestSourceAuthority, RegistryPackageFile, RegistrySignedManifest, RegistrySubject,
    RegistrySubjectKind, TrustTier, TrustedRegistryManifestKey, install_registry_connector,
    prepare_registry_connector_package,
};
use tempfile::tempdir;

const KEY_ID: &str = "connector-test-key";
const SEED: [u8; 32] = [9; 32];
const SOURCE: &str = "https://registry.example";
const ACME_OPS: &str = r#"schema: runx.connector.v1
id: acme-ops
name: Acme Ops
description: Syncs Acme items.
category: dev-tools
runtime_capability: false
auth: {none: {}}
placements: {local: {auth: [none]}}
operations:
  item.sync:
    description: Sync an item.
    verb: read
    scopes: [item.read]
    target: {template: "acme://items"}
    implementation:
      process:
        tool: acme.sync
"#;
const ACME_SYNC_MANIFEST: &str = r#"{"schema": "runx.tool.manifest.v1", "name": "acme.sync", "version": "0.1.0", "description": "Syncs.", "source": {"command": "node", "args": ["./sync.mjs"]}}"#;

#[test]
fn a_signed_connector_version_installs_into_the_package_cache_once()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let (published, package) = published_version(temp.path())?;
    let options = options(temp.path(), "acme")?;

    let installed = install_registry_connector(&published, &options)?;

    let destination = temp.path().join("connectors/acme/acme-ops/1.0.0");
    assert_eq!(installed.status, InstallStatus::Installed);
    assert_eq!(installed.destination, destination);
    assert_eq!(installed.content_digest, package.content_digest);
    assert_eq!(installed.manifest_digest, package.manifest_digest);
    assert_eq!(installed.connector_digest, package.connector_digest);
    for file in &package.files {
        assert_eq!(
            fs::read_to_string(destination.join(&file.path))?,
            file.content
        );
    }
    assert_eq!(staging_entries(temp.path())?, 0);

    let again = install_registry_connector(&published, &options)?;
    assert_eq!(again.status, InstallStatus::Unchanged);
    assert_eq!(again.content_digest, package.content_digest);
    Ok(())
}

#[test]
fn a_version_directory_is_never_rewritten() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let (published, _) = published_version(temp.path())?;
    let options = options(temp.path(), "acme")?;
    install_registry_connector(&published, &options)?;

    // Validly signed, but other files under the same version.
    let mut other = published.clone();
    readme(&mut other.files)?.content = "# Acme Ops, again\n".to_owned();
    other.signed_manifest = sign(
        RegistrySubjectKind::Connector,
        "acme/acme-ops",
        "1.0.0",
        &other.files,
    )?;

    let error = install_registry_connector(&other, &options)
        .err()
        .ok_or("other files under an installed version must be refused")?;

    assert!(
        matches!(error, ConnectorInstallError::ConflictingVersion { .. }),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("connectors/acme/acme-ops/1.0.0/README.md"))?,
        "# Acme Ops\n"
    );
    Ok(())
}

#[test]
fn a_skill_envelope_never_installs_as_a_connector() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let (mut published, _) = published_version(temp.path())?;
    // Trusted, over these exact files, for the same ref: only the kind differs.
    published.signed_manifest = sign(
        RegistrySubjectKind::Skill,
        "acme/acme-ops",
        "1.0.0",
        &published.files,
    )?;

    let error = install_registry_connector(&published, &options(temp.path(), "acme")?)
        .err()
        .ok_or("a skill envelope must not install a connector")?;

    let ConnectorInstallError::ManifestIdentityMismatch {
        expected, actual, ..
    } = error
    else {
        return Err(format!("expected an identity mismatch, got {error}").into());
    };
    assert_eq!(expected, "connector acme/acme-ops@1.0.0");
    assert_eq!(actual, "skill acme/acme-ops@1.0.0");
    assert_nothing_installed(temp.path())
}

#[test]
fn files_changed_after_signing_fail_the_content_digest() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let (mut published, _) = published_version(temp.path())?;
    readme(&mut published.files)?.content = "# Not what was signed\n".to_owned();

    let error = install_registry_connector(&published, &options(temp.path(), "acme")?)
        .err()
        .ok_or("changed files must be refused")?;

    assert!(
        matches!(error, ConnectorInstallError::DigestMismatch { .. }),
        "{error}"
    );
    assert_nothing_installed(temp.path())
}

#[test]
fn files_that_load_as_another_connector_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let (mut published, _) = published_version(temp.path())?;
    published
        .files
        .iter_mut()
        .find(|file| file.path == "plugin.json")
        .ok_or("plugin.json")?
        .content = r#"{"namespace":"other"}"#.to_owned();
    published.signed_manifest = sign(
        RegistrySubjectKind::Connector,
        "acme/acme-ops",
        "1.0.0",
        &published.files,
    )?;

    let error = install_registry_connector(&published, &options(temp.path(), "acme")?)
        .err()
        .ok_or("a package naming another publisher must be refused")?;

    assert!(
        matches!(&error, ConnectorInstallError::InvalidPackage { reason, .. } if reason.contains("other/acme-ops")),
        "{error}"
    );
    assert_nothing_installed(temp.path())
}

#[test]
fn a_file_the_packager_does_not_ship_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let (mut published, _) = published_version(temp.path())?;
    published.files.push(RegistryPackageFile {
        path: "notes.txt".to_owned(),
        content: "not part of a connector package\n".to_owned(),
    });
    published.signed_manifest = sign(
        RegistrySubjectKind::Connector,
        "acme/acme-ops",
        "1.0.0",
        &published.files,
    )?;

    let error = install_registry_connector(&published, &options(temp.path(), "acme")?)
        .err()
        .ok_or("files beyond the package must be refused")?;

    assert!(
        matches!(error, ConnectorInstallError::DigestMismatch { .. }),
        "{error}"
    );
    assert_nothing_installed(temp.path())
}

#[test]
fn a_pinned_content_digest_must_match() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let (published, _) = published_version(temp.path())?;
    let mut options = options(temp.path(), "acme")?;
    options.expected_content_digest = Some(format!("sha256:{}", "0".repeat(64)));

    let error = install_registry_connector(&published, &options)
        .err()
        .ok_or("a different pinned digest must be refused")?;

    assert!(
        matches!(error, ConnectorInstallError::DigestMismatch { .. }),
        "{error}"
    );
    assert_nothing_installed(temp.path())
}

#[test]
fn a_signer_scoped_to_another_publisher_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let (published, _) = published_version(temp.path())?;

    let error = install_registry_connector(&published, &options(temp.path(), "other")?)
        .err()
        .ok_or("a key for another publisher must not sign acme/*")?;

    assert!(
        matches!(
            error,
            ConnectorInstallError::ManifestTrustScopeViolation { .. }
        ),
        "{error}"
    );
    assert_nothing_installed(temp.path())
}

#[test]
fn a_newer_runx_maintained_connector_version_installs_beside_the_embedded_one()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../connectors/github");
    let mut files = prepare_registry_connector_package(&source)?.files;
    // A later runx/github: its manifest, so its digest, differs from the binary's.
    let manifest = files
        .iter_mut()
        .find(|file| file.path == "connector.yaml")
        .ok_or("connector.yaml")?;
    manifest.content = manifest
        .content
        .replacen("description: ", "description: Newer. ", 1);
    let published = RegistryConnectorVersion {
        connector_ref: runx_contracts::connector::ConnectorRef::new("runx/github").ok_or("ref")?,
        version: "9.0.0".to_owned(),
        trust_tier: TrustTier::FirstParty,
        signed_manifest: sign(
            RegistrySubjectKind::Connector,
            "runx/github",
            "9.0.0",
            &files,
        )?,
        files,
    };
    let options = InstallRegistryConnectorOptions {
        connectors_root: temp.path().join("connectors"),
        expected_content_digest: None,
        trusted_manifest_keys: vec![TrustedRegistryManifestKey::official_from_base64(
            KEY_ID.to_owned(),
            &STANDARD.encode(key_pair()?.public_key().as_ref()),
        )?],
        manifest_source_authority: RegistryManifestSourceAuthority::OfficialRunx,
    };

    let installed = install_registry_connector(&published, &options)?;

    assert_eq!(installed.status, InstallStatus::Installed);
    let embedded = runx_runtime::ConnectorScope::runx_maintained_only()?.connector("github")?;
    assert_ne!(installed.manifest_digest, embedded.manifest_digest);
    Ok(())
}

/// The `acme/acme-ops` package as `prepare_registry_connector_package` emits
/// it, published as 1.0.0 with a trusted signature.
fn published_version(
    root: &Path,
) -> Result<
    (
        RegistryConnectorVersion,
        runx_runtime::registry::RegistryConnectorPackage,
    ),
    Box<dyn std::error::Error>,
> {
    let source = root.join("source");
    let connector = source.join("connectors/acme-ops");
    fs::create_dir_all(connector.join("tools/acme/sync"))?;
    fs::write(source.join("plugin.json"), r#"{"namespace":"acme"}"#)?;
    fs::write(connector.join("connector.yaml"), ACME_OPS)?;
    fs::write(
        connector.join("tools/acme/sync/manifest.json"),
        ACME_SYNC_MANIFEST,
    )?;
    fs::write(
        connector.join("tools/acme/sync/sync.mjs"),
        "process.stdout.write('{}');\n",
    )?;
    fs::write(connector.join("README.md"), "# Acme Ops\n")?;
    let package = prepare_registry_connector_package(&connector)?;
    let published = RegistryConnectorVersion {
        connector_ref: package.connector_ref.clone(),
        version: "1.0.0".to_owned(),
        trust_tier: TrustTier::Community,
        signed_manifest: sign(
            RegistrySubjectKind::Connector,
            package.connector_ref.as_str(),
            "1.0.0",
            &package.files,
        )?,
        files: package.files.clone(),
    };
    Ok((published, package))
}

fn sign(
    kind: RegistrySubjectKind,
    subject_ref: &str,
    version: &str,
    files: &[RegistryPackageFile],
) -> Result<RegistrySignedManifest, Box<dyn std::error::Error>> {
    let content_digest = registry_content_digest(files.iter().map(|file| RegistryContentFile {
        path: &file.path,
        content: &file.content,
    }))?;
    let mut manifest = RegistrySignedManifest {
        schema: runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA.to_owned(),
        subject: RegistrySubject {
            kind,
            subject_ref: subject_ref.to_owned(),
        },
        version: version.to_owned(),
        content_digest,
        signer: RegistryManifestSigner {
            id: "connector-test-signer".to_owned(),
            key_id: KEY_ID.to_owned(),
        },
        signature: RegistryManifestSignature {
            alg: "ed25519".to_owned(),
            value: "base64:".to_owned(),
        },
    };
    let signature = key_pair()?.sign(&manifest.signing_payload()?);
    manifest.signature.value = format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
    Ok(manifest)
}

fn options(
    root: &Path,
    allowed_owner: &str,
) -> Result<InstallRegistryConnectorOptions, Box<dyn std::error::Error>> {
    Ok(InstallRegistryConnectorOptions {
        connectors_root: root.join("connectors"),
        expected_content_digest: None,
        trusted_manifest_keys: vec![TrustedRegistryManifestKey::from_base64(
            KEY_ID.to_owned(),
            &STANDARD.encode(key_pair()?.public_key().as_ref()),
            allowed_owner.to_owned(),
            SOURCE.to_owned(),
        )?],
        manifest_source_authority: RegistryManifestSourceAuthority::RegistrySource(
            SOURCE.to_owned(),
        ),
    })
}

fn key_pair() -> Result<ring::signature::Ed25519KeyPair, std::io::Error> {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(&SEED)
        .map_err(|error| std::io::Error::other(format!("test seed rejected: {error:?}")))
}

fn readme(files: &mut [RegistryPackageFile]) -> Result<&mut RegistryPackageFile, &'static str> {
    files
        .iter_mut()
        .find(|file| file.path == "README.md")
        .ok_or("the package ships its README")
}

/// No version directory and no staging left behind.
fn assert_nothing_installed(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let versions = root.join("connectors/acme/acme-ops");
    if versions.exists() {
        assert_eq!(
            fs::read_dir(&versions)?.count(),
            0,
            "{}",
            versions.display()
        );
    }
    Ok(())
}

fn staging_entries(root: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(fs::read_dir(root.join("connectors/acme/acme-ops"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".staging-"))
        .count())
}
