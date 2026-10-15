use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use runx_contracts::JsonObject;

use crate::effects::RuntimeEffectRegistry;
use crate::execution::skill_front::TurnSession;

#[derive(Clone, Debug)]
pub struct McpServerOptions {
    pub package_name: String,
    pub package_version: String,
    /// What the server tells a host when it connects.
    pub instructions: Option<String>,
    /// The effects every tool runs with: the ones `runx skill` runs with.
    pub effects: RuntimeEffectRegistry,
    pub tools: Vec<McpServerTool>,
}

#[derive(Clone, Debug, Default)]
pub struct McpServerExecutionOptions {
    pub runner: Option<String>,
    pub receipt_dir: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub effects: RuntimeEffectRegistry,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpServerTool {
    pub name: String,
    pub description: String,
    pub input_schema: JsonObject,
    pub result: McpServerToolBehavior,
}

#[derive(Clone, Debug, PartialEq)]
pub enum McpServerToolBehavior {
    Fixed(McpToolResult),
    Skill(Box<McpServerSkillExecution>),
    Turn(McpTurnTool),
}

/// A tool of a run's open turns: what calling it does, on which runs.
#[derive(Clone, Debug)]
pub struct McpTurnTool {
    pub runs: McpTurnRuns,
    pub action: McpTurnAction,
}

/// Which runs' turns a turn tool works.
#[derive(Clone, Debug)]
pub enum McpTurnRuns {
    /// The one run `runx mcp --run` serves.
    Bound(Arc<TurnSession>),
    /// A run the call names, among those the skill server's own calls
    /// opened: a turn its own skill call opened is worked on the same
    /// connection.
    Named(Arc<NamedRuns>),
}

/// The runs a skill server's own calls opened. Each is worked through one
/// session, its one driver, however many calls work it at once.
#[derive(Debug)]
pub struct NamedRuns {
    pub(super) execution: McpServerExecutionOptions,
    pub(super) sessions: std::sync::Mutex<BTreeMap<String, std::sync::Weak<TurnSession>>>,
}

impl NamedRuns {
    #[must_use]
    pub fn new(execution: McpServerExecutionOptions) -> Self {
        Self {
            execution,
            sessions: std::sync::Mutex::new(BTreeMap::new()),
        }
    }
}

/// The fixed tools a turn session serves; each takes a turn's address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpTurnAction {
    List,
    Status,
    Call,
    Complete,
}

impl PartialEq for McpTurnTool {
    fn eq(&self, other: &Self) -> bool {
        let same_runs = match (&self.runs, &other.runs) {
            (McpTurnRuns::Bound(left), McpTurnRuns::Bound(right)) => Arc::ptr_eq(left, right),
            (McpTurnRuns::Named(left), McpTurnRuns::Named(right)) => Arc::ptr_eq(left, right),
            _ => false,
        };
        same_runs && self.action == other.action
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpServerSkillExecution {
    pub skill_path: PathBuf,
    pub skill_name: String,
    pub runner: String,
    pub package_digest: String,
    pub execution_closure_digest: String,
    pub receipt_dir: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    pub structured_content: Option<JsonObject>,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpContent {
    pub text: String,
}

#[derive(Debug)]
pub struct McpServerError {
    message: String,
}

impl McpServerError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for McpServerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for McpServerError {}
