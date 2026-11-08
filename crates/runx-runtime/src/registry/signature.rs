//! A package's `signature.json`: the registry's signed manifest,
//! `runx.registry.signed_manifest.v1`, unchanged, at the package's root
//! (D30). Install writes it beside the files it signs, and bind verifies it
//! against the package directory's exact bytes, so the package carries its
//! identity wherever it is committed, copied or run, and no workspace file
//! states one.

use std::fs;
use std::path::Path;

use runx_contracts::{RegistryContentFile, RegistrySubjectKind, registry_content_digest};

use super::keys::{RegistryKey, registry_key_signs, verify_registry_signed_manifest};
use super::types::RegistrySignedManifest;

/// The signature's file, at a package's root.
pub const SIGNATURE_FILE: &str = "signature.json";

/// A file a package directory may hold beside what was signed: one a
/// desktop leaves in any directory it shows.
const UNSIGNED_FILES: [&str; 1] = [".DS_Store"];

/// The gitattributes install writes in the directories it adds packages to,
/// so a CRLF checkout of a committed package never changes its bytes.
pub(crate) const GITATTRIBUTES_FILE: &str = ".gitattributes";
const GITATTRIBUTES: &str = "* -text\n";

/// What a verified signature says a package is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedPackage {
    pub kind: RegistrySubjectKind,
    /// `<publisher>/<name>` of a skill; a connector's name.
    pub name: String,
    pub version: String,
    pub content_digest: String,
    pub key_id: String,
}

impl SignedPackage {
    /// Its publisher: a skill's first segment, and a connector's when its
    /// name has two.
    #[must_use]
    pub fn publisher(&self) -> Option<&str> {
        self.name.split_once('/').map(|(publisher, _)| publisher)
    }
}

/// The text install writes as a package's `signature.json`.
pub(crate) fn signature_document(manifest: &RegistrySignedManifest) -> Result<String, String> {
    serde_json::to_string_pretty(manifest)
        .map(|document| format!("{document}\n"))
        .map_err(|error| format!("encoding {SIGNATURE_FILE}: {error}"))
}

/// Writes `* -text` as the `.gitattributes` of `directory`, where install
/// adds packages, when it has none. No package's own file is touched.
pub(crate) fn write_gitattributes(directory: &Path) -> Result<(), String> {
    let path = directory.join(GITATTRIBUTES_FILE);
    if path.exists() {
        return Ok(());
    }
    fs::create_dir_all(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    fs::write(&path, GITATTRIBUTES).map_err(|error| format!("{}: {error}", path.display()))
}

/// Whether the package at `package_root` carries a signature.
#[must_use]
pub fn carries_signature(package_root: &Path) -> bool {
    fs::symlink_metadata(package_root.join(SIGNATURE_FILE)).is_ok()
}

/// The signature the package at `package_root` carries, verified: signed by
/// a key in `keys` whose scope covers its publisher, naming a `kind`, over a
/// content digest its directory's files reproduce exactly. None when it
/// carries none.
///
/// The digest is over every file in the directory but `signature.json` and
/// `.DS_Store`, so any file beyond what was signed refuses it; the loader's
/// ignore list is not used, because a planted `node_modules` is not signed.
///
/// # Errors
///
/// Returns why a signature it carries is refused: unreadable or malformed,
/// signed by a key outside the set or outside its scope, of another kind,
/// or over other bytes.
pub fn verify_package_signature(
    package_root: &Path,
    kind: RegistrySubjectKind,
    keys: &[RegistryKey],
) -> Result<Option<SignedPackage>, String> {
    let path = package_root.join(SIGNATURE_FILE);
    let refused = |reason: String| format!("{} is refused: {reason}", path.display());
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(refused(error.to_string())),
    };
    if !metadata.is_file() {
        return Err(refused("it is not a regular file".to_owned()));
    }
    let manifest: RegistrySignedManifest = fs::read_to_string(&path)
        .map_err(|error| error.to_string())
        .and_then(|text| serde_json::from_str(&text).map_err(|error| error.to_string()))
        .map_err(&refused)?;
    let key = verify_registry_signed_manifest(&manifest, keys)
        .map_err(|failure| refused(failure.reason().to_owned()))?;
    if manifest.subject.kind != kind {
        return Err(refused(format!(
            "it signs a {}, and this is a {}",
            manifest.subject.kind.as_str(),
            kind.as_str()
        )));
    }
    registry_key_signs(key, kind, &manifest.subject.name, None).map_err(&refused)?;
    let content_digest = directory_content_digest(package_root).map_err(&refused)?;
    if content_digest != manifest.content_digest {
        return Err(refused(format!(
            "the package's files are not what was signed: they digest to {content_digest}, and it signs {}",
            manifest.content_digest
        )));
    }
    Ok(Some(SignedPackage {
        kind,
        name: manifest.subject.name,
        version: manifest.version,
        content_digest,
        key_id: key.key_id.clone(),
    }))
}

/// The registry content digest of every file below `root` but its
/// `signature.json` and any `.DS_Store`, each by its POSIX path from `root`.
fn directory_content_digest(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    let mut bytes = 0usize;
    collect(root, root, &mut files, &mut bytes)?;
    registry_content_digest(
        files
            .iter()
            .map(|(path, content)| RegistryContentFile { path, content }),
    )
    .map_err(|error| error.to_string())
}

fn collect(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, String)>,
    bytes: &mut usize,
) -> Result<(), String> {
    let entries =
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if UNSIGNED_FILES.contains(&name.as_ref()) || (directory == root && name == SIGNATURE_FILE)
        {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| format!("{} escaped its package", path.display()))?
            .components()
            .map(|component| component.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| format!("{} is not a UTF-8 path", path.display()))?
            .join("/");
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| format!("{relative}: {error}"))?;
        if metadata.is_dir() {
            collect(root, &path, files, bytes)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(format!(
                "{relative} is neither a file nor a directory, and nothing signed is"
            ));
        }
        if files.len() >= crate::skill_package::MAX_PACKAGE_FILES {
            return Err(format!(
                "the package holds more than {} files",
                crate::skill_package::MAX_PACKAGE_FILES
            ));
        }
        let remaining = crate::skill_package::MAX_PACKAGE_BYTES.saturating_sub(*bytes);
        let content = crate::skill_package::read_within(&path, remaining)
            .map_err(|error| format!("{relative}: {error}"))?
            .ok_or_else(|| {
                format!(
                    "the package holds more than {} bytes",
                    crate::skill_package::MAX_PACKAGE_BYTES
                )
            })?;
        *bytes += content.len();
        let content = String::from_utf8(content)
            .map_err(|_| format!("{relative} is not UTF-8 text, and nothing signed is"))?;
        files.push((relative, content));
    }
    Ok(())
}

/// Signing packages in tests with a key `RUNX_REGISTRY_KEYS` trusts.
#[cfg(test)]
pub(crate) mod testing {
    use std::collections::BTreeMap;
    use std::path::Path;

    use base64::Engine;
    use ring::signature::KeyPair;
    use runx_contracts::{
        RegistryManifestSignature, RegistryManifestSigner, RegistrySignedManifest, RegistrySubject,
        RegistrySubjectKind,
    };

    pub(crate) const KEY_ID: &str = "runx-test-registry-key";
    /// The registry the test key signs from.
    pub(crate) const TEST_REGISTRY: &str = "http://127.0.0.1:9";
    const SEED: [u8; 32] = [11; 32];

    fn key_pair() -> Result<ring::signature::Ed25519KeyPair, String> {
        ring::signature::Ed25519KeyPair::from_seed_unchecked(&SEED)
            .map_err(|error| format!("{error:?}"))
    }

    /// The test key's signature over `payload`.
    pub(crate) fn sign_bytes(payload: &[u8]) -> Result<Vec<u8>, String> {
        Ok(key_pair()?.sign(payload).as_ref().to_vec())
    }

    /// `RUNX_REGISTRY_KEYS`, trusting the test key for `publisher` from a
    /// loopback registry that no test reaches: a refresh from it is refused
    /// before anything leaves the machine.
    pub(crate) fn trusting(publisher: &str) -> Result<BTreeMap<String, String>, String> {
        let public_key =
            base64::engine::general_purpose::STANDARD.encode(key_pair()?.public_key().as_ref());
        Ok(BTreeMap::from([(
            super::super::keys::RUNX_REGISTRY_KEYS_ENV.to_owned(),
            serde_json::json!([{
                "key_id": KEY_ID,
                "public_key": public_key,
                "publisher": publisher,
                "source": TEST_REGISTRY,
            }])
            .to_string(),
        )]))
    }

    /// The registry's signed manifest over `content_digest`.
    pub(crate) fn manifest(
        kind: RegistrySubjectKind,
        name: &str,
        version: &str,
        content_digest: String,
    ) -> Result<RegistrySignedManifest, String> {
        let mut manifest = RegistrySignedManifest {
            schema: runx_contracts::REGISTRY_SIGNED_MANIFEST_SCHEMA.to_owned(),
            subject: RegistrySubject {
                kind,
                name: name.to_owned(),
            },
            version: version.to_owned(),
            content_digest,
            signer: RegistryManifestSigner {
                id: "runx-test-registry".to_owned(),
                key_id: KEY_ID.to_owned(),
            },
            signature: RegistryManifestSignature {
                alg: "ed25519".to_owned(),
                value: String::new(),
            },
        };
        let payload = manifest
            .signing_payload()
            .map_err(|error| error.to_string())?;
        let signature = key_pair()?.sign(&payload);
        manifest.signature.value = format!(
            "base64:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref())
        );
        Ok(manifest)
    }

    /// Writes the `signature.json` the registry would issue for the package
    /// at `package_root` exactly as its files now are.
    pub(crate) fn sign(
        package_root: &Path,
        kind: RegistrySubjectKind,
        name: &str,
        version: &str,
    ) -> Result<RegistrySignedManifest, String> {
        let manifest = manifest(
            kind,
            name,
            version,
            super::directory_content_digest(package_root)?,
        )?;
        std::fs::write(
            package_root.join(super::SIGNATURE_FILE),
            super::signature_document(&manifest)?,
        )
        .map_err(|error| error.to_string())?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::keys::RegistryKeyScope;
    use crate::registry::source_authority::RegistryManifestSourceAuthority;
    use base64::Engine;
    use ring::signature::KeyPair;

    const SEED: [u8; 32] = [9; 32];

    fn key_pair() -> ring::signature::Ed25519KeyPair {
        ring::signature::Ed25519KeyPair::from_seed_unchecked(&SEED)
            .unwrap_or_else(|_| unreachable!("a 32-byte seed makes a key"))
    }

    fn operator_key(publisher: &str) -> Result<RegistryKey, String> {
        RegistryKey::from_base64(
            "operator-key",
            &base64::engine::general_purpose::STANDARD.encode(key_pair().public_key().as_ref()),
            RegistryKeyScope::ThirdParty {
                publisher: publisher.to_owned(),
                source: RegistryManifestSourceAuthority::RegistrySource(
                    "remote:https://registry.acme.test".to_owned(),
                ),
            },
        )
    }

    /// A package directory and the signature over exactly its files.
    fn signed_package(name: &str) -> Result<tempfile::TempDir, String> {
        let text = |error: std::io::Error| error.to_string();
        let package = tempfile::tempdir().map_err(text)?;
        fs::write(package.path().join("SKILL.md"), "---\nname: echo\n---\n").map_err(text)?;
        fs::create_dir_all(package.path().join("references")).map_err(text)?;
        fs::write(package.path().join("references/notes.md"), "notes\n").map_err(text)?;
        let content_digest = directory_content_digest(package.path())?;
        let mut manifest = RegistrySignedManifest {
            schema: runx_contracts::REGISTRY_SIGNED_MANIFEST_SCHEMA.to_owned(),
            subject: runx_contracts::RegistrySubject {
                kind: RegistrySubjectKind::Skill,
                name: name.to_owned(),
            },
            version: "1.0.0".to_owned(),
            content_digest,
            signer: runx_contracts::RegistryManifestSigner {
                id: "registry".to_owned(),
                key_id: "operator-key".to_owned(),
            },
            signature: runx_contracts::RegistryManifestSignature {
                alg: "ed25519".to_owned(),
                value: String::new(),
            },
        };
        let signature = key_pair().sign(&manifest.signing_payload().map_err(|e| e.to_string())?);
        manifest.signature.value = format!(
            "base64:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref())
        );
        fs::write(
            package.path().join(SIGNATURE_FILE),
            signature_document(&manifest)?,
        )
        .map_err(text)?;
        Ok(package)
    }

    #[test]
    fn a_signature_over_the_exact_files_names_the_package() -> Result<(), String> {
        let package = signed_package("acme/echo")?;
        let keys = [operator_key("acme")?];
        fs::write(package.path().join(".DS_Store"), [0, 159, 146, 150])
            .map_err(|e| e.to_string())?;
        let signed = verify_package_signature(package.path(), RegistrySubjectKind::Skill, &keys)?
            .ok_or("it carries a signature")?;
        assert_eq!(signed.name, "acme/echo");
        assert_eq!(signed.version, "1.0.0");
        assert_eq!(signed.publisher(), Some("acme"));
        let unsigned = tempfile::tempdir().map_err(|e| e.to_string())?;
        assert_eq!(
            verify_package_signature(unsigned.path(), RegistrySubjectKind::Skill, &keys)?,
            None
        );
        Ok(())
    }

    /// A signature that fails is refused, never read as the person's own:
    /// changed bytes, an added file (a planted `node_modules` included), a
    /// key outside the set, or a key whose scope doesn't cover the publisher.
    #[test]
    fn a_signature_that_fails_is_refused() -> Result<(), String> {
        let keys = [operator_key("acme")?];
        let refused = |package: &Path, keys: &[RegistryKey]| {
            verify_package_signature(package, RegistrySubjectKind::Skill, keys).is_err()
        };
        let changed = signed_package("acme/echo")?;
        fs::write(changed.path().join("references/notes.md"), "notes\r\n")
            .map_err(|e| e.to_string())?;
        assert!(refused(changed.path(), &keys));
        let planted = signed_package("acme/echo")?;
        fs::create_dir_all(planted.path().join("node_modules/left-pad"))
            .map_err(|e| e.to_string())?;
        fs::write(planted.path().join("node_modules/left-pad/index.js"), "x")
            .map_err(|e| e.to_string())?;
        assert!(refused(planted.path(), &keys));
        let package = signed_package("acme/echo")?;
        assert!(refused(package.path(), &[]));
        assert!(refused(package.path(), &[operator_key("other")?]));
        assert!(
            verify_package_signature(package.path(), RegistrySubjectKind::Connector, &keys)
                .is_err()
        );
        let runx = signed_package("runx/echo")?;
        assert!(refused(runx.path(), &keys));
        Ok(())
    }

    #[test]
    fn install_marks_where_it_adds_packages_as_binary() -> Result<(), String> {
        let root = tempfile::tempdir().map_err(|e| e.to_string())?;
        let added = root.path().join("added-skills");
        write_gitattributes(&added)?;
        assert_eq!(
            fs::read_to_string(added.join(GITATTRIBUTES_FILE)).map_err(|e| e.to_string())?,
            "* -text\n"
        );
        Ok(())
    }
}
