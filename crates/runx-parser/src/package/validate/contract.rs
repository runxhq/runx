use std::collections::BTreeMap;

use crate::{
    SkillMethodManifest, ValidatedSkill, parse_method_manifest_yaml, parse_skill_markdown,
    validate_method_manifest, validate_skill,
};

use super::{SkillPackageError, SkillPackageSource};

pub(super) fn required_text_file<'a>(
    source: &'a SkillPackageSource,
    path: &str,
) -> Result<&'a str, SkillPackageError> {
    let bytes = source
        .files
        .get(path)
        .ok_or_else(|| SkillPackageError::invalid(path, "required package file is missing"))?;
    text_file(path, bytes)
}

pub(super) fn text_file<'a>(path: &str, bytes: &'a [u8]) -> Result<&'a str, SkillPackageError> {
    std::str::from_utf8(bytes).map_err(|error| {
        SkillPackageError::invalid(path, format!("parser input must be UTF-8: {error}"))
    })
}

pub(super) fn validate_manual(markdown: &str) -> Result<ValidatedSkill, SkillPackageError> {
    let parsed = parse_skill_markdown(markdown).map_err(|source| SkillPackageError::Parse {
        path: "SKILL.md".to_owned(),
        source,
    })?;
    validate_skill(parsed).map_err(|source| SkillPackageError::Validation {
        path: "SKILL.md".to_owned(),
        source,
    })
}

/// The package's `run.yaml`, beside the `SKILL.md` that names it. A
/// `run.yaml` deeper in the package belongs to its own package, so one
/// without a `SKILL.md` beside it is refused.
pub(super) fn validate_profiles(
    source: &SkillPackageSource,
    skill: &ValidatedSkill,
) -> Result<BTreeMap<String, SkillMethodManifest>, SkillPackageError> {
    let mut profiles = BTreeMap::new();
    for path in owned_profile_paths(source) {
        if path != super::super::PROFILE_FILE {
            return Err(SkillPackageError::invalid(
                &path,
                "a run.yaml sits beside the SKILL.md that names its package",
            ));
        }
        let contents = source
            .files
            .get(&path)
            .ok_or_else(|| SkillPackageError::invalid(&path, "profile source is missing"))?;
        let manifest = validate_manifest(&path, &skill.name, text_file(&path, contents)?)?;
        profiles.insert(path, manifest);
    }
    Ok(profiles)
}

fn owned_profile_paths(source: &SkillPackageSource) -> Vec<String> {
    source
        .files
        .keys()
        .filter(|path| path.as_str() == super::super::PROFILE_FILE || path.ends_with("/run.yaml"))
        .filter(|path| !has_nested_manual_boundary(path, source))
        .cloned()
        .collect()
}

pub(super) fn has_nested_manual_boundary(path: &str, source: &SkillPackageSource) -> bool {
    let Some((directory, _)) = path.rsplit_once('/') else {
        return false;
    };
    let mut prefix = String::new();
    for segment in directory.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(segment);
        if source.files.contains_key(&format!("{prefix}/SKILL.md")) {
            return true;
        }
    }
    false
}

fn validate_manifest(
    path: &str,
    skill: &str,
    contents: &str,
) -> Result<SkillMethodManifest, SkillPackageError> {
    let parsed =
        parse_method_manifest_yaml(contents).map_err(|source| SkillPackageError::Parse {
            path: path.to_owned(),
            source,
        })?;
    validate_method_manifest(skill, parsed).map_err(|source| SkillPackageError::Validation {
        path: path.to_owned(),
        source,
    })
}

/// Every agent step's task is a `## Agent task: <name>` section of the
/// package's `SKILL.md`, which carries its instructions.
pub(super) fn validate_agent_task_sections(
    skill: &ValidatedSkill,
    manifest: Option<&SkillMethodManifest>,
) -> Result<(), SkillPackageError> {
    let Some(manifest) = manifest else {
        return Ok(());
    };
    let sections = agent_task_sections(&skill.body);
    for (name, method) in &manifest.methods {
        for step in &method.graph.steps {
            if let crate::StepKind::Agent(agent) = &step.kind
                && !sections.contains(agent.task.as_str())
            {
                return Err(SkillPackageError::invalid(
                    format!("run.yaml.methods.{name}.steps.{}", step.id),
                    format!(
                        "agent task {} has no \"## Agent task: {}\" section in SKILL.md, which carries its instructions",
                        agent.task, agent.task
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The names of a manual's `## Agent task: <name>` sections, outside code.
fn agent_task_sections(markdown: &str) -> std::collections::BTreeSet<&str> {
    let mut fence: Option<&str> = None;
    let mut sections = std::collections::BTreeSet::new();
    for line in markdown.lines() {
        let marker = ["```", "~~~"]
            .into_iter()
            .find(|marker| line.starts_with(marker));
        match (marker, fence) {
            (Some(marker), None) => fence = Some(marker),
            (Some(marker), Some(open)) if marker == open => fence = None,
            (_, None) => {
                if let Some(name) = line.strip_prefix("## Agent task: ") {
                    sections.insert(name.trim());
                }
            }
            _ => {}
        }
    }
    sections
}
