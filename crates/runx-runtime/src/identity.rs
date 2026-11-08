//! Who a package is (D30). The runtime sets it when it binds a package, from
//! how the package was bound; no manifest, input or workspace file states it.
//!
//! - Bundled in the CLI: `runx/<name>`, `first_party`.
//! - Carrying a `signature.json` that verifies against its exact bytes, a
//!   `registry:` dependency among them: `<publisher>/<name>`, from the
//!   signature, at the tier a fresh statement gives it.
//! - Incorporated code, a relative `skill:` path: the package carrying it.
//! - Anything else: `project:<name>`, its manifest's name, the person's own.
//!
//! A package whose signature fails is refused, never bound as the project's,
//! because that would quietly run altered code.

use std::collections::BTreeMap;

use runx_contracts::{RegistrySubjectKind, TrustTier};

use crate::kernel::connectors::{ConnectorOrigin, ConnectorScope, LoadedConnector};
use crate::registry::{SignedPackage, carries_signature, verify_package_signature};
use crate::{LoadedSkillPackage, RunxPaths};

/// `project:<name>` at `root`, as a run holds it: its directory relative to
/// the workspace when within it, else absolute.
#[must_use]
pub(crate) fn project_binding(
    name: &str,
    root: &std::path::Path,
    paths: &RunxPaths,
) -> runx_contracts::ProjectBinding {
    let workspace = std::fs::canonicalize(paths.workspace())
        .unwrap_or_else(|_| paths.workspace().to_path_buf());
    let directory = match root.strip_prefix(&workspace) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_owned(),
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => root.to_string_lossy().into_owned(),
    };
    runx_contracts::ProjectBinding {
        name: name.into(),
        directory: directory.into(),
    }
}

/// Who a bound package is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PackageIdentity {
    /// Bundled in the CLI: `runx/<name>`.
    Bundled { name: String },
    /// Its `signature.json` verified against its exact bytes.
    Signed(SignedPackage),
    /// The person's own: `project:<name>`.
    Project { name: String },
}

impl PackageIdentity {
    /// The identity as a rule and a person read it: `runx/<name>`,
    /// `<publisher>/<name>` or `project:<name>`.
    #[must_use]
    pub(crate) fn name(&self) -> String {
        match self {
            Self::Bundled { name } => name.clone(),
            Self::Signed(signed) => signed.name.clone(),
            Self::Project { name } => format!("project:{name}"),
        }
    }

    #[must_use]
    pub(crate) fn publisher(&self) -> Option<&str> {
        match self {
            Self::Bundled { .. } => Some("runx"),
            Self::Signed(signed) => signed.publisher(),
            Self::Project { .. } => None,
        }
    }

    #[must_use]
    pub(crate) const fn is_project(&self) -> bool {
        matches!(self, Self::Project { .. })
    }

    /// The tier a decision reads: a bundled package's is `first_party`, a
    /// project package has none, and a signed package's is `community` unless
    /// `stated`, a fresh tier statement, says more.
    #[must_use]
    pub(crate) fn tier(&self, stated: Option<TrustTier>) -> Option<TrustTier> {
        match self {
            Self::Bundled { .. } => Some(TrustTier::FirstParty),
            Self::Signed(_) => Some(stated.unwrap_or(TrustTier::Community)),
            Self::Project { .. } => None,
        }
    }
}

/// The identity of the skill package `loaded`, bound by itself: from its
/// signature when it carries one, `runx/<name>` when it is the CLI's own
/// materialized copy, and otherwise the project's.
///
/// # Errors
///
/// Returns why a signature it carries is refused, or why the keys or the
/// bundle cannot be read.
pub(crate) fn skill_identity(
    loaded: &LoadedSkillPackage,
    env: &BTreeMap<String, String>,
) -> Result<PackageIdentity, String> {
    let paths = RunxPaths::resolve(env, &loaded.package_root);
    if let Some(signed) = signed_skill(loaded, env, &paths)? {
        return Ok(PackageIdentity::Signed(signed));
    }
    if let Some(name) = crate::registry::bundled_identity(
        &loaded.package_root,
        &loaded.package.package_digest,
        &paths,
    )? {
        return Ok(PackageIdentity::Bundled { name });
    }
    Ok(PackageIdentity::Project {
        name: loaded.package.skill.name.clone(),
    })
}

/// The bundled skill `reference`, `runx/<name>`, resolved to `loaded`: it must
/// be the CLI's own copy, byte for byte.
///
/// # Errors
///
/// Returns an error when the copy is not the CLI's.
pub(crate) fn bundled_skill_identity(
    reference: &str,
    loaded: &LoadedSkillPackage,
    env: &BTreeMap<String, String>,
) -> Result<PackageIdentity, String> {
    let paths = RunxPaths::resolve(env, &loaded.package_root);
    match crate::registry::bundled_identity(
        &loaded.package_root,
        &loaded.package.package_digest,
        &paths,
    )? {
        Some(name) => Ok(PackageIdentity::Bundled { name }),
        None => Err(format!(
            "{reference} names the skill this runx bundles, and {} is not its copy",
            loaded.package_root.display()
        )),
    }
}

/// The installed copy `loaded` that the registry ref `reference` resolved to,
/// verified: it must carry a signature over its exact bytes that names the
/// skill the ref names, at the version it pins (D30).
///
/// # Errors
///
/// Returns why the copy is refused: no signature, a signature that fails, or
/// one naming another skill or version.
pub(crate) fn registry_copy_identity(
    reference: &str,
    loaded: &LoadedSkillPackage,
    env: &BTreeMap<String, String>,
) -> Result<SignedPackage, String> {
    let paths = RunxPaths::resolve(env, &loaded.package_root);
    let signed = signed_skill(loaded, env, &paths)?.ok_or_else(|| {
        format!(
            "{reference} resolves to {}, which carries no {}; install it again",
            loaded.package_root.display(),
            crate::registry::SIGNATURE_FILE
        )
    })?;
    let parsed = crate::registry::parse_registry_ref(reference);
    if signed.name != parsed.skill_id
        || parsed
            .version
            .as_deref()
            .is_some_and(|version| version != signed.version)
    {
        return Err(format!(
            "{reference} resolves to {}, which is signed as {}@{}",
            loaded.package_root.display(),
            signed.name,
            signed.version
        ));
    }
    Ok(signed)
}

/// The signature `loaded` carries at its package root, verified under the
/// registry keys `env` and its home name.
fn signed_skill(
    loaded: &LoadedSkillPackage,
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
) -> Result<Option<SignedPackage>, String> {
    if !carries_signature(&loaded.package_root) {
        return Ok(None);
    }
    let keys = crate::registry::registry_keys(env, paths).map_err(|error| error.to_string())?;
    verify_package_signature(&loaded.package_root, RegistrySubjectKind::Skill, &keys)
}

/// The identity of a connector bound by itself: a runtime capability is
/// runx's, a version carrying a signature for exactly this connector and
/// version is its signer's, and any other is the project's: a plugin's, or a
/// version with no signature of its own, such as one a skill package carried.
///
/// # Errors
///
/// Returns why a signature it carries is refused.
pub(crate) fn connector_identity(
    connector: &LoadedConnector,
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
) -> Result<PackageIdentity, String> {
    let capability = ConnectorScope::runtime_capabilities()
        .map_err(|error| error.to_string())?
        .iter()
        .any(|capability| {
            capability.name == connector.name
                && capability.connector_digest == connector.connector_digest
        });
    let bundled = || PackageIdentity::Bundled {
        name: format!("runx/{}", connector.name),
    };
    if capability {
        return Ok(bundled());
    }
    let project = || PackageIdentity::Project {
        name: connector.name.as_str().to_owned(),
    };
    match &connector.origin {
        ConnectorOrigin::RuntimeCapability => Ok(bundled()),
        ConnectorOrigin::Plugin { .. } => Ok(project()),
        ConnectorOrigin::Installed { directory } => {
            if !carries_signature(directory) {
                return Ok(project());
            }
            let keys =
                crate::registry::registry_keys(env, paths).map_err(|error| error.to_string())?;
            let Some(signed) =
                verify_package_signature(directory, RegistrySubjectKind::Connector, &keys)?
            else {
                return Ok(project());
            };
            let version = directory
                .file_name()
                .map(|version| version.to_string_lossy().into_owned());
            if signed.name != connector.name.as_str() || version.as_deref() != Some(&signed.version)
            {
                return Err(format!(
                    "{} holds connector {}, and its signature names {}@{}",
                    directory.display(),
                    connector.name,
                    signed.name,
                    signed.version
                ));
            }
            Ok(PackageIdentity::Signed(signed))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn text(error: impl std::fmt::Display) -> String {
        error.to_string()
    }

    fn write(path: &Path, contents: &str) -> Result<(), String> {
        std::fs::create_dir_all(path.parent().ok_or("a file has a directory")?).map_err(text)?;
        std::fs::write(path, contents).map_err(text)
    }

    /// A workspace with no runx home of its own: `HOME` is unset, so the home
    /// falls back to `<workspace>/.runx`, which the repository ships.
    fn fell_back(workspace: &Path) -> BTreeMap<String, String> {
        BTreeMap::from([(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.display().to_string(),
        )])
    }

    /// The cold read's case D, as a regression: a repository that ships
    /// `.runx/added-skills/runx/fake/1.0.0/` with a `.runx/installs` record
    /// claiming `runx/fake@1.0.0`, `first_party`, and a `.runx/trust` record,
    /// gains nothing. The package binds as the person's own, with no
    /// publisher and no tier, however the home resolves.
    #[test]
    fn a_repository_shipping_install_and_trust_records_states_no_identity() -> Result<(), String> {
        let workspace = tempfile::tempdir().map_err(text)?;
        let root = workspace.path();
        let package = root.join(".runx/added-skills/runx/fake/1.0.0");
        write(
            &package.join("SKILL.md"),
            "---\nname: fake\ndescription: Claims to be runx's.\n---\n# Fake\n",
        )?;
        write(
            &root.join(".runx/installs/added-skills/runx/fake/1.0.0.json"),
            r#"{"package": "runx/fake@1.0.0", "trust_tier": "first_party"}"#,
        )?;
        write(
            &root.join(
                ".runx/trust/0000000000000000000000000000000000000000000000000000000000000000.json",
            ),
            r#"{"closure_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000", "package": "skill runx/fake@1.0.0", "trust_tier": "first_party"}"#,
        )?;
        let loaded = crate::load_validated_skill_package(&package).map_err(text)?;
        let home = tempfile::tempdir().map_err(text)?;
        let mut rooted = fell_back(root);
        rooted.insert("HOME".to_owned(), home.path().display().to_string());
        for env in [fell_back(root), rooted] {
            let identity = skill_identity(&loaded, &env)?;
            assert_eq!(
                identity,
                PackageIdentity::Project {
                    name: "fake".to_owned()
                }
            );
            assert_eq!(identity.publisher(), None);
            assert_eq!(identity.tier(Some(TrustTier::FirstParty)), None);
        }
        Ok(())
    }

    /// The cold read's cases A to C, as a regression: a connector a
    /// repository ships in `.runx/added-connectors`, with an install record
    /// claiming `first_party`, is the person's own, which the load's rules
    /// and D12 decide on; the record states nothing.
    #[test]
    fn a_repository_shipping_a_connector_and_its_install_record_states_no_identity()
    -> Result<(), String> {
        let workspace = tempfile::tempdir().map_err(text)?;
        let root = workspace.path();
        let directory = root.join(".runx/added-connectors/marker/1.0.0");
        write(
            &directory.join("connector.yaml"),
            "schema: runx.connector.v1\nname: marker\ntitle: Marker\ndescription: Writes a marker.\nauth: {none: {}}\nplacements: {local: {auth: [none]}}\noperations:\n  record.read:\n    description: Read a record.\n    verb: read\n    scopes: [record.read]\n    target: {template: \"ops://records\"}\n",
        )?;
        write(
            &root.join(".runx/installs/added-connectors/marker/1.0.0.json"),
            r#"{"package": "marker@1.0.0", "trust_tier": "first_party"}"#,
        )?;
        let connector = crate::kernel::connectors::load_installed(&directory).map_err(text)?;
        let env = fell_back(root);
        let paths = RunxPaths::resolve(&env, root);
        let identity = connector_identity(&connector, &env, &paths)?;
        assert_eq!(
            identity,
            PackageIdentity::Project {
                name: "marker".to_owned()
            }
        );
        assert_eq!(identity.tier(Some(TrustTier::FirstParty)), None);
        Ok(())
    }

    /// A repository can plant `<workspace>/.runx/bundled/`, which is the home
    /// once it falls back; bytes that are not the CLI's are never
    /// `runx/<name>` there, or anywhere.
    #[test]
    fn planted_bytes_in_a_fallen_back_home_are_not_bundled() -> Result<(), String> {
        let workspace = tempfile::tempdir().map_err(text)?;
        let root = workspace.path();
        let package = root.join(".runx/bundled/skill-build/sha-000000000000");
        write(
            &package.join("SKILL.md"),
            "---\nname: skill-build\ndescription: Not the CLI's.\n---\n# Planted\n",
        )?;
        let env = fell_back(root);
        assert!(RunxPaths::resolve(&env, root).home_fell_back());
        let loaded = crate::load_validated_skill_package(&package).map_err(text)?;
        assert_eq!(
            skill_identity(&loaded, &env)?,
            PackageIdentity::Project {
                name: "skill-build".to_owned()
            }
        );
        assert!(bundled_skill_identity("runx/skill-build", &loaded, &env).is_err());
        Ok(())
    }

    /// A package carrying a signature binds as its signer's only while it
    /// verifies; one whose bytes changed is refused, never the project's.
    #[test]
    fn a_signed_package_binds_as_its_signer_and_altered_bytes_are_refused() -> Result<(), String> {
        let workspace = tempfile::tempdir().map_err(text)?;
        let root = workspace.path();
        let package = root.join(".runx/added-skills/acme/echo/1.0.0");
        write(
            &package.join("SKILL.md"),
            "---\nname: echo\ndescription: Echoes.\n---\n# Echo\n",
        )?;
        crate::registry::signature::testing::sign(
            &package,
            RegistrySubjectKind::Skill,
            "acme/echo",
            "1.0.0",
        )?;
        let mut env = crate::registry::signature::testing::trusting("acme")?;
        env.extend(fell_back(root));
        let loaded = crate::load_validated_skill_package(&package).map_err(text)?;
        let identity = skill_identity(&loaded, &env)?;
        assert_eq!(identity.publisher(), Some("acme"));
        assert_eq!(identity.tier(None), Some(TrustTier::Community));
        let signed = registry_copy_identity("registry:acme/echo@1.0.0", &loaded, &env)?;
        assert_eq!(
            (signed.name.as_str(), signed.version.as_str()),
            ("acme/echo", "1.0.0")
        );
        assert!(registry_copy_identity("registry:acme/echo@1.0.1", &loaded, &env).is_err());
        assert!(registry_copy_identity("registry:acme/other@1.0.0", &loaded, &env).is_err());
        // Without the key that signed it, it is refused, not taken as the
        // person's own.
        let untrusted = fell_back(root);
        assert!(skill_identity(&loaded, &untrusted).is_err());
        write(&package.join("references/added.md"), "added\n")?;
        let altered = crate::load_validated_skill_package(&package).map_err(text)?;
        assert!(skill_identity(&altered, &env).is_err());
        Ok(())
    }

    /// The installed copy a registry ref names binds only after its
    /// signature verifies at that version: an unsigned copy, as a repository
    /// can ship, is refused (row 987).
    #[test]
    fn an_unsigned_installed_copy_of_a_registry_ref_never_binds() -> Result<(), String> {
        let workspace = tempfile::tempdir().map_err(text)?;
        let root = workspace.path();
        let package = root.join(".runx/added-skills/acme/echo/1.0.0");
        write(
            &package.join("SKILL.md"),
            "---\nname: echo\ndescription: Echoes.\n---\n# Echo\n",
        )?;
        write(
            &root.join(".runx/installs/added-skills/acme/echo/1.0.0.json"),
            r#"{"package": "acme/echo@1.0.0", "trust_tier": "verified"}"#,
        )?;
        let loaded = crate::load_validated_skill_package(&package).map_err(text)?;
        let error = registry_copy_identity("registry:acme/echo@1.0.0", &loaded, &fell_back(root))
            .err()
            .ok_or("an unsigned copy binds")?;
        assert!(error.contains("carries no signature.json"), "{error}");
        Ok(())
    }

    #[test]
    fn an_identity_reads_as_its_kind_names_it() {
        let signed = PackageIdentity::Signed(SignedPackage {
            kind: RegistrySubjectKind::Skill,
            name: "acme/echo".to_owned(),
            version: "1.0.0".to_owned(),
            content_digest: format!("sha256:{}", "a".repeat(64)),
            key_id: "operator-key".to_owned(),
        });
        let bundled = PackageIdentity::Bundled {
            name: "runx/memory".to_owned(),
        };
        let project = PackageIdentity::Project {
            name: "github".to_owned(),
        };
        assert_eq!(
            [signed.name(), bundled.name(), project.name()],
            ["acme/echo", "runx/memory", "project:github"]
        );
        assert_eq!(
            [signed.publisher(), bundled.publisher(), project.publisher()],
            [Some("acme"), Some("runx"), None]
        );
        assert_eq!(signed.tier(None), Some(TrustTier::Community));
        assert_eq!(
            signed.tier(Some(TrustTier::Verified)),
            Some(TrustTier::Verified)
        );
        assert_eq!(
            bundled.tier(Some(TrustTier::Community)),
            Some(TrustTier::FirstParty)
        );
        assert_eq!(project.tier(Some(TrustTier::FirstParty)), None);
    }
}
