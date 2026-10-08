use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use runx_contracts::JsonObject;

use crate::RuntimeError;
use crate::receipts::paths::{RUNX_CWD_ENV, RUNX_RECEIPT_DIR_ENV};
use crate::receipts::signing::{
    RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_KID_ENV,
};

use super::paths::{invocation_error, workspace_cwd};
use super::template::json_value_env;

const MAX_INLINE_INPUTS_BYTES: usize = 48 * 1024;
const MAX_INLINE_INPUT_VALUE_BYTES: usize = 8 * 1024;

pub(super) fn child_env(
    declared_environment: &BTreeMap<String, String>,
    base_env: &BTreeMap<String, String>,
    inputs: &JsonObject,
    cleanup_paths: &mut Vec<PathBuf>,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    let mut env = child_base_env(base_env)?;
    env.extend(declared_environment.clone());
    let serialized = serde_json::to_string(inputs)
        .map_err(|source| RuntimeError::json("serializing runtime inputs", source))?;
    let (inputs_path, cleanup_path) = write_inputs_file(base_env, &serialized)?;
    env.insert("RUNX_INPUTS_PATH".to_owned(), inputs_path);
    if serialized.len() <= MAX_INLINE_INPUTS_BYTES {
        env.insert("RUNX_INPUTS_JSON".to_owned(), serialized);
    }
    push_cleanup_path(cleanup_paths, cleanup_path.clone());
    let mut input_env_names = BTreeMap::new();
    for (index, (key, value)) in inputs.iter().enumerate() {
        let serialized = json_value_env(value)?;
        let env_name = input_env_name(key);
        let path_env_name = format!("{env_name}_PATH");
        register_input_env_name(&mut input_env_names, &env_name, key)?;
        register_input_env_name(&mut input_env_names, &path_env_name, key)?;
        reject_runtime_input_env_collision(&env, &env_name, key)?;
        reject_runtime_input_env_collision(&env, &path_env_name, key)?;
        let value_path = write_input_value_file(&cleanup_path, index, &serialized)?;
        env.insert(path_env_name, value_path);
        if serialized.len() <= MAX_INLINE_INPUT_VALUE_BYTES {
            env.insert(env_name, serialized);
        }
    }
    Ok(env)
}

pub(super) fn child_base_env(
    base_env: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    let mut env = allowed_base_env(base_env);
    env.insert(
        RUNX_CWD_ENV.to_owned(),
        workspace_root(base_env)?.to_string_lossy().into_owned(),
    );
    if let Some(receipt_dir) = base_env.get(RUNX_RECEIPT_DIR_ENV) {
        env.insert(RUNX_RECEIPT_DIR_ENV.to_owned(), receipt_dir.clone());
    }
    for key in [
        RUNX_RECEIPT_VERIFY_KID_ENV,
        RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV,
    ] {
        if let Some(value) = base_env.get(key) {
            env.insert(key.to_owned(), value.clone());
        }
    }
    Ok(env)
}

fn workspace_root(base_env: &BTreeMap<String, String>) -> Result<PathBuf, RuntimeError> {
    workspace_cwd(base_env)?.ok_or_else(|| {
        invocation_error(format!(
            "process environment requires {}",
            crate::receipts::paths::RUNX_CWD_ENV
        ))
    })
}

fn write_inputs_file(
    base_env: &BTreeMap<String, String>,
    serialized: &str,
) -> Result<(String, PathBuf), RuntimeError> {
    let dir = create_workspace_tmp(base_env, "cli-inputs", "creating inputs temp dir")?;
    let path = dir.join("inputs.json");
    let mut file = fs::File::create(&path)
        .map_err(|source| RuntimeError::io("creating inputs temp file", source))?;
    file.write_all(serialized.as_bytes())
        .map_err(|source| RuntimeError::io("writing inputs temp file", source))?;
    Ok((path.to_string_lossy().into_owned(), dir))
}

fn allowed_base_env(base_env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    crate::execution_environment::process_baseline_environment(base_env)
}

fn reject_runtime_input_env_collision(
    environment: &BTreeMap<String, String>,
    name: &str,
    input: &str,
) -> Result<(), RuntimeError> {
    if environment.contains_key(name) {
        return Err(invocation_error(format!(
            "input {input:?} runtime environment variable {name} collides with declared environment"
        )));
    }
    Ok(())
}

fn register_input_env_name<'a>(
    names: &mut BTreeMap<String, &'a str>,
    env_name: &str,
    input: &'a str,
) -> Result<(), RuntimeError> {
    if let Some(prior_key) = names.insert(env_name.to_owned(), input) {
        return Err(invocation_error(format!(
            "input keys {prior_key:?} and {input:?} collide on environment variable {env_name}"
        )));
    }
    Ok(())
}

fn write_input_value_file(
    directory: &std::path::Path,
    index: usize,
    serialized: &str,
) -> Result<String, RuntimeError> {
    let path = directory.join(format!("input-{index}.json"));
    let mut file = fs::File::create(&path)
        .map_err(|source| RuntimeError::io("creating input value file", source))?;
    file.write_all(serialized.as_bytes())
        .map_err(|source| RuntimeError::io("writing input value file", source))?;
    Ok(path.to_string_lossy().into_owned())
}

fn create_workspace_tmp(
    base_env: &BTreeMap<String, String>,
    label: &str,
    operation: &'static str,
) -> Result<PathBuf, RuntimeError> {
    let root = workspace_root(base_env)?.join(".runx").join("tmp");
    fs::create_dir_all(&root).map_err(|source| RuntimeError::io(operation, source))?;
    let directory = tempfile::Builder::new()
        .prefix(&format!("{label}-"))
        .tempdir_in(root)
        .map_err(|source| RuntimeError::io(operation, source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .map_err(|source| RuntimeError::io(operation, source))?;
    }
    Ok(directory.keep())
}

fn push_cleanup_path(cleanup_paths: &mut Vec<PathBuf>, cleanup_path: PathBuf) {
    if cleanup_paths
        .iter()
        .any(|existing| cleanup_path.starts_with(existing))
    {
        return;
    }
    cleanup_paths.push(cleanup_path);
}

fn input_env_name(key: &str) -> String {
    let mut suffix = String::new();
    let mut pending_separator = false;
    for ch in key.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_separator && !suffix.is_empty() {
                suffix.push('_');
            }
            suffix.push(ch.to_ascii_uppercase());
            pending_separator = false;
        } else {
            pending_separator = true;
        }
    }
    format!("RUNX_INPUT_{suffix}")
}
