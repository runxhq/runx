use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::PacketId;

use super::insert_source_file;
use crate::LoadedSkillPackage;
use crate::packet_schemas::{
    PacketSchemaCatalog, declared_runner_packet_ids, declared_tool_packet_ids,
};
use crate::registry::RegistryPackageFile;
use crate::registry::publish_package::RegistryPublishPackageError;

/// Carry, pinned, each packet the package reads from another package (R6).
/// Its own packets are already package files, and a core packet is never
/// copied: the runx that runs the package carries it.
pub(super) fn append_declared_packet_schemas(
    files: &mut BTreeMap<String, RegistryPackageFile>,
    loaded: &LoadedSkillPackage,
    packet_ids: &BTreeSet<PacketId>,
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
    let schemas = PacketSchemaCatalog::for_validated_package(
        &loaded.package,
        &loaded.package_root,
        loaded.resolved_input_packet_schemas.values(),
        &required_packet_ids,
    )
    .map_err(|error| {
        RegistryPublishPackageError::invalid(format!("packet schema catalog failed: {error}"))
    })?;
    for packet_id in &required_packet_ids {
        let Some(schema) = schemas.get(packet_id) else {
            return Err(RegistryPublishPackageError::invalid(
                schemas.not_found(packet_id),
            ));
        };
        let Some(pinned) = packet_id.pinned_path() else {
            continue;
        };
        if loaded.package.packets.contains_key(packet_id) {
            continue;
        }
        insert_source_file(
            files,
            &format!("packets/{pinned}"),
            schema.source.as_bytes(),
        )?;
    }
    Ok(())
}
