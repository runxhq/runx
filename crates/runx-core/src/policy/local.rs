use super::{
    AdmissionDecision, LocalAdmissionOptions, LocalAdmissionSkill, ScopeGrantPolicy,
    credential_grant::{credential_grant_requirement, find_matching_grant},
    interpreter::strict_cli_tool_inline_code_denial,
};

const DEFAULT_ALLOWED_SOURCE_TYPES: [&str; 7] = [
    "agent",
    "agent-task",
    "approval",
    "cli-tool",
    "javascript",
    "catalog",
    "graph",
];

const DEFAULT_MAX_TIMEOUT_SECONDS: i64 = 300;

#[must_use]
pub fn admit_local_skill(
    skill: &LocalAdmissionSkill,
    options: &LocalAdmissionOptions,
) -> AdmissionDecision {
    let mut reasons = Vec::new();

    collect_source_type_reason(skill, options, &mut reasons);
    collect_timeout_reasons(skill, options, &mut reasons);
    collect_local_source_reasons(skill, options, &mut reasons);
    collect_credential_grant_reasons(skill, options, &mut reasons);

    if reasons.is_empty() {
        AdmissionDecision::Allow {
            reasons: vec!["local admission allowed".to_owned()],
        }
    } else {
        AdmissionDecision::Deny { reasons }
    }
}

fn collect_source_type_reason(
    skill: &LocalAdmissionSkill,
    options: &LocalAdmissionOptions,
    reasons: &mut Vec<String>,
) {
    if !allowed_source_types(options).contains(&skill.source.source_type.as_str()) {
        reasons.push(format!(
            "source type '{}' is not allowed for local execution",
            skill.source.source_type
        ));
    }
}

fn collect_timeout_reasons(
    skill: &LocalAdmissionSkill,
    options: &LocalAdmissionOptions,
    reasons: &mut Vec<String>,
) {
    let Some(timeout_seconds) = skill.source.timeout_seconds else {
        return;
    };
    let max_timeout_seconds = options
        .max_timeout_seconds
        .unwrap_or(DEFAULT_MAX_TIMEOUT_SECONDS);

    if timeout_seconds <= 0 {
        reasons.push("source timeout must be greater than zero seconds".to_owned());
    }
    if timeout_seconds > max_timeout_seconds {
        reasons.push(format!(
            "source timeout exceeds local maximum of {max_timeout_seconds} seconds"
        ));
    }
}

fn collect_local_source_reasons(
    skill: &LocalAdmissionSkill,
    options: &LocalAdmissionOptions,
    reasons: &mut Vec<String>,
) {
    if skill.source.source_type == "cli-tool"
        && let Some(reason) = strict_cli_tool_inline_code_denial(
            skill.source.command.as_deref(),
            skill.source.args.as_deref().unwrap_or_default(),
            options.execution_policy.as_ref(),
        )
    {
        reasons.push(reason);
    }
}

fn collect_credential_grant_reasons(
    skill: &LocalAdmissionSkill,
    options: &LocalAdmissionOptions,
    reasons: &mut Vec<String>,
) {
    if options.skip_connected_auth.unwrap_or(false) {
        return;
    }
    let requirement = match credential_grant_requirement(skill.auth.as_ref()) {
        Ok(Some(requirement)) => requirement,
        Ok(None) => return,
        Err(error) => {
            reasons.push(error.message().to_owned());
            return;
        }
    };
    let grants = options.connected_grants.as_deref().unwrap_or_default();

    if find_matching_grant(
        &requirement,
        grants,
        options.connected_auth_checked_at.as_deref(),
        ScopeGrantPolicy::Delegated,
    )
    .is_none()
    {
        reasons.push(format!(
            "connected auth grant required for provider '{}'",
            requirement.provider
        ));
    }
}

fn allowed_source_types(options: &LocalAdmissionOptions) -> Vec<&str> {
    options.allowed_source_types.as_ref().map_or_else(
        || DEFAULT_ALLOWED_SOURCE_TYPES.to_vec(),
        |source_types| source_types.iter().map(String::as_str).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{LocalAdmissionSource, LocalExecutionPolicy};

    #[test]
    fn javascript_is_a_default_local_source() {
        let allowed = admit_local_skill(
            &javascript_skill(),
            &LocalAdmissionOptions {
                execution_policy: Some(LocalExecutionPolicy {
                    strict_cli_tool_inline_code: Some(true),
                }),
                ..LocalAdmissionOptions::default()
            },
        );
        assert!(matches!(allowed, AdmissionDecision::Allow { .. }));
    }

    fn javascript_skill() -> LocalAdmissionSkill {
        LocalAdmissionSkill {
            name: "domain-module".to_owned(),
            source: LocalAdmissionSource {
                source_type: "javascript".to_owned(),
                command: None,
                args: None,
                timeout_seconds: Some(30),
            },
            auth: None,
            runtime: None,
        }
    }
}
