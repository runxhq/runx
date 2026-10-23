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

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::AnthropicMessages => "anthropic_messages",
            Self::OpenAiResponses => "openai_responses",
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
        _ => Err(ConfigError::UnsupportedKey {
            key: key.to_owned(),
        }),
    }
}

/// The home's config file, upgraded first when it was written in an earlier
/// shape.
pub fn load_runx_config_file(config_path: &Path) -> Result<RunxConfigFile, ConfigError> {
    let contents = match fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RunxConfigFile::default());
        }
        Err(error) => return Err(ConfigError::Io(error)),
    };
    let mut value =
        serde_json::from_str::<Value>(&contents).map_err(|error| ConfigError::InvalidJson {
            path: config_path.to_path_buf(),
            message: error.to_string(),
        })?;
    let Some(object) = value.as_object_mut() else {
        return Err(ConfigError::NonObjectJson {
            path: config_path.to_path_buf(),
        });
    };
    let reshaped = upgrade_to_secrets(object, config_path)?;
    let renamed = upgrade_material_names(object);
    let upgraded = reshaped || renamed;
    let config: RunxConfigFile =
        serde_json::from_value(value).map_err(|error| ConfigError::InvalidConfig {
            path: config_path.to_path_buf(),
            message: error.to_string(),
        })?;
    if upgraded {
        write_runx_config_file(config_path, &config)?;
    }
    Ok(config)
}

/// Rename the sealed-material ids a home written by runx 0.10 names as
/// references: `secrets.<name>.stored.material_ref`, `public.api_token_ref`
/// and `public.publish_api_token_ref`. Only keys move, and a key whose new
/// name is already present stays, so the typed read refuses the file rather
/// than choose between two ids. `false` when nothing was renamed.
fn upgrade_material_names(config: &mut serde_json::Map<String, Value>) -> bool {
    let rename = |object: &mut serde_json::Map<String, Value>, from: &str, to: &str| {
        if object.contains_key(to) {
            return false;
        }
        let Some(value) = object.remove(from) else {
            return false;
        };
        object.insert(to.to_owned(), value);
        true
    };
    let mut renamed = false;
    if let Some(public) = config.get_mut("public").and_then(Value::as_object_mut) {
        renamed |= rename(public, "api_token_ref", "api_token_material_id");
        renamed |= rename(
            public,
            "publish_api_token_ref",
            "publish_api_token_material_id",
        );
    }
    if let Some(secrets) = config.get_mut("secrets").and_then(Value::as_object_mut) {
        for record in secrets.values_mut() {
            if let Some(stored) = record.get_mut("stored").and_then(Value::as_object_mut) {
                renamed |= rename(stored, "material_ref", "material_id");
            }
        }
    }
    renamed
}

/// The home file before `runx secret`: connector credential profiles, each
/// a sealed file, and the managed agent's own key.
#[derive(Deserialize)]
struct LegacyCredentials {
    #[serde(default)]
    profiles: BTreeMap<String, LegacyProfile>,
}

#[derive(Deserialize)]
struct LegacyProfile {
    secret_ref: String,
}

/// Reshape a home file written before `runx secret`. Each credential profile
/// becomes the secret of its name over the same sealed file. The managed
/// agent's own key, stored or its reference and pin, becomes the secret
/// `agent`, or the first `agent-<n>` no profile holds, and `agent.secret`
/// names it. Nothing is read or sealed again: only records move, so the
/// upgrade gives the same file however often it runs. `false` when the file
/// has nothing to reshape.
fn upgrade_to_secrets(
    config: &mut serde_json::Map<String, Value>,
    path: &Path,
) -> Result<bool, ConfigError> {
    let invalid = |message: String| ConfigError::InvalidConfig {
        path: path.to_path_buf(),
        message,
    };
    let profiles = config
        .remove("credentials")
        .map(serde_json::from_value::<LegacyCredentials>)
        .transpose()
        .map_err(|error| invalid(format!("credentials: {error}")))?;
    let mut key = BTreeMap::new();
    if let Some(agent) = config.get_mut("agent").and_then(Value::as_object_mut) {
        for field in ["api_key_ref", "api_key_from", "api_key_pinned"] {
            if let Some(value) = agent.remove(field) {
                key.insert(field, value);
            }
        }
    }
    if profiles.is_none() && key.is_empty() {
        return Ok(false);
    }
    let text = |field: &str| {
        key.get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let agent_key = match (text("api_key_from"), text("api_key_ref")) {
        (Some(reference), _) => Some(serde_json::json!({
            "reference": {"reference": reference, "pinned": text("api_key_pinned")}
        })),
        (None, Some(material_id)) => {
            Some(serde_json::json!({"stored": {"material_id": material_id}}))
        }
        (None, None) => None,
    };
    let secrets = config
        .entry("secrets")
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| invalid("secrets is not an object".to_owned()))?;
    for (name, profile) in profiles.map(|held| held.profiles).unwrap_or_default() {
        secrets.insert(
            name,
            serde_json::json!({"stored": {"material_id": profile.secret_ref}}),
        );
    }
    if let Some(record) = agent_key {
        let name = std::iter::once("agent".to_owned())
            .chain((2..=secrets.len() + 1).map(|n| format!("agent-{n}")))
            .find(|name| !secrets.contains_key(name))
            .ok_or_else(|| invalid("no free name for the agent's key".to_owned()))?;
        secrets.insert(name.clone(), record);
        if let Some(agent) = config.get_mut("agent").and_then(Value::as_object_mut) {
            agent.insert("secret".to_owned(), Value::String(name));
        }
    }
    Ok(true)
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
            ManagedAgentAuthMode::parse(value)?;
            let mut agent = config.agent.unwrap_or_default();
            agent.auth_mode = Some(value.to_owned());
            config.agent = Some(agent);
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

pub fn clear_runx_agent_dialect(mut config: RunxConfigFile) -> RunxConfigFile {
    if let Some(agent) = config.agent.as_mut() {
        agent.dialect = None;
    }
    config
}

/// The managed agent no longer names a secret; the secret itself stays.
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

pub(crate) fn normalize_managed_agent_provider(value: &str) -> Option<NonEmptyString> {
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

    /// A home from before `runx secret` keeps every value: its profiles and
    /// the agent's key become secrets over the same sealed files.
    #[test]
    fn a_home_written_before_secrets_is_upgraded_once_and_keeps_every_value()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let path = home.path().join("config.json");
        fs::write(
            &path,
            r#"{
              "agent": {"provider": "anthropic", "model": "m", "api_key_ref": "local_agent_key_1"},
              "credentials": {"profiles": {
                "agent": {"connector": "x", "credential_declaration_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000", "credential": "token", "secret_ref": "local_credential_2"},
                "github": {"connector": "github", "credential_declaration_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000", "credential": "token", "secret_ref": "local_credential_3"}
              }}
            }"#,
        )?;

        let config = load_runx_config_file(&path)?;

        let stored = |material_id: &str| RunxSecretRecord::Stored {
            material_id: material_id.to_owned(),
        };
        let named = |name: &str| SecretName::new(name).ok_or("a secret name");
        assert_eq!(
            config.secrets.get(&named("github")?),
            Some(&stored("local_credential_3"))
        );
        assert_eq!(
            config.secrets.get(&named("agent")?),
            Some(&stored("local_credential_2"))
        );
        assert_eq!(
            config.secrets.get(&named("agent-2")?),
            Some(&stored("local_agent_key_1"))
        );
        assert_eq!(
            config
                .agent
                .as_ref()
                .and_then(|agent| agent.secret.as_ref()),
            Some(&named("agent-2")?)
        );
        let written = fs::read_to_string(&path)?;
        assert!(
            !written.contains("credentials") && !written.contains("api_key_ref"),
            "{written}"
        );
        assert_eq!(
            load_runx_config_file(&path)?,
            config,
            "upgraded once, read as it is after"
        );
        Ok(())
    }

    /// A home runx 0.10 wrote names its sealed material as references; its
    /// secrets and logins keep their sealed files under the 0.11 names.
    #[test]
    fn a_home_runx_010_wrote_keeps_its_secrets_and_logins_under_material_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let path = home.path().join("config.json");
        fs::write(
            &path,
            r#"{
              "public": {"api_token_ref": "public_api_token_1", "publish_api_token_ref": "public_api_token_2", "principal_id": "runx:principal:operator"},
              "secrets": {"github": {"stored": {"material_ref": "local_secret_3"}}, "vault": {"reference": {"reference": "aws-sm://team/env#KEY"}}}
            }"#,
        )?;

        let config = load_runx_config_file(&path)?;

        let public = config.public.as_ref().ok_or("public settings")?;
        assert_eq!(
            public.api_token_material_id.as_deref(),
            Some("public_api_token_1")
        );
        assert_eq!(
            public.publish_api_token_material_id.as_deref(),
            Some("public_api_token_2")
        );
        assert_eq!(
            config
                .secrets
                .get(&SecretName::new("github").ok_or("a secret name")?),
            Some(&RunxSecretRecord::Stored {
                material_id: "local_secret_3".to_owned()
            })
        );
        let written = fs::read_to_string(&path)?;
        assert!(!written.contains("_ref\""), "{written}");
        assert_eq!(
            load_runx_config_file(&path)?,
            config,
            "upgraded once, read as it is after"
        );
        Ok(())
    }

    #[test]
    fn an_agent_reference_becomes_a_secret_with_its_pin() -> Result<(), Box<dyn std::error::Error>>
    {
        let home = tempfile::tempdir()?;
        let path = home.path().join("config.json");
        fs::write(
            &path,
            r#"{"agent": {"api_key_from": "aws-sm://team/env#KEY", "api_key_pinned": "arn:aws:secretsmanager:eu-west-2:1:secret:team/env-Ab"}}"#,
        )?;

        let config = load_runx_config_file(&path)?;

        assert_eq!(
            config
                .secrets
                .get(&SecretName::new("agent").ok_or("a secret name")?),
            Some(&RunxSecretRecord::Reference {
                reference: SecretReference::new("aws-sm://team/env#KEY").ok_or("a reference")?,
                pinned: Some("arn:aws:secretsmanager:eu-west-2:1:secret:team/env-Ab".to_owned()),
            })
        );
        Ok(())
    }
}
