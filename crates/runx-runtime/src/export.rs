use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_parser::{
    CatalogVisibility, SkillInput, SkillMethodManifest, SkillPackageSource, ValidatedSkill,
    ValidatedSkillPackage, validate_skill_package,
};

mod discovery;
mod resolve;

use discovery::{canonicalize, discover_skill_paths, display_path};
use resolve::resolve_skill_ref;

/// The current generated shim's source binding. Its digest identifies the
/// complete package, independently of the source path's spelling.
#[derive(Clone, Debug, PartialEq)]
pub struct RunxExportBinding<'a> {
    pub target: &'a str,
    pub source: &'a str,
    pub package_digest: &'a str,
}

#[must_use]
pub fn parse_export_binding(contents: &str) -> Option<RunxExportBinding<'_>> {
    contents.lines().find_map(|line| {
        let value = line.strip_prefix("<!-- runx-export:")?;
        let (target, value) = value.split_once(" source=")?;
        if !matches!(target, "claude" | "codex") {
            return None;
        }
        let value = value.strip_suffix(" - generated, do not edit -->")?;
        let (source, package_digest) = value.rsplit_once(" package-digest=")?;
        if source.is_empty() || runx_contracts::Digest::new(package_digest).is_none() {
            return None;
        }
        Some(RunxExportBinding {
            target,
            source,
            package_digest,
        })
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunxExportSkill {
    pub name: String,
    pub description: String,
    pub methods: Vec<RunxExportMethod>,
    pub abs_dir: PathBuf,
    pub manual_markdown: String,
    pub manual_digest: String,
    pub package_digest: String,
    pub mode: RunxExportMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunxExportMode {
    Delegated,
    NativeInstructions,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunxExportMethod {
    /// `None` is the unnamed SKILL.md execution front used by packages without
    /// run.yaml. Named run.yaml methods are always explicit in generated commands.
    pub name: Option<String>,
    pub default: bool,
    pub execution_closure_digest: Option<String>,
    pub inputs: BTreeMap<String, SkillInput>,
    /// A run of this method reaches an agent turn that makes calls, so a host
    /// works it through the MCP tool `skill_tool_name` names.
    pub makes_calls: bool,
}

/// The MCP tool a skill server offers for a method: the skill's name for its
/// default method, and `<skill>__<method>` for another.
#[must_use]
pub fn skill_tool_name(skill: &str, method: Option<&str>) -> String {
    match method {
        Some(method) => format!("{skill}__{method}"),
        None => skill.to_owned(),
    }
}

#[derive(Clone, Debug)]
pub struct RunxExportLoadOptions<'a> {
    pub root: &'a Path,
    pub refs: &'a [String],
    pub source_roots: Vec<PathBuf>,
    pub execution_env: Option<&'a BTreeMap<String, String>>,
}

#[derive(Debug)]
pub enum RunxExportLoadError {
    InvalidArgs(String),
    Io {
        context: String,
        source: std::io::Error,
    },
    Parse(String),
}

impl std::fmt::Display for RunxExportLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArgs(message) | Self::Parse(message) => formatter.write_str(message),
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl std::error::Error for RunxExportLoadError {}

pub fn load_export_skills_with_options(
    options: RunxExportLoadOptions<'_>,
) -> Result<Vec<RunxExportSkill>, RunxExportLoadError> {
    let explicit = !options.refs.is_empty();
    let paths = if explicit {
        options
            .refs
            .iter()
            .map(|reference| {
                resolve_skill_ref(
                    options.root,
                    reference,
                    &options.source_roots,
                    options.execution_env,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        discover_skill_paths(options.root)?
    };

    let mut skills = Vec::new();
    for skill_dir in paths {
        let (directory, package, manifest) = load_export_package(options.root, &skill_dir)?;
        if !explicit && manifest_visibility(&manifest) == Some(CatalogVisibility::Internal) {
            continue;
        }
        if !has_agent_export_contract(&package, manifest.as_ref()) {
            if explicit {
                return Err(RunxExportLoadError::InvalidArgs(format!(
                    "cannot export {} as an agent skill because its default method has no sealed standalone invocation contract",
                    package.skill.name
                )));
            }
            continue;
        }
        skills.push(export_skill(
            directory,
            package,
            manifest,
            options.execution_env,
        )?);
    }
    validate_unique_export_names(&mut skills)?;
    Ok(skills)
}

fn has_agent_export_contract(
    package: &ValidatedSkillPackage,
    manifest: Option<&SkillMethodManifest>,
) -> bool {
    let Some(manifest) = manifest else {
        return true;
    };
    if manifest.catalog.as_ref().map(|catalog| catalog.visibility)
        == Some(CatalogVisibility::Internal)
    {
        return false;
    }
    // A host is offered a package whose default method a sealed test proves.
    manifest.default_method().is_some_and(|method| {
        crate::skill_package::method_has_sealed_test(package, manifest, method)
    })
}

fn validate_unique_export_names(skills: &mut [RunxExportSkill]) -> Result<(), RunxExportLoadError> {
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    for pair in skills.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(RunxExportLoadError::InvalidArgs(format!(
                "multiple skills normalize to the export name {:?}",
                pair[0].name
            )));
        }
    }
    Ok(())
}

fn export_skill(
    directory: PathBuf,
    package: ValidatedSkillPackage,
    manifest: Option<SkillMethodManifest>,
    execution_env: Option<&BTreeMap<String, String>>,
) -> Result<RunxExportSkill, RunxExportLoadError> {
    let mode = export_mode(&package.skill, manifest.as_ref());
    let mut methods = export_methods(&package, manifest.as_ref());
    let skill = package.skill;
    if mode == RunxExportMode::Delegated {
        let loaded = crate::load_validated_skill_package(&directory)
            .map_err(|error| RunxExportLoadError::Parse(error.to_string()))?;
        let empty_env = BTreeMap::new();
        let execution_env = execution_env.unwrap_or(&empty_env);
        for method in &mut methods {
            let Some(name) = method.name.as_deref() else {
                continue;
            };
            let binding = crate::skill_package::inspect_loaded_execution_closure_binding(
                loaded.clone().into(),
                name,
                execution_env,
            )
            .map_err(|error| RunxExportLoadError::Parse(error.to_string()))?;
            if !binding.fully_bound {
                return Err(RunxExportLoadError::InvalidArgs(format!(
                    "cannot export {} method {name:?} because its execution closure is not fully bound",
                    skill.name
                )));
            }
            method.execution_closure_digest = Some(binding.digest);
            method.makes_calls = crate::skill_package::method_makes_calls(
                loaded.clone().into(),
                name,
                execution_env,
            )
            .map_err(|error| RunxExportLoadError::Parse(error.to_string()))?;
        }
    }
    Ok(RunxExportSkill {
        // The strict manual rule makes every name one safe path segment.
        name: skill.name,
        description: skill
            .description
            .unwrap_or_else(|| "Run this skill through runx governance.".to_owned()),
        methods,
        abs_dir: directory,
        manual_markdown: package.manual_markdown,
        manual_digest: package.manual_digest,
        package_digest: package.package_digest,
        mode,
    })
}

fn load_export_package(
    workspace_root: &Path,
    skill_dir: &Path,
) -> Result<(PathBuf, ValidatedSkillPackage, Option<SkillMethodManifest>), RunxExportLoadError> {
    let workspace_root = canonicalize(workspace_root, "canonicalizing export workspace")?;
    let is_workspace_manual = skill_dir == workspace_root
        && workspace_root.join("skills").is_dir()
        && !workspace_root.join(runx_parser::PROFILE_FILE).is_file();
    if is_workspace_manual {
        let manual_path = workspace_root.join("SKILL.md");
        let manual =
            fs::read_to_string(&manual_path).map_err(|source| RunxExportLoadError::Io {
                context: format!(
                    "reading workspace skill manual {}",
                    display_path(&manual_path)
                ),
                source,
            })?;
        let package = validate_skill_package(SkillPackageSource::from_documents(manual, None))
            .map_err(|error| RunxExportLoadError::Parse(error.to_string()))?;
        return Ok((skill_dir.to_path_buf(), package, None));
    }

    let loaded = crate::load_validated_skill_package(skill_dir)
        .map_err(|error| RunxExportLoadError::Parse(error.to_string()))?;
    let manifest = loaded.manifest().cloned();
    Ok((loaded.directory, loaded.package, manifest))
}

fn export_mode(_skill: &ValidatedSkill, manifest: Option<&SkillMethodManifest>) -> RunxExportMode {
    if manifest.is_none() {
        return RunxExportMode::NativeInstructions;
    }
    RunxExportMode::Delegated
}

fn export_methods(
    package: &ValidatedSkillPackage,
    manifest: Option<&SkillMethodManifest>,
) -> Vec<RunxExportMethod> {
    let Some(manifest) = manifest else {
        return vec![RunxExportMethod {
            name: None,
            default: true,
            execution_closure_digest: None,
            inputs: std::collections::BTreeMap::new(),
            makes_calls: false,
        }];
    };
    let default = manifest.default_method().map(|method| method.name.as_str());
    let mut methods = manifest
        .methods
        .values()
        .filter(|method| {
            default == Some(method.name.as_str())
                || crate::skill_package::method_has_sealed_test(package, manifest, method)
        })
        .map(|method| RunxExportMethod {
            name: Some(method.name.clone()),
            default: default == Some(method.name.as_str()),
            execution_closure_digest: None,
            inputs: method.inputs.clone(),
            makes_calls: false,
        })
        .collect::<Vec<_>>();
    methods.sort_by(|left, right| {
        right
            .default
            .cmp(&left.default)
            .then_with(|| left.name.cmp(&right.name))
    });
    methods
}

fn manifest_visibility(
    manifest: &Option<runx_parser::SkillMethodManifest>,
) -> Option<CatalogVisibility> {
    manifest
        .as_ref()
        .and_then(|manifest| manifest.catalog.as_ref())
        .map(|catalog| catalog.visibility)
}
#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::fs;

    use super::{RunxExportLoadOptions, RunxExportMode, load_export_skills_with_options};

    #[test]
    fn exports_workspace_manual_without_admitting_repository_as_its_package() {
        let temp = tempfile::tempdir().expect("temporary workspace");
        let root = temp.path();
        let child = root.join("skills/demo");
        fs::create_dir_all(&child).expect("skill directory");
        fs::create_dir_all(root.join("unrelated")).expect("unrelated directory");
        fs::write(
            root.join("SKILL.md"),
            "---\nname: runx\ndescription: Runx runtime guide.\n---\n\n# Runx\n",
        )
        .expect("root manual");
        fs::write(
            child.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo skill.\n---\n\n# Demo\n",
        )
        .expect("child manual");
        fs::write(root.join("unrelated/run.yaml"), "not: [valid")
            .expect("unrelated repository file");

        let skills = load_export_skills_with_options(RunxExportLoadOptions {
            root,
            refs: &[],
            source_roots: Vec::new(),
            execution_env: None,
        })
        .expect("export skills");

        assert_eq!(
            skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["demo", "runx"]
        );
        let runx = skills
            .iter()
            .find(|skill| skill.name == "runx")
            .expect("runx manual");
        assert_eq!(runx.mode, RunxExportMode::NativeInstructions);
    }
}
