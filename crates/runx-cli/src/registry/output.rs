use runx_runtime::registry::{InstallStatus, RegistrySkillResolution, TrustTier};

use super::{RegistryCliError, RegistryCliOutput, internal_error};
use runx_runtime::registry::HostedSkillPublishResult;

#[derive(serde::Serialize)]
pub(super) struct RegistryEnvelope<T> {
    pub(super) status: &'static str,
    pub(super) registry: T,
}

#[derive(serde::Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(super) enum RegistryPayload {
    Search {
        source: &'static str,
        query: String,
        results: Vec<runx_runtime::registry::RegistrySearchResult>,
    },
    Read {
        source: &'static str,
        r#ref: String,
        skill: Box<runx_runtime::registry::RegistrySkillDetail>,
    },
    Resolve {
        source: &'static str,
        r#ref: String,
        resolution: RemoteOrLocalResolution,
    },
    Install {
        source: &'static str,
        r#ref: String,
        install: Box<runx_runtime::registry::InstallLocalSkillResult>,
        receipt_metadata: runx_contracts::JsonObject,
    },
    Package {
        package: Box<runx_runtime::registry::RegistryPublishPackageParts>,
    },
    ConnectorPackage {
        connector: Box<runx_runtime::registry::RegistryConnectorPackage>,
    },
    ConnectorPublish {
        publish: Box<runx_runtime::registry::HostedConnectorPublishResult>,
    },
    ConnectorRead {
        source: &'static str,
        connector: Box<runx_runtime::registry::RegistryConnectorDetail>,
    },
    ConnectorInstall {
        source: &'static str,
        install: Box<runx_runtime::registry::InstalledRegistryConnector>,
    },
    MarkCheck {
        mark: super::mark::CheckedMark,
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

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum RemoteOrLocalResolution {
    Remote(Box<runx_runtime::registry::RegistrySkillDetail>),
    Local(Box<RegistrySkillResolution>),
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
) -> String {
    let mut output = format!(
        "\n  registry search  {query}\n  source           {source}\n  results          {}\n\n",
        results.len()
    );
    for result in results {
        let category = result
            .category
            .as_ref()
            .map(|category| format!("    category {category}\n"))
            .unwrap_or_default();
        output.push_str(&format!(
            "  - {}@{}\n{}    digest   {}\n    trust    {}\n    install  {}\n    run      {}\n",
            result.skill_id,
            result.version.as_deref().unwrap_or("unknown"),
            category,
            result
                .digest
                .as_deref()
                .map_or("unknown".to_owned(), digest_label),
            trust_tier_label(&result.trust_tier),
            result.install_command.copy_text,
            result.run_command.copy_text,
        ));
    }
    output.push('\n');
    output
}

pub(super) fn render_read(
    source: &str,
    registry_ref: &str,
    skill: &runx_runtime::registry::RegistrySkillDetail,
) -> String {
    let category = skill
        .category
        .as_ref()
        .map(|category| format!("  category         {category}\n"))
        .unwrap_or_default();
    format!(
        "\n  registry read    {registry_ref}\n  source           {source}\n  skill            {}\n{}  version          {}\n  digest           {}\n  trust            {}\n  signed           {}\n  next             {}\n\n",
        skill.skill_id,
        category,
        skill.version,
        digest_label(&skill.digest),
        trust_tier_label(&skill.trust_tier),
        signed_manifest_label(skill.signed_manifest.as_ref()),
        skill.run_command.copy_text,
    )
}

pub(super) fn render_resolve(
    source: &str,
    registry_ref: &str,
    resolution: &RemoteOrLocalResolution,
) -> String {
    match resolution {
        RemoteOrLocalResolution::Remote(resolved) => format!(
            "\n  registry resolve {registry_ref}\n  source           {source}\n  skill            {}\n  version          {}\n  digest           {}\n  trust            {}\n  signed           {}\n  next             {}\n\n",
            resolved.skill_id,
            resolved.version,
            digest_label(&resolved.digest),
            trust_tier_label(&resolved.trust_tier),
            signed_manifest_label(resolved.signed_manifest.as_ref()),
            resolved.run_command.copy_text,
        ),
        RemoteOrLocalResolution::Local(resolved) => format!(
            "\n  registry resolve {registry_ref}\n  source           {source}\n  skill            {}\n  version          {}\n  digest           {}\n  trust            {}\n  signed           {}\n  next             {}\n\n",
            resolved.skill_id,
            resolved.version,
            digest_label(&resolved.digest),
            trust_tier_label(&resolved.trust_tier),
            signed_manifest_label(resolved.signed_manifest.as_ref()),
            resolved.run_command.copy_text,
        ),
    }
}

pub(super) fn render_install(
    source: &str,
    registry_ref: &str,
    install: &runx_runtime::registry::InstallLocalSkillResult,
    signed_manifest: Option<&runx_runtime::registry::RegistrySignedManifest>,
) -> String {
    format!(
        "\n  registry install {registry_ref}\n  source           {source}\n  status           {}\n  skill            {}\n  version          {}\n  digest           {}\n  trust            {}\n  signed           {}\n  destination      {}\n  next             {}\n\n",
        install_status_label(&install.status),
        install.skill_id.as_deref().unwrap_or(&install.skill_name),
        install.version.as_deref().unwrap_or("unknown"),
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
        "\n  registry connector publish  {}\n  connector        {}@{}\n  content digest   {}\n  manifest digest  {}\n  trust            {}\n  install          {}\n  page             {}\n\n",
        published.status,
        published.connector_ref,
        published.version,
        published.content_digest,
        published.manifest_digest,
        trust_tier_label(&published.trust_tier),
        published.install_command.copy_text,
        published.public_url,
    )
}

pub(super) fn render_connector_read(
    connector: &runx_runtime::registry::RegistryConnectorDetail,
) -> String {
    let mut output = format!(
        "\n  registry connector read  {}@{}\n  name             {}\n  content digest   {}\n  manifest digest  {}\n  trust            {}\n  operations       {}\n",
        connector.connector_ref,
        connector.version,
        connector.name,
        connector.content_digest,
        connector.manifest_digest,
        trust_tier_label(&connector.trust_tier),
        connector.operations.len(),
    );
    for operation in &connector.operations {
        output.push_str(&format!(
            "    - {}  {}\n",
            operation.name, operation.description
        ));
    }
    output.push_str(&format!(
        "  next             {}\n\n",
        connector.install_command.copy_text
    ));
    output
}

pub(super) fn render_connector_install(
    install: &runx_runtime::registry::InstalledRegistryConnector,
) -> String {
    format!(
        "\n  registry connector install  {}@{}\n  status           {}\n  content digest   {}\n  manifest digest  {}\n  trust            {}\n  destination      {}\n\n",
        install.connector_ref,
        install.version,
        install_status_label(&install.status),
        install.content_digest,
        install.manifest_digest,
        trust_tier_label(&install.trust_tier),
        install.destination.display(),
    )
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
