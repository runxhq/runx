use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_parser::{
    CatalogVisibility, SkillInput, SkillPackageSource, SkillRunnerManifest, ValidatedSkill,
    ValidatedSkillPackage, validate_skill_package,
};

mod discovery;
mod resolve;

use discovery::{canonicalize, discover_skill_paths, display_path};
use resolve::resolve_skill_ref;

#[derive(Clone, Debug, PartialEq)]
pub struct RunxExportSkill {
    pub name: String,
    pub description: String,
    pub runners: Vec<RunxExportRunner>,
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
pub struct RunxExportRunner {
    /// `None` is the unnamed SKILL.md execution front used by packages without
    /// run.yaml. Named run.yaml runners are always explicit in generated commands.
    pub name: Option<String>,
    pub default: bool,
    pub execution_closure_digest: Option<String>,
    pub inputs: BTreeMap<String, SkillInput>,
    pub examples: Vec<runx_contracts::JsonObject>,
}

#[derive(Clone, Debug)]
pub struct RunxExportLoadOptions<'a> {
    pub root: &'a Path,
    pub refs: &'a [String],
    pub official_roots: Vec<PathBuf>,
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

pub fn load_export_skills(
    root: &Path,
    refs: &[String],
) -> Result<Vec<RunxExportSkill>, RunxExportLoadError> {
    load_export_skills_with_options(RunxExportLoadOptions {
        root,
        refs,
        official_roots: Vec::new(),
        execution_env: None,
    })
}

pub fn load_export_skills_with_options(
    options: RunxExportLoadOptions<'_>,
) -> Result<Vec<RunxExportSkill>, RunxExportLoadError> {
    let explicit = !options.refs.is_empty();
    let paths = if explicit {
        options
            .refs
            .iter()
            .map(|reference| resolve_skill_ref(options.root, reference, &options.official_roots))
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
                    "cannot export {} as an agent skill because its default runner has no sealed standalone invocation contract",
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
    manifest: Option<&SkillRunnerManifest>,
) -> bool {
    let Some(manifest) = manifest else {
        return true;
    };
    if manifest.catalog.as_ref().map(|catalog| catalog.visibility)
        == Some(CatalogVisibility::Internal)
    {
        return false;
    }
    let report = runx_parser::analyze_package_catalog_semantics(
        &package.skill.name,
        manifest,
        &package.harness_fixtures,
    );
    let has_copy_valid_example = report
        .default_runner
        .as_deref()
        .and_then(|name| manifest.runners.get(name))
        .is_some_and(|runner| {
            !crate::skill_package::effective_runner_examples(package, manifest, runner).is_empty()
        });
    report.diagnostics.is_empty()
        && report.readiness.evaluated
        && report.readiness.cold_selection
        && report.readiness.standalone_default
        && has_copy_valid_example
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
    manifest: Option<SkillRunnerManifest>,
    execution_env: Option<&BTreeMap<String, String>>,
) -> Result<RunxExportSkill, RunxExportLoadError> {
    let mode = export_mode(&package.skill, manifest.as_ref());
    let mut runners = export_runners(&package, manifest.as_ref());
    let skill = package.skill;
    if mode == RunxExportMode::Delegated {
        let loaded = crate::load_validated_skill_package(&directory)
            .map_err(|error| RunxExportLoadError::Parse(error.to_string()))?;
        let empty_env = BTreeMap::new();
        let execution_env = execution_env.unwrap_or(&empty_env);
        for runner in &mut runners {
            let Some(name) = runner.name.as_deref() else {
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
                    "cannot export {} runner {name:?} because its execution closure is not fully bound",
                    skill.name
                )));
            }
            runner.execution_closure_digest = Some(binding.digest);
        }
    }
    for runner in &runners {
        validate_export_skill_inputs(&runner.inputs)?;
    }
    Ok(RunxExportSkill {
        // The strict manual rule makes every name one safe path segment.
        name: skill.name,
        description: skill
            .description
            .unwrap_or_else(|| "Run this skill through runx governance.".to_owned()),
        runners,
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
) -> Result<(PathBuf, ValidatedSkillPackage, Option<SkillRunnerManifest>), RunxExportLoadError> {
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

fn export_mode(_skill: &ValidatedSkill, manifest: Option<&SkillRunnerManifest>) -> RunxExportMode {
    if manifest.is_none() {
        return RunxExportMode::NativeInstructions;
    }
    RunxExportMode::Delegated
}

fn export_runners(
    package: &ValidatedSkillPackage,
    manifest: Option<&SkillRunnerManifest>,
) -> Vec<RunxExportRunner> {
    let Some(manifest) = manifest else {
        return vec![RunxExportRunner {
            name: None,
            default: true,
            execution_closure_digest: None,
            inputs: std::collections::BTreeMap::new(),
            examples: Vec::new(),
        }];
    };
    let mut runners = manifest
        .runners
        .values()
        .filter(|runner| {
            runner.default
                || manifest.runners.len() == 1
                || crate::skill_package::runner_has_sealed_standalone_journey(
                    package, manifest, runner,
                )
        })
        .map(|runner| RunxExportRunner {
            name: Some(runner.name.clone()),
            default: runner.default || manifest.runners.len() == 1,
            execution_closure_digest: None,
            inputs: runner.inputs.clone(),
            examples: crate::skill_package::effective_runner_examples(package, manifest, runner),
        })
        .collect::<Vec<_>>();
    runners.sort_by(|left, right| {
        right
            .default
            .cmp(&left.default)
            .then_with(|| left.name.cmp(&right.name))
    });
    runners
}

fn validate_export_skill_inputs(
    inputs: &BTreeMap<String, runx_parser::SkillInput>,
) -> Result<(), RunxExportLoadError> {
    for name in inputs.keys() {
        if !is_export_input_name(name) || is_reserved_skill_flag(name) {
            return Err(RunxExportLoadError::InvalidArgs(format!(
                "skill input {name:?} cannot be exported because it is not a safe runx skill flag"
            )));
        }
    }
    Ok(())
}

fn is_export_input_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn is_reserved_skill_flag(name: &str) -> bool {
    matches!(
        name,
        "answers" | "credential" | "json" | "receipt_dir" | "run_id" | "secret_env"
    )
}

fn manifest_visibility(
    manifest: &Option<runx_parser::SkillRunnerManifest>,
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

    use super::{RunxExportMode, load_export_skills};

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

        let skills = load_export_skills(root, &[]).expect("export skills");

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
