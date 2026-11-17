// Module rationale: the runx home's config file is one unit: its format, its
// one-time upgrades and the settings it holds. Secret values live in
// `secrets`, which names them in this file's `secrets` section.
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use runx_contracts::connector::{SecretName, SecretReference};
use runx_contracts::schema::NonEmptyString;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::credentials::SecretString;
use crate::private_file::{self, PrivateFileFault};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunxConfigFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<RunxAgentConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public: Option<RunxPublicConfig>,
    /// The secrets the operator keeps in runx, by name (`runx secret`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<SecretName, RunxSecretRecord>,
}

/// Where a secret's one value is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RunxSecretRecord {
    /// Sealed in this home under `material_id`.
    Stored { material_id: String },
    /// In a secrets manager at `reference`, read through its CLI on every
    /// use and only where setting it pinned (an AWS ARN, a Vault address).
    Reference {
        reference: SecretReference,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pinned: Option<String>,
    },
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
    /// The secret holding the model key (`runx config set agent.secret`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
    /// How many more times a transient model failure is asked again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    /// The first ask's output cap, in tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
    /// The most a reply cut at its cap is given on asking again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// One model request's time limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_timeout_seconds: Option<u32>,
    /// The rounds a driven turn has, unless a run says otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rounds: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunxPublicConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,
    /// Default operator credential used by connect, provider effects, and
    /// hosted artifact operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_token_material_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    /// Narrow credential used only by receipt and skill publication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_api_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_api_token_material_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_principal_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigKey {
    AgentProvider,
    AgentDialect,
    AgentModel,
    AgentSecret,
    AgentEndpointUrl,
    AgentAuthMode,
    Drive(DriveSetting),
}

/// One of the operator's settings for how a driven turn holds up
/// (contracts/driver.md): its key, its variable and its place in the config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveSetting {
    Retries,
    OutputTokens,
    MaxOutputTokens,
    RequestTimeoutSeconds,
    Rounds,
}

impl DriveSetting {
    pub const ALL: [Self; 5] = [
        Self::Retries,
        Self::OutputTokens,
        Self::MaxOutputTokens,
        Self::RequestTimeoutSeconds,
        Self::Rounds,
    ];

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Retries => "agent.retries",
            Self::OutputTokens => "agent.output_tokens",
            Self::MaxOutputTokens => "agent.max_output_tokens",
            Self::RequestTimeoutSeconds => "agent.request_timeout_seconds",
            Self::Rounds => "agent.rounds",
        }
    }

    #[must_use]
    pub const fn variable(self) -> &'static str {
        match self {
            Self::Retries => "RUNX_AGENT_RETRIES",
            Self::OutputTokens => "RUNX_AGENT_OUTPUT_TOKENS",
            Self::MaxOutputTokens => "RUNX_AGENT_MAX_OUTPUT_TOKENS",
            Self::RequestTimeoutSeconds => "RUNX_AGENT_REQUEST_TIMEOUT_SECONDS",
            Self::Rounds => "RUNX_AGENT_ROUNDS",
        }
    }

    fn slot(self, agent: &mut RunxAgentConfig) -> &mut Option<u32> {
        match self {
            Self::Retries => &mut agent.retries,
            Self::OutputTokens => &mut agent.output_tokens,
            Self::MaxOutputTokens => &mut agent.max_output_tokens,
            Self::RequestTimeoutSeconds => &mut agent.request_timeout_seconds,
            Self::Rounds => &mut agent.rounds,
        }
    }

    #[must_use]
    pub fn get(self, agent: &RunxAgentConfig) -> Option<u32> {
        *self.slot(&mut agent.clone())
    }

    fn set(self, agent: &mut RunxAgentConfig, value: Option<u32>) {
        *self.slot(agent) = value;
    }

    /// The value a setting reads as: a whole number, or its words why not.
    pub fn parse(self, value: &str) -> Result<u32, ConfigError> {
        value
            .trim()
            .parse::<u32>()
            .map_err(|_| ConfigError::InvalidAgentSetting {
                key: self.key(),
                message: "expected a whole number",
            })
    }
}

/// A model API's wire format. Dialects are code; providers are data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelDialect {
    /// The chat completions format most providers and local servers speak.
    ChatCompletions,
    AnthropicMessages,
    /// OpenAI's Responses API, which its current models take tools through.
    OpenAiResponses,
}

impl ModelDialect {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "chat_completions" => Some(Self::ChatCompletions),
            "anthropic_messages" => Some(Self::AnthropicMessages),
            "openai_responses" => Some(Self::OpenAiResponses),
            _ => None,
        }
    }
}

/// A provider runx knows: the dialect it speaks, where, and the variable that
/// holds its key. Every provider names its model in config; any other provider
/// also names its dialect and endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelProvider {
    pub id: &'static str,
    pub dialect: ModelDialect,
    pub endpoint: &'static str,
    pub key_env: &'static str,
}

pub const MODEL_PROVIDERS: &[ModelProvider] = &[
    ModelProvider {
        id: "anthropic",
        dialect: ModelDialect::AnthropicMessages,
        endpoint: "https://api.anthropic.com/v1/messages",
        key_env: "ANTHROPIC_API_KEY",
    },
    ModelProvider {
        id: "openai",
        dialect: ModelDialect::OpenAiResponses,
        endpoint: "https://api.openai.com/v1/responses",
        key_env: "OPENAI_API_KEY",
    },
    ModelProvider {
        id: "gemini",
        dialect: ModelDialect::ChatCompletions,
        endpoint: "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
        key_env: "GEMINI_API_KEY",
    },
];

/// The built-in profile of provider `id`.
#[must_use]
pub fn model_provider(id: &str) -> Option<&'static ModelProvider> {
    MODEL_PROVIDERS.iter().find(|provider| provider.id == id)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveConfig {
    /// The provider's id: a built-in profile, or a name the operator gave a
    /// provider configured by dialect and endpoint.
    pub provider: NonEmptyString,
    pub dialect: ModelDialect,
    pub model: String,
    pub api_key: Option<SecretString>,
    pub endpoint_url: String,
    pub auth_mode: DriveAuthMode,
    pub limits: DriveLimits,
}

/// How a driven turn holds up against its model, as the operator set it
/// (contracts/driver.md): how many more times a transient failure is asked
/// again, the first ask's output cap and the most a cut reply is given, one
/// request's time limit, and how many rounds a turn has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriveLimits {
    pub retries: u32,
    pub output_tokens: u32,
    pub max_output_tokens: u32,
    pub request_timeout: std::time::Duration,
    pub rounds: u32,
}

impl Default for DriveLimits {
    fn default() -> Self {
        Self {
            retries: 4,
            output_tokens: 8192,
            max_output_tokens: 32_768,
            request_timeout: crate::http::DRIVE_REQUEST_TIMEOUT,
            rounds: 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveAuthMode {
    ApiKey,
    LocalNone,
}

impl DriveAuthMode {
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

pub(crate) fn validate_agent_endpoint_syntax(value: &str) -> Result<(), ConfigError> {
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
    #[error("{path} is not a runx config: {message}")]
    InvalidConfig { path: PathBuf, message: String },
    #[error("unsupported runx config key {key}")]
    UnsupportedKey { key: String },
    #[error("invalid {key}: {message}")]
    InvalidAgentSetting {
        key: &'static str,
        message: &'static str,
    },
    #[error("no secret is named {name}; set it with `runx secret set {name}`")]
    UnknownSecret { name: String },
    #[error("the model key is set {sources} with different values; keep one")]
    ConflictingAgentKeys { sources: String },
    #[error("model key: {message}")]
    AgentKeyRead { message: String },
    #[error("{path} {fault}")]
    PrivateFile {
        path: PathBuf,
        fault: PrivateFileFault,
    },
    #[error(transparent)]
    Io(#[from] io::Error),
}

fn private(path: &Path) -> impl FnOnce(PrivateFileFault) -> ConfigError + '_ {
    move |fault| ConfigError::PrivateFile {
        path: path.to_path_buf(),
        fault,
    }
}

pub fn parse_config_key(key: &str) -> Result<ConfigKey, ConfigError> {
    match key {
        "agent.provider" => Ok(ConfigKey::AgentProvider),
        "agent.dialect" => Ok(ConfigKey::AgentDialect),
        "agent.model" => Ok(ConfigKey::AgentModel),
        "agent.secret" => Ok(ConfigKey::AgentSecret),
        "agent.endpoint_url" => Ok(ConfigKey::AgentEndpointUrl),
        "agent.auth_mode" => Ok(ConfigKey::AgentAuthMode),
        key => DriveSetting::ALL
            .into_iter()
            .find(|setting| setting.key() == key)
            .map(ConfigKey::Drive)
            .ok_or_else(|| ConfigError::UnsupportedKey {
                key: key.to_owned(),
            }),
    }
}

/// The home's config file, read in the one shape this runx writes.
pub fn load_runx_config_file(config_path: &Path) -> Result<RunxConfigFile, ConfigError> {
    let contents = match fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RunxConfigFile::default());
        }
        Err(error) => return Err(ConfigError::Io(error)),
    };
    let value =
        serde_json::from_str::<Value>(&contents).map_err(|error| ConfigError::InvalidJson {
            path: config_path.to_path_buf(),
            message: error.to_string(),
        })?;
    if !value.is_object() {
        return Err(ConfigError::NonObjectJson {
            path: config_path.to_path_buf(),
        });
    }
    serde_json::from_value(value).map_err(|error| ConfigError::InvalidConfig {
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
    private_file::replace_private(config_path, format!("{contents}\n").as_bytes())
        .map_err(private(config_path))
}

pub fn update_runx_config_value(
    mut config: RunxConfigFile,
    key: ConfigKey,
    value: &str,
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
                message: "expected chat_completions, anthropic_messages or openai_responses",
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
        ConfigKey::AgentSecret => {
            let name = SecretName::new(value).ok_or(ConfigError::InvalidAgentSetting {
                key: "agent.secret",
                message: "is not a secret name",
            })?;
            if !config.secrets.contains_key(&name) {
                return Err(ConfigError::UnknownSecret {
                    name: name.into_string(),
                });
            }
            let mut agent = config.agent.unwrap_or_default();
            agent.secret = Some(name);
            config.agent = Some(agent);
        }
        ConfigKey::AgentEndpointUrl => {
            validate_agent_endpoint_syntax(value)?;
            let mut agent = config.agent.unwrap_or_default();
            agent.endpoint_url = Some(value.to_owned());
            config.agent = Some(agent);
        }
        ConfigKey::AgentAuthMode => {
            DriveAuthMode::parse(value)?;
            let mut agent = config.agent.unwrap_or_default();
            agent.auth_mode = Some(value.to_owned());
            config.agent = Some(agent);
        }
        ConfigKey::Drive(setting) => {
            let mut agent = config.agent.unwrap_or_default();
            setting.set(&mut agent, Some(setting.parse(value)?));
            config.agent = Some(agent);
            // The settings bound one another, so the whole set is checked.
            crate::agent_config::drive_limits(&std::collections::BTreeMap::new(), &config)?;
        }
    }
    Ok(config)
}

/// The drive's setting is the default again.
pub fn clear_runx_drive_setting(
    mut config: RunxConfigFile,
    setting: DriveSetting,
) -> RunxConfigFile {
    if let Some(agent) = config.agent.as_mut() {
        setting.set(agent, None);
    }
    config
}

pub fn clear_runx_agent_endpoint_url(mut config: RunxConfigFile) -> RunxConfigFile {
    if let Some(agent) = config.agent.as_mut() {
        agent.endpoint_url = None;
    }
    config
}

pub fn clear_runx_agent_dialect(mut config: RunxConfigFile) -> RunxConfigFile {
    if let Some(agent) = config.agent.as_mut() {
        agent.dialect = None;
    }
    config
}

/// The drive no longer names a secret; the secret itself stays.
pub fn clear_runx_agent_secret(mut config: RunxConfigFile) -> RunxConfigFile {
    if let Some(agent) = config.agent.as_mut() {
        agent.secret = None;
    }
    config
}

pub fn lookup_runx_config_value(config: &RunxConfigFile, key: ConfigKey) -> Option<String> {
    match key {
        ConfigKey::AgentProvider => config.agent.as_ref()?.provider.clone(),
        ConfigKey::AgentDialect => config.agent.as_ref()?.dialect.clone(),
        ConfigKey::AgentModel => config.agent.as_ref()?.model.clone(),
        ConfigKey::AgentSecret => config
            .agent
            .as_ref()?
            .secret
            .as_ref()
            .map(|name| name.as_str().to_owned()),
        ConfigKey::AgentEndpointUrl => config.agent.as_ref()?.endpoint_url.clone(),
        ConfigKey::AgentAuthMode => config.agent.as_ref()?.auth_mode.clone(),
        ConfigKey::Drive(setting) => setting
            .get(config.agent.as_ref()?)
            .map(|value| value.to_string()),
    }
}

pub fn mask_runx_config_file(config: &RunxConfigFile) -> RunxConfigFile {
    let mut masked = config.clone();
    if let Some(public) = masked.public.as_mut()
        && public.api_token_material_id.is_some()
    {
        public.api_token_material_id = Some("[encrypted]".to_owned());
    }
    if let Some(public) = masked.public.as_mut()
        && public.publish_api_token_material_id.is_some()
    {
        public.publish_api_token_material_id = Some("[encrypted]".to_owned());
    }
    for record in masked.secrets.values_mut() {
        if let RunxSecretRecord::Stored { material_id } = record {
            *material_id = "[encrypted]".to_owned();
        }
    }
    masked
}

pub(crate) fn normalize_drive_provider(value: &str) -> Option<NonEmptyString> {
    NonEmptyString::new(value.trim().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_of_the_wrong_shape_is_not_called_invalid_json()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let path = home.path().join("config.json");
        fs::write(&path, r#"{"agent":{"provider":7}}"#)?;
        let error = load_runx_config_file(&path)
            .err()
            .ok_or("a wrong shape must be refused")?;
        assert!(
            matches!(error, ConfigError::InvalidConfig { .. }),
            "{error}"
        );
        assert!(
            error.to_string().contains("is not a runx config"),
            "{error}"
        );
        Ok(())
    }

    /// A home from before `runx secret`, its credentials as profiles and the
    /// agent's key on the agent, is refused by the typed read, naming the
    /// file and the key it doesn't read, and is left as it was.
    #[test]
    fn a_home_from_before_runx_secret_is_refused_and_left_as_it_was()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        for (contents, key) in [
            (
                r#"{"credentials": {"profiles": {"github": {"connector": "github", "credential": "token", "secret_ref": "local_credential_3"}}}}"#,
                "credentials",
            ),
            (
                r#"{"agent": {"provider": "anthropic", "model": "m", "api_key_ref": "local_agent_key_1"}}"#,
                "api_key_ref",
            ),
            (
                r#"{"agent": {"api_key_from": "aws-sm://team/env#KEY", "api_key_pinned": "arn:aws:secretsmanager:eu-west-2:1:secret:team/env-Ab"}}"#,
                "api_key_from",
            ),
        ] {
            let path = home.path().join("config.json");
            fs::write(&path, contents)?;

            let refused = load_runx_config_file(&path)
                .err()
                .ok_or("the home is refused")?;

            let ConfigError::InvalidConfig { path: named, .. } = &refused else {
                return Err(format!("expected an invalid config, got {refused}").into());
            };
            assert_eq!(named, &path);
            assert!(refused.to_string().contains(key), "{refused}");
            assert_eq!(fs::read_to_string(&path)?, contents, "nothing is rewritten");
        }
        Ok(())
    }

    /// A home that names its sealed material by the `_ref` keys is refused,
    /// naming the file and the key it doesn't read, and is left as it was.
    #[test]
    fn a_home_naming_sealed_material_by_ref_is_refused_and_left_as_it_was()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let path = home.path().join("config.json");
        let contents = r#"{"public": {"api_token_ref": "public_api_token_1", "principal_id": "runx:principal:operator"}}"#;
        fs::write(&path, contents)?;

        let refused = load_runx_config_file(&path)
            .err()
            .ok_or("the home is refused")?;

        let ConfigError::InvalidConfig { path: named, .. } = &refused else {
            return Err(format!("expected an invalid config, got {refused}").into());
        };
        assert_eq!(named, &path);
        assert!(refused.to_string().contains("api_token_ref"), "{refused}");
        assert_eq!(fs::read_to_string(&path)?, contents, "nothing is rewritten");
        Ok(())
    }
}
