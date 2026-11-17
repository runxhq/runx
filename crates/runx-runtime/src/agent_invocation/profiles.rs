use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use runx_contracts::{ProfileFile, sha256_prefixed};

use crate::{RuntimeError, SkillInvocation};

const RUNX_VOICE_PROFILE_PATH_ENV: &str = "RUNX_VOICE_PROFILE_PATH";
const MAX_PROFILE_BYTES: usize = 128 * 1024;

pub(super) fn resolve_voice_profile(
    request: &SkillInvocation,
) -> Result<Option<ProfileFile>, RuntimeError> {
    let paths = crate::RunxPaths::resolve(&request.env, &request.skill_directory);
    let selected = configured_profile_path(request, RUNX_VOICE_PROFILE_PATH_ENV).or_else(|| {
        [
            paths.project_file("VOICE.md"),
            paths.workspace().join("VOICE.md"),
        ]
        .into_iter()
        .find(|path| path.is_file())
    });
    selected
        .map(|path| read_profile_file(request, &path))
        .transpose()
}

/// The project's conventions and instructions an act carries. Memory is its
/// host's (`core-shape.md` §8.1): runx reads no `MEMORY.md` into an act, and
/// memory a host supplies travels as admitted context, read by its ref.
pub(super) fn resolve_context_profiles(
    request: &SkillInvocation,
) -> Result<Option<runx_contracts::AgentContextProfiles>, RuntimeError> {
    let conventions = configured_profile_path(request, "RUNX_CONVENTIONS_PROFILE_PATH")
        .or_else(|| {
            profile_candidates(request, "CONVENTIONS.md")
                .into_iter()
                .find(|path| path.is_file())
        })
        .map(|path| read_profile_file(request, &path))
        .transpose()?;
    let instructions = project_instructions(request)?;
    Ok(
        (conventions.is_some() || !instructions.is_empty()).then_some(
            runx_contracts::AgentContextProfiles {
                memory: None,
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
/// the workspace. Each is bounded, one that is a link leaving its directory is
/// refused, and a file two names reach, such as a `CLAUDE.md` linked to its
/// `AGENTS.md`, is read once.
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
    let mut read = BTreeSet::new();
    let mut instructions = Vec::new();
    for directory in directories {
        for name in INSTRUCTION_FILES {
            let path = directory.join(name);
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                let target = fs::canonicalize(&path).map_err(|error| {
                    RuntimeError::io(format!("resolving {}", named(request, &path)), error)
                })?;
                if target.parent() != Some(directory) {
                    return Err(RuntimeError::failed(
                        "the context profile",
                        format!(
                            "{} links outside its directory, so it is not read as the project's instructions",
                            named(request, &path)
                        ),
                    ));
                }
            }
            if !path.is_file() || !read.insert(fs::canonicalize(&path).unwrap_or(path.clone())) {
                continue;
            }
            instructions.push(read_profile_file(request, &path)?);
        }
    }
    Ok(instructions)
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

/// A project file's path as every party names it (rule 2): from the
/// workspace root, `$package/<path>` in the skill's own package outside it,
/// and `../` for a directory above it, where a repository's instructions are.
fn named(request: &SkillInvocation, path: &Path) -> String {
    let paths = crate::RunxPaths::resolve(&request.env, &request.skill_directory);
    let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // The file by its own name, a link as itself, in its canonical directory.
    let path = match (path.parent(), path.file_name()) {
        (Some(directory), Some(name)) => canonical(directory).join(name),
        _ => path.to_path_buf(),
    };
    let workspace = canonical(paths.workspace());
    let slashed = |relative: &Path| relative.to_string_lossy().replace('\\', "/");
    if let Ok(relative) = path.strip_prefix(&workspace) {
        return slashed(relative);
    }
    if let Ok(relative) = path.strip_prefix(canonical(&request.skill_directory)) {
        return format!("{}/{}", crate::services::PACKAGE_BASE, slashed(relative));
    }
    let common = workspace
        .ancestors()
        .find(|ancestor| path.starts_with(ancestor))
        .unwrap_or(Path::new("/"));
    let up = workspace.components().count() - common.components().count();
    format!(
        "{}{}",
        "../".repeat(up),
        slashed(path.strip_prefix(common).unwrap_or(&path))
    )
}

/// Who a project file may be sent to. The project's files instruct whatever
/// agent works in it, so they reach the configured model, as `CLAUDE.md`
/// reaches Claude Code's; `RUNX_PROFILE_SHARING=local_only` keeps them on this
/// machine.
fn profile_sharing(
    request: &SkillInvocation,
) -> Result<runx_contracts::ContextSharing, RuntimeError> {
    match request.env.get("RUNX_PROFILE_SHARING").map(String::as_str) {
        None | Some("configured_model") => Ok(runx_contracts::ContextSharing::ConfiguredModel),
        Some("local_only") => Ok(runx_contracts::ContextSharing::LocalOnly),
        Some(other) => Err(RuntimeError::Failed(Box::new(
            crate::Failure::new(
                "the context profile",
                format!("RUNX_PROFILE_SHARING is {other}, which names no sharing"),
            )
            .next("set it to configured_model or local_only, or leave it unset"),
        ))),
    }
}

fn read_profile_file(request: &SkillInvocation, path: &Path) -> Result<ProfileFile, RuntimeError> {
    let name = named(request, path);
    let file =
        fs::File::open(path).map_err(|error| RuntimeError::io(format!("reading {name}"), error))?;
    let mut content = String::new();
    file.take(MAX_PROFILE_BYTES as u64 + 1)
        .read_to_string(&mut content)
        .map_err(|error| RuntimeError::io(format!("reading {name}"), error))?;
    if content.len() > MAX_PROFILE_BYTES {
        return Err(RuntimeError::failed(
            "the context profile",
            format!("{name} is past 128 KiB, the most a turn is given of one file"),
        ));
    }
    Ok(ProfileFile {
        sharing: profile_sharing(request)?,
        path: name.into(),
        digest: sha256_prefixed(content.as_bytes()).into(),
        content,
    })
}
