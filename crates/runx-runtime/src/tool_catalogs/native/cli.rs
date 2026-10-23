//! Native bounded CLI documentation capture.

use std::path::Path;
use std::time::Duration;

use runx_contracts::schema::{BoundedVec, NonEmptyString};
use runx_contracts::{CLI_HELP_MAX_ARGS, CliHelp, CliHelpFlag};

use super::{NativeInvocation, invalid_input, resolve_repo_root_for};
use crate::{CapabilityOutput, RuntimeError};

mod capability;

use crate::process::{ProcessOutcome, ProcessSpec, run_process};
use crate::process_invocation::process_base_environment;
pub(super) use capability::CAPABILITIES;
use capability::CaptureHelpInput;

const TOOL: &str = "cli.capture_help";
const OUTPUT_LIMIT_BYTES: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CLI_RUN_ARG_BYTES: usize = 4 * 1024;

impl CapabilityOutput for CliHelp {}

fn capture_help(
    invocation: &NativeInvocation<'_, CaptureHelpInput>,
) -> Result<CliHelp, RuntimeError> {
    let root = resolve_repo_root_for(
        TOOL,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let cwd = working_directory(&invocation.inputs.cwd, &root)?;
    let command = bounded_text(&invocation.inputs.command, "command")?;
    let args = arguments(&invocation.inputs.args)?;
    let help_flag = invocation.inputs.help_flag;
    let argv = args
        .iter()
        .map(|arg| arg.as_str().to_owned())
        .chain([help_flag.as_str().to_owned()])
        .collect();
    let outcome = run_help(invocation, &cwd, &command, argv)?;
    Ok(help_output(
        invocation, command, args, help_flag, cwd, outcome,
    ))
}

fn run_help(
    invocation: &NativeInvocation<'_, CaptureHelpInput>,
    cwd: &Path,
    command: &NonEmptyString,
    argv: Vec<String>,
) -> Result<ProcessOutcome, RuntimeError> {
    let outcome = run_process(
        ProcessSpec::new(
            "native CLI help capture",
            command.as_str().to_owned(),
            OUTPUT_LIMIT_BYTES,
        )
        .args(argv)
        .cwd(cwd)
        .env(process_base_environment(invocation.env)?)
        .timeout(Some(TIMEOUT)),
    )
    .map_err(|error| invalid_input(TOOL, error.to_string()))?;
    if outcome.timed_out || outcome.stdout.truncated || outcome.stderr.truncated {
        return Err(invalid_input(
            TOOL,
            "CLI help capture exceeded runtime bounds",
        ));
    }
    Ok(outcome)
}

fn help_output(
    invocation: &NativeInvocation<'_, CaptureHelpInput>,
    command: NonEmptyString,
    args: BoundedVec<NonEmptyString, 0, CLI_HELP_MAX_ARGS>,
    help_flag: CliHelpFlag,
    cwd: std::path::PathBuf,
    outcome: ProcessOutcome,
) -> CliHelp {
    let stdout = invocation
        .credential_delivery
        .redact_bytes_to_string(outcome.stdout.bytes, OUTPUT_LIMIT_BYTES);
    let stderr = invocation
        .credential_delivery
        .redact_bytes_to_string(outcome.stderr.bytes, OUTPUT_LIMIT_BYTES);
    CliHelp {
        command,
        args,
        help_flag,
        cwd: cwd.to_string_lossy().into_owned().into(),
        stdout,
        stderr,
        exit_code: outcome.status.code().map(i64::from),
    }
}

fn arguments(
    values: &[String],
) -> Result<BoundedVec<NonEmptyString, 0, CLI_HELP_MAX_ARGS>, RuntimeError> {
    let args = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            if value.starts_with('-') {
                return Err(invalid_input(
                    TOOL,
                    format!("args[{index}] must be a subcommand or positional name, not a flag"),
                ));
            }
            bounded_text(value, &format!("args[{index}]"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    BoundedVec::new(args).ok_or_else(|| {
        invalid_input(
            TOOL,
            format!("args must contain no more than {CLI_HELP_MAX_ARGS} entries"),
        )
    })
}

fn working_directory(requested: &str, root: &Path) -> Result<std::path::PathBuf, RuntimeError> {
    let requested = Path::new(requested);
    if requested.is_absolute() {
        return Err(invalid_input(TOOL, "cwd must be relative to repo_root"));
    }
    let cwd = std::fs::canonicalize(root.join(requested))
        .map_err(|source| RuntimeError::io("resolving CLI help cwd", source))?;
    if !cwd.is_dir() || (cwd != root && !cwd.starts_with(root)) {
        return Err(invalid_input(
            TOOL,
            "cwd must resolve to a directory inside repo_root",
        ));
    }
    Ok(cwd)
}

fn bounded_text(value: &str, field: &str) -> Result<NonEmptyString, RuntimeError> {
    if value.len() > MAX_CLI_RUN_ARG_BYTES || value.contains('\0') {
        return Err(bounded_text_error(field));
    }
    NonEmptyString::new(value).ok_or_else(|| bounded_text_error(field))
}

fn bounded_text_error(field: &str) -> RuntimeError {
    invalid_input(
        TOOL,
        format!("{field} must contain 1-{MAX_CLI_RUN_ARG_BYTES} non-NUL bytes"),
    )
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::collections::BTreeMap;

    use runx_contracts::{JsonObject, JsonValue};

    use super::{CaptureHelpInput, capture_help};
    use crate::RuntimeEffectRegistry;
    use crate::credentials::CredentialDelivery;
    use crate::paths::RUNX_CWD_ENV;
    use crate::tool_catalogs::native::{NativeInvocation, fixture_input};

    #[test]
    fn captures_help_without_a_package_wrapper() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let env = BTreeMap::from([(
            RUNX_CWD_ENV.to_owned(),
            workspace.path().to_string_lossy().into_owned(),
        )]);
        let inputs = fixture_input::<CaptureHelpInput>(JsonObject::from([(
            "command".to_owned(),
            JsonValue::String("git".to_owned()),
        )]))?;
        let delivery = CredentialDelivery::none();
        let effects = RuntimeEffectRegistry::default();
        let output = capture_help(&NativeInvocation {
            inputs: &inputs,
            observed_at: "2026-01-01T00:00:00Z",
            env: &env,
            skill_directory: workspace.path(),
            credential_delivery: &delivery,
            local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
            effects: &effects,
            bounds: crate::services::WorkspaceBounds::Operator,
            call: None,
        })?;
        assert_eq!(output.exit_code, Some(0));
        assert!(output.stdout.contains("usage:"));
        Ok(())
    }

    #[test]
    fn rejects_flags_before_the_help_flag() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let env = BTreeMap::from([(
            RUNX_CWD_ENV.to_owned(),
            workspace.path().to_string_lossy().into_owned(),
        )]);
        let inputs = fixture_input::<CaptureHelpInput>(JsonObject::from([
            ("command".to_owned(), JsonValue::String("git".to_owned())),
            (
                "args".to_owned(),
                JsonValue::Array(vec![JsonValue::String("--exec-path".to_owned())]),
            ),
        ]))?;
        let delivery = CredentialDelivery::none();
        let effects = RuntimeEffectRegistry::default();
        let error = capture_help(&NativeInvocation {
            inputs: &inputs,
            observed_at: "2026-01-01T00:00:00Z",
            env: &env,
            skill_directory: workspace.path(),
            credential_delivery: &delivery,
            local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
            effects: &effects,
            bounds: crate::services::WorkspaceBounds::Operator,
            call: None,
        })
        .expect_err("flags before the help flag must be rejected");
        assert!(error.to_string().contains("not a flag"));
        Ok(())
    }
}
