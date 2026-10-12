//! Act result envelope returned across adapter boundaries.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

use crate::schema::RunxSchema;
use crate::{JsonObject, ResolutionRequest};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActResultTerminalStatus {
    Sealed,
    Failure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ActResultNeedsAgentStatus {
    #[serde(rename = "needs_agent")]
    NeedsAgent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ActResultSignal {
    SIGABRT,
    SIGALRM,
    SIGBUS,
    SIGCHLD,
    SIGCONT,
    SIGFPE,
    SIGHUP,
    SIGILL,
    SIGINT,
    SIGIO,
    SIGIOT,
    SIGKILL,
    SIGPIPE,
    SIGPOLL,
    SIGPROF,
    SIGPWR,
    SIGQUIT,
    SIGSEGV,
    SIGSTKFLT,
    SIGSTOP,
    SIGSYS,
    SIGTERM,
    SIGTRAP,
    SIGTSTP,
    SIGTTIN,
    SIGTTOU,
    SIGUNUSED,
    SIGURG,
    SIGUSR1,
    SIGUSR2,
    SIGVTALRM,
    SIGWINCH,
    SIGXCPU,
    SIGXFSZ,
    SIGBREAK,
    SIGLOST,
    SIGINFO,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActResultNull;

impl Serialize for ActResultNull {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_none()
    }
}

impl<'de> Deserialize<'de> for ActResultNull {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<Value>::deserialize(deserializer)?
            .is_none()
            .then_some(Self)
            .ok_or_else(|| serde::de::Error::custom("field must be null"))
    }
}

impl RunxSchema for ActResultNull {
    fn json_schema() -> Value {
        json!({ "type": "null" })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActResultTerminalEnvelope {
    pub status: ActResultTerminalStatus,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i64>,
    pub signal: Option<ActResultSignal>,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActResultNeedsAgentEnvelope {
    pub status: ActResultNeedsAgentStatus,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: ActResultNull,
    pub signal: ActResultNull,
    pub duration_ms: u64,
    pub request: ResolutionRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(untagged)]
#[runx_schema(id = "runx.act_result.v1")]
pub enum ActResultEnvelope {
    Terminal(ActResultTerminalEnvelope),
    NeedsAgent(Box<ActResultNeedsAgentEnvelope>),
}
