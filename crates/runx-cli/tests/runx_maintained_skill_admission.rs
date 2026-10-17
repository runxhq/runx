use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_runtime::load_validated_skill_package;
use runx_runtime::skill_front::{SkillOperatorContextOptions, load_skill_operator_context_chain};

#[test]
fn every_bundled_core_runner_prepares_through_the_cli_runtime()
-> Result<(), Box<dyn std::error::Error>> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let skills_root = repo_root.join("skills");
    let bundle: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        repo_root.join("crates/runx-runtime/bundled-skills.json"),
    )?)?;
    let skills = bundle["skills"].as_array().ok_or("bundle omitted skills")?;
    let effects = runx_cli::runtime::runtime_effect_registry()?;
    let mut failures = Vec::new();
    for entry in skills {
        let skill_id = entry["skill_id"]
            .as_str()
            .ok_or("bundle skill omitted id")?;
        let name = skill_id
            .strip_prefix("runx/")
            .ok_or("bundle skill has foreign owner")?;
        let directory = skills_root.join(name);
        let loaded = match load_validated_skill_package(&directory) {
            Ok(loaded) => loaded,
            Err(error) => {
                failures.push(format!("{}: {error}", package_name(&directory)));
                continue;
            }
        };
        let Some(manifest) = loaded.manifest() else {
            continue;
        };
        let env = operator_env(&repo_root, &directory)?;
        for runner_name in manifest.runners.keys() {
            if let Err(error) = load_skill_operator_context_chain(
                &directory,
                Some(runner_name),
                SkillOperatorContextOptions::new(env.clone(), repo_root.clone())
                    .with_effects(effects.clone()),
            ) {
                failures.push(format!(
                    "{}#{runner_name}: {error}",
                    package_name(&directory)
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "every bundled core runner must prepare through the complete CLI runtime:\n{}",
        failures.join("\n")
    );
    Ok(())
}

fn operator_env(
    repo_root: &Path,
    skill_dir: &Path,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let roots = [repo_root.join("tools"), skill_dir.join("tools")]
        .into_iter()
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    let mut env = BTreeMap::new();
    if !roots.is_empty() {
        env.insert(
            "RUNX_TOOL_ROOTS".to_owned(),
            std::env::join_paths(roots)?.to_string_lossy().into_owned(),
        );
    }
    Ok(env)
}

fn package_name(directory: &Path) -> String {
    directory
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| directory.display().to_string())
}
