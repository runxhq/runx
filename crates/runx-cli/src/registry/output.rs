use runx_runtime::registry::{InstallStatus, TrustTier};

use super::{RegistryCliError, RegistryCliOutput, internal_error};
use runx_runtime::registry::HostedSkillPublishResult;

#[derive(serde::Serialize)]
pub(super) struct RegistryEnvelope<T> {
    pub(super) status: &'static str,
    pub(super) registry: T,
}

/// Who publishes an acquired skill, and how many installations acquired it.
#[derive(serde::Serialize)]
pub(super) struct InstallAcquisition {
    pub(super) publisher: runx_runtime::registry::RegistryPublisher,
    pub(super) install_count: u64,
}

#[derive(serde::Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(super) enum RegistryPayload {
    Search {
        source: &'static str,
        query: String,
        results: Vec<runx_runtime::registry::RegistrySearchResult>,
        /// The published connectors that match; a hosted registry only.
        connectors: Vec<runx_runtime::registry::RegistryConnectorSearchResult>,
    },
    Install {
        source: &'static str,
        r#ref: String,
        install: Box<runx_runtime::registry::InstallLocalSkillResult>,
        /// The installed package's digest, when the registry states one.
        #[serde(skip_serializing_if = "Option::is_none")]
        package_digest: Option<String>,
        /// What the hosted registry reported acquiring it.
        #[serde(skip_serializing_if = "Option::is_none")]
        acquisition: Option<InstallAcquisition>,
    },
    Package {
        package: Box<runx_runtime::registry::RegistryPublishPackageParts>,
        /// The exact complete content identity used by default publication.
        content_digest: String,
    },
    ConnectorPackage {
        connector: Box<runx_runtime::registry::RegistryConnectorPackage>,
    },
    ConnectorPublish {
        publish: Box<runx_runtime::registry::HostedConnectorPublishResult>,
    },
    ConnectorInstall {
        source: &'static str,
        install: Box<runx_runtime::registry::InstalledRegistryConnector>,
    },
    Publish {
        publish: PublishPayload,
    },
}

#[derive(serde::Serialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub(super) enum PublishPayload {
    Hosted(Box<HostedSkillPublishResult>),
    Local(Box<runx_runtime::registry::PublishSkillMarkdownResult>),
}

pub(super) fn write_output<T: serde::Serialize>(
    json: bool,
    value: &T,
    human: impl FnOnce() -> String,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let stdout = if json {
        serde_json::to_string_pretty(value)
            .map(|json| format!("{json}\n"))
            .map_err(|error| internal_error(error.to_string()))?
    } else {
        human()
    };
    Ok(RegistryCliOutput {
        stdout,
        exit_code: 0,
    })
}

pub(super) fn render_search(
    query: &str,
    source: &str,
    results: &[runx_runtime::registry::RegistrySearchResult],
    connectors: &[runx_runtime::registry::RegistryConnectorSearchResult],
) -> String {
    let mut output = format!(
        "\n  registry search  {query}\n  source           {source}\n  results          {}\n\n",
        results.len() + connectors.len()
    );
    // A connector is named by its ref; its display name is only its title.
    for connector in connectors {
        output.push_str(&format!(
            "  - {}@{}  connector\n    {}\n    trust    {}\n    install  runx registry install {}@{}\n",
            connector.connector,
            connector.version,
            connector.name,
            trust_tier_label(&connector.trust_tier),
            connector.connector,
            connector.version,
        ));
    }
    for result in results {
        let category = result
            .category
            .as_ref()
            .map(|category| format!("    category {category}\n"))
            .unwrap_or_default();
        // A hosted search names each skill's current version, not its digest;
        // a line is shown only for what the registry stated.
        let digest = result
            .digest
            .as_deref()
            .map(|digest| format!("    digest   {}\n", digest_label(digest)))
            .unwrap_or_default();
        output.push_str(&format!(
            "  - {}@{}\n{}{}    trust    {}\n    install  {}\n    run      {}\n",
            result.skill_id,
            result.version.as_deref().unwrap_or("unknown"),
            category,
            digest,
            trust_tier_label(&result.trust_tier),
            result.install_command.copy_text,
            result.run_command.copy_text,
        ));
    }
    output.push('\n');
    output
}

pub(super) fn render_install(
    source: &str,
    registry_ref: &str,
    install: &runx_runtime::registry::InstallLocalSkillResult,
    signed_manifest: Option<&runx_runtime::registry::RegistrySignedManifest>,
) -> String {
    let skill = install.skill_id.as_deref().unwrap_or(&install.skill_name);
    format!(
        "\n  registry install {registry_ref}\n  source           {source}\n  status           {}\n  skill            {skill}\n  version          {}\n{}  digest           {}\n  trust            {}\n  signed           {}\n  destination      {}\n  next             {}\n\n",
        install_status_label(&install.status),
        install.version.as_deref().unwrap_or("unknown"),
        newest_unread_line(skill, install.newest_unread.as_ref()),
        digest_label(&install.digest),
        install
            .trust_tier
            .as_ref()
            .map_or("unknown", trust_tier_label),
        signed_manifest_label(signed_manifest),
        install.destination.display(),
        install_run_command(install),
    )
}

fn install_run_command(install: &runx_runtime::registry::InstallLocalSkillResult) -> String {
    let skill = match (&install.skill_id, &install.version) {
        (Some(skill_id), Some(version)) => format!("{skill_id}@{version}"),
        _ => install.skill_name.clone(),
    };
    runx_contracts::CopyableCommand::posix(vec!["runx".to_owned(), "skill".to_owned(), skill])
        .map_or_else(|_| "unavailable".to_owned(), |command| command.copy_text)
}

pub(super) fn render_connector_publish(
    published: &runx_runtime::registry::HostedConnectorPublishResult,
) -> String {
    format!(
        "\n  registry publish {}\n  connector        {}@{}\n  content digest   {}\n  manifest digest  {}\n  trust            {}\n  install          {}\n  page             {}\n\n",
        published.status,
        published.name,
        published.version,
        published.content_digest,
        published.manifest_digest,
        trust_tier_label(&published.trust_tier),
        published.install_command.copy_text,
        published.public_url,
    )
}

pub(super) fn render_connector_install(
    install: &runx_runtime::registry::InstalledRegistryConnector,
) -> String {
    format!(
        "\n  registry install {}@{}\n{}  status           {}\n  content digest   {}\n  manifest digest  {}\n  trust            {}\n  destination      {}\n\n",
        install.name,
        install.version,
        newest_unread_line(install.name.as_str(), install.newest_unread.as_ref()),
        install_status_label(&install.status),
        install.content_digest,
        install.manifest_digest,
        trust_tier_label(&install.trust_tier),
        install.destination.display(),
    )
}

/// The line naming the newest version this runx stepped past, when it did.
fn newest_unread_line(
    subject: &str,
    unread: Option<&runx_runtime::registry::UnreadVersion>,
) -> String {
    unread.map_or_else(String::new, |unread| {
        format!(
            "  newest           {subject}@{} is not read by this runx: {}\n",
            unread.version, unread.reason
        )
    })
}

fn signed_manifest_label(
    manifest: Option<&runx_runtime::registry::RegistrySignedManifest>,
) -> String {
    manifest.map_or_else(
        || "no".to_owned(),
        |manifest| format!("yes ({})", manifest.signer.key_id),
    )
}

fn digest_label(digest: &str) -> String {
    if digest.starts_with("sha256:") {
        digest.to_owned()
    } else {
        format!("sha256:{digest}")
    }
}

fn trust_tier_label(tier: &TrustTier) -> &'static str {
    match tier {
        TrustTier::FirstParty => "first_party",
        TrustTier::Verified => "verified",
        TrustTier::Community => "community",
    }
}

fn install_status_label(status: &InstallStatus) -> &'static str {
    match status {
        InstallStatus::Installed => "installed",
        InstallStatus::Unchanged => "unchanged",
    }
}
