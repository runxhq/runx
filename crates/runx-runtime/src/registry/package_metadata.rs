use std::collections::BTreeMap;

use runx_contracts::PaidSkillOfferTerms;
use runx_contracts::connector::{ConnectorRef, Digest};
use runx_parser::{CatalogMetadata, SkillRunnerManifest, ValidatedSkill};
use serde::{Deserialize, Serialize};

/// Parser-owned package metadata persisted by registry implementations.
///
/// This projection is deliberately produced by the native package validator.
/// Registry clients may store it, but must not reconstruct execution semantics
/// by parsing `SKILL.md` or `run.yaml` independently.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistryPackageMetadata {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub runner_names: Vec<String>,
    pub catalog_kind: String,
    pub catalog_audience: String,
    pub catalog_visibility: String,
    pub required_scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub marketplace_offers: BTreeMap<String, PaidSkillOfferTerms>,
    pub tags: Vec<String>,
    pub harness_cases: Vec<RegistryHarnessCaseMetadata>,
    /// Every input name the package declares: its shared input definitions
    /// and each runner's inputs, sorted and once each.
    pub input_names: Vec<String>,
    /// The child skills the package composes, as written: its graph steps'
    /// skill targets and the skills an agent turn may run, sorted and once
    /// each.
    pub skill_refs: Vec<String>,
    /// The connectors the package's `run.yaml` declares, in id order. Its own
    /// declarations only: a skill it composes declares its own.
    pub connectors: Vec<RegistryConnectorMetadata>,
}

/// One connector a package declares: where it comes from, the digests a
/// closure binds it by, and the operations the package lists for it. A
/// run-time choice names each connector it may choose, with the choice's
/// operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryConnectorMetadata {
    #[serde(rename = "ref")]
    pub connector_ref: ConnectorRef,
    pub version: String,
    pub manifest_digest: Digest,
    /// The manifest and every tool its process operations run.
    pub connector_digest: Digest,
    /// The connector is part of the runtime itself (store, web, vault).
    pub runtime_capability: bool,
    pub operations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryHarnessCaseMetadata {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_status: Option<String>,
}

pub(crate) fn project_registry_package_metadata(
    skill: &ValidatedSkill,
    manifest: Option<&SkillRunnerManifest>,
    connectors: Vec<RegistryConnectorMetadata>,
) -> RegistryPackageMetadata {
    let catalog = registry_catalog(manifest);
    RegistryPackageMetadata {
        name: skill.name.clone(),
        description: skill.description.clone(),
        category: catalog.category.clone(),
        runner_names: registry_runner_names(manifest),
        catalog_kind: catalog.kind.as_str().to_owned(),
        catalog_audience: catalog.audience.as_str().to_owned(),
        catalog_visibility: catalog.visibility.as_str().to_owned(),
        required_scopes: registry_required_scopes(manifest),
        marketplace_offers: manifest
            .and_then(|manifest| manifest.marketplace.as_ref())
            .map(|marketplace| marketplace.offers.clone())
            .unwrap_or_default(),
        tags: registry_tags(&catalog),
        harness_cases: registry_harness_cases(manifest),
        input_names: registry_input_names(manifest),
        skill_refs: registry_skill_refs(manifest),
        connectors,
    }
}

/// The declared connectors by id, each with its operations in the order the
/// package lists them.
pub(crate) fn declared_connectors(
    manifest: Option<&SkillRunnerManifest>,
) -> BTreeMap<String, Vec<String>> {
    let mut connectors = BTreeMap::<String, Vec<String>>::new();
    for (key, requirement) in manifest
        .into_iter()
        .flat_map(|manifest| &manifest.connectors)
    {
        let ids = if requirement.connector_from.is_some() {
            requirement
                .allowed_connectors
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect()
        } else {
            vec![key.clone()]
        };
        for id in ids {
            let operations = connectors.entry(id).or_default();
            for operation in &requirement.operations {
                if !operations.iter().any(|listed| listed == operation.as_str()) {
                    operations.push(operation.as_str().to_owned());
                }
            }
        }
    }
    connectors
}

fn registry_harness_cases(
    manifest: Option<&SkillRunnerManifest>,
) -> Vec<RegistryHarnessCaseMetadata> {
    manifest
        .and_then(|manifest| manifest.harness.as_ref())
        .into_iter()
        .flat_map(|harness| &harness.cases)
        .map(|case| RegistryHarnessCaseMetadata {
            name: case.name.clone(),
            runner: case.runner.clone(),
            expected_status: case.expect.status.as_ref().map(|status| {
                match status {
                    runx_parser::harness_fixture::HarnessExpectedStatus::Sealed => "sealed",
                    runx_parser::harness_fixture::HarnessExpectedStatus::Failure => "failure",
                    runx_parser::harness_fixture::HarnessExpectedStatus::NeedsAgent => {
                        "needs_agent"
                    }
                    runx_parser::harness_fixture::HarnessExpectedStatus::PolicyDenied => {
                        "policy_denied"
                    }
                    runx_parser::harness_fixture::HarnessExpectedStatus::Escalated => "escalated",
                }
                .to_owned()
            }),
        })
        .collect()
}

pub(crate) fn registry_catalog(manifest: Option<&SkillRunnerManifest>) -> CatalogMetadata {
    manifest
        .and_then(|manifest| manifest.catalog.clone())
        .unwrap_or(CatalogMetadata {
            kind: runx_parser::CatalogKind::Skill,
            audience: runx_parser::CatalogAudience::Public,
            visibility: runx_parser::CatalogVisibility::Public,
            role: runx_parser::CatalogRole::Context,
            canonical_skill: None,
            provider: None,
            part_of: Vec::new(),
            execution: None,
            completion: None,
            approval: None,
            category: None,
            tags: Vec::new(),
            owner: None,
            links: BTreeMap::new(),
        })
}

fn registry_input_names(manifest: Option<&SkillRunnerManifest>) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    if let Some(manifest) = manifest {
        names.extend(manifest.input_definitions.keys().cloned());
        for runner in manifest.runners.values() {
            names.extend(runner.inputs.keys().cloned());
        }
    }
    names.into_iter().collect()
}

fn registry_skill_refs(manifest: Option<&SkillRunnerManifest>) -> Vec<String> {
    let mut refs = std::collections::BTreeSet::new();
    if let Some(manifest) = manifest {
        refs.extend(manifest.skills.iter().cloned());
        for runner in manifest.runners.values() {
            refs.extend(
                runner
                    .graph
                    .steps
                    .iter()
                    .filter_map(|step| step.skill().map(|target| target.reference.clone())),
            );
        }
    }
    refs.into_iter().collect()
}

fn registry_runner_names(manifest: Option<&SkillRunnerManifest>) -> Vec<String> {
    manifest
        .map(|manifest| manifest.runners.keys().cloned().collect())
        .unwrap_or_default()
}

fn registry_required_scopes(manifest: Option<&SkillRunnerManifest>) -> Vec<String> {
    unique(
        manifest
            .into_iter()
            .flat_map(|manifest| manifest.runners.values())
            .flat_map(runx_parser::SkillRunnerDefinition::declared_scopes)
            .collect(),
    )
}

/// The catalog's tags, then its category as a tag.
fn registry_tags(catalog: &CatalogMetadata) -> Vec<String> {
    unique(
        catalog
            .tags
            .iter()
            .cloned()
            .chain(catalog.category.clone())
            .collect(),
    )
}

fn unique(values: Vec<String>) -> Vec<String> {
    let mut unique_values = Vec::new();
    for value in values {
        if !unique_values.contains(&value) {
            unique_values.push(value);
        }
    }
    unique_values
}
