// Module rationale: the native config command keeps parse,
// execute, render, and tests together for one audited CLI surface.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::Path;

use crate::cli_args::{os_arg, split_flag};
use runx_runtime::{
    ConfigError, RunxConfigFile, RunxPaths, clear_runx_agent_dialect,
    clear_runx_agent_endpoint_url, clear_runx_agent_secret, load_runx_config_file,
    lookup_runx_config_value, mask_runx_config_file, parse_config_key, update_runx_config_value,
    write_runx_config_file,
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

/// A config command. Config holds settings, never a secret: the model key
/// is a secret `runx secret` keeps, which `agent.secret` names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigPlan {
    pub action: ConfigAction,
    pub key: Option<String>,
    pub value: Option<String>,
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

/// What a model key is set with now: it is a secret, which the agent names.
const MODEL_KEY_IS_A_SECRET: &str = "the model key is a secret: set it with `runx secret set <name>`, then name it with `runx config set agent.secret <name>`";

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
    let mut positionals = Vec::new();
    for index in 2..args.len() {
        let token = os_arg(args, index, "config")?;
        if !token.starts_with('-') {
            positionals.push(token.to_owned());
            continue;
        }
        match split_flag(token) {
            ("--json" | "-j", None) => json = true,
            ("--json" | "-j", Some(_)) => return Err("--json does not take a value".to_owned()),
            (flag, _) => return Err(format!("unknown config flag {flag}")),
        }
    }

    let key = |key: &str| match normalize_config_key(key) {
        "agent.api_key" => Err(MODEL_KEY_IS_A_SECRET.to_owned()),
        key => Ok(key.to_owned()),
    };
    match action {
        ConfigAction::List => {
            if !positionals.is_empty() {
                return Err("runx config list does not accept extra arguments".to_owned());
            }
            Ok(ConfigPlan {
                action,
                key: None,
                value: None,
                json,
            })
        }
        ConfigAction::Get => {
            let [name] = positionals.as_slice() else {
                return Err("runx config get requires exactly one key".to_owned());
            };
            Ok(ConfigPlan {
                action,
                key: Some(key(name)?),
                value: None,
                json,
            })
        }
        ConfigAction::Unset => {
            let [name] = positionals.as_slice() else {
                return Err("runx config unset requires exactly one key".to_owned());
            };
            let key = key(name)?;
            if !matches!(
                key.as_str(),
                "agent.endpoint_url" | "agent.dialect" | "agent.secret"
            ) {
                return Err(UNSET_ONLY.to_owned());
            }
            Ok(ConfigPlan {
                action,
                key: Some(key),
                value: None,
                json,
            })
        }
        ConfigAction::Set => {
            let [name, values @ ..] = positionals.as_slice() else {
                return Err("runx config set requires a key".to_owned());
            };
            if values.is_empty() {
                return Err("runx config set requires a value".to_owned());
            }
            Ok(ConfigPlan {
                action,
                key: Some(key(name)?),
                value: Some(values.join(" ")),
                json,
            })
        }
    }
}

fn normalize_config_key(key: &str) -> &str {
    match key {
        "provider" => "agent.provider",
        "dialect" => "agent.dialect",
        "model" => "agent.model",
        "secret" => "agent.secret",
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
    let config_path = RunxPaths::resolve(env, cwd).config_file();
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
            let next = update_runx_config_value(config, parsed_key, value)?;
            write_runx_config_file(&config_path, &next)?;
            Ok(ConfigResult::Set {
                action: ConfigAction::Set,
                key: key.to_owned(),
                value: lookup_runx_config_value(&next, parsed_key),
            })
        }
        ConfigAction::Unset => {
            let key = required_key(plan)?;
            let next = match key {
                "agent.endpoint_url" => clear_runx_agent_endpoint_url(config),
                "agent.dialect" => clear_runx_agent_dialect(config),
                "agent.secret" => clear_runx_agent_secret(config),
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

/// The settings whose absence means something: the provider's own profile,
/// or a managed agent that names no key.
const UNSET_ONLY: &str = "runx config unset clears only endpoint-url, dialect or secret; set another value with runx config set";

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

/// The settings as rows. The secrets are `runx secret list`'s to show.
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
        if let Some(secret) = agent.secret.as_ref() {
            rows.push(("agent.secret", secret.as_str()));
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

    fn plan(args: &[&str]) -> Result<ConfigPlan, String> {
        parse_config_plan(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn parses_config_set_with_multi_word_value() {
        assert_eq!(
            plan(&["config", "set", "model", "gpt", "test", "-j"]),
            Ok(ConfigPlan {
                action: ConfigAction::Set,
                key: Some("agent.model".to_owned()),
                value: Some("gpt test".to_owned()),
                json: true,
            })
        );
    }

    #[test]
    fn unset_is_limited_to_the_settings_whose_absence_means_something() {
        assert_eq!(
            plan(&["config", "unset", "endpoint-url"]),
            Ok(ConfigPlan {
                action: ConfigAction::Unset,
                key: Some("agent.endpoint_url".to_owned()),
                value: None,
                json: false,
            })
        );
        assert!(plan(&["config", "unset", "dialect"]).is_ok());
        assert!(plan(&["config", "unset", "secret"]).is_ok());
        assert!(plan(&["config", "unset", "model"]).is_err());
    }

    #[test]
    fn a_model_key_is_never_config_and_says_what_sets_it() {
        for args in [
            &["config", "set", "api-key", "secret-on-argv"][..],
            &["config", "get", "agent-key"][..],
            &["config", "unset", "agent.api_key"][..],
        ] {
            assert_eq!(plan(args), Err(MODEL_KEY_IS_A_SECRET.to_owned()));
        }
        assert_eq!(
            plan(&["config", "set", "api-key", "--from-stdin"]),
            Err("unknown config flag --from-stdin".to_owned())
        );
    }

    #[test]
    // Function rationale: one temp config lifecycle proves set/get/list and
    // unset of the agent's secret against the same local state.
    fn the_agent_names_a_set_secret_and_config_never_holds_its_value() -> Result<(), ConfigTestError>
    {
        let temp = tempfile_dir()?;
        let runx_home = temp.join(".runx");
        let env = BTreeMap::from([(
            "RUNX_HOME".to_owned(),
            runx_home.to_string_lossy().to_string(),
        )]);
        let run = |args: &[&str]| -> Result<String, ConfigTestError> {
            Ok(run_config_command(
                &plan(args).map_err(io::Error::other)?,
                &env,
                &temp,
            )?)
        };

        assert!(
            run(&["config", "set", "secret", "anthropic"]).is_err(),
            "an unset secret is named"
        );
        let name = runx_contracts::connector::SecretName::new("anthropic")
            .ok_or_else(|| io::Error::other("a secret name"))?;
        runx_runtime::secrets::set_value(&RunxPaths::resolve(&env, &temp), &name, "sk-secret-test")
            .map_err(|error| io::Error::other(error.to_string()))?;
        run(&["config", "set", "provider", "anthropic"])?;
        let set = run(&["config", "set", "secret", "anthropic", "-j"])?;
        assert!(set.contains("\"value\": \"anthropic\""), "{set}");

        let list = run(&["config", "list"])?;
        assert!(list.contains("agent.secret"), "{list}");
        assert!(!list.contains("sk-secret-test"), "{list}");
        let config_contents = fs::read_to_string(runx_home.join("config.json"))?;
        assert!(!config_contents.contains("sk-secret-test"));

        run(&["config", "unset", "secret"])?;
        assert!(!run(&["config", "list"])?.contains("agent.secret"));
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
        let set = |key: &str, value: &str| {
            run_config_command(
                &ConfigPlan {
                    action: ConfigAction::Set,
                    key: Some(key.to_owned()),
                    value: Some(value.to_owned()),
                    json: false,
                },
                &env,
                &temp,
            )
        };
        for (key, value) in [
            ("agent.provider", "openai"),
            ("agent.model", "fixture-model"),
            ("agent.auth_mode", "local_none"),
            (
                "agent.endpoint_url",
                "http://127.0.0.1:18081/v1/chat/completions",
            ),
        ] {
            set(key, value)?;
        }
        let list = run_config_command(
            &ConfigPlan {
                action: ConfigAction::List,
                key: None,
                value: None,
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
                json: false,
            },
            &env,
            &temp,
        )?;
        assert!(get.contains("bad_mode"));
        assert!(set("agent.auth_mode", "bad_mode").is_err());
        set("agent.auth_mode", "local_none")?;
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
