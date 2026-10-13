use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LocalExecutionPolicy {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strict_process_tool_inline_code: bool,
    /// A host that runs code it did not choose, such as a registry running a
    /// publisher's harness, starts no trusted host process: process tools,
    /// connector process tools and stdio MCP servers are refused.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refuse_host_processes: bool,
}
