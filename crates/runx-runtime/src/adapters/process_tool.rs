use std::time::{Duration, Instant};

use runx_contracts::ExecutionBoundaryKind;
use runx_parser::ProcessTool;

use crate::RuntimeError;
use crate::adapter::{InvocationOutput, InvocationStatus, SkillAdapter, SkillInvocation};
use crate::adapter_pipeline::AdapterProjection;
use crate::credentials::CredentialDelivery;
use crate::observations::Observations;
use crate::process::{
    CapturedOutput, ProcessOutcome, ProcessSpec, ProcessStdin, ProcessSupervisorError,
    STANDARD_PROCESS_OUTPUT_BYTES, run_process,
};
use crate::process_invocation::prepare_process_invocation;

const DEFAULT_TIMEOUT_SECONDS: u64 = 60;
#[cfg(test)]
static DEFAULT_TIMEOUT_OVERRIDE_SECONDS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessToolAdapter;

impl ProcessToolAdapter {
    pub(crate) fn invoke_with_output_limit(
        &self,
        request: SkillInvocation,
        output_limit_bytes: usize,
    ) -> Result<InvocationOutput, RuntimeError> {
        let tool = request.executable.process().cloned().ok_or_else(|| {
            RuntimeError::UnsupportedExecutable {
                kind: request.executable.kind().to_owned(),
            }
        })?;
        crate::execution_environment::enforce_process_execution_policy(
            &tool.command,
            &tool.args,
            &request.env,
        )?;
        let credential_delivery = request.credential_delivery.clone();
        let process = prepare_process_invocation(
            &tool,
            &request.skill_directory,
            &request.inputs,
            &request.env,
        )?;
        credential_delivery
            .ensure_environment_disjoint(&process.env)
            .map_err(|error| RuntimeError::InvalidProcessInvocation {
                message: error.to_string(),
            })?;
        let stdin = process_stdin(&tool, &request)?;
        let mut process = process.into_execution_plan();
        for (name, value) in credential_delivery.secret_env().iter() {
            process.env.insert(name.to_owned(), value.to_owned());
        }
        let started = Instant::now();
        let outcome = match run_process(
            ProcessSpec::new("process", process.command, output_limit_bytes)
                .args(process.args)
                .cwd(process.cwd)
                .env(process.env)
                .stdin(stdin)
                .timeout(Some(process_timeout(tool.timeout_seconds)))
                .cleanup_paths(process.cleanup_paths),
        ) {
            Ok(outcome) => outcome,
            Err(ProcessSupervisorError::Spawn { context, source }) => {
                return Err(RuntimeError::io(context, source));
            }
            // The process ran with its environment: a failed invocation of
            // it, observed where it ran.
            Err(error @ ProcessSupervisorError::Supervise { .. }) => {
                return Ok(AdapterProjection::from_started(started).failure(
                    error.to_string(),
                    Observations {
                        boundary: Some(process.boundary),
                        ..Observations::default()
                    },
                ));
            }
        };
        Ok(process_output(
            outcome,
            &credential_delivery,
            process.boundary,
            output_limit_bytes,
        ))
    }
}

impl SkillAdapter for ProcessToolAdapter {
    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        self.invoke_with_output_limit(request, STANDARD_PROCESS_OUTPUT_BYTES)
    }
}

fn process_timeout(timeout_seconds: Option<u64>) -> Duration {
    Duration::from_secs(timeout_seconds.unwrap_or_else(default_timeout_seconds))
}

fn default_timeout_seconds() -> u64 {
    #[cfg(test)]
    {
        let seconds = DEFAULT_TIMEOUT_OVERRIDE_SECONDS.load(std::sync::atomic::Ordering::SeqCst);
        if seconds > 0 {
            return seconds;
        }
    }
    DEFAULT_TIMEOUT_SECONDS
}

fn process_stdin(
    tool: &ProcessTool,
    request: &SkillInvocation,
) -> Result<Option<ProcessStdin>, RuntimeError> {
    if tool.input_mode != Some(runx_parser::InputMode::Stdin) {
        return Ok(None);
    }
    let bytes = serde_json::to_vec(&request.inputs)
        .map_err(|source| RuntimeError::json("serializing stdin inputs", source))?;
    Ok(Some(ProcessStdin::new(bytes, "writing process stdin")))
}

fn redacted_capture(
    output: CapturedOutput,
    credential_delivery: &CredentialDelivery,
    output_limit_bytes: usize,
) -> CapturedText {
    if output.truncated {
        return CapturedText {
            text: String::new(),
            truncated: true,
        };
    }
    CapturedText {
        text: credential_delivery.redact_bytes_to_string(output.bytes, output_limit_bytes),
        truncated: false,
    }
}

fn process_output(
    outcome: ProcessOutcome,
    credential_delivery: &CredentialDelivery,
    boundary: ExecutionBoundaryKind,
    output_limit_bytes: usize,
) -> InvocationOutput {
    let stdout = redacted_capture(outcome.stdout, credential_delivery, output_limit_bytes);
    let stderr = redacted_capture(outcome.stderr, credential_delivery, output_limit_bytes);
    let output_truncated = stdout.truncated || stderr.truncated;
    let success = outcome.status.success() && !outcome.timed_out && !output_truncated;
    let (stdout, stderr) = if output_truncated {
        (
            String::new(),
            format!(
                "runx process output exceeded {output_limit_bytes} byte capture limit; stdout/stderr omitted"
            ),
        )
    } else {
        (stdout.text, stderr.text)
    };
    AdapterProjection::from_duration_ms(outcome.duration_ms).process_output(
        if success {
            InvocationStatus::Success
        } else {
            InvocationStatus::Failure
        },
        stdout,
        stderr,
        outcome.status.code(),
        Observations {
            boundary: Some(boundary),
            ..Observations::default()
        },
    )
}

struct CapturedText {
    text: String,
    truncated: bool,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use runx_contracts::JsonObject;

    use super::*;
    use crate::credentials::CredentialDelivery;

    #[test]
    fn process_without_declared_timeout_uses_default_timeout() -> Result<(), RuntimeError> {
        let started = Instant::now();
        let workspace =
            tempfile::tempdir().map_err(|source| RuntimeError::io("creating workspace", source))?;
        DEFAULT_TIMEOUT_OVERRIDE_SECONDS.store(1, std::sync::atomic::Ordering::SeqCst);
        let output = ProcessToolAdapter.invoke(SkillInvocation {
            historical_context: Vec::new(),
            skill_name: "default-timeout".to_owned(),
            step_id: None,
            artifacts: None,
            requirements: Default::default(),
            executable: crate::adapter::Executable::Process(ProcessTool {
                command: "/bin/sh".to_owned(),
                args: vec!["-c".to_owned(), "sleep 10".to_owned()],
                cwd: None,
                timeout_seconds: None,
                input_mode: None,
                environment: Default::default(),
            }),
            outputs: None,
            inputs: JsonObject::new(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: workspace.path().to_path_buf(),
            // The workspace cwd policy requires an explicit workspace anchor.
            env: BTreeMap::from([(
                crate::paths::RUNX_CWD_ENV.to_owned(),
                workspace.path().to_string_lossy().into_owned(),
            )]),
            credential_delivery: CredentialDelivery::none(),
        })?;
        DEFAULT_TIMEOUT_OVERRIDE_SECONDS.store(0, std::sync::atomic::Ordering::SeqCst);

        assert_eq!(output.status, InvocationStatus::Failure);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a process without a declared timeout must not run unbounded"
        );
        Ok(())
    }
}
