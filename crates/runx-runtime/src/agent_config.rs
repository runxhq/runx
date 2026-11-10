//! The managed agent's configuration, resolved for one run: its settings
//! from the environment over the home's config file, and its key from the
//! one place that holds it, the secret `agent.secret` names or a variable.

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::{
    ConfigError, ManagedAgentAuthMode, ManagedAgentConfig, ModelDialect, load_runx_config_file,
    model_provider, normalize_managed_agent_provider, validate_agent_endpoint_syntax,
};
use crate::credentials::SecretString;
use crate::paths::RunxPaths;

/// The provider a scripted model is served as (`scripted_model`): never a
/// real model, so evidence a turn it answered gives is never counted. Only a
/// build with `scripted-model` runs it, and only at a loopback endpoint.
pub const SCRIPTED_PROVIDER: &str = "scripted";

/// Whether this build runs `provider` at `endpoint`. Every provider runs a
/// real model but `scripted`, which a build runs only when it was built with
/// a scripted model (`scripted_model_built`), so a shipped build refuses it
/// by name.
fn accepts_provider(
    provider: &str,
    endpoint: Option<&str>,
    scripted_model_built: bool,
) -> Result<(), ConfigError> {
    if provider != SCRIPTED_PROVIDER {
        return Ok(());
    }
    if !scripted_model_built {
        return Err(ConfigError::InvalidAgentSetting {
            key: "agent.provider",
            message: "scripted is a scripted model's provider, which only a test build runs",
        });
    }
    match endpoint {
        Some(endpoint) if crate::http::validate_exact_loopback_agent_endpoint(endpoint).is_ok() => {
            Ok(())
        }
        _ => Err(ConfigError::InvalidAgentSetting {
            key: "agent.endpoint_url",
            message: "a scripted model is served at a loopback endpoint",
        }),
    }
}

pub fn load_managed_agent_config(
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Option<ManagedAgentConfig>, ConfigError> {
    let paths = RunxPaths::resolve(env, cwd);
    let config = load_runx_config_file(&paths.config_file())?;
    let auth_mode = env
        .get("RUNX_AGENT_AUTH_MODE")
        .or_else(|| {
            config
                .agent
                .as_ref()
                .and_then(|agent| agent.auth_mode.as_ref())
        })
        .map_or(Ok(ManagedAgentAuthMode::ApiKey), |value| {
            ManagedAgentAuthMode::parse(value)
        })?;
    let endpoint_url = env
        .get("RUNX_AGENT_ENDPOINT_URL")
        .or_else(|| {
            config
                .agent
                .as_ref()
                .and_then(|agent| agent.endpoint_url.as_ref())
        })
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if let Some(endpoint) = endpoint_url.as_deref() {
        validate_agent_endpoint_syntax(endpoint)?;
    }
    let provider = env
        .get("RUNX_AGENT_PROVIDER")
        .or_else(|| {
            config
                .agent
                .as_ref()
                .and_then(|agent| agent.provider.as_ref())
        })
        .and_then(|value| normalize_managed_agent_provider(value));
    let Some(provider) = provider else {
        if auth_mode == ManagedAgentAuthMode::LocalNone {
            return Err(ConfigError::InvalidAgentSetting {
                key: "agent.provider",
                message: "local_none requires a provider",
            });
        }
        return Ok(None);
    };
    accepts_provider(
        provider.as_str(),
        endpoint_url.as_deref(),
        cfg!(feature = "scripted-model"),
    )?;
    let profile = model_provider(provider.as_str());
    let dialect = match env.get("RUNX_AGENT_DIALECT").or_else(|| {
        config
            .agent
            .as_ref()
            .and_then(|agent| agent.dialect.as_ref())
    }) {
        Some(value) => ModelDialect::parse(value).ok_or(ConfigError::InvalidAgentSetting {
            key: "agent.dialect",
            message: "expected chat_completions, anthropic_messages or openai_responses",
        })?,
        None => profile
            .map(|profile| profile.dialect)
            .ok_or(ConfigError::InvalidAgentSetting {
                key: "agent.dialect",
                message: "a provider runx has no profile for needs a dialect and an endpoint",
            })?,
    };
    if profile.is_some_and(|profile| profile.dialect != dialect) {
        return Err(ConfigError::InvalidAgentSetting {
            key: "agent.dialect",
            message: "differs from the dialect the provider speaks",
        });
    }
    let model = env
        .get("RUNX_AGENT_MODEL")
        .or_else(|| config.agent.as_ref().and_then(|agent| agent.model.as_ref()))
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        // runx names no model of its own: which models a provider still
        // serves is the provider's to say.
        .ok_or(ConfigError::InvalidAgentSetting {
            key: "agent.model",
            message: "a provider needs a model; set RUNX_AGENT_MODEL or run `runx config set model <name>`",
        })?;
    // Every place the key can come from, and the one value they hold: two
    // that differ could be two accounts, and runx never picks one.
    let mut keys = Vec::new();
    for name in std::iter::once("RUNX_AGENT_API_KEY").chain(profile.map(|profile| profile.key_env))
    {
        if crate::services::env_file_differs(env, name) {
            return Err(ConfigError::ConflictingAgentKeys {
                sources: format!("in the environment and in the workspace .env as {name}"),
            });
        }
        if let Some(value) = env
            .get(name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            if crate::secrets::reference::looks_like_reference(value) {
                return Err(ConfigError::AgentKeyRead {
                    message: format!(
                        "{name} holds a reference to a secret, not the key; set a secret to it with `runx secret set <name> --from <reference>` and name it with `runx config set agent.secret <name>`, or run under the tool that resolves it"
                    ),
                });
            }
            keys.push((name.to_owned(), value.to_owned()));
        }
    }
    if auth_mode == ManagedAgentAuthMode::ApiKey
        && let Some(secret) = config
            .agent
            .as_ref()
            .and_then(|agent| agent.secret.as_ref())
    {
        let value = crate::secrets::read(env, cwd, secret, crate::secrets::Purpose::Call).map_err(
            |error| ConfigError::AgentKeyRead {
                message: error.to_string(),
            },
        )?;
        keys.push((
            format!("the secret {secret}"),
            value.expose().trim().to_owned(),
        ));
    }
    let api_key = match keys.as_slice() {
        [] => String::new(),
        [(_, first), rest @ ..] if rest.iter().all(|(_, value)| value == first) => first.clone(),
        several => {
            return Err(ConfigError::ConflictingAgentKeys {
                sources: format!(
                    "by {}",
                    several
                        .iter()
                        .map(|(source, _)| source.as_str())
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
            });
        }
    };
    match auth_mode {
        ManagedAgentAuthMode::ApiKey => {
            if endpoint_url
                .as_deref()
                .is_some_and(|endpoint| !endpoint.starts_with("https://"))
            {
                return Err(ConfigError::InvalidAgentSetting {
                    key: "agent.endpoint_url",
                    message: "api_key endpoints require HTTPS",
                });
            }
            if api_key.is_empty() {
                return Ok(None);
            }
        }
        ManagedAgentAuthMode::LocalNone => {
            if dialect != ModelDialect::ChatCompletions || !api_key.is_empty() {
                return Err(ConfigError::InvalidAgentSetting {
                    key: "agent.auth_mode",
                    message: "local_none requires the chat_completions dialect and no API key",
                });
            }
            let Some(endpoint) = endpoint_url.as_deref() else {
                return Err(ConfigError::InvalidAgentSetting {
                    key: "agent.endpoint_url",
                    message: "local_none requires an explicit loopback endpoint",
                });
            };
            crate::http::validate_exact_loopback_agent_endpoint(endpoint).map_err(|_| {
                ConfigError::InvalidAgentSetting {
                    key: "agent.endpoint_url",
                    message: "local_none requires a literal loopback chat-completions URL",
                }
            })?;
        }
    }
    let endpoint_url = endpoint_url
        .or_else(|| profile.map(|profile| profile.endpoint.to_owned()))
        .ok_or(ConfigError::InvalidAgentSetting {
            key: "agent.endpoint_url",
            message: "a provider runx has no profile for needs a dialect and an endpoint",
        })?;
    Ok(Some(ManagedAgentConfig {
        provider,
        dialect,
        model,
        api_key: (!api_key.is_empty()).then(|| SecretString::new(api_key)),
        endpoint_url,
        auth_mode,
    }))
}

#[cfg(test)]
mod tests {
    use super::{SCRIPTED_PROVIDER, accepts_provider};

    /// A shipped build refuses `scripted` by name, and a build with a scripted
    /// model runs it only at a loopback endpoint. Every other provider is a
    /// real model's, whatever the build.
    #[test]
    fn only_a_scripted_model_build_runs_the_scripted_provider_and_only_at_loopback() {
        let loopback = Some("http://127.0.0.1:4000/v1/chat/completions");
        let refused = accepts_provider(SCRIPTED_PROVIDER, loopback, false)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(refused.contains("only a test build"), "{refused}");
        assert!(accepts_provider(SCRIPTED_PROVIDER, loopback, true).is_ok());
        assert!(
            accepts_provider(
                SCRIPTED_PROVIDER,
                Some("https://models.example.com/v1/chat/completions"),
                true
            )
            .is_err()
        );
        assert!(accepts_provider(SCRIPTED_PROVIDER, None, true).is_err());
        for built in [false, true] {
            assert!(accepts_provider("ollama", loopback, built).is_ok());
            assert!(accepts_provider("anthropic", None, built).is_ok());
        }
    }
}
