use runx_runtime::{DEFAULT_MANAGED_AGENT_MAX_ROUNDS, ManagedAgentPolicy};

pub(crate) fn parse_boolean_flag(command: &str, flag: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(format!("runx {command} {flag} expects true or false")),
    }
}

pub(crate) fn parse_managed_agent_rounds(command: &str, value: &str) -> Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("runx {command} --drive-rounds expects a positive integer"))
}

pub(crate) fn managed_agent_policy(
    command: &str,
    enabled: bool,
    max_rounds: Option<u32>,
) -> Result<ManagedAgentPolicy, String> {
    if !enabled {
        if max_rounds.is_some() {
            return Err(format!(
                "runx {command} --drive-rounds requires --drive"
            ));
        }
        return Ok(ManagedAgentPolicy::HostDriven);
    }
    ManagedAgentPolicy::inline(max_rounds.unwrap_or(DEFAULT_MANAGED_AGENT_MAX_ROUNDS))
        .map_err(|error| format!("runx {command} {error}"))
}
