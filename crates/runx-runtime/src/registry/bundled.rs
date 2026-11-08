//! Core skills ship as ordinary, complete packages inside the runtime artifact.
//! Acquisition changes; package validation, execution and authority do not.
//! Each materializes into the runx home, at `<home>/bundled/<name>/<version>/`
//! and never into a project, and binds as `runx/<name>` only while its bytes
//! are the ones the CLI carries (D30).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use runx_contracts::sha256_prefixed;
use serde::Deserialize;

use super::install::{InstallCandidate, InstallLocalSkillOptions, install_bundled_skill};
use super::package_files::registry_bundle_digest;
use super::{InstallLocalSkillResult, RegistryPublishPackageParts};
use crate::RunxPaths;

static BUNDLE: LazyLock<Result<BundledSkills, String>> = LazyLock::new(|| {
    let mut bundle: BundledSkills = serde_json::from_str(include_str!("../../bundled-skills.json"))
        .map_err(|error| format!("invalid bundled core skills: {error}"))?;
    let manual =
        super::package_files::validate_registry_skill_package(&bundle.orientation, None, &[])
            .map_err(|error| error.to_string())?;
    bundle.skills.push(BundledSkill {
        skill_id: format!("runx/{}", manual.skill.name),
        package: RegistryPublishPackageParts {
            markdown: bundle.orientation.clone(),
            profile_document: None,
            package_files: Vec::new(),
            // The manual has no profile, so it declares no connectors.
            metadata: super::package_metadata::project_registry_package_metadata(
                &manual.skill,
                None,
                &manual.test_fixtures,
                Vec::new(),
            )?,
        },
        version: String::new(),
    });
    for skill in &mut bundle.skills {
        let bytes = serde_json::to_vec(&skill.package).map_err(|error| error.to_string())?;
        skill.version = format!("sha-{}", &sha256_prefixed(&bytes)[7..19]);
    }
    Ok(bundle)
});

#[derive(Deserialize)]
struct BundledSkills {
    orientation: String,
    skills: Vec<BundledSkill>,
}

#[derive(Deserialize)]
pub(crate) struct BundledSkill {
    pub(crate) skill_id: String,
    package: RegistryPublishPackageParts,
    #[serde(skip)]
    version: String,
}

/// Each bundled skill's execution package digest, by its `runx/<name>`: what a
/// materialized copy must load as to bind as the CLI's own.
static PACKAGE_DIGESTS: LazyLock<Result<BTreeMap<String, String>, String>> = LazyLock::new(|| {
    bundled_skills()?
        .iter()
        .map(|skill| {
            skill
                .validated_package()
                .map(|package| (skill.skill_id.clone(), package.package_digest))
                .map_err(|error| error.to_string())
        })
        .collect()
});

/// `runx/<name>` when the package at `package_root`, loaded with
/// `package_digest`, is a bundled skill as the CLI carries it: materialized
/// in the home's bundled skills and holding exactly its bytes. A copy
/// anywhere else, or with any other byte, is not the CLI's.
///
/// # Errors
///
/// Returns an error when the bundle the CLI carries does not read.
pub(crate) fn bundled_identity(
    package_root: &Path,
    package_digest: &str,
    paths: &RunxPaths,
) -> Result<Option<String>, String> {
    let Ok(home) = std::fs::canonicalize(paths.bundled_skills()) else {
        return Ok(None);
    };
    let Some(name) = package_root
        .strip_prefix(&home)
        .ok()
        .and_then(|relative| relative.components().next())
        .map(|name| format!("runx/{}", name.as_os_str().to_string_lossy()))
    else {
        return Ok(None);
    };
    let digests = PACKAGE_DIGESTS.as_ref().map_err(Clone::clone)?;
    Ok((digests.get(&name).map(String::as_str) == Some(package_digest)).then_some(name))
}

/// Whether `reference` names a bundled skill the way only a bundled skill is
/// named: `runx/<name>`, with no `registry:`, which resolves to the CLI's own
/// copy at every depth and to nothing else (D30).
#[must_use]
pub fn is_bundled_reference(reference: &str) -> bool {
    runx_parser::registry_ref(reference).is_none()
        && reference.strip_prefix("runx/").is_some_and(|rest| {
            !rest.is_empty() && !rest.contains(['/', '\\']) && rest != "." && rest != ".."
        })
}

fn bundled_skill(name: &str) -> Result<Option<&'static BundledSkill>, String> {
    Ok(bundled_skills()?
        .iter()
        .find(|skill| skill.skill_id == name || skill.package.metadata.name == name))
}

pub(crate) fn bundled_skills() -> Result<&'static [BundledSkill], String> {
    Ok(&BUNDLE.as_ref().map_err(Clone::clone)?.skills)
}

/// Resolve only bytes carried by this runtime. Other references return `None`;
/// this function never acquires packages or accepts caller-authored bundle bytes.
pub fn materialize_bundled_skill(
    reference: &str,
    paths: &RunxPaths,
    expected_digest: Option<&str>,
) -> Result<Option<InstallLocalSkillResult>, String> {
    let parsed = super::parse_registry_ref(reference);
    let Some(skill) = bundled_skill(&parsed.skill_id)? else {
        return Ok(None);
    };
    if let Some(version) = parsed.version
        && version != skill.version()
    {
        return Err(format!(
            "bundled core skill '{}' has version '{}', not '{}'; use an explicit path to retained package bytes or `registry:{}@{}` to acquire another version",
            skill.skill_id,
            skill.version(),
            version,
            skill.skill_id,
            version
        ));
    }
    skill.materialize(paths, expected_digest).map(Some)
}

impl BundledSkill {
    fn version(&self) -> &str {
        &self.version
    }

    pub(crate) fn validated_package(
        &self,
    ) -> Result<runx_parser::ValidatedSkillPackage, runx_parser::SkillPackageError> {
        super::package_files::validate_registry_skill_package(
            &self.package.markdown,
            self.package.profile_document.as_deref(),
            &self.package.package_files,
        )
    }

    fn materialize(
        &self,
        paths: &RunxPaths,
        expected_digest: Option<&str>,
    ) -> Result<InstallLocalSkillResult, String> {
        let version = self.version();
        let profile_digest = self
            .package
            .profile_document
            .as_ref()
            .map(|profile| sha256_prefixed(profile.as_bytes()));
        let bundle_digest = registry_bundle_digest(&self.package.package_files);
        let candidate = InstallCandidate {
            markdown: self.package.markdown.clone(),
            profile_document: self.package.profile_document.clone(),
            package_files: self.package.package_files.clone(),
            bundle_digest,
            source: "bundled:runx".to_owned(),
            source_label: "Bundled Runx core".to_owned(),
            // Named by its name and version alone: the home's bundled skills
            // are runx's, so no publisher directory is needed.
            package: format!("{}@{version}", self.package.metadata.name),
            skill_id: Some(self.skill_id.clone()),
            version: Some(version.to_owned()),
            signed_manifest: None,
            profile_digest,
            method_names: self.package.metadata.method_names.clone(),
            tier_statement: None,
            manifest_source_authority: None,
        };
        install_bundled_skill(
            &candidate,
            &InstallLocalSkillOptions {
                destination_root: paths.bundled_skills(),
                // The connectors it carries are added to the project, so the
                // project can bind their instances; the skill never is.
                connectors_root: paths.added_connectors(),
                expected_digest: expected_digest.map(str::to_owned),
                // Bundled bytes are the runtime's own, signed by nothing.
                registry_keys: Vec::new(),
                tier_statements: None,
            },
        )
        .map_err(|error| error.to_string())
    }
}
