use std::collections::BTreeMap;

use runx_contracts::{JsonNumber, JsonObject, JsonValue};

use super::{
    MAX_TIMEOUT_MS, MIN_TIMEOUT_MS, OutputMode, PreparedCommand, TOOL, capability::CommandInput,
    invalid_input,
};
use crate::RuntimeError;
use crate::tool_catalogs::native::NativeInvocation;
use crate::tool_catalogs::native::argv::{self, Argv, ArgvRequest};

pub(super) fn prepare(
    invocation: &NativeInvocation<'_, CommandInput>,
) -> Result<PreparedCommand, RuntimeError> {
    let inputs = invocation.inputs;
    let Argv {
        repo_root,
        command,
        args,
        cwd,
        cwd_relative,
        explicit_env,
    } = argv::admit(
        TOOL,
        invocation,
        ArgvRequest {
            repo_root: &inputs.repo_root,
            command: &inputs.command,
            args: &inputs.args,
            cwd: &inputs.cwd,
            env: &inputs.env,
        },
    )?;
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

fn digest_json(value: &JsonValue) -> Result<String, RuntimeError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|source| RuntimeError::json("serializing command identity", source))?;
    Ok(runx_contracts::sha256_prefixed(&bytes))
}
