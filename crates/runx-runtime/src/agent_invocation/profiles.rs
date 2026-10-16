use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use runx_contracts::{ProfileFile, sha256_prefixed};

use crate::{RuntimeError, SkillInvocation};

const RUNX_VOICE_PROFILE_PATH_ENV: &str = "RUNX_VOICE_PROFILE_PATH";
const MAX_PROFILE_BYTES: usize = 128 * 1024;
pub(super) const BUNDLED_VOICE_PROFILE_CONTENT: &str = include_str!("../../assets/VOICE.md");

pub(super) fn resolve_voice_profile(
    request: &SkillInvocation,
) -> Result<ProfileFile, RuntimeError> {
    resolve_profile(
        request,
        RUNX_VOICE_PROFILE_PATH_ENV,
        "VOICE.md",
        BUNDLED_VOICE_PROFILE_CONTENT,
    )
}

pub(super) fn resolve_context_profiles(
    request: &SkillInvocation,
) -> Result<Option<runx_contracts::AgentContextProfiles>, RuntimeError> {
    let optional = |env_key: &str, file_name: &str| -> Result<Option<ProfileFile>, RuntimeError> {
        let path = configured_profile_path(request, env_key).or_else(|| {
            profile_candidates(request, file_name)
                .into_iter()
                .find(|path| path.is_file())
        });
        path.as_deref()
            .map(|path| read_profile_file(request, path))
            .transpose()
    };
    let memory = optional("RUNX_MEMORY_PROFILE_PATH", "MEMORY.md")?;
    let conventions = optional("RUNX_CONVENTIONS_PROFILE_PATH", "CONVENTIONS.md")?;
    let instructions = project_instructions(request)?;
    Ok(
        (memory.is_some() || conventions.is_some() || !instructions.is_empty()).then_some(
            runx_contracts::AgentContextProfiles {
                memory,
                conventions,
                instructions,
            },
        ),
    )
}

/// The files of a project's instructions, in the order each directory reads
/// them.
const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md"];

/// The project's instructions (agent-sessions §3): each `AGENTS.md` and
/// `CLAUDE.md` from the repository root that contains the workspace down to
/// the workspace, in that order. These are the only files runx reads above
/// the workspace. Each is bounded, and one that is a link leaving its
/// directory is refused.
fn project_instructions(request: &SkillInvocation) -> Result<Vec<ProfileFile>, RuntimeError> {
    let paths = crate::RunxPaths::resolve(&request.env, &request.skill_directory);
    let workspace =
        fs::canonicalize(paths.workspace()).unwrap_or_else(|_| paths.workspace().to_path_buf());
    let root = workspace
        .ancestors()
        .find(|directory| directory.join(".git").exists())
        .unwrap_or(&workspace)
        .to_path_buf();
    let mut directories = workspace
        .ancestors()
        .take_while(|directory| directory.starts_with(&root))
        .collect::<Vec<_>>();
    directories.reverse();
    let mut instructions = Vec::new();
    for directory in directories {
        for name in INSTRUCTION_FILES {
            let path = directory.join(name);
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                let target = fs::canonicalize(&path).map_err(|error| {
                    RuntimeError::io(format!("resolving {}", path.display()), error)
                })?;
                if target.parent() != Some(directory) {
                    return Err(RuntimeError::SkillFailed {
                        skill_name: "agent".to_owned(),
                        message: format!(
                            "{} links outside its directory, so it is not read as the project's instructions",
                            path.display()
                        ),
                    });
                }
            }
            if !path.is_file() {
                continue;
            }
            let mut profile = read_profile_file(request, &path)?;
            profile.root_path = root.to_string_lossy().into_owned().into();
            profile.path = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned()
                .into();
            instructions.push(profile);
        }
    }
    Ok(instructions)
}

fn resolve_profile(
    request: &SkillInvocation,
    env_key: &str,
    file_name: &str,
    bundled_content: &str,
) -> Result<ProfileFile, RuntimeError> {
    if let Some(path) = configured_profile_path(request, env_key) {
        return read_profile_file(request, &path);
    }
    for candidate in profile_candidates(request, file_name) {
        if candidate.is_file() {
            return read_profile_file(request, &candidate);
        }
    }
    Ok(bundled_profile(file_name, bundled_content))
}

fn configured_profile_path(request: &SkillInvocation, env_key: &str) -> Option<PathBuf> {
    let configured = PathBuf::from(request.env.get(env_key)?.trim());
    if configured.as_os_str().is_empty() {
        return None;
    }
    Some(crate::RunxPaths::resolve(&request.env, &request.skill_directory).in_workspace(configured))
}

fn profile_candidates(request: &SkillInvocation, file_name: &str) -> Vec<PathBuf> {
    let paths = crate::RunxPaths::resolve(&request.env, &request.skill_directory);
    let workspace = paths.workspace();
    let mut candidates = vec![
        paths.project_file(file_name),
        workspace.join(file_name),
        workspace.join("skills").join(file_name),
        request.skill_directory.join(file_name),
    ];
    candidates.extend(
        request
            .skill_directory
            .ancestors()
            .filter(|path| path.file_name().and_then(|name| name.to_str()) == Some("skills"))
            .map(|path| path.join(file_name)),
    );
    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

fn read_profile_file(request: &SkillInvocation, path: &Path) -> Result<ProfileFile, RuntimeError> {
    let file = fs::File::open(path)
        .map_err(|error| RuntimeError::io(format!("reading {}", path.display()), error))?;
    let mut content = String::new();
    file.take(MAX_PROFILE_BYTES as u64 + 1)
        .read_to_string(&mut content)
        .map_err(|error| RuntimeError::io("reading context profile", error))?;
    if content.len() > MAX_PROFILE_BYTES {
        return Err(RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: "context profile exceeds 128 KiB".to_owned(),
        });
    }
    let sharing = match request.env.get("RUNX_PROFILE_SHARING").map(String::as_str) {
        None | Some("local_only") => runx_contracts::ContextSharing::LocalOnly,
        Some("configured_model") => runx_contracts::ContextSharing::ConfiguredModel,
        _ => {
            return Err(RuntimeError::SkillFailed {
                skill_name: "agent".to_owned(),
                message: "invalid context profile sharing policy".to_owned(),
            });
        }
    };
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root = canonical.parent().unwrap_or_else(|| Path::new("."));
    let relative = canonical
        .strip_prefix(root)
        .unwrap_or(&canonical)
        .to_string_lossy()
        .into_owned();
    Ok(ProfileFile {
        sharing,
        root_path: root.to_string_lossy().into_owned().into(),
        path: relative.into(),
        sha256: sha256_prefixed(content.as_bytes()).into(),
        content,
    })
}

pub(super) fn bundled_profile(file_name: &str, content: &str) -> ProfileFile {
    ProfileFile {
        sharing: runx_contracts::ContextSharing::ConfiguredModel,
        root_path: "runx://profiles".into(),
        path: file_name.to_owned().into(),
        sha256: sha256_prefixed(content.as_bytes()).into(),
        content: content.to_owned(),
    }
}
