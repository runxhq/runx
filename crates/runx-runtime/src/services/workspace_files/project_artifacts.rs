//! Durable, digest-bound project artifacts shared by CLI and operator applications.

use std::fs;
use std::io::Read;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue, sha256_prefixed};

pub fn persist_json_artifact(
    artifacts: &Path,
    run_id: &str,
    kind: &str,
    label: &str,
    value: &JsonValue,
) -> Result<JsonValue, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    persist_json_bytes(artifacts, run_id, kind, label, bytes)
}

/// A project artifact whose bytes are `value`'s canonical JSON
/// (`runx.stable-json.v1`), so its digest is the value's stable digest.
pub(crate) fn persist_canonical_json_artifact(
    artifacts: &Path,
    run_id: &str,
    kind: &str,
    label: &str,
    value: &JsonValue,
) -> Result<JsonValue, String> {
    let bytes = runx_contracts::to_canonical_bytes(value).map_err(|error| error.to_string())?;
    persist_json_bytes(artifacts, run_id, kind, label, bytes)
}

pub(crate) fn persist_json_bytes(
    artifacts: &Path,
    run_id: &str,
    kind: &str,
    label: &str,
    bytes: Vec<u8>,
) -> Result<JsonValue, String> {
    let digest = sha256_prefixed(&bytes);
    let digest_label = digest.strip_prefix("sha256:").unwrap_or(&digest);
    let skill_runs = artifacts.join("skill-runs");
    let run_directory = skill_runs.join(safe_path_segment(run_id));
    let directory = run_directory.join(safe_path_segment(kind));
    fs::create_dir_all(&directory)
        .map_err(|error| format!("creating {}: {error}", directory.display()))?;
    // Protect the whole subtree before writing any result or temporary file.
    restrict_artifact_directory(artifacts)?;
    restrict_artifact_directory(&skill_runs)?;
    restrict_artifact_directory(&run_directory)?;
    restrict_artifact_directory(&directory)?;
    let path = directory.join(format!(
        "{}-{}.json",
        safe_path_segment(label),
        digest_label
    ));
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(format!(
                "artifact target {} is not a regular file",
                path.display()
            ));
        }
        Ok(metadata) => {
            if metadata.len() != bytes.len() as u64
                || read_artifact_bytes(&path, bytes.len() as u64)? != bytes
            {
                return Err("existing project artifact differs from its content address".to_owned());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("checking {}: {error}", path.display())),
    }
    if !path.exists() {
        crate::receipts::store::write_atomic(
            &directory,
            path.file_name()
                .and_then(|name| name.to_str())
                .ok_or("artifact filename is invalid")?,
            &bytes,
        )
        .map_err(|error| error.to_string())?;
    }
    restrict_artifact_file(&path)?;
    Ok(JsonValue::Object(JsonObject::from([
        (
            "schema".to_owned(),
            JsonValue::String("runx.project_artifact.v1".to_owned()),
        ),
        (
            "uri".to_owned(),
            JsonValue::String(format!("runx:project-artifact:{digest}")),
        ),
        ("digest".to_owned(), JsonValue::String(digest)),
        (
            "bytes".to_owned(),
            JsonValue::Number(runx_contracts::JsonNumber::U64(bytes.len() as u64)),
        ),
        (
            "media_type".to_owned(),
            JsonValue::String("application/json".to_owned()),
        ),
        (
            "path".to_owned(),
            JsonValue::String(path.to_string_lossy().into_owned()),
        ),
    ])))
}

/// Validate the selected descriptor and resolve its contained regular source.
/// Both JSON reads and snapshot opening use this owner before reading content.
pub(super) fn project_artifact_source(
    artifacts: &Path,
    artifact: &runx_contracts::ProjectArtifact,
) -> Result<super::WorkspaceFile, String> {
    if runx_contracts::Digest::new(artifact.digest.clone()).is_none()
        || artifact.uri != format!("runx:project-artifact:{}", artifact.digest)
        || artifact.media_type != "application/json"
        || artifact.bytes == 0
    {
        return Err("project artifact has an invalid content address or byte count".to_owned());
    }
    let root = artifacts
        .canonicalize()
        .map_err(|error| format!("resolving project artifact root: {error}"))?;
    let candidate = Path::new(&artifact.path);
    if !candidate.is_absolute()
        || candidate
            .components()
            .any(|component| component == std::path::Component::ParentDir)
        || !(candidate.starts_with(artifacts) || candidate.starts_with(&root))
    {
        return Err("project artifact escapes the project artifact root".to_owned());
    }
    let path = candidate
        .canonicalize()
        .map_err(|error| format!("resolving project artifact: {error}"))?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| "project artifact escapes the project artifact root".to_owned())?;
    if fs::symlink_metadata(&artifact.path)
        .map_err(|error| format!("checking project artifact: {error}"))?
        .file_type()
        .is_symlink()
    {
        return Err("project artifact source is a symlink".to_owned());
    }
    let source = super::WorkspaceFile::resolve(&root, relative, super::WorkspaceBounds::Operator)
        .map_err(|error| error.to_string())?;
    if source.bytes() != artifact.bytes {
        return Err("project artifact byte count differs from its descriptor".to_owned());
    }
    Ok(source)
}

pub fn read_json_artifact(artifacts: &Path, artifact: &JsonValue) -> Result<JsonValue, String> {
    let artifact = artifact
        .clone()
        .deserialize_into::<runx_contracts::ProjectArtifact>()
        .map_err(|_| "project artifact has an invalid descriptor".to_owned())?;
    let source = project_artifact_source(artifacts, &artifact)?;
    let mut bytes = Vec::new();
    let (length, digest) = source
        .copy_and_digest(&mut bytes, artifact.bytes)
        .map_err(|error| error.to_string())?;
    if length != artifact.bytes || digest.as_str() != artifact.digest {
        return Err("project artifact digest differs from the stored artifact".to_owned());
    }
    serde_json::from_slice(&bytes).map_err(|_| "project artifact is invalid JSON".to_owned())
}

fn read_artifact_bytes(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let file =
        fs::File::open(path).map_err(|error| format!("opening project artifact: {error}"))?;
    let mut bytes = Vec::new();
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("reading project artifact: {error}"))?;
    if bytes.len() as u64 > maximum {
        return Err("project artifact grew beyond its selected byte count".to_owned());
    }
    Ok(bytes)
}

#[cfg(unix)]
fn restrict_artifact_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("restricting {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn restrict_artifact_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn restrict_artifact_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("restricting {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn restrict_artifact_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn safe_path_segment(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if value.is_empty() {
        "item".to_owned()
    } else {
        value
    }
}
