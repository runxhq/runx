pub(crate) mod dispatch;
pub mod error;
pub mod inspect;
pub(crate) mod manifest;
pub(crate) mod native;
pub(crate) mod projection;
pub mod search;

pub use error::ToolCatalogError;
pub use inspect::{
    LocalToolResolution, ToolInspectOptions, inspect_tool_with_effects, resolve_local_tool,
};
pub use search::{ToolSearchOptions, search_tools_with_effects};

/// The call a tool is invoked for: the key its run sends it under, the same
/// each time the call is sent, and the actor that run acts for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolCall {
    /// The run that owns this call; retained as structured context, not parsed from key.
    pub(crate) run_id: String,
    /// `<run id>#<step id>@<attempt>`.
    pub(crate) key: String,
    /// The run authority's actor; none for an operator's run, or for one
    /// whose authority only bounds it.
    pub(crate) actor: Option<String>,
    /// Where what the call is given may be shared: its inputs, the artifacts
    /// they name, and the earlier outputs they bind.
    pub(crate) given: runx_contracts::ArtifactLabels,
}

impl ToolCall {
    /// The call `step_id`'s `attempt` makes in run `run_id`, for the actor
    /// `authority` names, given what `given` labels.
    pub(crate) fn new(
        run_id: &str,
        step_id: &str,
        attempt: u32,
        authority: Option<&crate::RunAuthority>,
        given: runx_contracts::ArtifactLabels,
    ) -> Self {
        Self {
            run_id: run_id.to_owned(),
            key: format!("{run_id}#{step_id}@{attempt}"),
            actor: authority
                .and_then(|authority| authority.actor_ref.as_ref())
                .map(|actor| actor.uri.as_str().to_owned()),
            given,
        }
    }
}
