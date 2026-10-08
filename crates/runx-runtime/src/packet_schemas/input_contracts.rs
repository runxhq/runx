use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use runx_contracts::{InputDefinition, JsonObject};
use runx_parser::{
    SkillPackageError, ValidatedSkillPackage, ValidatedTool, validate_input_examples,
};

use super::{
    PacketSchemaCatalog, PacketSchemaEntry, declared_input_packet_ids, package_packet_directories,
    packet_schema_directories,
};

/// Resolve every packet-valued input exactly once at package admission. All
/// downstream consumers then use the ordinary input schema path; packet
/// references do not create a parallel validator or inspection surface.
pub(crate) fn hydrate_packet_input_contracts(
    package: &mut ValidatedSkillPackage,
    selected_directory: &Path,
    package_root: &Path,
) -> Result<BTreeMap<String, PacketSchemaEntry>, SkillPackageError> {
    if !package_uses_packet_inputs(package) {
        return Ok(BTreeMap::new());
    }

    let mut catalog = PacketSchemaCatalog::native_public().map_err(package_error)?;
    catalog
        .discover_validated_package(package, package_root)
        .map_err(package_error)?;
    let local_directories = package_packet_directories(package)
        .into_iter()
        .map(|directory| package_root.join(directory))
        .collect::<Vec<_>>();
    catalog
        .discover_directories(
            packet_schema_directories(selected_directory, package_root, package_root)
                .map_err(package_error)?
                .into_iter()
                .filter(|directory| !local_directories.contains(directory)),
        )
        .map_err(package_error)?;

    for (profile_path, manifest) in &mut package.profiles {
        hydrate_inputs(
            &mut manifest.input_definitions,
            &format!("{profile_path} input_definitions"),
            &catalog,
        )?;
        for (runner_name, runner) in &mut manifest.runners {
            let owner = format!("{profile_path} runner {runner_name}");
            hydrate_inputs(&mut runner.inputs, &owner, &catalog)?;
            validate_input_examples(
                &format!("runners.{runner_name}.examples"),
                &runner.examples,
                &runner.inputs,
            )
            .map_err(|source| SkillPackageError::Validation {
                path: profile_path.clone(),
                source,
            })?;
        }
    }
    for (manifest_path, package_tool) in &mut package.tools {
        hydrate_inputs(
            &mut package_tool.tool.inputs,
            &format!("tool {manifest_path}"),
            &catalog,
        )?;
    }
    package_input_packet_ids(package)
        .into_iter()
        .map(|packet_id| {
            let entry = catalog.get(&packet_id).cloned().ok_or_else(|| {
                SkillPackageError::invalid(
                    "packet schemas",
                    format!("admitted input packet schema '{packet_id}' disappeared"),
                )
            })?;
            Ok((packet_id, entry))
        })
        .collect()
}

pub(crate) fn hydrate_standalone_tool_input_contracts(
    tool: &mut ValidatedTool,
    manifest_path: &Path,
) -> Result<(), SkillPackageError> {
    if !inputs_use_packets(&tool.inputs) {
        return Ok(());
    }
    let tool_directory = manifest_path.parent().ok_or_else(|| {
        SkillPackageError::invalid(
            manifest_path.to_string_lossy(),
            "tool manifest has no owning directory",
        )
    })?;
    let catalog = PacketSchemaCatalog::discover(
        packet_schema_directories(tool_directory, tool_directory, tool_directory)
            .map_err(package_error)?,
    )
    .map_err(package_error)?;
    hydrate_inputs(
        &mut tool.inputs,
        &format!("tool {}", manifest_path.display()),
        &catalog,
    )
}

fn package_uses_packet_inputs(package: &ValidatedSkillPackage) -> bool {
    package.profiles.values().any(|manifest| {
        inputs_use_packets(&manifest.input_definitions)
            || manifest
                .runners
                .values()
                .any(|runner| inputs_use_packets(&runner.inputs))
    }) || package
        .tools
        .values()
        .any(|tool| inputs_use_packets(&tool.tool.inputs))
}

fn inputs_use_packets(inputs: &BTreeMap<String, InputDefinition>) -> bool {
    inputs.values().any(|input| input.packet.is_some())
}

fn package_input_packet_ids(package: &ValidatedSkillPackage) -> BTreeSet<String> {
    let mut packet_ids = BTreeSet::new();
    for manifest in package.profiles.values() {
        packet_ids.extend(declared_input_packet_ids(&manifest.input_definitions));
        for runner in manifest.runners.values() {
            packet_ids.extend(declared_input_packet_ids(&runner.inputs));
        }
    }
    for package_tool in package.tools.values() {
        packet_ids.extend(declared_input_packet_ids(&package_tool.tool.inputs));
    }
    packet_ids
}

fn hydrate_inputs(
    inputs: &mut BTreeMap<String, InputDefinition>,
    owner: &str,
    catalog: &PacketSchemaCatalog,
) -> Result<(), SkillPackageError> {
    for (name, input) in inputs {
        let Some(packet_id) = input.packet.as_deref() else {
            continue;
        };
        let entry = catalog.get(packet_id).ok_or_else(|| {
            SkillPackageError::invalid(
                owner,
                format!("input '{name}' declares missing packet schema '{packet_id}'"),
            )
        })?;
        let schema = entry.schema.value.as_object().cloned().ok_or_else(|| {
            SkillPackageError::invalid(
                owner,
                format!("packet schema '{packet_id}' must be a JSON Schema object"),
            )
        })?;
        validate_packet_schema(owner, name, packet_id, &schema)?;
        input.schema = Some(schema);
    }
    Ok(())
}

fn validate_packet_schema(
    owner: &str,
    input: &str,
    packet_id: &str,
    schema: &JsonObject,
) -> Result<(), SkillPackageError> {
    let schema = serde_json::to_value(schema).map_err(|source| {
        SkillPackageError::invalid(
            owner,
            format!(
                "input '{input}' packet schema '{packet_id}' could not be serialized: {source}"
            ),
        )
    })?;
    jsonschema::draft202012::meta::validate(&schema).map_err(|source| {
        SkillPackageError::invalid(
            owner,
            format!("input '{input}' packet schema '{packet_id}' is invalid: {source}"),
        )
    })
}

fn package_error(error: impl std::fmt::Display) -> SkillPackageError {
    SkillPackageError::invalid("packet schemas", error.to_string())
}
