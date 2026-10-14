// Module rationale: the native config command keeps parse,
// execute, render, and tests together for one audited CLI surface.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::Path;

use crate::cli_args::{os_arg, split_flag};
use runx_runtime::{
    ConfigError, RunxConfigFile, RunxPaths, clear_runx_agent_dialect,
    clear_runx_agent_endpoint_url, load_runx_config_file, lookup_runx_config_value,
    mask_runx_config_file, parse_config_key, update_runx_config_value, write_runx_config_file,
};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigAction {
    Set,
    Get,
    List,
    Unset,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigPlan {
    pub action: ConfigAction,
    pub key: Option<String>,
    pub value: Option<String>,
    pub value_from_stdin: bool,
    pub json: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ConfigResult {
    Set {
        action: ConfigAction,
        key: String,
        value: Option<String>,
    },
    Get {
        action: ConfigAction,
        key: String,
        value: Option<String>,
    },
    List {
        action: ConfigAction,
        values: Box<RunxConfigFile>,
    },
    Unset {
        action: ConfigAction,
        key: String,
        value: Option<String>,
    },
}

#[derive(Debug)]
pub enum ConfigCliError {
    InvalidArgs(String),
    Config(ConfigError),
    Serialize(serde_json::Error),
}

impl fmt::Display for ConfigCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgs(message) => formatter.write_str(message),
            Self::Config(error) => write!(formatter, "{error}"),
            Self::Serialize(error) => write!(formatter, "failed to serialize config: {error}"),
        }
    }
}

impl std::error::Error for ConfigCliError {}

impl From<ConfigError> for ConfigCliError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<serde_json::Error> for ConfigCliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

// Function rationale: config set/get/list/unset share one small
// flag grammar and keeping it adjacent avoids divergent command parsing.
pub fn parse_config_plan(args: &[OsString]) -> Result<ConfigPlan, String> {
    let command = os_arg(args, 0, "config")?;
    if command != "config" {
        return Err("config parser requires the config command".to_owned());
    }

    let Some(subcommand) = args.get(1).and_then(|arg| arg.to_str()) else {
        return Err("runx config requires set, get, list, or unset".to_owned());
    };
    let action = match subcommand {
        "set" => ConfigAction::Set,
        "get" => ConfigAction::Get,
        "list" => ConfigAction::List,
        "unset" => ConfigAction::Unset,
        _ => return Err(format!("unknown config subcommand {subcommand}")),
    };

    let mut json = false;
    let mut value_from_stdin = false;
    let mut positionals = Vec::new();
    let mut index = 2;
    while index < args.len() {
        let token = os_arg(args, index, "config")?;
        if !token.starts_with('-') {
            positionals.push(token.to_owned());
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        match flag {
            "--json" | "-j" => {
                if inline_value.is_some() {
                    return Err("--json does not take a value".to_owned());
                }
                json = true;
                index += 1;
            }
            "--from-stdin" => {
                if inline_value.is_some() {
                    return Err("--from-stdin does not take a value".to_owned());
                }
                value_from_stdin = true;
                index += 1;
            }
            _ => return Err(format!("unknown config flag {flag}")),
        }
    }

    match action {
        ConfigAction::List => {
            if value_from_stdin {
                return Err("runx config list does not accept --from-stdin".to_owned());
            }
            if !positionals.is_empty() {
                return Err("runx config list does not accept extra arguments".to_owned());
            }
            Ok(ConfigPlan {
                action,
                key: None,
                value: None,
                value_from_stdin: false,
                json,
            })
        }
        ConfigAction::Get => {
            if value_from_stdin {
                return Err("runx config get does not accept --from-stdin".to_owned());
            }
            let [key] = positionals.as_slice() else {
                return Err("runx config get requires exactly one key".to_owned());
            };
            Ok(ConfigPlan {
                action,
                key: Some(normalize_config_key(key).to_owned()),
                value: None,
                value_from_stdin: false,
                json,
            })
        }
        ConfigAction::Unset => {
            if value_from_stdin {
                return Err("runx config unset does not accept --from-stdin".to_owned());
            }
            let [key] = positionals.as_slice() else {
                return Err("runx config unset requires exactly one key".to_owned());
            };
            let key = normalize_config_key(key);
            if !matches!(key, "agent.endpoint_url" | "agent.dialect") {
                return Err(UNSET_ONLY.to_owned());
            }
            Ok(ConfigPlan {
                action,
                key: Some(key.to_owned()),
                value: None,
                value_from_stdin: false,
                json,
            })
        }
        ConfigAction::Set => {
            let [key, values @ ..] = positionals.as_slice() else {
                return Err("runx config set requires a key".to_owned());
            };
            let key = normalize_config_key(key);
            if is_secret_config_key(key) {
                if !values.is_empty() {
                    return Err(
                        "secret config values must be provided through --from-stdin".to_owned()
                    );
                }
                if !value_from_stdin {
                    return Err("secret config values require --from-stdin".to_owned());
                }
            } else if value_from_stdin {
                return Err("--from-stdin is only valid for secret config values".to_owned());
            } else if values.is_empty() {
                return Err("runx config set requires a value".to_owned());
            }
            Ok(ConfigPlan {
                action,
                key: Some(key.to_owned()),
                value: (!values.is_empty()).then(|| values.join(" ")),
                value_from_stdin,
                json,
            })
        }
    }
}

pub fn is_secret_config_key(key: &str) -> bool {
    normalize_config_key(key) == "agent.api_key"
}

fn normalize_config_key(key: &str) -> &str {
    match key {
        "provider" => "agent.provider",
        "dialect" => "agent.dialect",
        "model" => "agent.model",
        "endpoint-url" => "agent.endpoint_url",
        "auth-mode" => "agent.auth_mode",
        "api-key" | "agent-key" => "agent.api_key",
        _ => key,
    }
}

pub fn run_config_command(
    plan: &ConfigPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<String, ConfigCliError> {
    let result = execute_config_plan(plan, env, cwd)?;
    if plan.json {
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&ConfigJsonResult {
                status: "success",
                config: &result,
            })?
        ));
    }
    Ok(render_config_result(&result))
}

fn execute_config_plan(
    plan: &ConfigPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ConfigResult, ConfigCliError> {
    let paths = RunxPaths::resolve(env, cwd);
    let config_path = paths.config_file();
    let config = load_runx_config_file(&config_path)?;

    match plan.action {
        ConfigAction::List => Ok(ConfigResult::List {
            action: ConfigAction::List,
            values: Box::new(mask_runx_config_file(&config)),
        }),
        ConfigAction::Get => {
            let key = required_key(plan)?;
            let parsed_key = parse_config_key(key)?;
            Ok(ConfigResult::Get {
                action: ConfigAction::Get,
                key: key.to_owned(),
                value: lookup_runx_config_value(&config, parsed_key),
            })
        }
        ConfigAction::Set => {
            let key = required_key(plan)?;
            let value = plan.value.as_deref().ok_or_else(|| {
                ConfigCliError::InvalidArgs("config value is required.".to_owned())
            })?;
            let parsed_key = parse_config_key(key)?;
            let next = update_runx_config_value(config, parsed_key, value, &paths)?;
            write_runx_config_file(&config_path, &next)?;
            Ok(ConfigResult::Set {
                action: ConfigAction::Set,
                key: key.to_owned(),
                value: lookup_runx_config_value(&mask_runx_config_file(&next), parsed_key),
            })
        }
        ConfigAction::Unset => {
            let key = required_key(plan)?;
            let next = match key {
                "agent.endpoint_url" => clear_runx_agent_endpoint_url(config),
                "agent.dialect" => clear_runx_agent_dialect(config),
                _ => return Err(ConfigCliError::InvalidArgs(UNSET_ONLY.to_owned())),
            };
            write_runx_config_file(&config_path, &next)?;
            Ok(ConfigResult::Unset {
                action: ConfigAction::Unset,
                key: key.to_owned(),
                value: None,
            })
        }
    }
}

/// The settings whose absence means the provider's own profile.
const UNSET_ONLY: &str =
    "runx config unset clears only endpoint-url or dialect; set another value with runx config set";

fn required_key(plan: &ConfigPlan) -> Result<&str, ConfigCliError> {
    plan.key
        .as_deref()
        .ok_or_else(|| ConfigCliError::InvalidArgs("config key is required.".to_owned()))
}

fn render_config_result(result: &ConfigResult) -> String {
    match result {
        ConfigResult::List { values, .. } => {
            let entries = flatten_config(values);
            if entries.is_empty() {
                return "\n  No config values set.\n\n".to_owned();
            }
            let rows = entries
                .iter()
                .map(|(key, value)| (*key, Some(*value)))
                .collect::<Vec<_>>();
            render_key_value("config", "success", &rows)
        }
        ConfigResult::Get { key, value, .. }
        | ConfigResult::Set { key, value, .. }
        | ConfigResult::Unset { key, value, .. } => {
            render_key_value("config", "success", &[(key.as_str(), value.as_deref())])
        }
    }
}

fn flatten_config(config: &RunxConfigFile) -> Vec<(&'static str, &str)> {
    let mut rows = Vec::new();
    if let Some(agent) = config.agent.as_ref() {
        if let Some(provider) = agent.provider.as_deref() {
            rows.push(("agent.provider", provider));
        }
        if let Some(dialect) = agent.dialect.as_deref() {
            rows.push(("agent.dialect", dialect));
        }
        if let Some(model) = agent.model.as_deref() {
            rows.push(("agent.model", model));
        }
        if let Some(endpoint_url) = agent.endpoint_url.as_deref() {
            rows.push(("agent.endpoint_url", endpoint_url));
        }
        if let Some(auth_mode) = agent.auth_mode.as_deref() {
            rows.push(("agent.auth_mode", auth_mode));
        }
        if let Some(api_key_ref) = agent.api_key_ref.as_deref() {
            rows.push(("agent.api_key", api_key_ref));
        }
    }
    if let Some(public) = config.public.as_ref() {
        if let Some(api_token_ref) = public.api_token_ref.as_deref() {
            rows.push(("public.api_token", api_token_ref));
        }
        if let Some(api_token_ref) = public.publish_api_token_ref.as_deref() {
            rows.push(("public.publish_api_token", api_token_ref));
        }
    }
    rows
}

fn render_key_value(title: &str, status: &str, rows: &[(&str, Option<&str>)]) -> String {
    let visible = rows
        .iter()
        .filter(|(_label, value)| value.is_some_and(|value| !value.is_empty()))
        .collect::<Vec<_>>();
    let width = visible
        .iter()
        .map(|(label, _value)| label.len())
        .max()
        .unwrap_or(0);
    let mut lines = vec![String::new(), format!("  ✓  {title}  {status}")];
    lines.extend(
        visible
            .into_iter()
            .map(|(label, value)| format!("  {label:<width$}  {}", value.unwrap_or_default())),
    );
    lines.push(String::new());
    lines.join("\n")
}

#[derive(Serialize)]
struct ConfigJsonResult<'a> {
    status: &'static str,
    config: &'a ConfigResult,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn parses_config_set_with_multi_word_value() {
        assert_eq!(
            parse_config_plan(&[
                "config".into(),
                "set".into(),
                "model".into(),
                "gpt".into(),
                "test".into(),
                "-j".into(),
            ]),
            Ok(ConfigPlan {
                action: ConfigAction::Set,
                key: Some("agent.model".to_owned()),
                value: Some("gpt test".to_owned()),
                value_from_stdin: false,
                json: true,
            })
        );
    }

    #[test]
    fn unset_is_limited_to_the_settings_a_profile_supplies() {
        assert_eq!(
            parse_config_plan(&["config".into(), "unset".into(), "endpoint-url".into()]),
            Ok(ConfigPlan {
                action: ConfigAction::Unset,
                key: Some("agent.endpoint_url".to_owned()),
                value: None,
                value_from_stdin: false,
                json: false,
            })
        );
        assert!(parse_config_plan(&["config".into(), "unset".into(), "dialect".into()]).is_ok());
        assert!(parse_config_plan(&["config".into(), "unset".into(), "api-key".into()]).is_err());
    }

    #[test]
    fn secret_config_values_require_stdin_and_never_accept_argv() {
        assert_eq!(
            parse_config_plan(&[
                "config".into(),
                "set".into(),
                "api-key".into(),
                "secret-on-argv".into(),
            ]),
            Err("secret config values must be provided through --from-stdin".to_owned())
        );
        assert_eq!(
            parse_config_plan(&[
                "config".into(),
                "set".into(),
                "api-key".into(),
                "--from-stdin".into(),
            ]),
            Ok(ConfigPlan {
                action: ConfigAction::Set,
                key: Some("agent.api_key".to_owned()),
                value: None,
                value_from_stdin: true,
                json: false,
            })
        );
    }

    #[test]
    // Function rationale: one temp config lifecycle proves
    // set/get/list masking against the same encrypted local state.
    fn config_set_get_list_masks_api_key() -> Result<(), ConfigTestError> {
        let temp = tempfile_dir()?;
        let runx_home = temp.join(".runx");
        let env = BTreeMap::from([(
            "RUNX_HOME".to_owned(),
            runx_home.to_string_lossy().to_string(),
        )]);

        let set_provider = ConfigPlan {
            action: ConfigAction::Set,
            key: Some("agent.provider".to_owned()),
            value: Some("openai".to_owned()),
            value_from_stdin: false,
            json: true,
        };
        let set_key = ConfigPlan {
            action: ConfigAction::Set,
            key: Some("agent.api_key".to_owned()),
            value: Some("sk-secret-test".to_owned()),
            value_from_stdin: true,
            json: true,
        };
        run_config_command(&set_provider, &env, &temp)?;
        let key_output = run_config_command(&set_key, &env, &temp)?;
        assert!(key_output.contains("\"value\": \"[encrypted]\""));
        assert!(!key_output.contains("sk-secret-test"));

        let get_output = run_config_command(
            &ConfigPlan {
                action: ConfigAction::Get,
                key: Some("agent.api_key".to_owned()),
                value: None,
                value_from_stdin: false,
                json: false,
            },
            &env,
            &temp,
        )?;
        assert!(get_output.contains("agent.api_key"));
        assert!(get_output.contains("[encrypted]"));
        assert!(!get_output.contains("sk-secret-test"));

        let list_output = run_config_command(
            &ConfigPlan {
                action: ConfigAction::List,
                key: None,
                value: None,
                value_from_stdin: false,
                json: false,
            },
            &env,
            &temp,
        )?;
        assert!(list_output.contains("agent.provider"));
        assert!(list_output.contains("openai"));
        assert!(list_output.contains("agent.api_key"));
        assert!(list_output.contains("[encrypted]"));
        assert!(!list_output.contains("sk-secret-test"));

        let config_contents = fs::read_to_string(runx_home.join("config.json"))?;
        assert!(config_contents.contains("api_key_ref"));
        assert!(!config_contents.contains("sk-secret-test"));
        fs::remove_dir_all(temp)?;
        Ok(())
    }

    #[test]
    fn local_model_settings_round_trip_and_invalid_mode_remains_repairable()
    -> Result<(), ConfigTestError> {
        let temp = tempfile_dir()?;
        let runx_home = temp.join(".runx");
        let env = BTreeMap::from([(
            "RUNX_HOME".to_owned(),
            runx_home.to_string_lossy().into_owned(),
        )]);
        for (key, value) in [
            ("agent.provider", "openai"),
            ("agent.model", "fixture-model"),
            ("agent.auth_mode", "local_none"),
            (
                "agent.endpoint_url",
                "http://127.0.0.1:18081/v1/chat/completions",
            ),
        ] {
            run_config_command(
                &ConfigPlan {
                    action: ConfigAction::Set,
                    key: Some(key.to_owned()),
                    value: Some(value.to_owned()),
                    value_from_stdin: false,
                    json: false,
                },
                &env,
                &temp,
            )?;
        }
        let list = run_config_command(
            &ConfigPlan {
                action: ConfigAction::List,
                key: None,
                value: None,
                value_from_stdin: false,
                json: true,
            },
            &env,
            &temp,
        )?;
        let listed: serde_json::Value =
            serde_json::from_str(&list).map_err(|error| io::Error::other(error.to_string()))?;
        assert_eq!(
            listed["config"]["values"]["agent"]["auth_mode"],
            "local_none"
        );
        assert_eq!(
            listed["config"]["values"]["agent"]["endpoint_url"],
            "http://127.0.0.1:18081/v1/chat/completions"
        );

        let path = runx_home.join("config.json");
        let mut raw: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)
            .map_err(|error| io::Error::other(error.to_string()))?;
        raw["agent"]["auth_mode"] = serde_json::Value::String("bad_mode".to_owned());
        fs::write(
            &path,
            serde_json::to_vec(&raw).map_err(|error| io::Error::other(error.to_string()))?,
        )?;
        let get = run_config_command(
            &ConfigPlan {
                action: ConfigAction::Get,
                key: Some("agent.auth_mode".to_owned()),
                value: None,
                value_from_stdin: false,
                json: false,
            },
            &env,
            &temp,
        )?;
        assert!(get.contains("bad_mode"));
        let bad_set = run_config_command(
            &ConfigPlan {
                action: ConfigAction::Set,
                key: Some("agent.auth_mode".to_owned()),
                value: Some("bad_mode".to_owned()),
                value_from_stdin: false,
                json: false,
            },
            &env,
            &temp,
        );
        assert!(bad_set.is_err());
        run_config_command(
            &ConfigPlan {
                action: ConfigAction::Set,
                key: Some("agent.auth_mode".to_owned()),
                value: Some("local_none".to_owned()),
                value_from_stdin: false,
                json: false,
            },
            &env,
            &temp,
        )?;
        fs::remove_dir_all(temp)?;
        Ok(())
    }

    #[derive(Debug)]
    enum ConfigTestError {
        Io(io::Error),
        Cli(ConfigCliError),
    }

    impl std::fmt::Display for ConfigTestError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Io(error) => write!(formatter, "{error}"),
                Self::Cli(error) => write!(formatter, "{error}"),
            }
        }
    }

    impl std::error::Error for ConfigTestError {}

    impl From<io::Error> for ConfigTestError {
        fn from(error: io::Error) -> Self {
            Self::Io(error)
        }
    }

    impl From<ConfigCliError> for ConfigTestError {
        fn from(error: ConfigCliError) -> Self {
            Self::Cli(error)
        }
    }

    fn tempfile_dir() -> Result<PathBuf, io::Error> {
        static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "runx-cli-config-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(path)
    }
}
