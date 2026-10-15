// Module rationale: trust projection keeps source,
// publisher, and local registry search/readback signals together for stable
// registry output.
use runx_contracts::{JsonObject, JsonValue};

use super::util::{display_sha256, trust_tier_string};
use super::{LocalRegistryError, runx_link_for_version};
use crate::registry::types::{
    ProfileMode, RegistryAttestation, RegistryPublisher, RegistrySearchResult, RegistrySkillDetail,
    RegistrySkillVersion, RegistrySourceMetadata, TrustSignal, TrustTier,
};

pub(super) fn build_source_attestations(
    source_metadata: Option<&RegistrySourceMetadata>,
    issued_at: &str,
) -> Vec<RegistryAttestation> {
    let Some(source_metadata) = source_metadata else {
        return Vec::new();
    };
    let mut metadata = JsonObject::new();
    metadata.insert(
        "repo".to_owned(),
        JsonValue::String(source_metadata.repo.clone()),
    );
    metadata.insert(
        "ref".to_owned(),
        JsonValue::String(source_metadata.r#ref.clone()),
    );
    metadata.insert(
        "sha".to_owned(),
        JsonValue::String(source_metadata.sha.clone()),
    );
    metadata.insert(
        "event".to_owned(),
        JsonValue::String(source_metadata.event.clone()),
    );
    metadata.insert(
        "skill_path".to_owned(),
        JsonValue::String(source_metadata.skill_path.clone()),
    );
    if let Some(profile_path) = &source_metadata.profile_path {
        metadata.insert(
            "profile_path".to_owned(),
            JsonValue::String(profile_path.clone()),
        );
    }
    vec![RegistryAttestation {
        kind: "source".to_owned(),
        id: format!("{}_source", source_metadata.provider),
        status: "verified".to_owned(),
        summary: format!(
            "{}:{}@{}",
            source_metadata.provider, source_metadata.repo, source_metadata.sha
        ),
        source: Some(source_metadata.repo_url.clone()),
        issued_at: Some(issued_at.to_owned()),
        metadata: Some(JsonValue::Object(metadata)),
    }]
}

pub(super) fn build_publisher_attestations(
    publisher: &RegistryPublisher,
    trust_tier: &TrustTier,
    issued_at: &str,
) -> Vec<RegistryAttestation> {
    let label = publisher
        .display_name
        .as_ref()
        .or(publisher.handle.as_ref())
        .unwrap_or(&publisher.id);
    let mut metadata = JsonObject::new();
    metadata.insert(
        "publisher_id".to_owned(),
        JsonValue::String(publisher.id.clone()),
    );
    metadata.insert(
        "publisher_kind".to_owned(),
        JsonValue::String(publisher.kind.clone()),
    );
    if let Some(handle) = &publisher.handle {
        metadata.insert(
            "publisher_handle".to_owned(),
            JsonValue::String(handle.clone()),
        );
    }
    if let Some(display_name) = &publisher.display_name {
        metadata.insert(
            "publisher_display_name".to_owned(),
            JsonValue::String(display_name.clone()),
        );
    }
    metadata.insert(
        "trust_tier".to_owned(),
        JsonValue::String(trust_tier_string(trust_tier).to_owned()),
    );
    vec![RegistryAttestation {
        kind: "publisher".to_owned(),
        id: format!("publisher:{}", publisher.id),
        status: if *trust_tier == TrustTier::Community {
            "declared".to_owned()
        } else {
            "verified".to_owned()
        },
        summary: label.clone(),
        source: None,
        issued_at: Some(issued_at.to_owned()),
        metadata: Some(JsonValue::Object(metadata)),
    }]
}

pub(super) fn merge_registry_attestations(
    groups: Vec<Vec<RegistryAttestation>>,
) -> Vec<RegistryAttestation> {
    let mut keys: Vec<String> = Vec::new();
    let mut merged: Vec<RegistryAttestation> = Vec::new();
    for attestation in groups.into_iter().flatten() {
        let key = format!("{}:{}", attestation.kind, attestation.id);
        if let Some(index) = keys.iter().position(|candidate| candidate == &key) {
            merged[index] = attestation;
        } else {
            keys.push(key);
            merged.push(attestation);
        }
    }
    merged
}

pub(super) fn normalize_attestations(
    attestations: Vec<RegistryAttestation>,
    source_metadata: Option<&RegistrySourceMetadata>,
    publisher: &RegistryPublisher,
    trust_tier: &TrustTier,
    created_at: &str,
) -> Vec<RegistryAttestation> {
    merge_registry_attestations(vec![
        build_publisher_attestations(publisher, trust_tier, created_at),
        build_source_attestations(source_metadata, created_at),
        attestations,
    ])
}

pub(super) fn derive_trust_signals(version: &RegistrySkillVersion) -> Vec<TrustSignal> {
    vec![
        digest_trust_signal(version),
        trust_tier_signal(version),
        publisher_trust_signal(version),
        provenance_trust_signal(version),
        scopes_trust_signal(version),
        runner_metadata_trust_signal(version),
    ]
}

pub(super) fn digest_trust_signal(version: &RegistrySkillVersion) -> TrustSignal {
    TrustSignal {
        id: "digest".to_owned(),
        label: "Immutable digest".to_owned(),
        status: "verified".to_owned(),
        value: display_sha256(&version.digest),
    }
}

pub(super) fn trust_tier_signal(version: &RegistrySkillVersion) -> TrustSignal {
    TrustSignal {
        id: "trust_tier".to_owned(),
        label: "Trust tier".to_owned(),
        status: if version.trust_tier == TrustTier::Community {
            "declared".to_owned()
        } else {
            "verified".to_owned()
        },
        value: trust_tier_string(&version.trust_tier).to_owned(),
    }
}

pub(super) fn publisher_trust_signal(version: &RegistrySkillVersion) -> TrustSignal {
    let attestation = version
        .attestations
        .iter()
        .find(|attestation| attestation.kind == "publisher");
    TrustSignal {
        id: "publisher".to_owned(),
        label: "Publisher identity".to_owned(),
        status: attestation
            .map_or("not_declared", |attestation| attestation.status.as_str())
            .to_owned(),
        value: publisher_label(&version.publisher).to_owned(),
    }
}

pub(super) fn provenance_trust_signal(version: &RegistrySkillVersion) -> TrustSignal {
    let provenance = source_provenance(version);
    TrustSignal {
        id: "provenance".to_owned(),
        label: "Source provenance".to_owned(),
        status: if provenance.is_some() {
            "verified".to_owned()
        } else {
            "not_declared".to_owned()
        },
        value: provenance.unwrap_or_else(|| "no source attestation".to_owned()),
    }
}

pub(super) fn scopes_trust_signal(version: &RegistrySkillVersion) -> TrustSignal {
    TrustSignal {
        id: "scopes".to_owned(),
        label: "Required scopes".to_owned(),
        status: declared_status(!version.required_scopes.is_empty()).to_owned(),
        value: if version.required_scopes.is_empty() {
            "none declared".to_owned()
        } else {
            version.required_scopes.join(", ")
        },
    }
}

pub(super) fn runner_metadata_trust_signal(version: &RegistrySkillVersion) -> TrustSignal {
    TrustSignal {
        id: "runner_metadata".to_owned(),
        label: "Materialized binding".to_owned(),
        status: if version.profile_digest.is_some() {
            "verified".to_owned()
        } else {
            "not_declared".to_owned()
        },
        value: runner_metadata_value(version),
    }
}

pub(super) fn publisher_label(publisher: &RegistryPublisher) -> &str {
    publisher
        .display_name
        .as_ref()
        .or(publisher.handle.as_ref())
        .unwrap_or(&publisher.id)
}

pub(super) fn runner_metadata_value(version: &RegistrySkillVersion) -> String {
    version.profile_digest.as_ref().map_or_else(
        || "portable agent runner".to_owned(),
        |digest| {
            format!(
                "{} runner(s), binding {}",
                version.runner_names.len(),
                display_sha256(digest)
            )
        },
    )
}

pub(super) fn declared_status(is_declared: bool) -> &'static str {
    if is_declared {
        "declared"
    } else {
        "not_declared"
    }
}

pub(super) fn source_provenance(version: &RegistrySkillVersion) -> Option<String> {
    if let Some(source_metadata) = &version.source_metadata {
        return Some(format!(
            "{}:{}@{}",
            source_metadata.provider, source_metadata.repo, source_metadata.sha
        ));
    }
    version
        .attestations
        .iter()
        .find(|attestation| attestation.kind == "source")
        .map(|attestation| attestation.summary.clone())
}

pub(super) fn search_result_for_version(
    version: &RegistrySkillVersion,
    registry_url: Option<&str>,
) -> Result<RegistrySearchResult, LocalRegistryError> {
    let link = runx_link_for_version(version, registry_url)?;
    Ok(RegistrySearchResult {
        skill_id: version.skill_id.clone(),
        name: version.name.clone(),
        summary: version.description.clone(),
        category: version.category.clone(),
        owner: version.owner.clone(),
        version: Some(version.version.clone()),
        digest: Some(version.digest.clone()),
        source: Some("runx-registry".to_owned()),
        source_label: Some("runx registry".to_owned()),
        profile_mode: if version.profile_document.is_some() {
            ProfileMode::Profiled
        } else {
            ProfileMode::Portable
        },
        runner_names: version.runner_names.clone(),
        profile_digest: version.profile_digest.clone(),
        profile_trust_tier: version
            .profile_document
            .as_ref()
            .map(|_| version.trust_tier.clone()),
        required_scopes: version.required_scopes.clone(),
        tags: version.tags.clone(),
        trust_tier: version.trust_tier.clone(),
        trust_signals: derive_trust_signals(version),
        install_command: link.install_command,
        run_command: link.run_command,
    })
}

pub(super) fn detail_for_version(
    version: &RegistrySkillVersion,
    registry_url: Option<&str>,
) -> Result<RegistrySkillDetail, LocalRegistryError> {
    let link = runx_link_for_version(version, registry_url)?;
    Ok(RegistrySkillDetail {
        skill_id: version.skill_id.clone(),
        owner: version.owner.clone(),
        name: version.name.clone(),
        description: version.description.clone(),
        category: version.category.clone(),
        version: version.version.clone(),
        digest: version.digest.clone(),
        signed_manifest: version.signed_manifest.clone(),
        markdown: version.markdown.clone(),
        profile_digest: version.profile_digest.clone(),
        package_files: version.package_files.clone(),
        package_digest: version.package_digest.clone(),
        paid_listing: version.paid_listing.clone(),
        runner_names: version.runner_names.clone(),
        trust_tier: version.trust_tier.clone(),
        required_scopes: version.required_scopes.clone(),
        tags: version.tags.clone(),
        publisher: version.publisher.clone(),
        source_metadata: version.source_metadata.clone(),
        attestations: version.attestations.clone(),
        install_command: link.install_command,
        run_command: link.run_command,
    })
}

pub(super) fn searchable_text(version: &RegistrySkillVersion) -> String {
    let mut parts = vec![
        version.skill_id.clone(),
        version.name.clone(),
        version.owner.clone(),
    ];
    if let Some(description) = &version.description {
        parts.push(description.clone());
    }
    if let Some(category) = &version.category {
        parts.push(category.clone());
    }
    parts.extend(version.runner_names.clone());
    parts.extend(version.tags.clone());
    normalize(&parts.join(" "))
}

pub(super) fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}
