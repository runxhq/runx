use std::collections::BTreeMap;

use runx_contracts::PaidSkillOfferTerms;
use runx_contracts::connector::{ConnectorName, Digest};
use runx_parser::{CatalogMetadata, SkillMethodManifest, TestFixture, ValidatedSkill};
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
    pub method_names: Vec<String>,
    pub catalog_visibility: String,
    /// Each paid method's offer, with the digests of the input and output
    /// schemas its listing, purchases and receipts bind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub offers: BTreeMap<String, RegistryMethodOffer>,
    pub tags: Vec<String>,
    pub test_cases: Vec<RegistryTestCaseMetadata>,
    /// Every input name the package declares: its shared inputs and each
    /// method's inputs, sorted and once each.
    pub input_names: Vec<String>,
    /// The child skills the package composes, as written: its skill steps'
    /// targets and the skills an agent turn may run, sorted and once each.
    pub skills: Vec<String>,
    /// The connectors the package's steps call, in name order. Its own only:
    /// a skill it composes calls its own.
    pub connectors: Vec<RegistryConnectorMetadata>,
}

/// A paid method's offer as its `run.yaml` states it, and the digests of the
/// method's input and output schemas, derived from its fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryMethodOffer {
    pub terms: PaidSkillOfferTerms,
    pub input_schema_digest: Digest,
    pub output_schema_digest: Digest,
}

/// One connector a package declares: where it comes from, the digests a
/// closure binds it by, and the operations the package lists for it. A
/// run-time choice names each connector it may choose, with the choice's
/// operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryConnectorMetadata {
    pub connector: ConnectorName,
    pub version: String,
    pub manifest_digest: Digest,
    /// The manifest and every tool its process operations run.
    pub connector_digest: Digest,
    /// The connector is part of the runtime itself (store, web, vault,
    /// artifact).
    pub runtime_capability: bool,
    pub operations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryTestCaseMetadata {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_status: Option<String>,
}

pub(crate) fn project_registry_package_metadata(
    skill: &ValidatedSkill,
    manifest: Option<&SkillMethodManifest>,
    tests: &BTreeMap<String, TestFixture>,
    connectors: Vec<RegistryConnectorMetadata>,
) -> Result<RegistryPackageMetadata, String> {
    let catalog = registry_catalog(manifest);
    Ok(RegistryPackageMetadata {
        name: skill.name.clone(),
        description: skill.description.clone(),
        category: catalog.category.clone(),
        method_names: registry_method_names(manifest),
        catalog_visibility: catalog.visibility.as_str().to_owned(),
        offers: registry_offers(manifest)?,
        tags: registry_tags(&catalog),
        test_cases: registry_test_cases(tests),
        input_names: registry_input_names(manifest),
        skills: registry_skills(manifest),
        connectors,
    })
}

/// Each paid method's offer, bound to the digests of the method's schemas.
fn registry_offers(
    manifest: Option<&SkillMethodManifest>,
) -> Result<BTreeMap<String, RegistryMethodOffer>, String> {
    let mut offers = BTreeMap::new();
    for (name, method) in manifest.into_iter().flat_map(|manifest| &manifest.methods) {
        let Some(terms) = &method.offer else {
            continue;
        };
        let schemas = method
            .schemas()
            .map_err(|error| format!("method {name}'s schemas cannot be digested: {error}"))?;
        offers.insert(
            name.clone(),
            RegistryMethodOffer {
                terms: terms.clone(),
                input_schema_digest: schemas.input_schema_digest,
                output_schema_digest: schemas.output_schema_digest,
            },
        );
    }
    Ok(offers)
}

/// The declared connectors by id, each with its operations in the order the
/// package lists them.
pub(crate) fn declared_connectors(
    manifest: Option<&SkillMethodManifest>,
) -> BTreeMap<String, Vec<String>> {
    let mut connectors = BTreeMap::<String, Vec<String>>::new();
    for (key, requirement) in manifest
        .into_iter()
        .flat_map(|manifest| &manifest.connectors)
    {
        let ids = if requirement.choose.is_some() {
            requirement
                .from
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

fn registry_test_cases(tests: &BTreeMap<String, TestFixture>) -> Vec<RegistryTestCaseMetadata> {
    tests
        .values()
        .map(|test| RegistryTestCaseMetadata {
            name: test.name.clone(),
            method: test.method.clone(),
            expected_status: test.expect.status.as_ref().map(|status| {
                match status {
                    runx_parser::test_fixture::TestExpectedStatus::Sealed => "sealed",
                    runx_parser::test_fixture::TestExpectedStatus::Failure => "failure",
                    runx_parser::test_fixture::TestExpectedStatus::NeedsAgent => "needs_agent",
                    runx_parser::test_fixture::TestExpectedStatus::PolicyDenied => "policy_denied",
                    runx_parser::test_fixture::TestExpectedStatus::Escalated => "escalated",
                }
                .to_owned()
            }),
        })
        .collect()
}

pub(crate) fn registry_catalog(manifest: Option<&SkillMethodManifest>) -> CatalogMetadata {
    manifest
        .and_then(|manifest| manifest.catalog.clone())
        .unwrap_or(CatalogMetadata {
            visibility: runx_parser::CatalogVisibility::Public,
            category: None,
            tags: Vec::new(),
        })
}

fn registry_input_names(manifest: Option<&SkillMethodManifest>) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    if let Some(manifest) = manifest {
        names.extend(manifest.shared_inputs.keys().cloned());
        for method in manifest.methods.values() {
            names.extend(method.inputs.keys().cloned());
        }
    }
    names.into_iter().collect()
}

fn registry_skills(manifest: Option<&SkillMethodManifest>) -> Vec<String> {
    let mut refs = std::collections::BTreeSet::new();
    if let Some(manifest) = manifest {
        for method in manifest.methods.values() {
            for step in &method.graph.steps {
                match &step.kind {
                    runx_parser::StepKind::Skill(target) => {
                        refs.insert(target.skill.clone());
                    }
                    runx_parser::StepKind::Agent(agent) => {
                        refs.extend(agent.allowance.named_skills().map(|skill| {
                            skill
                                .as_str()
                                .split_once('#')
                                .map_or(skill.as_str(), |(skill, _)| skill)
                                .to_owned()
                        }));
                    }
                    _ => {}
                }
            }
        }
    }
    refs.into_iter().collect()
}

fn registry_method_names(manifest: Option<&SkillMethodManifest>) -> Vec<String> {
    manifest
        .map(|manifest| manifest.methods.keys().cloned().collect())
        .unwrap_or_default()
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
