//! Canonical packet-schema catalog for runtime consumers.
//!
//! The parser owns document meaning. This module owns deterministic discovery,
//! exact-source collision handling, and search-root policy. Execution, listing,
//! and registry publication consume this catalog instead of maintaining their
//! own packet registries.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use runx_parser::SkillPackageSource;
use runx_parser::{
    PacketSchemaError, SkillArtifactContract, SkillInput, SkillRunnerDefinition,
    ValidatedPacketSchema, ValidatedSkillPackage, ValidatedTool, parse_packet_schema_document,
};
use thiserror::Error;

use crate::RuntimeError;
use crate::filesystem::{read_dir_sorted, read_to_string};

mod input_contracts;

pub(crate) use input_contracts::{
    hydrate_packet_input_contracts, hydrate_standalone_tool_input_contracts,
};

/// Build the exact root schema the agent's final-result tool must enforce.
/// Named packet outputs refine one declared field; `wrap_as` packet contracts
/// constrain the complete result unless the wrapper is itself a declared field.
/// Native receipt verification independently enforces the same packet bytes.
pub(crate) fn agent_output_contract_schema(
    output: &BTreeMap<String, runx_contracts::OutputField>,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
    env: &BTreeMap<String, String>,
) -> Result<runx_contracts::JsonValue, RuntimeError> {
    let mut root = runx_contracts::output_value_schema(Some(output));
    let Some(artifacts) = artifacts else {
        return Ok(root);
    };
    let named_bindings = artifacts.packets.clone().unwrap_or_default();
    let wrapped_binding = artifacts
        .wrap_as
        .as_ref()
        .zip(artifacts.packet.as_ref())
        .map(|(name, packet)| (name.clone(), packet.clone()));
    if named_bindings.is_empty() && wrapped_binding.is_none() {
        return Ok(root);
    }

    let package_root = crate::skill_package::find_owning_package_root(skill_directory)
        .unwrap_or_else(|| skill_directory.to_path_buf());
    let workspace = crate::RunxPaths::resolve(env, skill_directory)
        .workspace()
        .to_path_buf();
    let directories = packet_schema_directories(skill_directory, &package_root, &workspace)
        .map_err(agent_packet_contract_error)?;
    let catalog =
        PacketSchemaCatalog::discover(directories).map_err(agent_packet_contract_error)?;

    for (name, packet) in named_bindings {
        if !output.contains_key(&name) {
            return Err(RuntimeError::SkillFailed {
                skill_name: "agent".to_owned(),
                message: format!(
                    "packet output '{name}' for '{packet}' is not declared by the agent output contract"
                ),
            });
        }
        replace_output_property_schema(&mut root, &name, packet_schema(&catalog, &packet)?)?;
    }
    if let Some((name, packet)) = wrapped_binding {
        let packet_schema = packet_schema(&catalog, &packet)?;
        if output.contains_key(&name) {
            replace_output_property_schema(&mut root, &name, packet_schema)?;
        } else {
            merge_wrapped_packet_schema(&mut root, packet_schema)?;
        }
    }
    Ok(root)
}

fn merge_wrapped_packet_schema(
    root: &mut runx_contracts::JsonValue,
    mut packet: runx_contracts::JsonObject,
) -> Result<(), RuntimeError> {
    use runx_contracts::JsonValue;

    let JsonValue::Object(root) = root else {
        return Err(agent_packet_shape_error());
    };
    let Some(JsonValue::Object(root_properties)) = root.get_mut("properties") else {
        return Err(agent_packet_shape_error());
    };
    let packet_properties = match packet.remove("properties") {
        Some(JsonValue::Object(properties)) => properties,
        _ => return Err(agent_packet_shape_error()),
    };
    if packet.get("additionalProperties") == Some(&JsonValue::Bool(false))
        && let Some(name) = root_properties
            .keys()
            .find(|name| !packet_properties.contains_key(*name))
    {
        return Err(RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: format!("wrapped packet schema does not declare agent output field '{name}'"),
        });
    }
    for (name, field_schema) in packet_properties {
        if let Some(field) = root_properties.get_mut(&name) {
            *field = field_schema;
        }
    }

    let declared = root_properties
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut required = root
        .get("required")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(JsonValue::Array(packet_required)) = packet.remove("required") {
        for field in packet_required {
            let Some(name) = field.as_str() else {
                return Err(agent_packet_shape_error());
            };
            if !declared.contains(name) {
                return Err(RuntimeError::SkillFailed {
                    skill_name: "agent".to_owned(),
                    message: format!(
                        "wrapped packet requires undeclared agent output field '{name}'"
                    ),
                });
            }
            if !required
                .iter()
                .any(|existing| existing.as_str() == Some(name))
            {
                required.push(JsonValue::String(name.to_owned()));
            }
        }
    }
    root.insert("required".to_owned(), JsonValue::Array(required));

    // Preserve root-level semantic constraints while retaining the ordinary
    // object/properties form accepted by provider tool APIs.
    for key in [
        "$defs",
        "allOf",
        "anyOf",
        "oneOf",
        "not",
        "if",
        "then",
        "else",
        "dependentRequired",
        "dependentSchemas",
        "minProperties",
        "maxProperties",
    ] {
        if let Some(value) = packet.remove(key) {
            root.insert(key.to_owned(), value);
        }
    }
    Ok(())
}

fn agent_packet_shape_error() -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: "agent".to_owned(),
        message: "wrapped packet schema is not a mergeable object contract".to_owned(),
    }
}

fn packet_schema(
    catalog: &PacketSchemaCatalog,
    packet: &str,
) -> Result<runx_contracts::JsonObject, RuntimeError> {
    catalog
        .get(packet)
        .ok_or_else(|| RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: format!("declared packet schema '{packet}' was not found"),
        })?
        .schema
        .value
        .as_object()
        .cloned()
        .ok_or_else(|| RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: format!("declared packet schema '{packet}' must be an object"),
        })
}

fn replace_output_property_schema(
    root: &mut runx_contracts::JsonValue,
    name: &str,
    schema: runx_contracts::JsonObject,
) -> Result<(), RuntimeError> {
    let runx_contracts::JsonValue::Object(root) = root else {
        return Err(RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: "agent output schema has no properties object".to_owned(),
        });
    };
    let Some(runx_contracts::JsonValue::Object(properties)) = root.get_mut("properties") else {
        return Err(RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: "agent output schema has no properties object".to_owned(),
        });
    };
    properties.insert(name.to_owned(), runx_contracts::JsonValue::Object(schema));
    Ok(())
}

fn agent_packet_contract_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: "agent".to_owned(),
        message: format!("agent packet output contract failed: {error}"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PacketSchemaEntry {
    pub(crate) path: PathBuf,
    pub(crate) file_name: String,
    pub(crate) source: String,
    pub(crate) schema: ValidatedPacketSchema,
}

#[derive(Debug, Error)]
pub(crate) enum PacketSchemaCatalogError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Parse(#[from] PacketSchemaError),
    #[error("{0}")]
    PackageBundle(String),
    #[error("packet schema path is not valid UTF-8: {path}")]
    InvalidPath { path: PathBuf },
    #[error("packet schema is not valid UTF-8: {path}")]
    InvalidSource { path: PathBuf },
    #[error("native packet schema {file_name} is invalid: {message}")]
    Native { file_name: String, message: String },
    #[error(
        "packet schema id '{packet_id}' resolves to conflicting documents at {existing_path} and {path}"
    )]
    Conflict {
        packet_id: String,
        existing_path: PathBuf,
        path: PathBuf,
    },
}

#[derive(Default)]
pub(crate) struct PacketSchemaCatalog {
    entries: BTreeMap<String, PacketSchemaEntry>,
}

impl PacketSchemaCatalog {
    /// Start with reusable packet contracts owned by the native Runx contract
    /// catalog. Product packages consume these contracts by id and never carry
    /// private copies of Runx-owned schema bytes.
    pub(crate) fn native_public() -> Result<Self, PacketSchemaCatalogError> {
        let mut catalog = Self::default();
        for artifact in runx_contracts::generated_schema_artifacts() {
            if !artifact.native_packet {
                continue;
            }
            let Some(schema) = artifact.schema.as_object() else {
                continue;
            };
            if schema
                .get("x-runx-packet")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            {
                continue;
            }
            let Some(packet_id) = schema
                .get("x-runx-schema")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
            else {
                continue;
            };
            let mut document = schema.clone();
            document.insert(
                runx_parser::PACKET_ID_FIELD.to_owned(),
                serde_json::Value::String(packet_id.to_owned()),
            );
            document.insert(
                "x-runx-generated-from".to_owned(),
                serde_json::Value::String(format!("schemas/{}", artifact.file_name)),
            );
            let source = serde_json::to_string_pretty(&serde_json::Value::Object(document))
                .map(|source| format!("{source}\n"))
                .map_err(|error| PacketSchemaCatalogError::Native {
                    file_name: artifact.file_name.to_owned(),
                    message: error.to_string(),
                })?;
            let path = PathBuf::from("[runx-native-packets]").join(artifact.file_name);
            let entry = packet_schema_entry(path, source)?.ok_or_else(|| {
                PacketSchemaCatalogError::Native {
                    file_name: artifact.file_name.to_owned(),
                    message: "generated public packet has no packet identity".to_owned(),
                }
            })?;
            catalog.insert(entry)?;
        }
        Ok(catalog)
    }

    pub(crate) fn discover(
        directories: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, PacketSchemaCatalogError> {
        let mut catalog = Self::native_public()?;
        catalog.discover_directories(directories)?;
        Ok(catalog)
    }

    pub(crate) fn discover_directories(
        &mut self,
        directories: impl IntoIterator<Item = PathBuf>,
    ) -> Result<(), PacketSchemaCatalogError> {
        for directory in stable_unique_paths(directories) {
            self.discover_directory(&directory)?;
        }
        Ok(())
    }

    /// Add packet schemas from every canonical `packets/` directory in the
    /// parser-admitted package snapshot. Hydration and publication therefore
    /// resolve the same immutable bytes instead of rereading package files.
    pub(crate) fn discover_validated_package(
        &mut self,
        package: &ValidatedSkillPackage,
        package_root: &Path,
    ) -> Result<(), PacketSchemaCatalogError> {
        for directory in package_packet_directories(package) {
            self.discover_source_directory(&package.source, package_root, &directory)?;
        }
        Ok(())
    }

    /// Add packet schemas from the exact package snapshot already admitted by
    /// the parser. Registry publication uses this instead of rereading package
    /// files after validation.
    #[cfg(feature = "process-tool")]
    pub(crate) fn discover_loaded_package(
        &mut self,
        loaded: &crate::LoadedSkillPackage,
    ) -> Result<(), PacketSchemaCatalogError> {
        self.discover_validated_package(&loaded.package, &loaded.package_root)
    }

    pub(crate) fn discover_directory(
        &mut self,
        directory: &Path,
    ) -> Result<(), PacketSchemaCatalogError> {
        for entry in read_dir_sorted(directory)? {
            if !entry.is_file
                || entry.path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            if let Some(schema) = read_packet_schema(&entry.path)? {
                self.insert(schema)?;
            }
        }
        Ok(())
    }

    pub(crate) fn insert(
        &mut self,
        entry: PacketSchemaEntry,
    ) -> Result<(), PacketSchemaCatalogError> {
        let packet_id = &entry.schema.packet_id;
        if let Some(existing) = self.entries.get(packet_id) {
            if existing.schema.value != entry.schema.value {
                return Err(PacketSchemaCatalogError::Conflict {
                    packet_id: packet_id.clone(),
                    existing_path: existing.path.clone(),
                    path: entry.path,
                });
            }
            return Ok(());
        }
        self.entries.insert(packet_id.clone(), entry);
        Ok(())
    }

    pub(crate) fn get(&self, packet_id: &str) -> Option<&PacketSchemaEntry> {
        self.entries.get(packet_id)
    }

    #[cfg(feature = "process-tool")]
    pub(crate) fn entries(&self) -> impl Iterator<Item = &PacketSchemaEntry> {
        self.entries.values()
    }

    fn discover_source_directory(
        &mut self,
        source: &SkillPackageSource,
        package_root: &Path,
        relative_directory: &str,
    ) -> Result<(), PacketSchemaCatalogError> {
        let prefix = format!("{relative_directory}/");
        for (relative, contents) in source
            .files
            .range(prefix.clone()..)
            .take_while(|(path, _)| path.starts_with(&prefix))
        {
            let file_name = &relative[prefix.len()..];
            if file_name.contains('/') || !file_name.ends_with(".json") {
                continue;
            }
            let path = package_root.join(relative);
            let source = std::str::from_utf8(contents)
                .map(str::to_owned)
                .map_err(|_| PacketSchemaCatalogError::InvalidSource { path: path.clone() })?;
            if let Some(schema) = packet_schema_entry(path, source)? {
                self.insert(schema)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn read_packet_schema(
    path: &Path,
) -> Result<Option<PacketSchemaEntry>, PacketSchemaCatalogError> {
    let source = read_to_string(path)?;
    packet_schema_entry(path.to_path_buf(), source)
}

pub(crate) fn packet_schema_entry(
    path: PathBuf,
    source: String,
) -> Result<Option<PacketSchemaEntry>, PacketSchemaCatalogError> {
    let label = path.to_string_lossy().into_owned();
    let Some(schema) = parse_packet_schema_document(label, &source)? else {
        return Ok(None);
    };
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| PacketSchemaCatalogError::InvalidPath { path: path.clone() })?;
    Ok(Some(PacketSchemaEntry {
        path,
        file_name,
        source,
        schema,
    }))
}

/// Search only the selected profile, its owning package, the package's source
/// workspace, and the execution workspace. The source and execution
/// workspaces intentionally differ for isolated harness runs.
pub(crate) fn packet_schema_directories(
    profile_directory: &Path,
    package_root: &Path,
    workspace: &Path,
) -> Result<Vec<PathBuf>, PacketSchemaCatalogError> {
    let source_workspace = crate::RunxPaths::resolve(&BTreeMap::new(), package_root)
        .workspace()
        .to_path_buf();
    let bundle_root = crate::registry::package_bundle::package_bundle_root(profile_directory)
        .map_err(PacketSchemaCatalogError::PackageBundle)?;
    let mut directories = vec![
        profile_directory.join("packets"),
        package_root.join("packets"),
    ];
    if let Some(root) = bundle_root {
        directories.push(root.join("packets"));
        directories.push(root.join("dist").join("packets"));
    }
    directories.extend([
        source_workspace.join("packets"),
        source_workspace.join("dist").join("packets"),
        workspace.join("packets"),
        workspace.join("dist").join("packets"),
    ]);
    Ok(stable_unique_paths(directories))
}

pub(crate) fn declared_runner_packet_ids(runner: &SkillRunnerDefinition) -> BTreeSet<String> {
    let mut packet_ids = BTreeSet::new();
    extend_input_packet_ids(&runner.inputs, &mut packet_ids);
    for step in &runner.graph.steps {
        extend_packet_ids(step.artifacts.as_ref(), &mut packet_ids);
    }
    packet_ids
}

pub(crate) fn declared_tool_packet_ids(tool: &ValidatedTool) -> BTreeSet<String> {
    let mut packet_ids = declared_artifact_packet_ids(tool.artifacts.as_ref());
    extend_input_packet_ids(&tool.inputs, &mut packet_ids);
    packet_ids
}

pub(crate) fn declared_input_packet_ids(inputs: &BTreeMap<String, SkillInput>) -> BTreeSet<String> {
    inputs
        .values()
        .filter_map(|input| input.packet.clone())
        .collect()
}

pub(crate) fn declared_artifact_packet_ids(
    artifacts: Option<&SkillArtifactContract>,
) -> BTreeSet<String> {
    let mut packet_ids = BTreeSet::new();
    extend_packet_ids(artifacts, &mut packet_ids);
    packet_ids
}

fn extend_packet_ids(artifacts: Option<&SkillArtifactContract>, packet_ids: &mut BTreeSet<String>) {
    let Some(artifacts) = artifacts else {
        return;
    };
    packet_ids.extend(artifacts.packet.iter().cloned());
    if let Some(packets) = &artifacts.packets {
        packet_ids.extend(packets.values().cloned());
    }
}

fn extend_input_packet_ids(
    inputs: &BTreeMap<String, SkillInput>,
    packet_ids: &mut BTreeSet<String>,
) {
    packet_ids.extend(inputs.values().filter_map(|input| input.packet.clone()));
}

fn package_packet_directories(package: &ValidatedSkillPackage) -> BTreeSet<String> {
    let mut directories = BTreeSet::from(["packets".to_owned()]);
    directories.extend(package.profiles.keys().filter_map(|profile| {
        runx_parser::profile_directory(profile)
            .filter(|directory| !directory.is_empty())
            .map(|directory| format!("{directory}/packets"))
    }));
    directories
}

fn stable_unique_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut unique = Vec::new();
    for path in paths {
        if !unique.iter().any(|existing| existing == &path) {
            unique.push(path);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::{
        PacketSchemaCatalog, PacketSchemaCatalogError, packet_schema_directories,
        packet_schema_entry,
    };
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn catalog_accepts_identical_documents_and_rejects_conflicts()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"{"x-runx-packet-id":"runx.test.plan.v1","type":"object"}"#;
        let mut catalog = PacketSchemaCatalog::default();
        catalog.insert(
            packet_schema_entry(PathBuf::from("first.json"), source.to_owned())?
                .ok_or("first schema missing")?,
        )?;
        catalog.insert(
            packet_schema_entry(PathBuf::from("second.json"), source.to_owned())?
                .ok_or("second schema missing")?,
        )?;
        catalog.insert(
            packet_schema_entry(
                PathBuf::from("formatted.json"),
                "{\n  \"type\": \"object\",\n  \"x-runx-packet-id\": \"runx.test.plan.v1\"\n}\n"
                    .to_owned(),
            )?
            .ok_or("formatted schema missing")?,
        )?;

        let conflict = packet_schema_entry(
            PathBuf::from("conflict.json"),
            r#"{"x-runx-packet-id":"runx.test.plan.v1","type":"string"}"#.to_owned(),
        )?
        .ok_or("conflicting schema missing")?;
        assert!(matches!(
            catalog.insert(conflict),
            Err(PacketSchemaCatalogError::Conflict { .. })
        ));
        Ok(())
    }

    #[test]
    fn native_public_packets_are_available_without_workspace_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let catalog = PacketSchemaCatalog::native_public()?;
        assert!(catalog.get("runx.external_job_stage.request.v1").is_some());
        assert!(catalog.get("runx.external_job_stage.result.v1").is_some());
        assert!(
            catalog
                .get("runx.external_job_schedule_intent.v1")
                .is_some()
        );
        let repository_packets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("dist/packets");
        let discovered = PacketSchemaCatalog::discover([repository_packets])?;
        assert!(
            discovered
                .get("runx.external_job_stage.request.v1")
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn packet_schema_roots_keep_source_workspace_when_execution_is_isolated()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        fs::write(source.path().join("pnpm-workspace.yaml"), "packages: []\n")?;
        let package = source.path().join("skills/work-plan");
        let profile = package.join("graph/plan");
        let execution = tempfile::tempdir()?;

        let roots = packet_schema_directories(&profile, &package, execution.path())?;

        assert!(roots.contains(&source.path().join("dist/packets")));
        assert!(roots.contains(&execution.path().join("dist/packets")));
        Ok(())
    }
}
