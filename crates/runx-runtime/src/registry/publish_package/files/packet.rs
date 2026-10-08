use std::collections::BTreeMap;
use std::path::Path;

use super::insert_source_file;
use crate::LoadedSkillPackage;
use crate::packet_schemas::{
    PacketSchemaCatalog, declared_runner_packet_ids, declared_tool_packet_ids,
    packet_schema_directories,
};
use crate::registry::RegistryPackageFile;
use crate::registry::publish_package::RegistryPublishPackageError;

pub(super) fn append_declared_packet_schemas(
    files: &mut BTreeMap<String, RegistryPackageFile>,
    loaded: &LoadedSkillPackage,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    packet_ids: &std::collections::BTreeSet<String>,
) -> Result<(), RegistryPublishPackageError> {
    // Bundles retain the full manifest, including runners outside the selected execution closure.
    let mut required_packet_ids = packet_ids.clone();
    required_packet_ids.extend(loaded.resolved_input_packet_schemas.keys().cloned());
    for manifest in loaded.package.profiles.values() {
        for runner in manifest.runners.values() {
            required_packet_ids.extend(declared_runner_packet_ids(runner));
        }
    }
    for package_tool in loaded.package.tools.values() {
        required_packet_ids.extend(declared_tool_packet_ids(&package_tool.tool));
    }
    if required_packet_ids.is_empty() {
        return Ok(());
    }
    let workspace = crate::RunxPaths::resolve(env, cwd)
        .workspace()
        .to_path_buf();
    let mut schemas = PacketSchemaCatalog::native_public().map_err(|error| {
        RegistryPublishPackageError::invalid(format!("packet schema catalog failed: {error}"))
    })?;
    schemas.discover_loaded_package(loaded).map_err(|error| {
        RegistryPublishPackageError::invalid(format!("packet schema catalog failed: {error}"))
    })?;
    schemas
        .discover_directories(
            packet_schema_directories(&loaded.directory, &loaded.package_root, &workspace)
                .map_err(|error| {
                    RegistryPublishPackageError::invalid(format!(
                        "packet schema roots failed: {error}"
                    ))
                })?
                .into_iter()
                .filter(|directory| {
                    directory != &loaded.directory.join("packets")
                        && directory != &loaded.package_root.join("packets")
                }),
        )
        .map_err(|error| {
            RegistryPublishPackageError::invalid(format!("packet schema catalog failed: {error}"))
        })?;
    for packet_id in &required_packet_ids {
        let Some(schema) = loaded
            .resolved_input_packet_schemas
            .get(packet_id)
            .or_else(|| schemas.get(packet_id))
        else {
            return Err(RegistryPublishPackageError::invalid(format!(
                "declared packet schema '{packet_id}' was not found"
            )));
        };
        insert_source_file(
            files,
            &format!("packets/{}", schema.file_name),
            schema.source.as_bytes(),
        )?;
    }
    Ok(())
}
