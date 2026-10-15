use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{JsonNumber, JsonObject, JsonValue};

use super::{
    MAX_COMMAND_ARG_BYTES, MAX_COMMAND_ARGS, MAX_ENV, MAX_ENV_VALUE_BYTES, MAX_TIMEOUT_MS,
    MIN_TIMEOUT_MS, OutputMode, PreparedCommand, TOOL, capability::CommandInput, invalid_input,
};
use crate::RuntimeError;
use crate::tool_catalogs::native::{NativeInvocation, resolve_repo_root_for};

pub(super) fn prepare(
    invocation: &NativeInvocation<'_, CommandInput>,
) -> Result<PreparedCommand, RuntimeError> {
    let repo_root = resolve_repo_root_for(
        TOOL,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let command = bounded_text(&invocation.inputs.command, "command", MAX_COMMAND_ARG_BYTES)?;
    let args = arguments(&invocation.inputs.args)?;
    let cwd = working_directory(&invocation.inputs.cwd, &repo_root)?;
    let cwd_relative = relative_path(&repo_root, &cwd)?;
    let explicit_env = explicit_env(&invocation.inputs.env)?;
    let timeout_ms = timeout_ms(invocation.inputs.timeout_ms)?;
    let output_mode = output_mode(&invocation.inputs.output_mode)?;
    let command_identity = command_identity(
        &command,
        &args,
        &cwd_relative,
        &explicit_env,
        timeout_ms,
        output_mode,
    );
    let command_digest = digest_json(&command_identity)?;
    validate_expected_digest(
        invocation.inputs.expected_command_digest.as_deref(),
        &command_digest,
    )?;
    Ok(PreparedCommand {
        command,
        args,
        repo_root,
        cwd,
        cwd_relative,
        explicit_env,
        timeout_ms,
        output_mode,
        command_digest,
    })
}

fn command_identity(
    command: &str,
    args: &[String],
    cwd_relative: &str,
    explicit_env: &BTreeMap<String, String>,
    timeout_ms: u64,
    output_mode: OutputMode,
) -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("command".to_owned(), JsonValue::String(command.to_owned())),
        (
            "args".to_owned(),
            JsonValue::Array(args.iter().cloned().map(JsonValue::String).collect()),
        ),
        ("cwd".to_owned(), JsonValue::String(cwd_relative.to_owned())),
        (
            "env".to_owned(),
            JsonValue::Object(
                explicit_env
                    .iter()
                    .map(|(name, value)| (name.clone(), JsonValue::String(value.clone())))
                    .collect(),
            ),
        ),
        (
            "timeout_ms".to_owned(),
            JsonValue::Number(JsonNumber::U64(timeout_ms)),
        ),
        (
            "output_mode".to_owned(),
            JsonValue::String(output_mode.as_str().to_owned()),
        ),
    ]))
}

fn validate_expected_digest(
    expected: Option<&str>,
    command_digest: &str,
) -> Result<(), RuntimeError> {
    if let Some(expected) = expected
        && expected != command_digest
    {
        return Err(invalid_input(
            TOOL,
            "expected_command_digest does not match the normalized command",
        ));
    }
    Ok(())
}

fn arguments(values: &[String]) -> Result<Vec<String>, RuntimeError> {
    if values.len() > MAX_COMMAND_ARGS {
        return Err(invalid_input(
            TOOL,
            format!("args must contain no more than {MAX_COMMAND_ARGS} entries"),
        ));
    }
    values
        .iter()
        .enumerate()
        .map(|(index, value)| bounded_text(value, &format!("args[{index}]"), MAX_COMMAND_ARG_BYTES))
        .collect()
}

fn explicit_env(
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    if values.len() > MAX_ENV {
        return Err(invalid_input(
            TOOL,
            format!("env must contain no more than {MAX_ENV} entries"),
        ));
    }
    values
        .iter()
        .map(|(name, value)| {
            if !valid_env_name(name) {
                return Err(invalid_input(TOOL, format!("invalid env name {name:?}")));
            }
            Ok((
                name.clone(),
                bounded_text(value, &format!("env.{name}"), MAX_ENV_VALUE_BYTES)?,
            ))
        })
        .collect()
}

fn working_directory(requested: &str, root: &Path) -> Result<PathBuf, RuntimeError> {
    let requested_path = Path::new(requested);
    if requested_path.is_absolute() {
        return Err(invalid_input(TOOL, "cwd must be relative to repo_root"));
    }
    let candidate = std::fs::canonicalize(root.join(requested_path))
        .map_err(|source| RuntimeError::io("resolving command cwd", source))?;
    if candidate != root && !candidate.starts_with(root) {
        return Err(invalid_input(TOOL, "cwd escapes repo_root"));
    }
    if !candidate.is_dir() {
        return Err(invalid_input(TOOL, "cwd must resolve to a directory"));
    }
    Ok(candidate)
}

fn timeout_ms(value: u64) -> Result<u64, RuntimeError> {
    if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&value) {
        return Err(invalid_input(
            TOOL,
            format!("timeout_ms must be {MIN_TIMEOUT_MS}-{MAX_TIMEOUT_MS}"),
        ));
    }
    Ok(value)
}

fn output_mode(value: &str) -> Result<OutputMode, RuntimeError> {
    match value {
        "digest" => Ok(OutputMode::Digest),
        "text" => Ok(OutputMode::Text),
        "json" => Ok(OutputMode::Json),
        _ => Err(invalid_input(
            TOOL,
            "output_mode must be digest, text, or json",
        )),
    }
}

fn bounded_text(value: &str, field: &str, max_bytes: usize) -> Result<String, RuntimeError> {
    if value.is_empty() {
        return Err(invalid_input(TOOL, format!("{field} must not be empty")));
    }
    if value.len() > max_bytes {
        return Err(invalid_input(
            TOOL,
            format!("{field} exceeds {max_bytes} bytes"),
        ));
    }
    if value.contains('\0') {
        return Err(invalid_input(TOOL, format!("{field} contains NUL")));
    }
    Ok(value.to_owned())
}

fn relative_path(root: &Path, path: &Path) -> Result<String, RuntimeError> {
    path.strip_prefix(root)
        .map(|value| {
            let value = value
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            if value.is_empty() {
                ".".to_owned()
            } else {
                value
            }
        })
        .map_err(|_| invalid_input(TOOL, "cwd escapes repo_root"))
}

fn valid_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some('_' | 'A'..='Z'))
        && chars.all(|character| matches!(character, '_' | 'A'..='Z' | '0'..='9'))
}

fn digest_json(value: &JsonValue) -> Result<String, RuntimeError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|source| RuntimeError::json("serializing command identity", source))?;
    Ok(runx_contracts::sha256_prefixed(&bytes))
}
