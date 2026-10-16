//! The catalog as tools (agent-sessions §4.2): `registry.search` finds
//! skills and connectors in the registry and the project, and
//! `registry.inspect` reports one without adding it to the project (N6).
//! `runx registry search` and `runx skill inspect` read the same functions.

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::schema::RunxSchema;
use runx_contracts::{
    CatalogHit, CatalogPackageKind, CatalogSource, ConnectorInspection, InspectedOperation,
    JsonObject, JsonValue, RegistryInspection, RegistrySearchPacket,
};
use serde::{Deserialize, Serialize};

use super::NativeInvocation;
use super::capability::{NativeCapability, TypedNativeCapability};
use crate::RuntimeError;
use crate::kernel::connectors::{ConnectorOrigin, ConnectorScope};
use crate::paths::RunxPaths;
use crate::registry::resolve::{
    AbsentPins, Holder, RegistryTarget, SkillResolverOptions, registry_client,
    resolve_registry_target, resolve_skill_ref_details,
};
use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

const SEARCH: &str = "registry.search";
const INSPECT: &str = "registry.inspect";

/// The most hits of each kind a search returns.
const MAX_HITS: u64 = 50;

#[derive(Clone, Debug, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchInput {
    pub(super) query: String,
    pub(super) limit: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) kind: Option<CatalogPackageKind>,
}

impl CapabilityInput for SearchInput {
    fn defaults() -> JsonObject {
        JsonObject::from([(
            "limit".to_owned(),
            JsonValue::Number(runx_contracts::JsonNumber::U64(10)),
        )])
    }
}

impl CapabilityOutput for RegistrySearchPacket {}

#[derive(Clone, Debug, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct InspectInput {
    /// A skill or connector ref, as `registry.search` returns it.
    #[serde(rename = "ref")]
    pub(super) r#ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) runner: Option<String>,
}

impl CapabilityInput for InspectInput {}

impl CapabilityOutput for RegistryInspection {}

fn search(
    invocation: &NativeInvocation<'_, SearchInput>,
) -> Result<RegistrySearchPacket, RuntimeError> {
    let inputs = invocation.inputs;
    if !(1..=MAX_HITS).contains(&inputs.limit) {
        return Err(super::invalid_input(
            SEARCH,
            format!("limit is 1-{MAX_HITS}"),
        ));
    }
    let limit = usize::try_from(inputs.limit).unwrap_or(usize::MAX);
    let paths = RunxPaths::resolve(invocation.env, invocation.skill_directory);
    let wants = |kind: CatalogPackageKind| inputs.kind.is_none_or(|wanted| wanted == kind);
    let mut hits = project_hits(&paths, &inputs.query, &wants)?;
    let target = resolve_registry_target(None, invocation.env, paths.workspace());
    let found =
        crate::registry::resolve::search(&inputs.query, Some(limit), target, invocation.env)
            .map_err(|error| super::invalid_input(SEARCH, error.into_message()))?;
    if wants(CatalogPackageKind::Skill) {
        hits.extend(found.results.into_iter().map(|result| CatalogHit {
            r#ref: match &result.version {
                Some(version) => format!("{}@{version}", result.skill_id),
                None => result.skill_id.clone(),
            },
            kind: CatalogPackageKind::Skill,
            source: CatalogSource::Registry,
            publisher: Some(result.owner),
            trust_tier: Some(result.trust_tier),
            summary: result.summary,
            runners: result.runner_names,
        }));
    }
    if wants(CatalogPackageKind::Connector) {
        hits.extend(found.connectors.into_iter().map(|connector| CatalogHit {
            r#ref: connector.connector.as_str().to_owned(),
            kind: CatalogPackageKind::Connector,
            source: CatalogSource::Registry,
            publisher: None,
            trust_tier: Some(connector.trust_tier),
            summary: Some(connector.description),
            runners: Vec::new(),
        }));
    }
    Ok(RegistrySearchPacket {
        query: inputs.query.clone(),
        hits,
    })
}

/// The project's own skills and connectors whose name or summary holds the
/// query. A package a registry added is the registry's, found there.
fn project_hits(
    paths: &RunxPaths,
    query: &str,
    wants: &dyn Fn(CatalogPackageKind) -> bool,
) -> Result<Vec<CatalogHit>, RuntimeError> {
    let needle = query.to_lowercase();
    let matches = |texts: &[Option<&str>]| {
        needle.is_empty()
            || texts
                .iter()
                .flatten()
                .any(|text| text.to_lowercase().contains(&needle))
    };
    let workspace = paths.workspace();
    let mut hits = Vec::new();
    if wants(CatalogPackageKind::Skill) {
        for directory in crate::skill_package::discover_workspace_skill_package_dirs(workspace)? {
            if crate::registry::read_install_record(paths, &directory)
                .map_err(|message| super::invalid_input(SEARCH, message))?
                .is_some()
            {
                continue;
            }
            let Ok(loaded) = crate::load_validated_skill_package(&directory) else {
                continue;
            };
            let skill = &loaded.package.skill;
            if !matches(&[Some(&skill.name), skill.description.as_deref()]) {
                continue;
            }
            hits.push(CatalogHit {
                r#ref: relative(workspace, &directory),
                kind: CatalogPackageKind::Skill,
                source: CatalogSource::Project,
                publisher: None,
                trust_tier: None,
                summary: skill.description.clone(),
                runners: loaded
                    .manifest()
                    .map(|manifest| manifest.runners.keys().cloned().collect())
                    .unwrap_or_default(),
            });
        }
    }
    if wants(CatalogPackageKind::Connector) {
        let scope = ConnectorScope::at(workspace)
            .map_err(|error| super::invalid_input(SEARCH, error.to_string()))?
            .in_project(paths.connectors());
        for connector in scope
            .connectors()
            .map_err(|error| super::invalid_input(SEARCH, error.to_string()))?
        {
            if !matches!(connector.origin, ConnectorOrigin::Plugin { .. })
                || !matches(&[
                    Some(connector.name.as_str()),
                    Some(connector.manifest.description.as_str()),
                ])
            {
                continue;
            }
            hits.push(CatalogHit {
                r#ref: connector.name.as_str().to_owned(),
                kind: CatalogPackageKind::Connector,
                source: CatalogSource::Project,
                publisher: None,
                trust_tier: None,
                summary: Some(connector.manifest.description.as_str().to_owned()),
                runners: Vec::new(),
            });
        }
    }
    Ok(hits)
}

fn inspect(
    invocation: &NativeInvocation<'_, InspectInput>,
) -> Result<RegistryInspection, RuntimeError> {
    let inputs = invocation.inputs;
    let env = invocation.env;
    let paths = RunxPaths::resolve(env, invocation.skill_directory);
    let workspace = paths.workspace();
    // A connector the project holds, or one a hosted registry publishes; a
    // ref no connector answers is a skill's.
    if let Some(connector) = inspect_connector(&inputs.r#ref, &paths, env)? {
        return Ok(RegistryInspection::Connector(connector));
    }
    let resolved = resolve_skill_ref_details(
        Path::new(&inputs.r#ref),
        workspace,
        SkillResolverOptions {
            env,
            registry: None,
            expected_digest: None,
            absent_pins: AbsentPins::Report,
            holder: Holder::Cache,
        },
    )
    .map_err(|message| super::invalid_input(INSPECT, message))?;
    let inspection = crate::inspect_skill_readiness(
        &resolved.runnable_path,
        inputs.runner.as_deref(),
        None,
        env,
        workspace,
    )
    .map_err(|error| super::invalid_input(INSPECT, error.to_string()))?;
    Ok(RegistryInspection::Skill(Box::new(inspection)))
}

/// The connector `reference` names: the project's copy, with its readiness,
/// or the hosted registry's description of it, adding nothing.
fn inspect_connector(
    reference: &str,
    paths: &RunxPaths,
    env: &BTreeMap<String, String>,
) -> Result<Option<ConnectorInspection>, RuntimeError> {
    let workspace = paths.workspace();
    let scope = ConnectorScope::at(workspace)
        .map_err(|error| super::invalid_input(INSPECT, error.to_string()))?
        .in_project(paths.connectors());
    if let Ok(connector) = scope.named(reference) {
        let readiness = crate::kernel::admission::connector_readiness(&connector, env, workspace)
            .map_err(|message| super::invalid_input(INSPECT, message))?;
        let record = match &connector.origin {
            ConnectorOrigin::Installed { directory } => {
                crate::registry::read_install_record(paths, directory)
                    .map_err(|message| super::invalid_input(INSPECT, message))?
            }
            ConnectorOrigin::Plugin { .. } | ConnectorOrigin::RuntimeCapability => None,
        };
        return Ok(Some(ConnectorInspection {
            connector: connector.name.as_str().to_owned(),
            // An added version is named by its directory.
            version: match &connector.origin {
                ConnectorOrigin::Installed { directory } => directory
                    .file_name()
                    .map(|version| version.to_string_lossy().into_owned()),
                ConnectorOrigin::Plugin { .. } | ConnectorOrigin::RuntimeCapability => None,
            },
            description: connector.manifest.description.as_str().to_owned(),
            source: match connector.origin {
                ConnectorOrigin::Plugin { .. } => CatalogSource::Project,
                _ => CatalogSource::Registry,
            },
            trust_tier: record.map(|record| record.trust_tier),
            operations: connector
                .manifest
                .operations
                .iter()
                .map(|(operation, declared)| InspectedOperation {
                    operation: operation.as_str().to_owned(),
                    description: declared.description.as_str().to_owned(),
                    access: Some(declared.access().as_str().to_owned()),
                    approval_required: Some(
                        declared.approval == runx_contracts::connector::ApprovalFloor::Required,
                    ),
                })
                .collect(),
            readiness,
        }));
    }
    let (name, version) = reference
        .split_once('@')
        .map_or((reference, None), |(name, version)| (name, Some(version)));
    let (Some(name), RegistryTarget::Remote { registry_url }) = (
        runx_contracts::connector::ConnectorName::new(name),
        resolve_registry_target(None, env, workspace),
    ) else {
        return Ok(None);
    };
    let Some(detail) = registry_client(&registry_url, env)
        .and_then(|client| client.read_connector(&name, version).map_err(Into::into))
        .map_err(|error| super::invalid_input(INSPECT, error.into_message()))?
    else {
        return Ok(None);
    };
    Ok(Some(ConnectorInspection {
        connector: detail.connector.as_str().to_owned(),
        version: Some(detail.version),
        description: detail.description,
        source: CatalogSource::Registry,
        trust_tier: Some(detail.trust_tier),
        operations: detail
            .operations
            .into_iter()
            .map(|operation| InspectedOperation {
                operation: operation.name.as_str().to_owned(),
                description: operation.description,
                access: None,
                approval_required: None,
            })
            .collect(),
        readiness: Vec::new(),
    }))
}

fn relative(workspace: &Path, directory: &Path) -> String {
    let workspace = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let directory = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
    directory
        .strip_prefix(&workspace)
        .unwrap_or(&directory)
        .to_string_lossy()
        .into_owned()
}

const SEARCH_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "query",
        description: "Words a skill's or connector's name or summary holds; empty lists the project's own.",
    },
    CapabilityField {
        name: "limit",
        description: "The most registry hits of each kind, 1-50.",
    },
    CapabilityField {
        name: "kind",
        description: "skill or connector, to search one kind.",
    },
];

const INSPECT_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "ref",
        description: "A ref registry.search returned: a registry ref, or a project package's path.",
    },
    CapabilityField {
        name: "runner",
        description: "The skill runner to inspect; its default when omitted.",
    },
];

static SEARCH_CAPABILITY: TypedNativeCapability<SearchInput, RegistrySearchPacket> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: SEARCH,
            owner: "runx-runtime/registry",
            summary: "Search the registry and the project for skills and connectors: each hit's ref, kind, publisher, trust tier, summary, runners, and whether the registry or the project holds it.",
            scopes: &["registry.read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Wrapped {
                output: "registry_search",
                packet: "runx.registry.search.v1",
            },
            fields: SEARCH_FIELDS,
        },
        search,
    );

static INSPECT_CAPABILITY: TypedNativeCapability<InspectInput, RegistryInspection> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: INSPECT,
            owner: "runx-runtime/registry",
            summary: "Inspect a skill or connector without adding it to the project: a skill's runners with their inputs, effects, connectors and readiness; a connector's operations, approvals and readiness.",
            scopes: &["registry.read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Wrapped {
                output: "registry_inspection",
                packet: "runx.registry.inspection.v1",
            },
            fields: INSPECT_FIELDS,
        },
        inspect,
    );

pub(super) const CAPABILITIES: &[&dyn NativeCapability] =
    &[&SEARCH_CAPABILITY, &INSPECT_CAPABILITY];
