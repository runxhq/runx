use std::fs;
use std::path::Path;

use super::error::ToolCatalogError;

pub(crate) struct ValidatedToolDocument {
    pub(crate) source: String,
    pub(crate) tool: runx_parser::ValidatedTool,
}

pub(crate) fn read(path: &Path) -> Result<runx_parser::ValidatedTool, ToolCatalogError> {
    read_document(path).map(|document| document.tool)
}

pub(crate) fn read_document(path: &Path) -> Result<ValidatedToolDocument, ToolCatalogError> {
    let source = fs::read_to_string(path)
        .map_err(|error| ToolCatalogError::io("reading tool manifest", path, error))?;
    let tool = parse(path, &source)?;
    Ok(ValidatedToolDocument { source, tool })
}

pub(crate) fn parse(
    path: &Path,
    source: &str,
) -> Result<runx_parser::ValidatedTool, ToolCatalogError> {
    let raw = runx_parser::parse_tool_manifest_json(source).map_err(|error| {
        ToolCatalogError::InvalidManifest {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    let mut tool = runx_parser::validate_tool_manifest(raw).map_err(|error| {
        ToolCatalogError::InvalidManifest {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    crate::packet_schemas::hydrate_standalone_tool_input_contracts(&mut tool, path).map_err(
        |error| ToolCatalogError::InvalidManifest {
            path: path.to_path_buf(),
            message: error.to_string(),
        },
    )?;
    Ok(tool)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::read;

    #[test]
    fn standalone_tool_resolves_packet_input_through_workspace_catalog()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("pnpm-workspace.yaml"), "packages: []\n")?;
        fs::create_dir_all(temp.path().join("dist/packets"))?;
        fs::write(
            temp.path().join("dist/packets/tool-plan.schema.json"),
            r#"{"x-runx-packet-id":"runx.test.tool-plan.v1","type":"object","required":["operation"],"properties":{"operation":{"const":"inspect"}},"additionalProperties":false}"#,
        )?;
        let tool = temp.path().join("tools/example/packet");
        fs::create_dir_all(&tool)?;
        let manifest = tool.join("manifest.json");
        fs::write(
            &manifest,
            r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.packet",
  "source": { "command": "example" },
  "inputs": {
    "plan": {
      "type": "json",
      "required": true,
      "packet": "runx.test.tool-plan.v1"
    }
  }
}"#,
        )?;

        let tool = read(&manifest)?;
        let plan = tool.inputs.get("plan").ok_or("plan input missing")?;
        assert_eq!(plan.packet.as_deref(), Some("runx.test.tool-plan.v1"));
        assert!(
            plan.schema
                .as_ref()
                .is_some_and(|schema| schema.contains_key("properties"))
        );
        Ok(())
    }
}
