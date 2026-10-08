// Module rationale: native list discovery keeps tool, skill,
// graph, packet, and overlay projection in one audited surface.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub use runx_contracts::{
    RunxListEmit, RunxListItem, RunxListItemKind, RunxListReport, RunxListRequestedKind,
    RunxListSchema, RunxListSource, RunxListStatus,
};
use serde::Deserialize;

use crate::RuntimeError;
use crate::filesystem::{read_dir_sorted, read_to_string};
use crate::path_util::{count_yaml_files, display_path, lexical_normalize, project_path};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunxListOptions {
    pub root: PathBuf,
    pub requested_kind: RunxListRequestedKind,
}

#[must_use]
pub fn default_list_options(root: PathBuf) -> RunxListOptions {
    RunxListOptions {
        root,
        requested_kind: RunxListRequestedKind::All,
    }
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
        RunxListRequestedKind::All | RunxListRequestedKind::Skills | RunxListRequestedKind::Graphs
    ) {
        items.extend(
            discover_skill_and_graph_list_items(root)?
                .into_iter()
                .filter(|item| match requested_kind {
                    RunxListRequestedKind::All => true,
                    RunxListRequestedKind::Skills => {
                        matches!(item.kind, RunxListItemKind::Skill | RunxListItemKind::Graph)
                    }
                    RunxListRequestedKind::Graphs => item.kind == RunxListItemKind::Graph,
                    _ => false,
                }),
        );
    }
    if matches!(
        requested_kind,
        RunxListRequestedKind::All | RunxListRequestedKind::Packets
    ) {
        items.extend(discover_packet_list_items(root)?);
    }
    if matches!(
        requested_kind,
        RunxListRequestedKind::All | RunxListRequestedKind::Overlays
    ) {
        items.extend(discover_overlay_list_items(root)?);
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
                    diagnostics: None,
                    scopes: Some(tool.scopes),
                    emits: tool
                        .artifacts
                        .as_ref()
                        .map(tool_emits)
                        .filter(|items| !items.is_empty()),
                    fixtures: Some(count_yaml_files(&tool_entry.path.join("fixtures"))?),
                    harness_cases: None,
                    steps: None,
                    wraps: None,
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

fn tool_emits(artifacts: &runx_parser::SkillArtifactContract) -> Vec<RunxListEmit> {
    if let Some(named_emits) = &artifacts.named_emits {
        return named_emits
            .keys()
            .map(|name| RunxListEmit {
                name: name.clone(),
                packet: artifacts
                    .packets
                    .as_ref()
                    .and_then(|packets| packets.get(name))
                    .cloned(),
            })
            .collect();
    }
    artifacts
        .wrap_as
        .iter()
        .map(|name| RunxListEmit {
            name: name.clone(),
            packet: artifacts.packet.clone(),
        })
        .collect()
}

fn discover_skill_and_graph_list_items(root: &Path) -> Result<Vec<RunxListItem>, RuntimeError> {
    let mut items = Vec::new();
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
            let steps = manifest
                .runners
                .values()
                .map(|runner| runner.graph.steps.len() as u64)
                .sum();
            RunxListItem {
                kind: RunxListItemKind::Skill,
                name: manifest.skill.clone(),
                source: RunxListSource::Local,
                path: project_path(root, &profile_path),
                status: RunxListStatus::Ok,
                diagnostics: None,
                scopes: skill_scopes(manifest),
                emits: skill_emits(manifest),
                fixtures: Some(loaded.package.harness_fixtures.len() as u64),
                harness_cases: Some(
                    manifest
                        .harness
                        .as_ref()
                        .map_or(0, |harness| harness.cases.len() as u64),
                ),
                steps: Some(steps),
                wraps: None,
            }
        })
        .collect()
}

fn skill_scopes(manifest: &runx_parser::SkillRunnerManifest) -> Option<Vec<String>> {
    let scopes = manifest
        .runners
        .values()
        .flat_map(runx_parser::SkillRunnerDefinition::declared_scopes)
        .collect::<BTreeSet<_>>();
    (!scopes.is_empty()).then(|| scopes.into_iter().collect())
}

fn skill_emits(manifest: &runx_parser::SkillRunnerManifest) -> Option<Vec<RunxListEmit>> {
    let mut emits = BTreeSet::<(String, Option<String>)>::new();
    for step in manifest
        .runners
        .values()
        .flat_map(|runner| runner.graph.steps.iter())
    {
        if let Some(artifacts) = step.artifacts.as_ref() {
            emits.extend(
                tool_emits(artifacts)
                    .into_iter()
                    .map(|emit| (emit.name, emit.packet)),
            );
        }
    }
    (!emits.is_empty()).then(|| {
        emits
            .into_iter()
            .map(|(name, packet)| RunxListEmit { name, packet })
            .collect()
    })
}

// Function rationale: packet discovery keeps glob expansion,
// schema-id extraction, and duplicate-id diagnostics in one deterministic pass.
fn discover_packet_list_items(root: &Path) -> Result<Vec<RunxListItem>, RuntimeError> {
    let package_json_path = root.join("package.json");
    if !package_json_path.exists() {
        return Ok(Vec::new());
    }

    let source = read_to_string(&package_json_path)?;
    let package_json = match serde_json::from_str::<PackageJson>(&source) {
        Ok(package_json) => package_json,
        Err(_) => {
            return Ok(vec![invalid_item(
                RunxListItemKind::Packet,
                "package.json".to_owned(),
                "package.json".to_owned(),
                "runx.packet.package.invalid",
            )]);
        }
    };

    let mut items = Vec::new();
    let mut catalog = crate::packet_schemas::PacketSchemaCatalog::default();
    for packet_glob in package_json
        .runx
        .as_ref()
        .map(|runx| runx.packets.as_slice())
        .unwrap_or_default()
    {
        let files = expand_local_glob(root, packet_glob)?;
        if files.is_empty() {
            items.push(invalid_item(
                RunxListItemKind::Packet,
                packet_glob.clone(),
                "package.json".to_owned(),
                "runx.packet.ref.missing",
            ));
            continue;
        }
        for file_path in files {
            let relative_path = project_path(root, &file_path);
            let schema = match crate::packet_schemas::read_packet_schema(&file_path) {
                Ok(Some(schema)) => schema,
                Ok(None) => {
                    items.push(invalid_item(
                        RunxListItemKind::Packet,
                        relative_path.clone(),
                        relative_path,
                        "runx.packet.id.mismatch",
                    ));
                    continue;
                }
                Err(_) => {
                    items.push(invalid_item(
                        RunxListItemKind::Packet,
                        relative_path.clone(),
                        relative_path,
                        "runx.packet.schema.invalid",
                    ));
                    continue;
                }
            };
            let packet_id = schema.schema.packet_id.clone();
            if catalog.insert(schema).is_err() {
                items.push(invalid_item(
                    RunxListItemKind::Packet,
                    packet_id,
                    relative_path,
                    "runx.packet.id.collision",
                ));
                continue;
            }
            items.push(RunxListItem {
                kind: RunxListItemKind::Packet,
                name: packet_id,
                source: RunxListSource::Local,
                path: relative_path,
                status: RunxListStatus::Ok,
                diagnostics: None,
                scopes: None,
                emits: None,
                fixtures: None,
                harness_cases: None,
                steps: None,
                wraps: None,
            });
        }
    }
    Ok(items)
}

fn discover_overlay_list_items(root: &Path) -> Result<Vec<RunxListItem>, RuntimeError> {
    let overlays_root = root.join("skills-overlays");
    let mut items = Vec::new();
    for vendor_entry in read_dir_sorted(&overlays_root)? {
        if !vendor_entry.is_dir {
            continue;
        }
        for skill_entry in read_dir_sorted(&vendor_entry.path)? {
            if !skill_entry.is_dir {
                continue;
            }
            let profile_path = skill_entry.path.join(runx_parser::PROFILE_FILE);
            if !profile_path.exists() {
                continue;
            }
            let contents = read_to_string(&profile_path)?;
            items.push(RunxListItem {
                kind: RunxListItemKind::Overlay,
                name: format!("{}/{}", vendor_entry.name, skill_entry.name),
                source: RunxListSource::Local,
                path: project_path(root, &profile_path),
                status: RunxListStatus::Ok,
                diagnostics: None,
                scopes: None,
                emits: None,
                fixtures: None,
                harness_cases: None,
                steps: None,
                wraps: overlay_wraps(&contents),
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
        diagnostics: Some(vec![diagnostic.to_owned()]),
        scopes: None,
        emits: None,
        fixtures: None,
        harness_cases: None,
        steps: None,
        wraps: None,
    }
}

#[derive(Deserialize)]
struct PackageJson {
    runx: Option<PackageRunxConfig>,
}

#[derive(Deserialize)]
struct PackageRunxConfig {
    #[serde(default)]
    packets: Vec<String>,
}

fn expand_local_glob(root: &Path, glob: &str) -> Result<Vec<PathBuf>, RuntimeError> {
    if !glob.contains('*') {
        let path = root.join(glob);
        return Ok(path.exists().then_some(path).into_iter().collect());
    }

    let normalized = glob.replace('\\', "/");
    let Some(star) = normalized.find('*') else {
        return Ok(Vec::new());
    };
    let base = &normalized[..star];
    let base_dir = base.rfind('/').map_or("", |slash| &base[..=slash]);
    let suffix = &normalized[star + 1..];
    let mut files = read_dir_sorted(&root.join(base_dir))?
        .into_iter()
        .filter(|entry| entry.is_file && display_path(&entry.path).ends_with(suffix))
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
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

fn overlay_wraps(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("wraps:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
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
        RunxListItemKind::Graph => 2,
        RunxListItemKind::Packet => 3,
        RunxListItemKind::Overlay => 4,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn overlay_wraps_reads_plain_wraps_line() {
        assert_eq!(
            overlay_wraps("name: demo\n  wraps: vendor/base\n"),
            Some("vendor/base".to_owned())
        );
    }

    #[test]
    fn named_emit_lists_only_explicit_packet_binding() {
        let artifacts = runx_parser::SkillArtifactContract {
            emits: None,
            named_emits: Some(BTreeMap::from([("plan".to_owned(), "plan".to_owned())])),
            packets: Some(BTreeMap::from([(
                "plan".to_owned(),
                "runx.plan.v1".to_owned(),
            )])),
            wrap_as: None,
            packet: None,
        };

        assert_eq!(
            tool_emits(&artifacts),
            vec![RunxListEmit {
                name: "plan".to_owned(),
                packet: Some("runx.plan.v1".to_owned()),
            }]
        );
    }

    #[test]
    fn named_emit_output_name_is_not_reported_as_a_packet() {
        let artifacts = runx_parser::SkillArtifactContract {
            emits: None,
            named_emits: Some(BTreeMap::from([("plan".to_owned(), "plan".to_owned())])),
            packets: None,
            wrap_as: None,
            packet: None,
        };

        assert_eq!(
            tool_emits(&artifacts),
            vec![RunxListEmit {
                name: "plan".to_owned(),
                packet: None,
            }]
        );
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
            diagnostics: None,
            scopes: None,
            emits: None,
            fixtures: None,
            harness_cases: None,
            steps: None,
            wraps: None,
        }
    }
}
