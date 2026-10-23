//! What the native command tool plans and returns.

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::schema::{BoundedU64, BoundedVec, NonEmptyString, RunxSchema};
use crate::{Digest, JsonValue};

/// The shortest timeout a command may ask for.
pub const COMMAND_MIN_TIMEOUT_MS: u64 = 1_000;
/// The longest timeout a command may ask for.
pub const COMMAND_MAX_TIMEOUT_MS: u64 = 3_600_000;
/// The most environment variables a command may set.
pub const COMMAND_MAX_ENV: usize = 64;
/// The most bytes of each output stream a command's execution keeps.
pub const COMMAND_OUTPUT_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// The most errors one execution reports.
pub const COMMAND_MAX_ERRORS: usize = 4;
/// What a process environment variable's name may be.
pub const PROCESS_ENV_NAME_PATTERN: &str = "^[A-Z_][A-Z0-9_]*$";

/// A process environment variable's name: an uppercase letter or an
/// underscore, then uppercase letters, digits and underscores.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ProcessEnvName(String);

impl ProcessEnvName {
    /// Returns `None` when the value is not a process environment name.
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let mut chars = value.chars();
        let valid = matches!(chars.next(), Some('_' | 'A'..='Z'))
            && chars.all(|character| matches!(character, '_' | 'A'..='Z' | '0'..='9'));
        valid.then_some(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ProcessEnvName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("not a process environment name"))
    }
}

impl RunxSchema for ProcessEnvName {
    fn json_schema() -> Value {
        json!({ "type": "string", "pattern": PROCESS_ENV_NAME_PATTERN })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum CommandPlanSchema {
    #[serde(rename = "runx.command.plan.v1")]
    V1,
}

/// How a command's output is returned: its digest only, as text, or parsed
/// as JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutputMode {
    Digest,
    Text,
    Json,
}

impl CommandOutputMode {
    /// Its wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Digest => "digest",
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}

/// What `command.execute` would run, before it runs: the command's identity
/// and the bounds it runs under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.command.plan.v1")]
pub struct CommandPlan {
    pub schema: CommandPlanSchema,
    pub command_digest: Digest,
    /// The directory it runs in, relative to the workspace.
    pub cwd: NonEmptyString,
    pub timeout_ms: BoundedU64<COMMAND_MIN_TIMEOUT_MS, COMMAND_MAX_TIMEOUT_MS>,
    pub output_mode: CommandOutputMode,
    /// The names of the variables it sets, never their values.
    pub env_names: BoundedVec<ProcessEnvName, 0, COMMAND_MAX_ENV>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum CommandExecutionSchema {
    #[serde(rename = "runx.command.execution.v1")]
    V1,
}

/// How an execution ended: it completed, it failed, or it was stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandDecision {
    Completed,
    Failed,
    Stopped,
}

/// What `command.execute` returns: how the command ended, its bounded output
/// and the digest of each stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.command.execution.v1")]
pub struct CommandExecution {
    pub schema: CommandExecutionSchema,
    pub decision: CommandDecision,
    pub command_digest: Digest,
    pub cwd: NonEmptyString,
    pub exit_code: Option<i64>,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// The stream's text in `text` mode; empty otherwise.
    pub stdout: String,
    pub stderr: String,
    pub stdout_digest: Digest,
    pub stderr_digest: Digest,
    pub stdout_bytes: BoundedU64<0, COMMAND_OUTPUT_MAX_BYTES>,
    pub stderr_bytes: BoundedU64<0, COMMAND_OUTPUT_MAX_BYTES>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// The parsed output in `json` mode; null otherwise.
    pub json: JsonValue,
    pub errors: BoundedVec<CommandError, 0, COMMAND_MAX_ERRORS>,
}

/// One reason an execution failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandError {
    pub code: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
}
