use runx_contracts::{JsonNumber, JsonObject, JsonValue};

use super::capability::CommandExecutionOutput;
use super::{OUTPUT_LIMIT_BYTES, OutputMode, PreparedCommand, error, exit_code};
use crate::RuntimeError;
use crate::tool_catalogs::native::capability::decode_typed_output;
pub(super) struct CommandObservation {
    exit_code: Option<i32>,
    timed_out: bool,
    duration_ms: u64,
    stdout: String,
    stderr: String,
    stdout_digest: String,
    stderr_digest: String,
    stdout_bytes: u64,
    stderr_bytes: u64,
    stdout_truncated: bool,
    stderr_truncated: bool,
    parsed_json: JsonValue,
    errors: Vec<JsonValue>,
}

pub(super) fn observe_command(
    output_mode: OutputMode,
    outcome: crate::process::ProcessOutcome,
    credentials: &crate::credentials::CredentialDelivery,
) -> CommandObservation {
    let stdout_truncated = outcome.stdout.truncated;
    let stderr_truncated = outcome.stderr.truncated;
    let stdout_digest = outcome.stdout.sha256.clone();
    let stderr_digest = outcome.stderr.sha256.clone();
    let stdout_bytes = outcome.stdout.total_bytes;
    let stderr_bytes = outcome.stderr.total_bytes;
    let stdout = credentials.redact_bytes_to_string(outcome.stdout.bytes, OUTPUT_LIMIT_BYTES);
    let stderr = credentials.redact_bytes_to_string(outcome.stderr.bytes, OUTPUT_LIMIT_BYTES);
    let mut errors = Vec::new();
    let parsed_json = parse_command_json(output_mode, &stdout, stdout_truncated, &mut errors);
    if outcome.timed_out {
        errors.push(error("command.timeout", "command exceeded timeout_ms"));
    }
    if output_mode != OutputMode::Digest && (stdout_truncated || stderr_truncated) {
        errors.push(error(
            "command.output_truncated",
            "command output exceeded the runtime capture limit",
        ));
    }
    if !outcome.status.success() {
        errors.push(JsonValue::Object(JsonObject::from([
            (
                "code".to_owned(),
                JsonValue::String("command.exit".to_owned()),
            ),
            ("exit_code".to_owned(), exit_code(outcome.status.code())),
        ])));
    }
    CommandObservation {
        exit_code: outcome.status.code(),
        timed_out: outcome.timed_out,
        duration_ms: outcome.duration_ms,
        stdout_digest,
        stderr_digest,
        stdout_bytes,
        stderr_bytes,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        parsed_json,
        errors,
    }
}

fn parse_command_json(
    output_mode: OutputMode,
    stdout: &str,
    stdout_truncated: bool,
    errors: &mut Vec<JsonValue>,
) -> JsonValue {
    if output_mode != OutputMode::Json {
        return JsonValue::Null;
    }
    if stdout_truncated {
        errors.push(error(
            "command.output_truncated",
            "stdout exceeded the runtime capture limit",
        ));
        return JsonValue::Null;
    }
    match serde_json::from_str::<JsonValue>(stdout) {
        Ok(JsonValue::Object(value)) => JsonValue::Object(value),
        Ok(_) => {
            errors.push(error("command.output", "stdout must be one JSON object"));
            JsonValue::Null
        }
        Err(_) => {
            errors.push(error("command.output", "stdout was not valid JSON"));
            JsonValue::Null
        }
    }
}

// Function rationale: this declaratively projects one stable
// execution packet; process supervision and output admission happen upstream.
pub(super) fn render_execution(
    command: PreparedCommand,
    observation: CommandObservation,
) -> Result<CommandExecutionOutput, RuntimeError> {
    let completed = observation.errors.is_empty();
    let execution = JsonObject::from([
        (
            "schema".to_owned(),
            JsonValue::String("runx.command.execution.v1".to_owned()),
        ),
        (
            "decision".to_owned(),
            JsonValue::String(if completed { "completed" } else { "failed" }.to_owned()),
        ),
        (
            "command_digest".to_owned(),
            JsonValue::String(command.command_digest),
        ),
        ("cwd".to_owned(), JsonValue::String(command.cwd_relative)),
        ("exit_code".to_owned(), exit_code(observation.exit_code)),
        (
            "timed_out".to_owned(),
            JsonValue::Bool(observation.timed_out),
        ),
        (
            "duration_ms".to_owned(),
            JsonValue::Number(JsonNumber::U64(observation.duration_ms)),
        ),
        (
            "stdout".to_owned(),
            JsonValue::String(if command.output_mode == OutputMode::Text {
                observation.stdout.clone()
            } else {
                String::new()
            }),
        ),
        (
            "stderr".to_owned(),
            JsonValue::String(if command.output_mode == OutputMode::Text {
                observation.stderr.clone()
            } else {
                String::new()
            }),
        ),
        (
            "stdout_digest".to_owned(),
            JsonValue::String(observation.stdout_digest),
        ),
        (
            "stderr_digest".to_owned(),
            JsonValue::String(observation.stderr_digest),
        ),
        (
            "stdout_bytes".to_owned(),
            JsonValue::Number(JsonNumber::U64(observation.stdout_bytes)),
        ),
        (
            "stderr_bytes".to_owned(),
            JsonValue::Number(JsonNumber::U64(observation.stderr_bytes)),
        ),
        (
            "stdout_truncated".to_owned(),
            JsonValue::Bool(observation.stdout_truncated),
        ),
        (
            "stderr_truncated".to_owned(),
            JsonValue::Bool(observation.stderr_truncated),
        ),
        ("json".to_owned(), observation.parsed_json),
        ("errors".to_owned(), JsonValue::Array(observation.errors)),
    ]);
    decode_typed_output(
        "command.execute",
        JsonValue::Object(JsonObject::from([(
            "command_execution".to_owned(),
            JsonValue::Object(execution),
        )])),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    use super::*;
    use crate::process::{CapturedOutput, ProcessOutcome};

    #[test]
    fn digest_mode_reports_the_complete_stream_beyond_retained_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let complete = b"complete process output";
        let complete_bytes = u64::try_from(complete.len())?;
        let outcome = ProcessOutcome {
            status: ExitStatus::from_raw(0),
            timed_out: false,
            stdout: CapturedOutput {
                bytes: complete[..8].to_vec(),
                truncated: true,
                total_bytes: complete_bytes,
                sha256: runx_contracts::sha256_prefixed(complete),
            },
            stderr: CapturedOutput {
                bytes: Vec::new(),
                truncated: false,
                total_bytes: 0,
                sha256: runx_contracts::sha256_prefixed(&[]),
            },
            duration_ms: 1,
        };

        let observation = observe_command(
            OutputMode::Digest,
            outcome,
            &crate::credentials::CredentialDelivery::none(),
        );

        assert!(observation.errors.is_empty());
        assert!(observation.stdout_truncated);
        assert_eq!(observation.stdout_bytes, complete_bytes);
        assert_eq!(
            observation.stdout_digest,
            runx_contracts::sha256_prefixed(complete)
        );
        Ok(())
    }
}
