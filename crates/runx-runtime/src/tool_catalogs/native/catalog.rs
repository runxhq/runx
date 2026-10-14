use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::tools::{
    ToolBuildStatus, ToolCatalogSearchResult, ToolInput, ToolInspectOrigin, ToolInspectProvenance,
    ToolInspectReport, ToolInspectResult,
};
use runx_contracts::{
    JsonObject, JsonValue, RunxListEmit, RunxListItem, RunxListItemKind, RunxListSource,
    RunxListStatus,
};
use runx_parser::SkillArtifactContract;

use crate::effects::RuntimeEffectRegistry;
use crate::{CapabilityArtifacts, CapabilityContract};

use super::{NativeCapability, core_capabilities, definition};

pub(crate) fn artifacts(
    tool_ref: &str,
    effects: &RuntimeEffectRegistry,
) -> Option<SkillArtifactContract> {
    if let Some(tool) = definition(tool_ref) {
        return artifact_contract(tool.definition().artifacts);
    }
    artifact_contract(effects.capability(tool_ref)?.definition().artifacts)
}

pub(crate) fn inspect(
    tool_ref: &str,
    root: &Path,
    effects: &RuntimeEffectRegistry,
) -> Option<ToolInspectReport> {
    if let Some(tool) = definition(tool_ref) {
        let definition = tool.definition();
        return Some(inspect_report(
            definition.id,
            definition.owner,
            definition.summary,
            tool.catalog_inputs().ok()?,
            definition.scopes,
            root,
        ));
    }

    let tool = effects.capability(tool_ref)?;
    let definition = tool.definition();
    Some(inspect_report(
        definition.id,
        definition.owner,
        definition.summary,
        tool.catalog_inputs().ok()?,
        definition.scopes,
        root,
    ))
}

fn inspect_report(
    name: &str,
    source: &str,
    description: &str,
    inputs: BTreeMap<String, ToolInput>,
    scopes: &[&str],
    root: &Path,
) -> ToolInspectReport {
    ToolInspectReport {
        status: ToolBuildStatus::Success,
        tool: ToolInspectResult {
            tool_ref: name.to_owned(),
            name: name.to_owned(),
            description: Some(description.to_owned()),
            execution: "native".to_owned(),
            inputs,
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            runtime: None,
            reference_path: format!("native:{name}"),
            skill_directory: root.to_string_lossy().into_owned(),
            provenance: ToolInspectProvenance {
                origin: ToolInspectOrigin::Native,
                source: Some(source.to_owned()),
                source_label: Some(source.to_owned()),
                namespace: name
                    .split_once('.')
                    .map(|(namespace, _)| namespace.to_owned()),
                external_name: name
                    .split_once('.')
                    .map(|(_, external_name)| external_name.to_owned()),
                catalog_ref: Some(name.to_owned()),
                tool_id: Some(format!("native/{name}")),
                tags: Some(vec!["native".to_owned(), "runx".to_owned()]),
            },
        },
    }
}

pub(crate) fn search(
    query: &str,
    limit: usize,
    effects: &RuntimeEffectRegistry,
) -> Vec<ToolCatalogSearchResult> {
    let query = query.trim().to_ascii_lowercase();
    let core = core_capabilities().map(|tool| {
        let definition = tool.definition();
        search_result(
            definition.id,
            definition.owner,
            definition.summary,
            definition.scopes,
        )
    });
    let extensions = effects.capabilities().into_iter().map(|tool| {
        let definition = tool.definition();
        search_result(
            definition.id,
            definition.owner,
            definition.summary,
            definition.scopes,
        )
    });

    core.chain(extensions)
        .filter(|tool| query.is_empty() || searchable_tool_text(tool).contains(&query))
        .take(limit)
        .collect()
}

pub(crate) fn inventory(effects: &RuntimeEffectRegistry) -> Vec<JsonValue> {
    list_items(effects)
        .into_iter()
        .map(|tool| {
            let description = definition(&tool.name)
                .map(|definition| definition.definition().summary)
                .or_else(|| {
                    effects
                        .capability(&tool.name)
                        .map(|capability| capability.definition().summary)
                })
                .unwrap_or("Runx-native tool");
            JsonValue::Object(JsonObject::from([
                ("name".to_owned(), JsonValue::String(tool.name)),
                ("kind".to_owned(), JsonValue::String("tool".to_owned())),
                (
                    "description".to_owned(),
                    JsonValue::String(description.to_owned()),
                ),
                ("path".to_owned(), JsonValue::String(tool.path)),
                ("status".to_owned(), JsonValue::String("ok".to_owned())),
                (
                    "scopes".to_owned(),
                    JsonValue::Array(
                        tool.scopes
                            .unwrap_or_default()
                            .into_iter()
                            .map(JsonValue::String)
                            .collect(),
                    ),
                ),
            ]))
        })
        .collect()
}

pub(crate) fn list_items(effects: &RuntimeEffectRegistry) -> Vec<RunxListItem> {
    let core = core_capabilities().map(list_native_tool);
    let extensions = effects.capabilities().into_iter().map(list_capability);
    core.chain(extensions).collect()
}

fn list_native_tool(tool: &dyn NativeCapability) -> RunxListItem {
    let definition = tool.definition();
    list_item(
        definition.id,
        definition.scopes,
        emits(definition.artifacts),
    )
}

fn list_capability(tool: &dyn CapabilityContract) -> RunxListItem {
    let definition = tool.definition();
    list_item(
        definition.id,
        definition.scopes,
        emits(definition.artifacts),
    )
}

fn list_item(name: &str, scopes: &[&str], emits: Option<Vec<RunxListEmit>>) -> RunxListItem {
    RunxListItem {
        kind: RunxListItemKind::Tool,
        name: name.to_owned(),
        source: RunxListSource::BuiltIn,
        path: format!("native:{name}"),
        status: RunxListStatus::Ok,
        visibility: None,
        description: None,
        diagnostics: None,
        scopes: (!scopes.is_empty())
            .then(|| scopes.iter().map(|scope| (*scope).to_owned()).collect()),
        emits,
        fixtures: None,
        harness_cases: None,
        steps: None,
        wraps: None,
    }
}

fn emits(artifacts: CapabilityArtifacts) -> Option<Vec<RunxListEmit>> {
    match artifacts {
        CapabilityArtifacts::None => None,
        CapabilityArtifacts::Named { output, packet }
        | CapabilityArtifacts::Wrapped { output, packet } => Some(vec![RunxListEmit {
            name: output.to_owned(),
            packet: Some(packet.to_owned()),
        }]),
    }
}

fn search_result(
    name: &str,
    source: &str,
    description: &str,
    scopes: &[&str],
) -> ToolCatalogSearchResult {
    ToolCatalogSearchResult {
        tool_id: format!("native/{name}"),
        name: name.to_owned(),
        summary: Some(description.to_owned()),
        source: source.to_owned(),
        source_label: source.to_owned(),
        namespace: name
            .split_once('.')
            .map_or("runx", |(namespace, _)| namespace)
            .to_owned(),
        external_name: name
            .split_once('.')
            .map_or(name, |(_, external_name)| external_name)
            .to_owned(),
        required_scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        tags: vec!["native".to_owned(), "runx".to_owned()],
        catalog_ref: name.to_owned(),
    }
}

fn searchable_tool_text(tool: &ToolCatalogSearchResult) -> String {
    format!(
        "{} {}",
        tool.name,
        tool.summary.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase()
}

fn artifact_contract(artifacts: CapabilityArtifacts) -> Option<SkillArtifactContract> {
    match artifacts {
        CapabilityArtifacts::None => None,
        CapabilityArtifacts::Named { output, packet } => named_artifact(output, packet),
        CapabilityArtifacts::Wrapped { output, packet } => wrapped_artifact(output, packet),
    }
}

fn named_artifact(output: &str, packet: &str) -> Option<SkillArtifactContract> {
    Some(SkillArtifactContract {
        emits: None,
        named_emits: Some(BTreeMap::from([(output.to_owned(), output.to_owned())])),
        packets: Some(BTreeMap::from([(output.to_owned(), packet.to_owned())])),
        wrap_as: None,
        packet: None,
    })
}

fn wrapped_artifact(output: &str, packet: &str) -> Option<SkillArtifactContract> {
    Some(SkillArtifactContract {
        emits: None,
        named_emits: None,
        packets: None,
        wrap_as: Some(output.to_owned()),
        packet: Some(packet.to_owned()),
    })
}
