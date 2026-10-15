pub mod build;
pub(crate) mod dispatch;
pub mod error;
pub mod inspect;
pub(crate) mod manifest;
pub(crate) mod native;
pub(crate) mod projection;
pub mod search;

pub use build::{ToolBuildOptions, build_tool_catalogs};
pub use error::ToolCatalogError;
pub use inspect::{
    LocalToolResolution, ToolInspectOptions, inspect_tool, inspect_tool_with_effects,
    resolve_local_tool,
};
pub use search::{ToolSearchOptions, search_tools, search_tools_with_effects};

/// The call a tool is invoked for: the key its run sends it under, the same
/// each time the call is sent, and the actor that run acts for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolCall {
    /// `<run id>#<step id>@<attempt>`.
    pub(crate) key: String,
    /// The run authority's actor; none for an operator's run.
    pub(crate) actor: Option<String>,
}

impl ToolCall {
    /// The call `step_id`'s `attempt` makes in run `run_id`, for the actor
    /// `authority` names.
    pub(crate) fn new(
        run_id: &str,
        step_id: &str,
        attempt: u32,
        authority: Option<&crate::RunAuthority>,
    ) -> Self {
        Self {
            key: format!("{run_id}#{step_id}@{attempt}"),
            actor: authority.map(|authority| authority.actor_ref.uri.as_str().to_owned()),
        }
    }
}
