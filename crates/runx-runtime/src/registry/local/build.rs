// Module rationale: local registry ingestion keeps package
// validation, binding metadata, and registry-version projection in one atomic
// build boundary.
use runx_contracts::maturity::MaturityTier;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    MediationListingRef, OfferRevisionRef, PaidSkillListing, PaidSkillOfferTerms, PaidSkillOffers,
    PaidSkillRunnerOffer, PrincipalReference, RunxPrincipalId, Sha256Digest, sha256_hex,
};
use serde::Deserialize;

use super::super::package_files::{
    normalize_registry_package_files, registry_package_digest, skill_content_digest,
    validate_registry_skill_package,
};
use super::super::types::{
    RegistryAttestation, RegistryPackageFile, RegistryPublisher, RegistrySkillVersion,
    RegistrySourceMetadata, TrustTier,
};
use super::{IngestSkillOptions, LocalRegistryError, build_skill_id};
use crate::registry::local::trust::{
    build_publisher_attestations, build_source_attestations, merge_registry_attestations,
    normalize_attestations,
};
use crate::registry::local::util::{
    missing_field, now_iso8601, required_string, validate_publisher, validate_source_metadata,
};
use crate::registry::package_metadata::project_registry_package_metadata;

pub(super) fn build_registry_skill_version(
    markdown: &str,
    options: &IngestSkillOptions,
) -> Result<RegistrySkillVersion, LocalRegistryError> {
    let package_files = normalize_package_files(options.package_files.clone())?;
    let package = validate_registry_skill_package(
        markdown,
        options.profile_document.as_deref(),
        &package_files,
    )?;
    let skill = &package.skill;
    let manifest = package.root_manifest();
    let RegistryPackageDigests {
        digest,
        profile_digest,
        package_digest,
    } = registry_package_digests(markdown, options, &package_files);
    // A local registry version keeps no connector bindings; it reads the
    // catalog fields below.
    let metadata = project_registry_package_metadata(skill, manifest, Vec::new());
    let content_digest = skill_content_digest(
        markdown,
        options.profile_document.as_deref(),
        &package_files,
    )
    .map_err(|error| LocalRegistryError::InvalidVersionPayload {
        field: "package_files".to_owned(),
        message: error.to_string(),
    })?;
    let defaults = registry_version_defaults(&content_digest, options);
    let skill_id = build_skill_id(&defaults.owner, &skill.name)?;
    let paid_listing = resolve_paid_listing(
        &metadata.marketplace_offers,
        &skill_id,
        &defaults.version,
        &digest,
        profile_digest.as_deref(),
        package_digest.as_deref(),
        &defaults.publisher,
    )?;
    Ok(RegistrySkillVersion {
        skill_id,
        owner: defaults.owner,
        name: metadata.name,
        description: metadata.description,
        category: metadata.category,
        version: defaults.version,
        digest,
        signed_manifest: None,
        markdown: markdown.to_owned(),
        profile_document: options.profile_document.clone(),
        profile_digest,
        package_files,
        package_digest,
        paid_listing,
        runner_names: metadata.runner_names,
        trust_tier: defaults.trust_tier,
        maturity: options.maturity,
        catalog_kind: Some(metadata.catalog_kind),
        catalog_audience: Some(metadata.catalog_audience),
        catalog_visibility: Some(metadata.catalog_visibility),
        source_metadata: defaults.source_metadata,
        attestations: defaults.attestations,
        required_scopes: metadata.required_scopes,
        tags: metadata.tags,
        harness_cases: metadata.harness_cases,
        publisher: defaults.publisher,
        created_at: defaults.created_at,
        updated_at: now_iso8601(),
    })
}

struct RegistryPackageDigests {
    digest: String,
    profile_digest: Option<String>,
    package_digest: Option<String>,
}

fn registry_package_digests(
    markdown: &str,
    options: &IngestSkillOptions,
    package_files: &[RegistryPackageFile],
) -> RegistryPackageDigests {
    RegistryPackageDigests {
        digest: sha256_hex(markdown.as_bytes()),
        profile_digest: options
            .profile_document
            .as_ref()
            .map(|document| sha256_hex(document.as_bytes())),
        package_digest: registry_package_digest(package_files),
    }
}

fn resolve_paid_listing(
    offers: &std::collections::BTreeMap<String, PaidSkillOfferTerms>,
    skill_id: &str,
    version: &str,
    skill_digest: &str,
    profile_digest: Option<&str>,
    package_digest: Option<&str>,
    publisher: &RegistryPublisher,
) -> Result<Option<PaidSkillListing>, LocalRegistryError> {
    if offers.is_empty() {
        return Ok(None);
    }
    let profile_digest = required_paid_digest(profile_digest, "profile_digest")?;
    let package_digest = required_paid_digest(package_digest, "package_digest")?;
    let skill_digest = registry_sha256(skill_digest, "skill_digest")?;
    let principal_id = RunxPrincipalId::new(publisher.id.clone()).ok_or_else(|| {
        invalid_paid_listing(format!(
            "publisher id '{}' cannot identify a seller principal",
            publisher.id
        ))
    })?;
    let runner_offers = offers
        .iter()
        .map(|(runner, terms)| {
            let listing_ref = MediationListingRef::new(format!(
                "runx:listing:{skill_id}@{version}#{runner}"
            ))
            .ok_or_else(|| invalid_paid_listing("derived listing_ref is invalid"))?;
            let offer = PaidSkillRunnerOffer::from_terms(
                OfferRevisionRef {
                    offer_id: NonEmptyString::from(format!("{skill_id}#{runner}")),
                    revision: NonEmptyString::from(version.to_owned()),
                    revision_digest: profile_digest.clone(),
                    input_schema_digest: terms.input_schema_digest().clone(),
                    output_schema_digest: terms.output_schema_digest().clone(),
                },
                listing_ref,
                terms,
            )
            .ok_or_else(|| {
                invalid_paid_listing(
                    "mediated offer must bind an executor and its total must equal vendor amount plus platform fee in one currency",
                )
            })?;
            Ok::<_, LocalRegistryError>((
                runner.clone(),
                offer,
            ))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    let offers = PaidSkillOffers::new(runner_offers)
        .ok_or_else(|| invalid_paid_listing("offers must contain non-empty runner names"))?;
    Ok(Some(PaidSkillListing {
        skill_id: NonEmptyString::from(skill_id.to_owned()),
        version: NonEmptyString::from(version.to_owned()),
        skill_digest,
        profile_digest,
        package_digest,
        vendor_ref: PrincipalReference::from_runx_principal_id(principal_id),
        offers,
    }))
}

fn required_paid_digest(
    value: Option<&str>,
    field: &str,
) -> Result<Sha256Digest, LocalRegistryError> {
    value
        .ok_or_else(|| invalid_paid_listing(format!("paid offers require {field}")))
        .and_then(|value| registry_sha256(value, field))
}

fn registry_sha256(value: &str, field: &str) -> Result<Sha256Digest, LocalRegistryError> {
    let value = if value.starts_with("sha256:") {
        value.to_owned()
    } else {
        format!("sha256:{value}")
    };
    Sha256Digest::new(value)
        .ok_or_else(|| invalid_paid_listing(format!("{field} must be a lowercase SHA-256 digest")))
}

fn invalid_paid_listing(message: impl Into<String>) -> LocalRegistryError {
    LocalRegistryError::InvalidVersionPayload {
        field: "registry_version.paid_listing".to_owned(),
        message: message.into(),
    }
}

fn normalize_profile_digest(
    profile_document: Option<&str>,
    declared: Option<&str>,
) -> Result<Option<String>, LocalRegistryError> {
    let computed = profile_document.map(|document| sha256_hex(document.as_bytes()));
    if declared.is_some() && declared != computed.as_deref() {
        return Err(LocalRegistryError::InvalidVersionPayload {
            field: "registry_version.profile_digest".to_owned(),
            message: "does not match profile_document".to_owned(),
        });
    }
    Ok(computed)
}

struct RegistryVersionDefaults {
    owner: String,
    created_at: String,
    publisher: RegistryPublisher,
    trust_tier: TrustTier,
    version: String,
    source_metadata: Option<RegistrySourceMetadata>,
    attestations: Vec<RegistryAttestation>,
}

fn registry_version_defaults(
    content_digest: &str,
    options: &IngestSkillOptions,
) -> RegistryVersionDefaults {
    let owner = options.owner.clone().unwrap_or_else(|| "local".to_owned());
    let created_at = options.created_at.clone().unwrap_or_else(now_iso8601);
    let publisher = options
        .publisher
        .clone()
        .unwrap_or_else(|| default_registry_publisher(&owner));
    let trust_tier = options
        .trust_tier
        .clone()
        .unwrap_or_else(|| derive_registry_trust_tier(&owner, None));
    let version = options.version.clone().unwrap_or_else(|| {
        // A version without a label is named by its content digest, as the
        // hosted registry names it.
        let hex = content_digest.trim_start_matches("sha256:");
        format!("sha-{}", &hex[..12.min(hex.len())])
    });
    let source_metadata = options.source_metadata.clone();
    let attestations = merge_registry_attestations(vec![
        build_publisher_attestations(&publisher, &trust_tier, &created_at),
        build_source_attestations(source_metadata.as_ref(), &created_at),
        options.attestations.clone(),
    ]);
    RegistryVersionDefaults {
        owner,
        created_at,
        publisher,
        trust_tier,
        version,
        source_metadata,
        attestations,
    }
}

// Function rationale: normalization validates the package digest,
// manifest, and registry row in one pass over the submitted version payload.
pub(super) fn normalize_registry_skill_version(
    payload: RegistrySkillVersionPayload,
) -> Result<RegistrySkillVersion, LocalRegistryError> {
    let governance = normalize_registry_version_governance(&payload)?;
    let package_files = normalize_package_files(payload.package_files.unwrap_or_default())?;
    let computed_package_digest = registry_package_digest(&package_files);
    if let (Some(declared), Some(computed)) = (&payload.package_digest, &computed_package_digest)
        && declared != computed
    {
        return Err(LocalRegistryError::InvalidVersionPayload {
            field: "registry_version.package_digest".to_owned(),
            message: format!("declared digest {declared} does not match package files {computed}"),
        });
    }
    if payload.package_digest.is_some() && computed_package_digest.is_none() {
        return Err(LocalRegistryError::InvalidVersionPayload {
            field: "registry_version.package_digest".to_owned(),
            message: "declared without package_files".to_owned(),
        });
    }
    let markdown = required_string(payload.markdown, "registry_version.markdown")?;
    let package = validate_registry_skill_package(
        &markdown,
        payload.profile_document.as_deref(),
        &package_files,
    )?;
    let category = payload.category.or_else(|| {
        package
            .root_manifest()
            .and_then(|manifest| manifest.catalog.as_ref())
            .and_then(|catalog| catalog.category.clone())
    });
    let skill_id = required_string(payload.skill_id, "registry_version.skill_id")?;
    let version = required_string(payload.version, "registry_version.version")?;
    let digest = required_string(payload.digest, "registry_version.digest")?;
    let profile_digest = normalize_profile_digest(
        payload.profile_document.as_deref(),
        payload.profile_digest.as_deref(),
    )?;
    let package_digest = payload.package_digest.or(computed_package_digest);
    let metadata =
        project_registry_package_metadata(&package.skill, package.root_manifest(), Vec::new());
    let paid_listing = resolve_paid_listing(
        &metadata.marketplace_offers,
        &skill_id,
        &version,
        &sha256_hex(markdown.as_bytes()),
        profile_digest.as_deref(),
        package_digest.as_deref(),
        &governance.publisher,
    )?;
    if payload
        .paid_listing
        .as_ref()
        .is_some_and(|declared| Some(declared) != paid_listing.as_ref())
    {
        return Err(LocalRegistryError::InvalidVersionPayload {
            field: "registry_version.paid_listing".to_owned(),
            message: "does not match the parser-resolved immutable listing".to_owned(),
        });
    }

    Ok(RegistrySkillVersion {
        skill_id,
        owner: governance.owner,
        name: required_string(payload.name, "registry_version.name")?,
        description: payload.description,
        category,
        version,
        digest,
        signed_manifest: payload.signed_manifest,
        markdown,
        profile_document: payload.profile_document,
        profile_digest,
        package_files,
        package_digest,
        paid_listing,
        runner_names: payload.runner_names.unwrap_or_default(),
        trust_tier: governance.trust_tier,
        // Preserved through re-ingest; defaults to the Alpha floor when absent.
        maturity: payload.maturity.unwrap_or_default(),
        catalog_kind: Some(governance.catalog.kind.as_str().to_owned()),
        catalog_audience: Some(governance.catalog.audience.as_str().to_owned()),
        catalog_visibility: Some(governance.catalog.visibility.as_str().to_owned()),
        source_metadata: governance.source_metadata,
        attestations: governance.attestations,
        required_scopes: payload.required_scopes.unwrap_or_default(),
        tags: payload.tags.unwrap_or_default(),
        harness_cases: payload.harness_cases.unwrap_or_default(),
        publisher: governance.publisher,
        updated_at: governance.updated_at,
        created_at: governance.created_at,
    })
}

struct NormalizedRegistryVersionGovernance {
    owner: String,
    created_at: String,
    publisher: RegistryPublisher,
    trust_tier: TrustTier,
    source_metadata: Option<RegistrySourceMetadata>,
    attestations: Vec<RegistryAttestation>,
    catalog: runx_parser::CatalogMetadata,
    updated_at: String,
}

fn normalize_registry_version_governance(
    payload: &RegistrySkillVersionPayload,
) -> Result<NormalizedRegistryVersionGovernance, LocalRegistryError> {
    let owner = required_string(payload.owner.clone(), "registry_version.owner")?;
    let created_at = required_string(payload.created_at.clone(), "registry_version.created_at")?;
    let publisher = validate_publisher(
        payload
            .publisher
            .clone()
            .ok_or_else(|| missing_field("registry_version.publisher"))?,
        "registry_version.publisher",
    )?;
    let trust_tier = payload.trust_tier.clone().unwrap_or(TrustTier::Community);
    let source_metadata = normalize_source_metadata(payload.source_metadata.clone())?;
    let attestations = normalize_attestations(
        payload.attestations.clone().unwrap_or_default(),
        source_metadata.as_ref(),
        &publisher,
        &trust_tier,
        &created_at,
    );
    let catalog = normalize_registry_catalog(
        payload.catalog_kind.as_deref(),
        payload.catalog_audience.as_deref(),
        payload.catalog_visibility.as_deref(),
    );
    let updated_at = payload
        .updated_at
        .as_ref()
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| created_at.clone());
    Ok(NormalizedRegistryVersionGovernance {
        owner,
        created_at,
        publisher,
        trust_tier,
        source_metadata,
        attestations,
        catalog,
        updated_at,
    })
}

pub(super) fn normalize_source_metadata(
    source_metadata: Option<RegistrySourceMetadata>,
) -> Result<Option<RegistrySourceMetadata>, LocalRegistryError> {
    source_metadata.map(validate_source_metadata).transpose()
}

pub(super) fn normalize_registry_catalog(
    kind: Option<&str>,
    audience: Option<&str>,
    visibility: Option<&str>,
) -> runx_parser::CatalogMetadata {
    runx_parser::CatalogMetadata {
        kind: match kind {
            Some("graph") => runx_parser::CatalogKind::Graph,
            _ => runx_parser::CatalogKind::Skill,
        },
        audience: match audience {
            Some("builder") => runx_parser::CatalogAudience::Builder,
            Some("operator") => runx_parser::CatalogAudience::Operator,
            Some("system") => runx_parser::CatalogAudience::System,
            _ => runx_parser::CatalogAudience::Public,
        },
        visibility: match visibility {
            Some("internal") => runx_parser::CatalogVisibility::Internal,
            _ => runx_parser::CatalogVisibility::Public,
        },
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
        links: std::collections::BTreeMap::new(),
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct RegistrySkillVersionPayload {
    skill_id: Option<String>,
    owner: Option<String>,
    name: Option<String>,
    description: Option<String>,
    category: Option<String>,
    version: Option<String>,
    digest: Option<String>,
    signed_manifest: Option<super::super::types::RegistrySignedManifest>,
    markdown: Option<String>,
    profile_document: Option<String>,
    profile_digest: Option<String>,
    package_files: Option<Vec<RegistryPackageFile>>,
    package_digest: Option<String>,
    paid_listing: Option<PaidSkillListing>,
    runner_names: Option<Vec<String>>,
    trust_tier: Option<TrustTier>,
    maturity: Option<MaturityTier>,
    catalog_kind: Option<String>,
    catalog_audience: Option<String>,
    catalog_visibility: Option<String>,
    source_metadata: Option<RegistrySourceMetadata>,
    attestations: Option<Vec<RegistryAttestation>>,
    required_scopes: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    harness_cases: Option<Vec<super::super::RegistryHarnessCaseMetadata>>,
    publisher: Option<RegistryPublisher>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

fn normalize_package_files(
    files: Vec<RegistryPackageFile>,
) -> Result<Vec<RegistryPackageFile>, LocalRegistryError> {
    normalize_registry_package_files(files).map_err(|message| {
        LocalRegistryError::InvalidVersionPayload {
            field: "registry_version.package_files".to_owned(),
            message,
        }
    })
}

pub(super) fn default_registry_publisher(owner: &str) -> RegistryPublisher {
    RegistryPublisher {
        kind: if owner == "runx" {
            "organization".to_owned()
        } else {
            "publisher".to_owned()
        },
        id: owner.to_owned(),
        handle: Some(owner.to_owned()),
        display_name: None,
    }
}

pub(super) fn derive_registry_trust_tier(
    _owner: &str,
    trust_tier: Option<&TrustTier>,
) -> TrustTier {
    trust_tier.cloned().unwrap_or(TrustTier::Community)
}
