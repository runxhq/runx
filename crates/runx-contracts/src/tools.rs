//! Contract types for tool manifests and tool catalog JSON surfaces.
// Module rationale: tool catalog contracts keep serde parity shapes together.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;
use crate::{
    ArtifactContract, EnvironmentRequirements, IdempotencyPolicy, InputDefinition, JsonValue,
    RetryPolicy,
};

pub const TOOL_MANIFEST_SCHEMA: &str = "runx.tool.manifest.v1";
pub const TOOL_BUILD_REPORT_SCHEMA: &str = "runx.tool.build.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ToolManifestSchema {
    #[serde(rename = "runx.tool.manifest.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ToolBuildReportSchema {
    #[serde(rename = "runx.tool.build.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCommandInputMode {
    Args,
    Stdin,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ToolSourceType {
    CliTool,
    Javascript,
}

impl ToolSourceType {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::CliTool => "cli-tool",
            Self::Javascript => "javascript",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolBuildStatus {
    Success,
    Failure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolInspectOrigin {
    Local,
    Native,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.tool.manifest.v1")]
pub struct ToolManifest {
    pub schema: ToolManifestSchema,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub source: ToolSource,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, ToolInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<ArtifactContract>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<ToolRetryPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency: Option<ToolIdempotencyPolicy>,
}

pub type ToolInput = InputDefinition;
pub type ToolRetryPolicy = RetryPolicy;
pub type ToolIdempotencyPolicy = IdempotencyPolicy;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolSource {
    #[serde(rename = "type")]
    pub source_type: ToolSourceType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(rename = "export", skip_serializing_if = "Option::is_none")]
    pub javascript_export: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mode: Option<ToolCommandInputMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "EnvironmentRequirements::is_empty")]
    pub environment: EnvironmentRequirements,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCommand {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltToolItem {
    pub path: String,
    pub manifest: String,
    pub source_hash: String,
    pub schema_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolBuildReport {
    pub schema: ToolBuildReportSchema,
    pub status: ToolBuildStatus,
    pub built: Vec<BuiltToolItem>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCatalogSearchOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCatalogSearchResult {
    pub tool_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub source: String,
    pub source_label: String,
    pub source_type: String,
    pub namespace: String,
    pub external_name: String,
    pub required_scopes: Vec<String>,
    pub tags: Vec<String>,
    pub catalog_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCatalogSearchReport {
    pub status: ToolBuildStatus,
    pub query: String,
    pub source: String,
    pub results: Vec<ToolCatalogSearchResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolInspectResult {
    #[serde(rename = "ref")]
    pub tool_ref: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub execution_source_type: String,
    pub inputs: BTreeMap<String, ToolInput>,
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<JsonValue>,
    pub reference_path: String,
    pub skill_directory: String,
    pub provenance: ToolInspectProvenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolInspectReport {
    pub status: ToolBuildStatus,
    pub tool: ToolInspectResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolInspectOptions {
    #[serde(rename = "ref")]
    pub tool_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_from_directory: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolInspectProvenance {
    pub origin: ToolInspectOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::{
        ToolBuildReport, ToolBuildReportSchema, ToolBuildStatus, ToolCatalogSearchResult,
        ToolCommandInputMode, ToolInput, ToolInspectOrigin, ToolInspectProvenance,
        ToolInspectResult, ToolManifest, ToolManifestSchema, ToolSource, ToolSourceType,
    };
    use crate::EnvironmentRequirements;

    #[test]
    fn tool_manifest_round_trips_snake_case_fields() -> Result<(), serde_json::Error> {
        let json = r#"{
          "schema": "runx.tool.manifest.v1",
          "name": "fs.read",
          "description": "Read a UTF-8 text file.",
          "source": {
            "type": "cli-tool",
            "command": "node",
            "args": ["./run.mjs"],
            "timeout_seconds": 30,
            "input_mode": "stdin"
          },
          "inputs": {
            "path": {
              "type": "string",
              "required": true,
              "description": "Path to read."
            }
          },
          "artifacts": {
            "packet": "runx.fs.file_read.v1",
            "wrap_as": "file_read"
          },
          "scopes": ["fs.read"]
        }"#;

        let manifest: ToolManifest = serde_json::from_str(json)?;

        assert_eq!(manifest.schema, ToolManifestSchema::V1);
        assert_eq!(manifest.source.source_type, ToolSourceType::CliTool);
        assert_eq!(
            manifest.source.input_mode,
            Some(ToolCommandInputMode::Stdin)
        );
        assert_eq!(
            manifest
                .artifacts
                .as_ref()
                .and_then(|value| value.wrap_as.as_deref()),
            Some("file_read")
        );

        let encoded = serde_json::to_value(&manifest)?;
        assert_eq!(encoded["source"]["timeout_seconds"], 30);
        assert!(encoded.get("runtime").is_none());
        assert!(encoded.get("output").is_none());
        assert!(encoded.get("source_hash").is_none());
        assert!(encoded.get("risk").is_none());
        Ok(())
    }

    #[test]
    fn tool_optional_manifest_fields_are_omitted() -> Result<(), serde_json::Error> {
        let encoded = serde_json::to_value(tool_manifest_fixture())?;

        assert!(encoded.get("description").is_none());
        assert!(encoded["source"].get("args").is_none());
        assert!(encoded.get("artifacts").is_none());
        Ok(())
    }

    fn tool_manifest_fixture() -> ToolManifest {
        ToolManifest {
            schema: ToolManifestSchema::V1,
            name: "fixture.echo".to_owned(),
            version: None,
            description: None,
            source: tool_source_fixture(),
            inputs: [(
                "message".to_owned(),
                ToolInput {
                    input_type: "string".to_owned(),
                    required: true,
                    description: None,
                    default: None,
                    artifact: None,
                    packet: None,
                    schema: None,
                },
            )]
            .into_iter()
            .collect(),
            scopes: Vec::new(),
            risk: None,
            artifacts: None,
            retry: None,
            idempotency: None,
        }
    }

    fn tool_source_fixture() -> ToolSource {
        ToolSource {
            source_type: ToolSourceType::CliTool,
            command: Some("node".to_owned()),
            module: None,
            javascript_export: None,
            args: Vec::new(),
            cwd: None,
            timeout_seconds: None,
            input_mode: None,
            environment: EnvironmentRequirements::default(),
        }
    }

    #[test]
    fn tool_build_report_uses_cli_json_shape() -> Result<(), serde_json::Error> {
        let report: ToolBuildReport = serde_json::from_str(
            r#"{
              "schema": "runx.tool.build.v1",
              "status": "success",
              "built": [{
                "path": "tools/demo/echo",
                "manifest": "tools/demo/echo/manifest.json",
                "source_hash": "sha256:source",
                "schema_hash": "sha256:schema"
              }],
              "errors": []
            }"#,
        )?;

        assert_eq!(report.schema, ToolBuildReportSchema::V1);
        assert_eq!(report.status, ToolBuildStatus::Success);
        assert_eq!(report.built[0].manifest, "tools/demo/echo/manifest.json");
        Ok(())
    }

    #[test]
    fn tool_catalog_search_result_uses_executor_json_shape() -> Result<(), serde_json::Error> {
        let result: ToolCatalogSearchResult = serde_json::from_str(
            r#"{
              "tool_id": "runx-runtime/filesystem/fs.read",
              "name": "fs.read",
              "summary": "Read a UTF-8 text file.",
              "source": "runx-runtime/filesystem",
              "source_label": "runx-runtime/filesystem",
              "source_type": "native",
              "namespace": "fs",
              "external_name": "read",
              "required_scopes": ["fs.read"],
              "tags": ["native"],
              "catalog_ref": "runx-runtime/filesystem:fs.read"
            }"#,
        )?;

        assert_eq!(result.catalog_ref, "runx-runtime/filesystem:fs.read");
        assert_eq!(result.required_scopes, ["fs.read"]);
        Ok(())
    }

    #[test]
    fn tool_inspect_result_uses_provenance_shape() -> Result<(), serde_json::Error> {
        let result: ToolInspectResult = serde_json::from_str(
            r#"{
              "ref": "fs.read",
              "name": "fs.read",
              "execution_source_type": "native",
              "inputs": {},
              "scopes": ["fs.read"],
              "reference_path": "native:runx-runtime/filesystem:fs.read",
              "skill_directory": ".",
              "provenance": {
                "origin": "native",
                "source": "runx-runtime/filesystem",
                "source_label": "runx-runtime/filesystem",
                "source_type": "native",
                "catalog_ref": "runx-runtime/filesystem:fs.read"
              }
            }"#,
        )?;

        assert_eq!(result.tool_ref, "fs.read");
        assert_eq!(
            result.provenance,
            ToolInspectProvenance {
                origin: ToolInspectOrigin::Native,
                source: Some("runx-runtime/filesystem".to_owned()),
                source_label: Some("runx-runtime/filesystem".to_owned()),
                source_type: Some("native".to_owned()),
                namespace: None,
                external_name: None,
                catalog_ref: Some("runx-runtime/filesystem:fs.read".to_owned()),
                tool_id: None,
                tags: None,
            }
        );
        Ok(())
    }
}
