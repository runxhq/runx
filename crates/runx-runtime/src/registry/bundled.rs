//! Core skills ship as ordinary, complete packages inside the runtime artifact.
//! Acquisition changes; package validation, execution and authority do not.

use std::sync::LazyLock;

use runx_contracts::sha256_prefixed;
use serde::Deserialize;

use super::install::{InstallCandidate, InstallLocalSkillOptions, install_bundled_skill};
use super::package_files::registry_package_digest;
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
                Vec::new(),
            ),
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

pub fn bundled_runx_manual() -> Result<&'static str, String> {
    Ok(BUNDLE.as_ref().map_err(Clone::clone)?.orientation.as_str())
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

    /// This skill as the runx-maintained source tree at `root` holds it, when
    /// it does.
    fn source(
        &self,
        root: &std::path::Path,
        expected_digest: Option<&str>,
    ) -> Result<Option<InstallLocalSkillResult>, String> {
        let directory = root.join("skills").join(&self.package.metadata.name);
        let destination = directory.join("SKILL.md");
        let Ok(markdown) = std::fs::read_to_string(&destination) else {
            return Ok(None);
        };
        let digest = sha256_prefixed(markdown.as_bytes());
        if let Some(expected) = expected_digest
            && expected != digest
        {
            return Err(format!(
                "runx-maintained skill '{}' at {} has digest {digest}, not {expected}",
                self.skill_id,
                directory.display()
            ));
        }
        let profile_digest = std::fs::read_to_string(directory.join(runx_parser::PROFILE_FILE))
            .ok()
            .map(|profile| sha256_prefixed(profile.as_bytes()));
        Ok(Some(InstallLocalSkillResult {
            status: super::install::InstallStatus::Unchanged,
            destination,
            skill_name: self.package.metadata.name.clone(),
            source: "source:runx".to_owned(),
            source_label: "runx-maintained source".to_owned(),
            skill_id: Some(self.skill_id.clone()),
            version: Some(self.version().to_owned()),
            digest,
            profile_digest,
            runner_names: self.package.metadata.runner_names.clone(),
            trust_tier: None,
        }))
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
        // Inside the runx-maintained source, the skill is its source.
        if let Some(source) = crate::kernel::connectors::runx_maintained_source(paths.workspace())
            && let Some(result) = self.source(&source, expected_digest)?
        {
            return Ok(result);
        }
        let version = self.version();
        let profile_digest = self
            .package
            .profile_document
            .as_ref()
            .map(|profile| sha256_prefixed(profile.as_bytes()));
        let package_digest = registry_package_digest(&self.package.package_files);
        let candidate = InstallCandidate {
            markdown: self.package.markdown.clone(),
            profile_document: self.package.profile_document.clone(),
            package_files: self.package.package_files.clone(),
            package_digest,
            source: "bundled:runx".to_owned(),
            source_label: "Bundled Runx core".to_owned(),
            r#ref: format!("{}@{version}", self.skill_id),
            skill_id: Some(self.skill_id.clone()),
            version: Some(version.to_owned()),
            signed_manifest: None,
            profile_digest,
            runner_names: self.package.metadata.runner_names.clone(),
            trust_tier: None,
            manifest_source_authority: None,
        };
        install_bundled_skill(
            &candidate,
            &InstallLocalSkillOptions {
                destination_root: paths.skills(),
                connectors_root: paths.connectors(),
                install_records: paths.install_records(),
                expected_digest: expected_digest.map(str::to_owned),
                trusted_manifest_keys: Vec::new(),
            },
        )
        .map_err(|error| error.to_string())
    }
}
