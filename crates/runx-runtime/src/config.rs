// Module rationale: local config, encrypted local key
// storage, managed-agent overlay, and profile resolution are one unit.
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, Generate, KeyInit, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use runx_contracts::JsonValue;
use runx_contracts::schema::NonEmptyString;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::credentials::SecretString;
use crate::paths::RunxPaths;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunxConfigFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<RunxAgentConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public: Option<RunxPublicConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials: Option<RunxCredentialsConfig>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunxCredentialsConfig {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, RunxCredentialProfile>,
}

/// A stored secret: which connector credential it holds, and where the
/// encrypted material is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunxCredentialProfile {
    pub connector: String,
    pub credential: String,
    pub secret_ref: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunxAgentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The wire format of a provider runx has no built-in profile for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunxPublicConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,
    /// Default operator credential used by connect, provider effects, and
    /// hosted artifact operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_token_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    /// Narrow credential used only by receipt and skill publication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_api_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_api_token_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_principal_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigKey {
    AgentProvider,
    AgentDialect,
    AgentModel,
    AgentApiKey,
    AgentEndpointUrl,
    AgentAuthMode,
    PublicApiToken,
}

/// A model API's wire format. Dialects are code; providers are data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelDialect {
    /// The chat completions format most providers and local servers speak.
    ChatCompletions,
    AnthropicMessages,
}

impl ModelDialect {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "chat_completions" => Some(Self::ChatCompletions),
            "anthropic_messages" => Some(Self::AnthropicMessages),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::AnthropicMessages => "anthropic_messages",
        }
    }
}

/// A provider runx knows: the dialect it speaks, where, the variable that
/// holds its key, and the model a run uses when none is configured. Any other
/// provider names its dialect, endpoint and model in config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelProvider {
    pub id: &'static str,
    pub dialect: ModelDialect,
    pub endpoint: &'static str,
    pub key_env: &'static str,
    pub default_model: &'static str,
}

pub const MODEL_PROVIDERS: &[ModelProvider] = &[
    ModelProvider {
        id: "anthropic",
        dialect: ModelDialect::AnthropicMessages,
        endpoint: "https://api.anthropic.com/v1/messages",
        key_env: "ANTHROPIC_API_KEY",
        default_model: "claude-opus-5-5",
    },
    ModelProvider {
        id: "openai",
        dialect: ModelDialect::ChatCompletions,
        endpoint: "https://api.openai.com/v1/chat/completions",
        key_env: "OPENAI_API_KEY",
        default_model: "gpt-6-sol",
    },
    ModelProvider {
        id: "gemini",
        dialect: ModelDialect::ChatCompletions,
        endpoint: "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
        key_env: "GEMINI_API_KEY",
        default_model: "gemini-3-pro-preview",
    },
];

/// The built-in profile of provider `id`.
#[must_use]
pub fn model_provider(id: &str) -> Option<&'static ModelProvider> {
    MODEL_PROVIDERS.iter().find(|provider| provider.id == id)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedAgentConfig {
    /// The provider's id: a built-in profile, or a name the operator gave a
    /// provider configured by dialect and endpoint.
    pub provider: NonEmptyString,
    pub dialect: ModelDialect,
    pub model: String,
    pub api_key: Option<SecretString>,
    pub endpoint_url: String,
    pub auth_mode: ManagedAgentAuthMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedAgentAuthMode {
    ApiKey,
    LocalNone,
}

impl ManagedAgentAuthMode {
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        match value.trim() {
            "api_key" => Ok(Self::ApiKey),
            "local_none" => Ok(Self::LocalNone),
            _ => Err(ConfigError::InvalidAgentSetting {
                key: "agent.auth_mode",
                message: "expected api_key or local_none",
            }),
        }
    }
}

fn validate_agent_endpoint_syntax(value: &str) -> Result<(), ConfigError> {
    let url = url::Url::parse(value).map_err(|_| ConfigError::InvalidAgentSetting {
        key: "agent.endpoint_url",
        message: "expected an absolute HTTP or HTTPS URL",
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::InvalidAgentSetting {
            key: "agent.endpoint_url",
            message: "expected an HTTP or HTTPS URL without credentials, query or fragment",
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{path} is not valid JSON: {message}")]
    InvalidJson { path: PathBuf, message: String },
    #[error("{path} must contain a JSON object.")]
    NonObjectJson { path: PathBuf },
    #[error("unsupported runx config key {key}")]
    UnsupportedKey { key: String },
    #[error("invalid {key}: {message}")]
    InvalidAgentSetting {
        key: &'static str,
        message: &'static str,
    },
    #[error("runx local agent key corrupted or unreadable at {path}{suffix}")]
    LocalAgentKeyCorrupt { path: PathBuf, suffix: String },
    #[error("config crypto failed: {0}")]
    Crypto(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub fn parse_config_key(key: &str) -> Result<ConfigKey, ConfigError> {
    match key {
        "agent.provider" => Ok(ConfigKey::AgentProvider),
        "agent.dialect" => Ok(ConfigKey::AgentDialect),
        "agent.model" => Ok(ConfigKey::AgentModel),
        "agent.api_key" => Ok(ConfigKey::AgentApiKey),
        "agent.endpoint_url" => Ok(ConfigKey::AgentEndpointUrl),
        "agent.auth_mode" => Ok(ConfigKey::AgentAuthMode),
        "public.api_token" => Ok(ConfigKey::PublicApiToken),
        _ => Err(ConfigError::UnsupportedKey {
            key: key.to_owned(),
        }),
    }
}

pub fn load_runx_config_file(config_path: &Path) -> Result<RunxConfigFile, ConfigError> {
    let contents = match fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RunxConfigFile::default());
        }
        Err(error) => return Err(ConfigError::Io(error)),
    };
    let value =
        serde_json::from_str::<JsonValue>(&contents).map_err(|error| ConfigError::InvalidJson {
            path: config_path.to_path_buf(),
            message: error.to_string(),
        })?;
    if !matches!(value, JsonValue::Object(_)) {
        return Err(ConfigError::NonObjectJson {
            path: config_path.to_path_buf(),
        });
    }
    serde_json::from_str(&contents).map_err(|error| ConfigError::InvalidJson {
        path: config_path.to_path_buf(),
        message: error.to_string(),
    })
}

pub fn write_runx_config_file(
    config_path: &Path,
    config: &RunxConfigFile,
) -> Result<(), ConfigError> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents =
        serde_json::to_string_pretty(config).map_err(|error| ConfigError::InvalidJson {
            path: config_path.to_path_buf(),
            message: error.to_string(),
        })?;
    write_private_file(config_path, format!("{contents}\n").as_bytes())
}

pub fn update_runx_config_value(
    mut config: RunxConfigFile,
    key: ConfigKey,
    value: &str,
    paths: &RunxPaths,
) -> Result<RunxConfigFile, ConfigError> {
    match key {
        ConfigKey::AgentProvider => {
            let mut agent = config.agent.unwrap_or_default();
            agent.provider = Some(value.to_owned());
            config.agent = Some(agent);
        }
        ConfigKey::AgentDialect => {
            ModelDialect::parse(value).ok_or(ConfigError::InvalidAgentSetting {
                key: "agent.dialect",
                message: "expected chat_completions or anthropic_messages",
            })?;
            let mut agent = config.agent.unwrap_or_default();
            agent.dialect = Some(value.trim().to_owned());
            config.agent = Some(agent);
        }
        ConfigKey::AgentModel => {
            let mut agent = config.agent.unwrap_or_default();
            agent.model = Some(value.to_owned());
            config.agent = Some(agent);
        }
        ConfigKey::AgentApiKey => {
            let mut agent = config.agent.unwrap_or_default();
            agent.api_key_ref = Some(store_local_agent_api_key(paths, value)?);
            config.agent = Some(agent);
        }
        ConfigKey::AgentEndpointUrl => {
            validate_agent_endpoint_syntax(value)?;
            let mut agent = config.agent.unwrap_or_default();
            agent.endpoint_url = Some(value.to_owned());
            config.agent = Some(agent);
        }
        ConfigKey::AgentAuthMode => {
            ManagedAgentAuthMode::parse(value)?;
            let mut agent = config.agent.unwrap_or_default();
            agent.auth_mode = Some(value.to_owned());
            config.agent = Some(agent);
        }
        ConfigKey::PublicApiToken => {
            let mut public = config.public.unwrap_or_default();
            public.api_token_ref = Some(store_local_public_api_token(paths, value)?);
            config.public = Some(public);
        }
    }
    Ok(config)
}

pub fn clear_runx_agent_endpoint_url(mut config: RunxConfigFile) -> RunxConfigFile {
    if let Some(agent) = config.agent.as_mut() {
        agent.endpoint_url = None;
    }
    config
}

pub fn lookup_runx_config_value(config: &RunxConfigFile, key: ConfigKey) -> Option<String> {
    match key {
        ConfigKey::AgentProvider => config.agent.as_ref()?.provider.clone(),
        ConfigKey::AgentDialect => config.agent.as_ref()?.dialect.clone(),
        ConfigKey::AgentModel => config.agent.as_ref()?.model.clone(),
        ConfigKey::AgentEndpointUrl => config.agent.as_ref()?.endpoint_url.clone(),
        ConfigKey::AgentAuthMode => config.agent.as_ref()?.auth_mode.clone(),
        ConfigKey::AgentApiKey => config
            .agent
            .as_ref()?
            .api_key_ref
            .as_ref()
            .map(|_| "[encrypted]".to_owned()),
        ConfigKey::PublicApiToken => config
            .public
            .as_ref()?
            .api_token_ref
            .as_ref()
            .map(|_| "[encrypted]".to_owned()),
    }
}

pub fn mask_runx_config_file(config: &RunxConfigFile) -> RunxConfigFile {
    let mut masked = config.clone();
    if let Some(agent) = masked.agent.as_mut()
        && agent.api_key_ref.is_some()
    {
        agent.api_key_ref = Some("[encrypted]".to_owned());
    }
    if let Some(public) = masked.public.as_mut()
        && public.api_token_ref.is_some()
    {
        public.api_token_ref = Some("[encrypted]".to_owned());
    }
    if let Some(public) = masked.public.as_mut()
        && public.publish_api_token_ref.is_some()
    {
        public.publish_api_token_ref = Some("[encrypted]".to_owned());
    }
    if let Some(credentials) = masked.credentials.as_mut() {
        for profile in credentials.profiles.values_mut() {
            profile.secret_ref = "[encrypted]".to_owned();
        }
    }
    masked
}

pub fn load_local_agent_api_key(paths: &RunxPaths, key_ref: &str) -> Result<String, ConfigError> {
    load_local_config_secret_value(paths, key_ref)
}

pub fn load_local_public_api_token(
    paths: &RunxPaths,
    token_ref: &str,
) -> Result<String, ConfigError> {
    load_local_config_secret_value(paths, token_ref)
}

pub fn store_local_credential_secret(
    paths: &RunxPaths,
    value: &str,
) -> Result<String, ConfigError> {
    store_local_config_secret_value(paths, value, "local_credential")
}

pub fn load_local_credential_secret(
    paths: &RunxPaths,
    secret_ref: &str,
) -> Result<String, ConfigError> {
    load_local_config_secret_value(paths, secret_ref)
}

pub fn remove_local_credential_secret(
    paths: &RunxPaths,
    secret_ref: &str,
) -> Result<(), ConfigError> {
    let path = paths.keys().join(format!("{secret_ref}.json"));
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ConfigError::Io(error)),
    }
}

fn load_local_config_secret_value(paths: &RunxPaths, key_ref: &str) -> Result<String, ConfigError> {
    let key_path = paths.keys().join(format!("{key_ref}.json"));
    let payload = load_key_payload(&key_path)?;
    if payload.alg != "aes-256-gcm" {
        return Err(config_key_read_error(&key_path, None));
    }
    let secret = load_or_create_local_config_secret(&paths.keys())?;
    let key = Sha256::digest(secret.as_bytes());
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|error| ConfigError::Crypto(error.to_string()))?;
    let nonce_bytes = decode_key_part(&key_path, &payload.iv)?;
    let ciphertext = decode_key_part(&key_path, &payload.ciphertext)?;
    let auth_tag = decode_key_part(&key_path, &payload.auth_tag)?;
    let mut sealed = ciphertext;
    sealed.extend(auth_tag);
    let nonce = config_nonce(&key_path, &nonce_bytes)?;
    let plaintext = cipher
        .decrypt(&nonce, sealed.as_ref())
        .map_err(|error| config_key_read_error(&key_path, Some(error.to_string())))?;
    String::from_utf8(plaintext)
        .map_err(|error| config_key_read_error(&key_path, Some(error.to_string())))
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
    let profile = model_provider(provider.as_str());
    let dialect = match env.get("RUNX_AGENT_DIALECT").or_else(|| {
        config
            .agent
            .as_ref()
            .and_then(|agent| agent.dialect.as_ref())
    }) {
        Some(value) => ModelDialect::parse(value).ok_or(ConfigError::InvalidAgentSetting {
            key: "agent.dialect",
            message: "expected chat_completions or anthropic_messages",
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
        // A profile's default model is the model at its own endpoint; a
        // configured endpoint serves whatever models it serves, so it names
        // its own.
        .or_else(|| {
            profile
                .filter(|_| endpoint_url.is_none())
                .map(|profile| profile.default_model.to_owned())
        })
        .unwrap_or_default();
    if model.is_empty() {
        if auth_mode == ManagedAgentAuthMode::LocalNone {
            return Err(ConfigError::InvalidAgentSetting {
                key: "agent.model",
                message: "local_none requires a model",
            });
        }
        return Ok(None);
    }
    let provider_key = profile.and_then(|profile| env.get(profile.key_env));
    let mut api_key = env
        .get("RUNX_AGENT_API_KEY")
        .or(provider_key)
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    if auth_mode == ManagedAgentAuthMode::ApiKey
        && api_key.is_empty()
        && let Some(key_ref) = config
            .agent
            .as_ref()
            .and_then(|agent| agent.api_key_ref.as_ref())
            .filter(|value| !value.is_empty())
    {
        api_key = load_local_agent_api_key(&paths, key_ref)?.trim().to_owned();
    }
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

#[derive(Deserialize)]
struct LocalConfigSecretPayload {
    alg: String,
    iv: String,
    ciphertext: String,
    auth_tag: String,
}

#[derive(Serialize)]
struct StoredLocalConfigSecretPayload<'a> {
    #[serde(rename = "ref")]
    key_ref: &'a str,
    alg: &'static str,
    iv: String,
    ciphertext: String,
    auth_tag: String,
}

type ConfigNonce = Nonce<Aes256Gcm>;

fn config_nonce(key_path: &Path, nonce_bytes: &[u8]) -> Result<ConfigNonce, ConfigError> {
    ConfigNonce::try_from(nonce_bytes).map_err(|_| {
        config_key_read_error(
            key_path,
            Some(format!(
                "expected 12-byte aes-256-gcm nonce, found {} bytes",
                nonce_bytes.len()
            )),
        )
    })
}

fn random_config_nonce() -> Result<ConfigNonce, ConfigError> {
    ConfigNonce::try_generate().map_err(|error| ConfigError::Crypto(error.to_string()))
}

fn random_config_secret_bytes() -> Result<[u8; 32], ConfigError> {
    <[u8; 32]>::try_generate().map_err(|error| ConfigError::Crypto(error.to_string()))
}

fn store_local_agent_api_key(paths: &RunxPaths, api_key: &str) -> Result<String, ConfigError> {
    store_local_config_secret_value(paths, api_key, "local_agent_key")
}

pub(crate) fn store_local_public_api_token(
    paths: &RunxPaths,
    api_token: &str,
) -> Result<String, ConfigError> {
    store_local_config_secret_value(paths, api_token, "local_public_api_token")
}

fn store_local_config_secret_value(
    paths: &RunxPaths,
    value: &str,
    ref_prefix: &str,
) -> Result<String, ConfigError> {
    let key_dir = paths.keys();
    fs::create_dir_all(&key_dir)?;
    let secret = load_or_create_local_config_secret(&key_dir)?;
    let key = Sha256::digest(secret.as_bytes());
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|error| ConfigError::Crypto(error.to_string()))?;
    let nonce = random_config_nonce()?;
    let mut sealed = cipher
        .encrypt(&nonce, value.as_bytes())
        .map_err(|error| ConfigError::Crypto(error.to_string()))?;
    let auth_tag = sealed.split_off(sealed.len().saturating_sub(16));
    let key_ref = format!(
        "{ref_prefix}_{}",
        hex_prefix(
            &Sha256::digest([nonce.as_slice(), sealed.as_slice()].concat()),
            24
        )
    );
    let payload = StoredLocalConfigSecretPayload {
        key_ref: &key_ref,
        alg: "aes-256-gcm",
        iv: URL_SAFE_NO_PAD.encode(nonce),
        ciphertext: URL_SAFE_NO_PAD.encode(sealed),
        auth_tag: URL_SAFE_NO_PAD.encode(auth_tag),
    };
    let contents = serde_json::to_string_pretty(&payload)
        .map_err(|error| ConfigError::Crypto(error.to_string()))?;
    write_private_file(
        &key_dir.join(format!("{key_ref}.json")),
        format!("{contents}\n").as_bytes(),
    )?;
    Ok(key_ref)
}

fn load_or_create_local_config_secret(key_dir: &Path) -> Result<String, ConfigError> {
    fs::create_dir_all(key_dir)?;
    let key_path = key_dir.join("local-config-secret");
    match fs::read_to_string(&key_path) {
        Ok(secret) => Ok(secret),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let secret_bytes = random_config_secret_bytes()?;
            let secret = URL_SAFE_NO_PAD.encode(secret_bytes);
            match write_private_file_new(&key_path, secret.as_bytes()) {
                Ok(()) => Ok(secret),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    Ok(fs::read_to_string(&key_path)?)
                }
                Err(error) => Err(ConfigError::Io(error)),
            }
        }
        Err(error) => Err(ConfigError::Io(error)),
    }
}

fn load_key_payload(key_path: &Path) -> Result<LocalConfigSecretPayload, ConfigError> {
    let contents = fs::read_to_string(key_path)
        .map_err(|error| config_key_read_error(key_path, Some(error.to_string())))?;
    serde_json::from_str(&contents)
        .map_err(|error| config_key_read_error(key_path, Some(error.to_string())))
}

fn decode_key_part(key_path: &Path, value: &str) -> Result<Vec<u8>, ConfigError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|error| config_key_read_error(key_path, Some(error.to_string())))
}

fn config_key_read_error(path: &Path, cause: Option<String>) -> ConfigError {
    ConfigError::LocalAgentKeyCorrupt {
        path: path.to_path_buf(),
        suffix: cause.map_or_else(String::new, |message| format!(": {message}")),
    }
}

fn normalize_managed_agent_provider(value: &str) -> Option<NonEmptyString> {
    NonEmptyString::new(value.trim().to_lowercase())
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    set_private_permissions(path)?;
    Ok(())
}

fn write_private_file_new(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(path)?;
    file.write_all(contents)
}

fn set_private_permissions(path: &Path) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn hex_prefix(bytes: &[u8], len: usize) -> String {
    let mut value = String::new();
    for byte in bytes {
        value.push_str(&format!("{byte:02x}"));
    }
    value.chars().take(len).collect()
}
