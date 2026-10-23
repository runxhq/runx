//! Canonical packet-schema catalog for runtime consumers.
//!
//! The parser owns document meaning. This module owns resolution, which
//! follows the id: a core packet resolves from the contracts compiled into
//! runx, and a package packet from the package its id names. That is the
//! package itself, the pinned copy a package carries of another's packet,
//! or, for a package outside any installed bundle, the owning package in the
//! workspace or added to the project. Execution, listing, and registry
//! publication consume this catalog instead of maintaining their own packet
//! registries.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use runx_contracts::PacketId;
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
    let catalog = PacketSchemaCatalog::for_package(
        &package_root,
        named_bindings
            .values()
            .chain(wrapped_binding.iter().map(|(_, packet)| packet)),
    )
    .map_err(agent_packet_contract_error)?;

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
    packet: &PacketId,
) -> Result<runx_contracts::JsonObject, RuntimeError> {
    catalog
        .get(packet)
        .ok_or_else(|| RuntimeError::SkillFailed {
            skill_name: "agent".to_owned(),
            message: catalog.not_found(packet),
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
    #[error("packet schema is not valid UTF-8: {path}")]
    InvalidSource { path: PathBuf },
    #[error("native packet schema {file_name} is invalid: {message}")]
    Native { file_name: String, message: String },
    #[error(
        "packet schema id '{packet_id}' resolves to conflicting documents at {existing_path} and {path}"
    )]
    Conflict {
        packet_id: PacketId,
        existing_path: PathBuf,
        path: PathBuf,
    },
}

#[derive(Clone, Default)]
pub(crate) struct PacketSchemaCatalog {
    entries: BTreeMap<PacketId, PacketSchemaEntry>,
    /// Where each requested id that resolved nowhere was looked for.
    searched: BTreeMap<PacketId, Vec<PathBuf>>,
}

impl PacketSchemaCatalog {
    /// The core packets: the reusable contracts the native Runx contract
    /// catalog owns. A core packet's bytes are exactly its published
    /// `schemas/` document, and no package carries a copy.
    pub(crate) fn native_public() -> Result<Self, PacketSchemaCatalogError> {
        // The contracts are compiled in, so the catalog is the same for the
        // whole process: built once, then copied for each caller to extend.
        static NATIVE: std::sync::OnceLock<PacketSchemaCatalog> = std::sync::OnceLock::new();
        if let Some(catalog) = NATIVE.get() {
            return Ok(catalog.clone());
        }
        let catalog = Self::build_native_public()?;
        Ok(NATIVE.get_or_init(|| catalog).clone())
    }

    fn build_native_public() -> Result<Self, PacketSchemaCatalogError> {
        let mut catalog = Self::default();
        for artifact in runx_contracts::generated_schema_artifacts() {
            let Some(packet_id) = artifact.packet_id() else {
                continue;
            };
            let native_error = |message: String| PacketSchemaCatalogError::Native {
                file_name: artifact.file_name.to_owned(),
                message,
            };
            let source = artifact
                .document()
                .map_err(|error| native_error(error.to_string()))?;
            let value =
                serde_json::from_str(&source).map_err(|error| native_error(error.to_string()))?;
            let digest = runx_contracts::sha256_prefixed(source.as_bytes());
            catalog.insert(PacketSchemaEntry {
                path: PathBuf::from("[runx-native-packets]").join(artifact.file_name),
                source,
                schema: ValidatedPacketSchema {
                    packet_id,
                    value,
                    digest,
                },
            })?;
        }
        Ok(catalog)
    }

    /// The packets the package at `package_root` resolves for `packet_ids`:
    /// every core packet, and each named package packet from where its id
    /// says it lives (R5).
    pub(crate) fn for_package<'a>(
        package_root: &Path,
        packet_ids: impl IntoIterator<Item = &'a PacketId>,
    ) -> Result<Self, PacketSchemaCatalogError> {
        let mut catalog = Self::native_public()?;
        catalog.resolve_all(package_root, packet_ids)?;
        Ok(catalog)
    }

    /// As [`Self::for_package`], starting from the packets the parser
    /// admitted with the package and the documents `admitted` already
    /// resolved for it, so hydration and publication resolve the same
    /// immutable bytes instead of rereading files.
    pub(crate) fn for_validated_package<'a>(
        package: &ValidatedSkillPackage,
        package_root: &Path,
        admitted: impl IntoIterator<Item = &'a PacketSchemaEntry>,
        packet_ids: impl IntoIterator<Item = &'a PacketId>,
    ) -> Result<Self, PacketSchemaCatalogError> {
        let mut catalog = Self::native_public()?;
        for entry in admitted {
            catalog.insert(entry.clone())?;
        }
        for packet in package.packets.values() {
            let path = package_root.join(&packet.path);
            let source = package
                .file_text(&packet.path)
                .ok_or_else(|| PacketSchemaCatalogError::InvalidSource { path: path.clone() })?
                .to_owned();
            catalog.insert(PacketSchemaEntry {
                path,
                source,
                schema: packet.schema.clone(),
            })?;
        }
        catalog.resolve_all(package_root, packet_ids)?;
        Ok(catalog)
    }

    fn resolve_all<'a>(
        &mut self,
        package_root: &Path,
        packet_ids: impl IntoIterator<Item = &'a PacketId>,
    ) -> Result<(), PacketSchemaCatalogError> {
        let mut lookup = None;
        for packet_id in packet_ids {
            if self.entries.contains_key(packet_id) || self.searched.contains_key(packet_id) {
                continue;
            }
            let lookup = match &lookup {
                Some(lookup) => lookup,
                None => lookup.insert(PacketLookup::for_package(package_root)?),
            };
            self.resolve(packet_id, lookup)?;
        }
        Ok(())
    }

    fn resolve(
        &mut self,
        packet_id: &PacketId,
        lookup: &PacketLookup,
    ) -> Result<(), PacketSchemaCatalogError> {
        let mut searched = Vec::new();
        for candidates in lookup.candidates(packet_id)? {
            let mut found = false;
            for path in candidates {
                let declares = if path.is_file() {
                    read_packet_schema(&path)?.filter(|entry| &entry.schema.packet_id == packet_id)
                } else {
                    None
                };
                searched.push(path);
                if let Some(entry) = declares {
                    self.insert(entry)?;
                    found = true;
                }
            }
            if found {
                return Ok(());
            }
        }
        self.searched.insert(packet_id.clone(), searched);
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
        self.searched.remove(packet_id);
        self.entries.insert(packet_id.clone(), entry);
        Ok(())
    }

    pub(crate) fn get(&self, packet_id: &PacketId) -> Option<&PacketSchemaEntry> {
        self.entries.get(packet_id)
    }

    /// Why a requested id is not in the catalog.
    pub(crate) fn not_found(&self, packet_id: &PacketId) -> String {
        if packet_id.is_core() {
            return format!("'{packet_id}' is not a core packet this runx carries");
        }
        let searched = self
            .searched
            .get(packet_id)
            .map(|paths| {
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        format!("declared packet schema '{packet_id}' was not found; searched {searched}")
    }
}

/// Where one package looks for the packets other packages own.
struct PacketLookup {
    package_root: PathBuf,
    /// The workspace and project-added packages, for a package outside any
    /// installed bundle. An installed package resolves only what its bundle
    /// carries.
    owners: Option<(PathBuf, PathBuf)>,
}

impl PacketLookup {
    fn for_package(package_root: &Path) -> Result<Self, PacketSchemaCatalogError> {
        let bundled = crate::registry::package_bundle::package_bundle_root(package_root)
            .map_err(PacketSchemaCatalogError::PackageBundle)?
            .is_some();
        // The package's own workspace, never the execution workspace an
        // isolated run sets: what a packet means never depends on where the
        // package runs.
        let paths = crate::RunxPaths::resolve(&BTreeMap::new(), package_root);
        Ok(Self {
            package_root: package_root.to_path_buf(),
            owners: (!bundled).then(|| (paths.skills(), paths.added_skills())),
        })
    }

    /// Candidate paths in precedence order, each group a source that settles
    /// the id when any of its paths declares it: the package's own packet
    /// and its pinned copy, then the owning package in the workspace, then
    /// every version of the owning package added to the project.
    fn candidates(
        &self,
        packet_id: &PacketId,
    ) -> Result<Vec<Vec<PathBuf>>, PacketSchemaCatalogError> {
        let (Some((publisher, package)), Some(file_name), Some(pinned)) = (
            packet_id.owner(),
            packet_id.file_name(),
            packet_id.pinned_path(),
        ) else {
            return Ok(Vec::new());
        };
        let packets = self.package_root.join("packets");
        let mut candidates = vec![vec![packets.join(&file_name), packets.join(pinned)]];
        if let Some((skills, added_skills)) = &self.owners {
            candidates.push(vec![skills.join(package).join("packets").join(&file_name)]);
            let versions = added_skills.join(publisher).join(package);
            candidates.push(
                read_dir_sorted(&versions)?
                    .into_iter()
                    .filter(|entry| entry.is_dir)
                    .map(|entry| entry.path.join("packets").join(&file_name))
                    .collect(),
            );
        }
        Ok(candidates)
    }
}

pub(crate) fn read_packet_schema(
    path: &Path,
) -> Result<Option<PacketSchemaEntry>, PacketSchemaCatalogError> {
    let source = read_to_string(path)?;
    packet_schema_entry(path.to_path_buf(), source)
}

/// A schema file's entry, parsed once per exact path and bytes: every output
/// check of a run reads the same packet files, and a changed file is another
/// source.
pub(crate) fn packet_schema_entry(
    path: PathBuf,
    source: String,
) -> Result<Option<PacketSchemaEntry>, PacketSchemaCatalogError> {
    use sha2::{Digest, Sha256};
    use std::sync::{Mutex, OnceLock};

    type Parsed = BTreeMap<(PathBuf, [u8; 32]), Option<PacketSchemaEntry>>;
    const RETAINED: usize = 1024;
    static PARSED: OnceLock<Mutex<Parsed>> = OnceLock::new();

    let key = (
        path.clone(),
        <[u8; 32]>::from(Sha256::digest(source.as_bytes())),
    );
    let cache = PARSED.get_or_init(Mutex::default);
    if let Some(entry) = cache.lock().ok().and_then(|cache| cache.get(&key).cloned()) {
        return Ok(entry);
    }
    let label = path.to_string_lossy().into_owned();
    let entry = parse_packet_schema_document(label, &source)?.map(|schema| PacketSchemaEntry {
        path,
        source,
        schema,
    });
    if let Ok(mut cache) = cache.lock() {
        if cache.len() >= RETAINED {
            cache.clear();
        }
        cache.insert(key, entry.clone());
    }
    Ok(entry)
}

pub(crate) fn declared_runner_packet_ids(runner: &SkillRunnerDefinition) -> BTreeSet<PacketId> {
    let mut packet_ids = BTreeSet::new();
    extend_input_packet_ids(&runner.inputs, &mut packet_ids);
    for step in &runner.graph.steps {
        extend_packet_ids(step.artifacts.as_ref(), &mut packet_ids);
    }
    packet_ids
}

pub(crate) fn declared_tool_packet_ids(tool: &ValidatedTool) -> BTreeSet<PacketId> {
    let mut packet_ids = declared_artifact_packet_ids(tool.artifacts.as_ref());
    extend_input_packet_ids(&tool.inputs, &mut packet_ids);
    packet_ids
}

pub(crate) fn declared_input_packet_ids(
    inputs: &BTreeMap<String, SkillInput>,
) -> BTreeSet<PacketId> {
    inputs
        .values()
        .filter_map(|input| input.packet.clone())
        .collect()
}

pub(crate) fn declared_artifact_packet_ids(
    artifacts: Option<&SkillArtifactContract>,
) -> BTreeSet<PacketId> {
    let mut packet_ids = BTreeSet::new();
    extend_packet_ids(artifacts, &mut packet_ids);
    packet_ids
}

fn extend_packet_ids(
    artifacts: Option<&SkillArtifactContract>,
    packet_ids: &mut BTreeSet<PacketId>,
) {
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
    packet_ids: &mut BTreeSet<PacketId>,
) {
    packet_ids.extend(inputs.values().filter_map(|input| input.packet.clone()));
}

#[cfg(test)]
mod tests {
    use super::{PacketSchemaCatalog, PacketSchemaCatalogError, packet_schema_entry};
    use runx_contracts::PacketId;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn packet(id: &str, shape: &str) -> Result<String, &'static str> {
        let packet_id = PacketId::new(id).ok_or("packet id")?;
        Ok(format!(
            r#"{{"$id":"{}","x-runx-packet-id":"{id}","type":"{shape}"}}"#,
            packet_id.schema_url()
        ))
    }

    fn write_skill(root: &Path, name: &str) -> Result<PathBuf, std::io::Error> {
        let package = root.join("skills").join(name);
        fs::create_dir_all(package.join("packets"))?;
        fs::write(
            package.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Test.\n---\n"),
        )?;
        Ok(package)
    }

    #[test]
    fn catalog_accepts_identical_documents_and_rejects_conflicts()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = packet("acme/test.plan.v1", "object")?;
        let mut catalog = PacketSchemaCatalog::default();
        catalog.insert(
            packet_schema_entry(PathBuf::from("first.json"), source.clone())?
                .ok_or("first schema missing")?,
        )?;
        catalog.insert(
            packet_schema_entry(PathBuf::from("second.json"), source)?
                .ok_or("second schema missing")?,
        )?;

        let conflict = packet_schema_entry(
            PathBuf::from("conflict.json"),
            packet("acme/test.plan.v1", "string")?,
        )?
        .ok_or("conflicting schema missing")?;
        assert!(matches!(
            catalog.insert(conflict),
            Err(PacketSchemaCatalogError::Conflict { .. })
        ));
        Ok(())
    }

    #[test]
    fn core_packets_are_the_published_schema_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = PacketSchemaCatalog::native_public()?;
        let id = PacketId::new("runx.external_job_stage.request.v1").ok_or("id")?;
        let entry = catalog.get(&id).ok_or("core packet missing")?;
        let published = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../schemas/external-job-stage-request.schema.json"),
        )?;
        assert_eq!(entry.source, published);
        assert_eq!(
            entry.schema.digest,
            runx_contracts::sha256_prefixed(published.as_bytes())
        );
        Ok(())
    }

    #[test]
    fn a_package_packet_resolves_from_the_package_its_id_names()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let reader = write_skill(workspace.path(), "cold-outreach")?;
        let owner = write_skill(workspace.path(), "lead-qualify")?;
        let qualification = packet("acme/lead-qualify.qualification.v1", "object")?;
        fs::write(
            owner.join("packets/qualification.v1.schema.json"),
            &qualification,
        )?;
        // The execution workspace and any workspace build output are never
        // packet sources.
        fs::create_dir_all(workspace.path().join("dist/packets"))?;
        fs::write(
            workspace.path().join("dist/packets/stale.schema.json"),
            packet("acme/lead-qualify.stale.v1", "object")?,
        )?;

        let id = PacketId::new("acme/lead-qualify.qualification.v1").ok_or("id")?;
        let stale = PacketId::new("acme/lead-qualify.stale.v1").ok_or("id")?;
        let catalog = PacketSchemaCatalog::for_package(&reader, [&id, &stale])?;
        assert_eq!(
            catalog.get(&id).map(|entry| &entry.source),
            Some(&qualification)
        );
        assert!(catalog.get(&stale).is_none());
        assert!(catalog.not_found(&stale).contains("lead-qualify"));
        Ok(())
    }

    #[test]
    fn an_installed_package_resolves_only_what_its_bundle_carries()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let owner = write_skill(workspace.path(), "lead-qualify")?;
        fs::write(
            owner.join("packets/qualification.v1.schema.json"),
            packet("acme/lead-qualify.qualification.v1", "object")?,
        )?;
        let installed = workspace
            .path()
            .join(".runx/added-skills/acme/cold-outreach/1.0.0");
        fs::create_dir_all(installed.join("packets/acme/lead-qualify"))?;
        fs::write(
            installed.join("SKILL.md"),
            "---\nname: cold-outreach\ndescription: Test.\n---\n",
        )?;
        fs::write(
            installed.join("runx.package.json"),
            r#"{"schema":"runx.registry.package_bundle.v1","dependencies":[],"connectors":[]}"#,
        )?;
        let id = PacketId::new("acme/lead-qualify.qualification.v1").ok_or("id")?;

        let catalog = PacketSchemaCatalog::for_package(&installed, [&id])?;
        assert!(
            catalog.get(&id).is_none(),
            "a bundle never reads the workspace"
        );

        let pinned = packet("acme/lead-qualify.qualification.v1", "object")?;
        fs::write(
            installed.join("packets/acme/lead-qualify/qualification.v1.schema.json"),
            &pinned,
        )?;
        let catalog = PacketSchemaCatalog::for_package(&installed, [&id])?;
        assert_eq!(catalog.get(&id).map(|entry| &entry.source), Some(&pinned));
        Ok(())
    }
}
