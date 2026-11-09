// Module rationale: native list discovery keeps tool, skill and
// packet projection in one audited surface.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub use runx_contracts::{
    RunxListItem, RunxListItemKind, RunxListOutputPacket, RunxListReport, RunxListRequestedKind,
    RunxListSchema, RunxListSource, RunxListStatus, RunxListVisibility,
};

use crate::RuntimeError;
use crate::filesystem::read_dir_sorted;
use crate::path_util::{count_yaml_files, display_path, lexical_normalize, project_path};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunxListOptions {
    pub root: PathBuf,
    pub requested_kind: RunxListRequestedKind,
}

pub fn list_authoring_primitives(
    options: &RunxListOptions,
) -> Result<RunxListReport, RuntimeError> {
    list_authoring_primitives_with_effects(options, &crate::RuntimeEffectRegistry::default())
}

pub fn list_authoring_primitives_with_effects(
    options: &RunxListOptions,
    effects: &crate::RuntimeEffectRegistry,
) -> Result<RunxListReport, RuntimeError> {
    let root = lexical_normalize(&options.root);
    let mut items = discover_list_items(&root, options.requested_kind, effects)?;
    sort_list_items(&mut items);
    Ok(RunxListReport {
        schema: RunxListSchema::V1,
        root: display_path(&root),
        requested_kind: options.requested_kind,
        items,
    })
}

fn discover_list_items(
    root: &Path,
    requested_kind: RunxListRequestedKind,
    effects: &crate::RuntimeEffectRegistry,
) -> Result<Vec<RunxListItem>, RuntimeError> {
    let mut items = Vec::new();
    if matches!(
        requested_kind,
        RunxListRequestedKind::All | RunxListRequestedKind::Tools
    ) {
        items.extend(discover_tool_list_items(root, effects)?);
    }
    if matches!(
        requested_kind,
        RunxListRequestedKind::All | RunxListRequestedKind::Skills
    ) {
        items.extend(discover_skill_list_items(root)?);
    }
    if matches!(
        requested_kind,
        RunxListRequestedKind::All | RunxListRequestedKind::Packets
    ) {
        items.extend(discover_packet_list_items(root)?);
    }
    Ok(items)
}

fn discover_tool_list_items(
    root: &Path,
    effects: &crate::RuntimeEffectRegistry,
) -> Result<Vec<RunxListItem>, RuntimeError> {
    let tools_root = root.join("tools");
    let mut items = crate::tool_catalogs::native::list_items(effects);
    let native_names = items
        .iter()
        .map(|item| item.name.clone())
        .collect::<BTreeSet<_>>();
    for namespace_entry in read_dir_sorted(&tools_root)? {
        if !namespace_entry.is_dir {
            continue;
        }
        for tool_entry in read_dir_sorted(&namespace_entry.path)? {
            if !tool_entry.is_dir {
                continue;
            }
            let manifest_path = tool_entry.path.join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            let relative_path = project_path(root, &manifest_path);
            match read_validated_tool_manifest(&manifest_path) {
                Ok(tool) if native_names.contains(&tool.name) => {}
                Ok(tool) => items.push(RunxListItem {
                    kind: RunxListItemKind::Tool,
                    name: tool.name,
                    source: RunxListSource::Local,
                    path: relative_path,
                    status: RunxListStatus::Ok,
                    visibility: None,
                    description: None,
                    methods: None,
                    diagnostics: None,
                    packets: tool
                        .artifacts
                        .as_ref()
                        .map(tool_packets)
                        .filter(|items| !items.is_empty()),
                    fixtures: Some(count_yaml_files(&tool_entry.path.join("fixtures"))?),
                    test_cases: None,
                    steps: None,
                }),
                Err(()) => items.push(invalid_item(
                    RunxListItemKind::Tool,
                    format!("{}.{}", namespace_entry.name, tool_entry.name),
                    relative_path,
                    "runx.tool.manifest.invalid",
                )),
            }
        }
    }
    Ok(items)
}

fn read_validated_tool_manifest(manifest_path: &Path) -> Result<runx_parser::ValidatedTool, ()> {
    crate::tool_catalogs::manifest::read(manifest_path).map_err(|_| ())
}

/// The outputs a tool keeps as artifacts, each with its packet.
fn tool_packets(artifacts: &runx_parser::OutputArtifacts) -> Vec<RunxListOutputPacket> {
    artifacts
        .outputs()
        .map(|name| RunxListOutputPacket {
            name: name.clone(),
            packet: artifacts.packets.get(name).cloned(),
        })
        .collect()
}

fn discover_skill_list_items(root: &Path) -> Result<Vec<RunxListItem>, RuntimeError> {
    let mut items = Vec::new();
    for skill in crate::registry::bundled_skills().map_err(|message| RuntimeError::SkillFailed {
        skill_name: "runx core".to_owned(),
        message,
    })? {
        let package = skill.validated_package()?;
        if let Some(manifest) = package.root_manifest() {
            let mut item = skill_profile_list_item(
                manifest,
                package.skill.description.clone(),
                skill.skill_id.clone(),
                RunxListSource::BuiltIn,
                package.test_fixtures.len() as u64,
            );
            item.name.clone_from(&skill.skill_id);
            items.push(item);
        }
    }
    for skill_dir in crate::skill_package::discover_workspace_skill_package_dirs(root)? {
        let fallback_name = fallback_skill_name(root, &skill_dir);
        match crate::load_validated_skill_package(&skill_dir) {
            Ok(loaded) => items.extend(skill_list_items(root, &loaded)),
            Err(_) => items.push(invalid_item(
                RunxListItemKind::Skill,
                fallback_name,
                project_path(root, &skill_dir.join("SKILL.md")),
                "runx.skill.profile.invalid",
            )),
        }
    }
    Ok(items)
}

fn skill_list_items(
    root: &Path,
    loaded: &crate::skill_package::LoadedSkillPackage,
) -> Vec<RunxListItem> {
    loaded
        .package
        .profiles
        .iter()
        .map(|(profile_relative, manifest)| {
            let profile_path = loaded.package_root.join(profile_relative);
            skill_profile_list_item(
                manifest,
                profile_description(&loaded.package, profile_relative),
                project_path(root, &profile_path),
                RunxListSource::Local,
                loaded.package.test_fixtures.len() as u64,
            )
        })
        .collect()
}

/// The description in a profile's own manual: the package's `SKILL.md` for
/// the root profile, and the `SKILL.md` beside a nested stage's profile.
fn profile_description(
    package: &runx_parser::ValidatedSkillPackage,
    profile: &str,
) -> Option<String> {
    let directory = runx_parser::profile_directory(profile)?;
    if directory.is_empty() {
        return package.skill.description.clone();
    }
    let manual = package.file_text(&format!("{directory}/SKILL.md"))?;
    runx_parser::validate_skill(runx_parser::parse_skill_markdown(manual).ok()?)
        .ok()?
        .description
}

fn skill_profile_list_item(
    manifest: &runx_parser::SkillMethodManifest,
    description: Option<String>,
    path: String,
    source: RunxListSource,
    fixtures: u64,
) -> RunxListItem {
    RunxListItem {
        kind: RunxListItemKind::Skill,
        name: manifest.skill.clone(),
        source,
        path,
        status: RunxListStatus::Ok,
        visibility: Some(
            match manifest
                .catalog
                .as_ref()
                .map_or(runx_parser::CatalogVisibility::Public, |catalog| {
                    catalog.visibility
                }) {
                runx_parser::CatalogVisibility::Public => RunxListVisibility::Public,
                runx_parser::CatalogVisibility::Internal => RunxListVisibility::Internal,
            },
        ),
        description,
        methods: Some(crate::skill_package::catalog_methods(manifest)),
        diagnostics: None,
        packets: skill_packets(manifest),
        fixtures: Some(fixtures),
        test_cases: None,
        steps: Some(
            manifest
                .methods
                .values()
                .map(|method| method.graph.steps.len() as u64)
                .sum(),
        ),
    }
}

fn skill_packets(manifest: &runx_parser::SkillMethodManifest) -> Option<Vec<RunxListOutputPacket>> {
    let mut packets = BTreeSet::<(String, Option<runx_contracts::PacketId>)>::new();
    for step in manifest
        .methods
        .values()
        .flat_map(|method| method.graph.steps.iter())
    {
        if let Some(artifacts) = step.artifacts.as_ref() {
            packets.extend(
                tool_packets(artifacts)
                    .into_iter()
                    .map(|output| (output.name, output.packet)),
            );
        }
    }
    (!packets.is_empty()).then(|| {
        packets
            .into_iter()
            .map(|(name, packet)| RunxListOutputPacket { name, packet })
            .collect()
    })
}

/// The packets each workspace package owns, as the parser admitted them with
/// the package. A package that does not admit lists each file in its
/// `packets/` as invalid, since none of them resolves.
fn discover_packet_list_items(root: &Path) -> Result<Vec<RunxListItem>, RuntimeError> {
    let mut items = Vec::new();
    for skill_dir in crate::skill_package::discover_workspace_skill_package_dirs(root)? {
        let Ok(loaded) = crate::load_validated_skill_package(&skill_dir) else {
            for entry in read_dir_sorted(&skill_dir.join("packets"))? {
                if entry.is_file {
                    let path = project_path(root, &entry.path);
                    items.push(invalid_item(
                        RunxListItemKind::Packet,
                        path.clone(),
                        path,
                        "runx.packet.package.invalid",
                    ));
                }
            }
            continue;
        };
        for (packet_id, packet) in &loaded.package.packets {
            let own = packet_id
                .file_name()
                .is_some_and(|file_name| packet.path == format!("packets/{file_name}"));
            if !own {
                continue;
            }
            items.push(RunxListItem {
                kind: RunxListItemKind::Packet,
                name: packet_id.to_string(),
                source: RunxListSource::Local,
                path: project_path(root, &loaded.package_root.join(&packet.path)),
                status: RunxListStatus::Ok,
                visibility: None,
                description: None,
                methods: None,
                diagnostics: None,
                packets: None,
                fixtures: None,
                test_cases: None,
                steps: None,
            });
        }
    }
    Ok(items)
}

fn invalid_item(
    kind: RunxListItemKind,
    name: String,
    path: String,
    diagnostic: &str,
) -> RunxListItem {
    RunxListItem {
        kind,
        name,
        source: RunxListSource::Local,
        path,
        status: RunxListStatus::Invalid,
        visibility: None,
        description: None,
        methods: None,
        diagnostics: Some(vec![diagnostic.to_owned()]),
        packets: None,
        fixtures: None,
        test_cases: None,
        steps: None,
    }
}

fn fallback_skill_name(root: &Path, skill_dir: &Path) -> String {
    if skill_dir == root {
        return root.file_name().map_or_else(
            || ".".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
    }
    skill_dir.file_name().map_or_else(
        || ".".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn sort_list_items(items: &mut [RunxListItem]) {
    items.sort_by(|left, right| {
        source_order(left.source)
            .cmp(&source_order(right.source))
            .then_with(|| kind_order(left.kind).cmp(&kind_order(right.kind)))
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn source_order(source: RunxListSource) -> u8 {
    match source {
        RunxListSource::Local => 0,
        RunxListSource::Workspace => 1,
        RunxListSource::Dependencies => 2,
        RunxListSource::BuiltIn => 3,
    }
}

fn kind_order(kind: RunxListItemKind) -> u8 {
    match kind {
        RunxListItemKind::Tool => 0,
        RunxListItemKind::Skill => 1,
        RunxListItemKind::Packet => 2,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn an_artifact_output_lists_its_packet() -> Result<(), &'static str> {
        let artifacts = runx_parser::OutputArtifacts {
            result_as: None,
            packets: BTreeMap::from([(
                "plan".to_owned(),
                runx_contracts::PacketId::new("runx.plan.v1").ok_or("packet id")?,
            )]),
        };

        assert_eq!(
            tool_packets(&artifacts),
            vec![RunxListOutputPacket {
                name: "plan".to_owned(),
                packet: runx_contracts::PacketId::new("runx.plan.v1"),
            }]
        );
        Ok(())
    }

    #[test]
    fn sorts_by_kind_then_name() {
        let mut items = vec![
            valid_item(RunxListItemKind::Packet, "b"),
            valid_item(RunxListItemKind::Tool, "z"),
            valid_item(RunxListItemKind::Tool, "a"),
            valid_item(RunxListItemKind::Skill, "a"),
        ];
        sort_list_items(&mut items);
        assert_eq!(
            items
                .iter()
                .map(|item| (item.kind, item.name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (RunxListItemKind::Tool, "a"),
                (RunxListItemKind::Tool, "z"),
                (RunxListItemKind::Skill, "a"),
                (RunxListItemKind::Packet, "b"),
            ]
        );
    }

    fn valid_item(kind: RunxListItemKind, name: &str) -> RunxListItem {
        RunxListItem {
            kind,
            name: name.to_owned(),
            source: RunxListSource::Local,
            path: ".".to_owned(),
            status: RunxListStatus::Ok,
            visibility: None,
            description: None,
            methods: None,
            diagnostics: None,
            packets: None,
            fixtures: None,
            test_cases: None,
            steps: None,
        }
    }
}
