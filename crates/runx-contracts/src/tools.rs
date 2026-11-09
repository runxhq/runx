//! Contract types for tool manifests and tool catalog JSON surfaces.
// Module rationale: tool catalog contracts keep their serde shapes together.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;
use crate::{EnvironmentRequirements, InputDefinition, JsonObject};

pub const TOOL_MANIFEST_SCHEMA: &str = "runx.tool.manifest.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ToolManifestSchema {
    #[serde(rename = "runx.tool.manifest.v1")]
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
#[serde(rename_all = "snake_case")]
pub enum ToolInspectOrigin {
    Local,
    Native,
}

/// A tool: a named process that declares its interface the way a method does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.tool.manifest.v1")]
pub struct ToolManifest {
    pub schema: ToolManifestSchema,
    /// The catalog name a step's `tool:` uses.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub process: ToolProcess,
    /// Each input is its JSON Schema, required unless it says
    /// `optional: true`, as a method's are.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, JsonObject>,
    /// Each output is its JSON Schema, as a method's are. One with a `packet`
    /// is also kept as an artifact of that packet.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, JsonObject>,
    /// The output the whole result becomes, as a step's `result_as` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_as: Option<String>,
}

pub type ToolInput = InputDefinition;

/// A tool's executable: a trusted host process, the same object as a step's
/// process tool. runx controls its argv, cwd, environment and lifetime, but
/// does not confine it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolProcess {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs_via: Option<ToolCommandInputMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
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
pub struct ToolCatalogSearchResult {
    pub tool_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub source: String,
    pub source_label: String,
    pub namespace: String,
    pub external_name: String,
    pub tags: Vec<String>,
    pub catalog_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCatalogSearchReport {
    pub query: String,
    pub source: String,
    pub results: Vec<ToolCatalogSearchResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolInspectResult {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How the tool executes: `native` (runx's own code) or `process`.
    pub execution: String,
    pub inputs: BTreeMap<String, ToolInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeCommand>,
    pub reference_path: String,
    pub skill_directory: String,
    pub provenance: ToolInspectProvenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolInspectReport {
    pub tool: ToolInspectResult,
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
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::{
        ToolCatalogSearchResult, ToolCommandInputMode, ToolInspectOrigin, ToolInspectProvenance,
        ToolInspectResult, ToolManifest, ToolManifestSchema, ToolProcess,
    };
    use std::collections::BTreeMap;

    use crate::{EnvironmentRequirements, JsonValue};

    #[test]
    fn tool_manifest_round_trips_snake_case_fields() -> Result<(), serde_json::Error> {
        let json = r#"{
          "schema": "runx.tool.manifest.v1",
          "name": "fs.read",
          "description": "Read a UTF-8 text file.",
          "process": {
            "command": "node",
            "args": ["./run.mjs"],
            "timeout_seconds": 30,
            "inputs_via": "stdin"
          },
          "inputs": {
            "path": {
              "type": "string",
              "description": "Path to read."
            }
          },
          "outputs": {
            "file_read": { "packet": "runx.fs.file_read.v1" }
          },
          "result_as": "file_read"
        }"#;

        let manifest: ToolManifest = serde_json::from_str(json)?;

        assert_eq!(manifest.schema, ToolManifestSchema::V1);
        assert_eq!(manifest.process.command, "node");
        assert_eq!(
            manifest.process.inputs_via,
            Some(ToolCommandInputMode::Stdin)
        );
        assert_eq!(manifest.result_as.as_deref(), Some("file_read"));
        assert_eq!(
            manifest.outputs["file_read"]["packet"],
            JsonValue::String("runx.fs.file_read.v1".to_owned())
        );

        let encoded = serde_json::to_value(&manifest)?;
        assert_eq!(encoded["process"]["timeout_seconds"], 30);
        assert!(encoded.get("runtime").is_none());
        assert!(encoded.get("output").is_none());
        assert!(encoded.get("source_digest").is_none());
        Ok(())
    }

    #[test]
    fn tool_optional_manifest_fields_are_omitted() -> Result<(), serde_json::Error> {
        let encoded = serde_json::to_value(tool_manifest_fixture())?;

        assert!(encoded.get("description").is_none());
        assert!(encoded["process"].get("args").is_none());
        assert!(encoded.get("outputs").is_none());
        assert!(encoded.get("result_as").is_none());
        Ok(())
    }

    fn tool_manifest_fixture() -> ToolManifest {
        ToolManifest {
            schema: ToolManifestSchema::V1,
            name: "fixture.echo".to_owned(),
            version: None,
            description: None,
            process: tool_process_fixture(),
            inputs: [(
                "message".to_owned(),
                [("type".to_owned(), JsonValue::String("string".to_owned()))]
                    .into_iter()
                    .collect(),
            )]
            .into_iter()
            .collect(),
            outputs: BTreeMap::new(),
            result_as: None,
        }
    }

    fn tool_process_fixture() -> ToolProcess {
        ToolProcess {
            command: "node".to_owned(),
            args: Vec::new(),
            cwd: None,
            timeout_seconds: None,
            inputs_via: None,
            environment: EnvironmentRequirements::default(),
        }
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
              "namespace": "fs",
              "external_name": "read",
              "tags": ["native"],
              "catalog_id": "runx-runtime/filesystem:fs.read"
            }"#,
        )?;

        assert_eq!(result.catalog_id, "runx-runtime/filesystem:fs.read");
        Ok(())
    }

    #[test]
    fn tool_inspect_result_uses_provenance_shape() -> Result<(), serde_json::Error> {
        let result: ToolInspectResult = serde_json::from_str(
            r#"{
              "name": "fs.read",
              "execution": "native",
              "inputs": {},
              "reference_path": "native:runx-runtime/filesystem:fs.read",
              "skill_directory": ".",
              "provenance": {
                "origin": "native",
                "source": "runx-runtime/filesystem",
                "source_label": "runx-runtime/filesystem",
                "catalog_id": "runx-runtime/filesystem:fs.read"
              }
            }"#,
        )?;

        assert_eq!(result.name, "fs.read");
        assert_eq!(
            result.provenance,
            ToolInspectProvenance {
                origin: ToolInspectOrigin::Native,
                source: Some("runx-runtime/filesystem".to_owned()),
                source_label: Some("runx-runtime/filesystem".to_owned()),
                namespace: None,
                external_name: None,
                catalog_id: Some("runx-runtime/filesystem:fs.read".to_owned()),
                tool_id: None,
                tags: None,
            }
        );
        Ok(())
    }
}
