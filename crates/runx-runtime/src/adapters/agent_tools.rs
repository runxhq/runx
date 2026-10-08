//! Recursive tool executor for the managed-agent loop.
//!
//! When the model chooses a tool, the agent invokes it through the governed
//! runtime. This reuses the canonical native-or-local dispatcher, so agent
//! tool calls get the same resolution, authority, credential
//! delivery, and artifact projection as graph tool steps.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use runx_contracts::JsonValue;
use runx_core::policy::admit_agent_tool_ref;

use super::agent_loop::ToolExecutor;
use crate::RuntimeError;
use crate::adapter::InvocationOutput;
use crate::credentials::CredentialDelivery;
use crate::effects::RuntimeEffectRegistry;
use crate::tool_catalogs::dispatch::{ToolDispatchRequest, dispatch_tool};

const MANAGED_AGENT_SKILL: &str = "managed-agent";

/// Executes the agent's chosen tools through the governed runtime, carrying the
/// run context (env, skill directory) the resolver captured from the agent
/// invocation. An agent's tools receive no credential; a connector operation
/// is where credentials are delivered.
pub struct RuntimeToolExecutor {
    env: BTreeMap<String, String>,
    skill_directory: PathBuf,
    effects: RuntimeEffectRegistry,
    observed_at: String,
    allowed_tools: BTreeSet<String>,
    scopes: Vec<String>,
    local_artifacts: crate::services::LocalArtifactService,
}

impl RuntimeToolExecutor {
    #[must_use]
    pub fn new(
        env: BTreeMap<String, String>,
        skill_directory: PathBuf,
        effects: RuntimeEffectRegistry,
        observed_at: impl Into<String>,
        allowed_tools: impl IntoIterator<Item = String>,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            env,
            skill_directory,
            effects,
            observed_at: observed_at.into(),
            allowed_tools: allowed_tools.into_iter().collect(),
            scopes,
            local_artifacts: crate::services::LocalArtifactService::default(),
        }
    }
}

impl ToolExecutor for RuntimeToolExecutor {
    fn admitted_tool_name(&self, tool: &str) -> Option<String> {
        (admit_agent_tool_ref(tool).allowed && self.allowed_tools.contains(tool))
            .then(|| tool.to_owned())
    }

    fn execute(&self, tool: &str, input: &JsonValue) -> Result<InvocationOutput, RuntimeError> {
        let admission = admit_agent_tool_ref(tool);
        if !admission.allowed {
            return Err(RuntimeError::SkillFailed {
                skill_name: MANAGED_AGENT_SKILL.to_owned(),
                message: format!(
                    "managed agent tool '{tool}' is not an admissible tool ref: {}",
                    admission.reason
                ),
            });
        }
        if !self.allowed_tools.contains(tool) {
            return Err(RuntimeError::SkillFailed {
                skill_name: MANAGED_AGENT_SKILL.to_owned(),
                message: format!("managed agent tool '{tool}' is not in the run's allowed_tools"),
            });
        }
        // The model supplies the tool arguments already resolved, so pass them as
        // both inputs and resolved_inputs.
        let inputs = input.as_object().cloned().unwrap_or_default();
        let request = ToolDispatchRequest {
            tool_ref: Cow::Borrowed(tool),
            inputs: Cow::Owned(inputs.clone()),
            resolved_inputs: Cow::Owned(inputs),
            scopes: &self.scopes,
            env: &self.env,
            skill_directory: &self.skill_directory,
            credential_delivery: &CredentialDelivery::none(),
            local_artifacts: &self.local_artifacts,
            skill_name: tool,
            allow_explicit_manifest_path: false,
            effect_admission: None,
        };
        dispatch_tool(request, &self.effects, &self.observed_at, Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::adapter::InvocationStatus;
    use crate::receipts::paths::RUNX_CWD_ENV;

    #[test]
    fn allowlisted_native_tool_uses_the_catalog_execution_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let status = Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(workspace.path())
            .status()?;
        assert!(status.success());
        let executor = RuntimeToolExecutor::new(
            BTreeMap::from([(
                RUNX_CWD_ENV.to_owned(),
                workspace.path().to_string_lossy().into_owned(),
            )]),
            workspace.path().to_path_buf(),
            RuntimeEffectRegistry::default(),
            "2026-01-01T00:00:00Z",
            ["git.status".to_owned()],
            vec!["git.read".to_owned()],
        );

        let output = executor.execute("git.status", &JsonValue::Object(Default::default()))?;

        assert_eq!(output.status, InvocationStatus::Success);
        let value = serde_json::to_string(&output.value)?;
        assert!(value.contains("\"git_status\""));
        assert!(value.contains("\"clean\":true"));
        Ok(())
    }

    #[test]
    fn unresolved_tool_is_an_error() {
        // A non-object input (here Null) also exercises the coercion to empty args
        // on the way to a clean failure, so this covers that path too.
        let executor = RuntimeToolExecutor::new(
            BTreeMap::new(),
            PathBuf::from("."),
            RuntimeEffectRegistry::default(),
            "2026-01-01T00:00:00Z",
            ["definitely-not-a-real-tool".to_owned()],
            Vec::new(),
        );
        let result = executor.execute("definitely-not-a-real-tool", &JsonValue::Null);
        assert!(
            matches!(&result, Err(RuntimeError::SkillFailed { .. })),
            "an unresolved tool must fail, not panic or succeed; got: {result:?}"
        );
    }

    #[test]
    fn managed_agent_uses_an_explicitly_allowed_and_scoped_write()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let executor = RuntimeToolExecutor::new(
            BTreeMap::from([(
                RUNX_CWD_ENV.to_owned(),
                workspace.path().to_string_lossy().into_owned(),
            )]),
            workspace.path().to_path_buf(),
            RuntimeEffectRegistry::default(),
            "2026-01-01T00:00:00Z",
            ["fs.write".to_owned()],
            vec!["fs.write".to_owned()],
        );
        let input = JsonValue::Object(BTreeMap::from([
            (
                "repo_root".to_owned(),
                JsonValue::String(workspace.path().to_string_lossy().into_owned()),
            ),
            (
                "path".to_owned(),
                JsonValue::String("scoped.txt".to_owned()),
            ),
            (
                "contents".to_owned(),
                JsonValue::String("scoped write".to_owned()),
            ),
        ]));

        let output = executor.execute("fs.write", &input)?;

        assert_eq!(output.status, InvocationStatus::Success);
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("scoped.txt"))?,
            "scoped write"
        );
        Ok(())
    }

    #[test]
    fn tool_outside_allowed_tools_is_rejected_before_resolution() {
        let executor = RuntimeToolExecutor::new(
            BTreeMap::new(),
            PathBuf::from("."),
            RuntimeEffectRegistry::default(),
            "2026-01-01T00:00:00Z",
            ["fs.read".to_owned()],
            Vec::new(),
        );
        let result = executor.execute("git.status", &JsonValue::Null);
        assert!(
            matches!(&result, Err(RuntimeError::SkillFailed { message, .. }) if message.contains("not in the run's allowed_tools")),
            "a model-selected tool outside allowed_tools must fail before local resolution; got: {result:?}"
        );
    }

    #[test]
    fn path_like_tool_is_rejected_even_when_allowlisted() {
        let executor = RuntimeToolExecutor::new(
            BTreeMap::new(),
            PathBuf::from("."),
            RuntimeEffectRegistry::default(),
            "2026-01-01T00:00:00Z",
            ["/tmp/manifest.json".to_owned()],
            Vec::new(),
        );
        let result = executor.execute("/tmp/manifest.json", &JsonValue::Null);
        assert!(
            matches!(&result, Err(RuntimeError::SkillFailed { message, .. }) if message.contains("not an admissible tool ref")),
            "a path-like model-selected tool must fail before local resolution; got: {result:?}"
        );
    }
}
