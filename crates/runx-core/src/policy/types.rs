use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LocalExecutionPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict_process_tool_inline_code: Option<bool>,
}
