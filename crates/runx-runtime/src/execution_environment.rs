//! One resolver for manifest-declared environment requirements.
//!
//! The runtime keeps values out of manifests, inspection, receipts, and agent
//! context. Executable adapters receive only values whose exact names were
//! declared by the selected act.

use std::collections::BTreeMap;

use runx_contracts::{
    EnvironmentRequirementStatus, EnvironmentRequirements, ExecutionRequirements,
};

use crate::RuntimeError;

pub(crate) const RUNX_HOSTED_WORKSPACE_POLICY_JSON_ENV: &str = "RUNX_HOSTED_WORKSPACE_POLICY_JSON";

pub(crate) const PROCESS_BASELINE_ENV: [&str; 25] = [
    "PATH",
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SystemRoot",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LOGNAME",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    "SSL_CERT_DIR",
    "SSL_CERT_FILE",
    "TERM",
    "TZ",
    "USER",
    "COLORTERM",
];

pub(crate) fn process_baseline_environment(
    environment: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    PROCESS_BASELINE_ENV
        .iter()
        .filter_map(|name| {
            environment
                .get(*name)
                .cloned()
                .map(|value| ((*name).to_owned(), value))
        })
        .collect()
}

pub(crate) fn resolve_declared_environment(
    requirements: &ExecutionRequirements,
    environment: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    resolve_environment(&requirements.environment, environment)
}

pub(crate) fn resolve_environment(
    requirements: &EnvironmentRequirements,
    environment: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    let missing = requirements
        .required
        .iter()
        .filter(|name| !environment.contains_key(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(RuntimeError::MissingEnvironment { names: missing });
    }
    Ok(requirements
        .names()
        .filter_map(|name| {
            environment
                .get(name)
                .cloned()
                .map(|value| (name.to_owned(), value))
        })
        .collect())
}

/// The host's workspace policy, when it sets one.
fn workspace_policy(
    environment: &BTreeMap<String, String>,
) -> Result<Option<runx_core::policy::LocalExecutionPolicy>, RuntimeError> {
    environment
        .get(RUNX_HOSTED_WORKSPACE_POLICY_JSON_ENV)
        .map(|raw| {
            serde_json::from_str::<runx_core::policy::LocalExecutionPolicy>(raw).map_err(|source| {
                RuntimeError::InvalidProcessInvocation {
                    message: format!("hosted workspace policy is invalid: {source}"),
                }
            })
        })
        .transpose()
}

/// Whether the host lets this run start `what`, a trusted host process.
pub(crate) fn admit_host_process(
    environment: &BTreeMap<String, String>,
    what: &str,
) -> Result<(), RuntimeError> {
    refuse_host_process(workspace_policy(environment)?.as_ref(), what)
}

fn refuse_host_process(
    policy: Option<&runx_core::policy::LocalExecutionPolicy>,
    what: &str,
) -> Result<(), RuntimeError> {
    if policy.is_some_and(|policy| policy.refuse_host_processes) {
        return Err(RuntimeError::InvalidProcessInvocation {
            message: format!("this host starts no trusted host process; {what} is refused"),
        });
    }
    Ok(())
}

pub(crate) fn enforce_process_execution_policy(
    command: &str,
    args: &[String],
    environment: &BTreeMap<String, String>,
) -> Result<(), RuntimeError> {
    let policy = workspace_policy(environment)?;
    refuse_host_process(policy.as_ref(), &format!("process {command}"))?;
    if let Some(message) = runx_core::policy::strict_process_tool_inline_code_denial(
        Some(command),
        args,
        policy.as_ref(),
    ) {
        return Err(RuntimeError::InvalidProcessInvocation { message });
    }
    Ok(())
}

/// Project environment-name availability without exposing values.
#[must_use]
pub fn environment_requirement_statuses(
    requirements: &EnvironmentRequirements,
    environment: &BTreeMap<String, String>,
) -> Vec<EnvironmentRequirementStatus> {
    requirements
        .required
        .iter()
        .map(|name| EnvironmentRequirementStatus {
            name: name.clone(),
            required: true,
            available: environment.contains_key(name),
        })
        .chain(
            requirements
                .optional
                .iter()
                .map(|name| EnvironmentRequirementStatus {
                    name: name.clone(),
                    required: false,
                    available: environment.contains_key(name),
                }),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_workspace_policy_rejects_inline_cli_code() -> Result<(), Box<dyn std::error::Error>> {
        let environment = BTreeMap::from([(
            RUNX_HOSTED_WORKSPACE_POLICY_JSON_ENV.to_owned(),
            r#"{"strictProcessToolInlineCode":true}"#.to_owned(),
        )]);
        let error = enforce_process_execution_policy(
            "node",
            &["--eval".to_owned(), "process.stdout.write('no')".to_owned()],
            &environment,
        )
        .err()
        .ok_or("strict hosted policy did not reject inline code")?;
        assert!(
            error
                .to_string()
                .contains("rejected by strict workspace policy")
        );
        Ok(())
    }

    #[test]
    fn a_host_that_refuses_host_processes_starts_none() -> Result<(), Box<dyn std::error::Error>> {
        let environment = BTreeMap::from([(
            RUNX_HOSTED_WORKSPACE_POLICY_JSON_ENV.to_owned(),
            r#"{"refuseHostProcesses":true}"#.to_owned(),
        )]);
        let error = enforce_process_execution_policy("sh", &["./run.sh".to_owned()], &environment)
            .err()
            .ok_or("a refusing host started a process")?;
        assert!(error.to_string().contains("starts no trusted host process"));
        assert!(admit_host_process(&BTreeMap::new(), "stdio MCP server node").is_ok());
        Ok(())
    }

    #[test]
    fn hosted_workspace_policy_is_typed_and_fail_closed() -> Result<(), Box<dyn std::error::Error>>
    {
        let environment = BTreeMap::from([(
            RUNX_HOSTED_WORKSPACE_POLICY_JSON_ENV.to_owned(),
            r#"{"strictProcessToolInlineCode":true,"unknown":true}"#.to_owned(),
        )]);
        let error = enforce_process_execution_policy("node", &[], &environment)
            .err()
            .ok_or("unknown hosted policy fields did not fail closed")?;
        assert!(
            error
                .to_string()
                .contains("hosted workspace policy is invalid")
        );
        Ok(())
    }
}
