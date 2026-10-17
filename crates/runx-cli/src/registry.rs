// Module rationale: native registry CLI keeps local and hosted
// search/install/package/publish command wiring together, for skills and
// connectors alike, so the command matrix and output envelope stay auditable.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::connector::ConnectorName;
use runx_runtime::ConfigError;
use runx_runtime::WorkspaceEnv;
use runx_runtime::registry::{
    AcquireOptions, FileRegistryStore, IngestSkillOptions, InstallCandidate,
    InstallLocalSkillOptions, PublishSkillMarkdownOptions, RegistryClient,
    RegistryManifestSourceAuthority, RegistryPublishPackageRequest, RegistryResolveOptions,
    RegistrySearchOptions, RegistrySkillResolution, TrustTier, TrustedRegistryManifestKey,
    install_local_skill, materialize_publish_closure, prepare_registry_publish_package,
    publish_skill_markdown, resolve_registry_skill, search_registry_with_options,
};

mod connector;
mod output;
mod remote_publish;
mod target;

pub(crate) use target::{RegistryTarget, registry_source_description, resolve_registry_target};

/// The registry's four verbs; each takes a skill or a connector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryAction {
    Search,
    /// Add what is published under a name to the project.
    Install,
    /// The exact package a skill or connector directory publishes as.
    Package,
    Publish,
}

#[derive(Debug, Eq, PartialEq)]
pub struct RegistryPlan {
    pub action: RegistryAction,
    /// The query, the `<publisher>/<name>` to install, or the package path.
    pub subject: String,
    pub registry: Option<String>,
    /// The version to install (from `<ref>@<version>`), or the label to
    /// publish.
    pub version: Option<String>,
    pub expected_digest: Option<String>,
    /// The namespace a publish goes into.
    pub publisher: Option<String>,
    pub profile: Option<PathBuf>,
    pub trust_tier: Option<TrustTier>,
    pub limit: Option<usize>,
    pub upsert: bool,
    pub json: bool,
}

impl RegistryPlan {
    /// The `<publisher>/<name>[@version]` an install was asked for.
    pub(crate) fn requested_ref(&self) -> String {
        match &self.version {
            Some(version) => format!("{}@{version}", self.subject),
            None => self.subject.clone(),
        }
    }
}

pub fn run_native_registry(plan: RegistryPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let json = plan.json;
    match run_registry(plan, workspace.env(), workspace.cwd()) {
        Ok(output) => crate::cli_io::write_stdout_code(&output.stdout, output.exit_code),
        Err(error) => {
            if json {
                return crate::cli_io::write_stdout_code(
                    &crate::router::json_failure_output(&error.message, error.code()),
                    error.exit_code,
                );
            }
            let _ignored = crate::cli_io::write_stderr(&format!("\n  ✗  {}\n\n", error.message));
            ExitCode::from(error.exit_code)
        }
    }
}

struct RegistryCliOutput {
    stdout: String,
    exit_code: u8,
}

fn run_registry(
    plan: RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    if let RegistryAction::Package | RegistryAction::Publish = plan.action
        && let PackageKind::Connector(directory) = package_kind(&plan.subject, env, cwd)?
    {
        // A connector names itself and carries no harness profile; the skill
        // flags name nothing here.
        if plan.publisher.is_some()
            || plan.profile.is_some()
            || plan.trust_tier.is_some()
            || plan.upsert
        {
            return Err(usage_error(
                "a connector names itself: package accepts only --json, publish only --registry, --version and --json",
            ));
        }
        return match plan.action {
            RegistryAction::Package => connector::package(&plan, &directory),
            _ => connector::publish(&plan, &directory, env, cwd),
        };
    }
    if plan.action == RegistryAction::Package {
        return run_package(plan, env, cwd);
    }
    let target = resolve_registry_target(&plan, env, cwd);
    match plan.action {
        RegistryAction::Search => run_search(plan, target, env),
        RegistryAction::Install => run_install(plan, target, env, cwd),
        RegistryAction::Publish => run_publish(plan, target, env, cwd),
        RegistryAction::Package => unreachable!("packaging is handled before registry resolution"),
    }
}

/// What a package path names.
enum PackageKind {
    Skill,
    /// The connector directory holding `connector.yaml`.
    Connector(PathBuf),
}

/// A connector is `connector.yaml` or a directory holding one; anything else
/// is a skill's `SKILL.md` or directory. A directory holding both is refused.
fn package_kind(
    subject: &str,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PackageKind, RegistryCliError> {
    let path = runx_runtime::RunxPaths::resolve(env, cwd).existing_input(subject);
    if path
        .file_name()
        .is_some_and(|name| name == "connector.yaml")
    {
        return Ok(PackageKind::Connector(
            path.parent()
                .map_or_else(|| path.clone(), Path::to_path_buf),
        ));
    }
    if !path.is_dir() {
        return Ok(PackageKind::Skill);
    }
    match (
        path.join("connector.yaml").is_file(),
        path.join("SKILL.md").is_file(),
    ) {
        (true, true) => Err(usage_error(format!(
            "{subject} holds both SKILL.md and connector.yaml; name the file you mean"
        ))),
        (true, false) => Ok(PackageKind::Connector(path)),
        _ => Ok(PackageKind::Skill),
    }
}

// Function rationale: package materialization is the read-only bridge for
// hosted and third-party publishers that need the exact parser-owned artifact
// without reimplementing package discovery.
fn run_package(
    plan: RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let package = prepare_registry_publish_package(RegistryPublishPackageRequest {
        subject: &plan.subject,
        profile: plan.profile.as_deref(),
        env,
        cwd,
    })
    .map_err(|error| internal_error(error.to_string()))?
    .into_parts();
    let content_digest = runx_runtime::registry::skill_content_digest(
        &package.markdown,
        package.profile_document.as_deref(),
        &package.package_files,
    )
    .map_err(|error| internal_error(error.to_string()))?;
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::Package {
                package: Box::new(package),
                content_digest,
            },
        },
        || "\n  registry package ready\n\n".to_owned(),
    )
}

fn run_search(
    plan: RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let source = target.label();
    let query = plan.subject;
    let limit = plan.limit.unwrap_or(20);
    // Connectors are published only to a hosted registry.
    let mut connectors = Vec::new();
    let results = match target {
        RegistryTarget::Remote { registry_url } => {
            let client = remote_registry_client(&registry_url, env)?;
            connectors = client.search_connectors(&query, limit)?;
            client.search_with_limit(&query, limit)?
        }
        RegistryTarget::Local {
            registry_path,
            registry_url,
            ..
        } => search_registry_with_options(
            &FileRegistryStore::new(registry_path),
            &query,
            RegistrySearchOptions {
                limit: plan.limit,
                registry_url,
            },
        )?,
    };
    let human = output::render_search(&query, source, &results, &connectors);
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::Search {
                source,
                query: query.clone(),
                results,
                connectors,
            },
        },
        || human,
    )
}

/// Adds what is published under the name. A skill carries the connectors it
/// binds, so a name both kinds publish adds the skill; a connector alone adds
/// the connector. A version names one package, since the registry refuses a
/// version the other kind already holds under that name.
fn run_install(
    plan: RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let source = target.label();
    let source_authority = target.manifest_source_authority();
    let fallback = target.clone();
    let Some((candidate, acquisition)) = install_candidate(&plan, target, env, cwd)? else {
        return match (fallback, ConnectorName::new(&plan.subject)) {
            // No skill is published under the name; a connector may be.
            (RegistryTarget::Remote { registry_url }, Some(connector_ref)) => {
                connector::install(&plan, &connector_ref, &registry_url, env, cwd)
            }
            (RegistryTarget::Remote { .. }, None) => {
                Err(connector::not_found(&plan.requested_ref()))
            }
            (RegistryTarget::Local { registry_path, .. }, _) => {
                Err(not_found_local(&plan.requested_ref(), &registry_path))
            }
        };
    };
    let install = install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: runx_runtime::RunxPaths::resolve(env, cwd).skills(),
            connectors_root: runx_runtime::RunxPaths::resolve(env, cwd).connectors(),
            expected_digest: plan.expected_digest.clone(),
            trusted_manifest_keys: trusted_manifest_keys_from_env_for_source(
                env,
                source_authority,
            )?,
        },
    )?;
    let requested = plan.requested_ref();
    let human = output::render_install(
        source,
        &requested,
        &install,
        candidate.signed_manifest.as_ref(),
    );
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::Install {
                source,
                r#ref: requested,
                install: Box::new(install),
                package_digest: candidate.package_digest.clone(),
                acquisition: acquisition.map(|acquired| output::InstallAcquisition {
                    publisher: acquired.publisher,
                    install_count: acquired.install_count,
                }),
            },
        },
        || human,
    )
}

// Function rationale: local and hosted publish share the same
// package-read and harness gate before diverging at the storage boundary.
fn run_publish(
    plan: RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let request = RegistryPublishPackageRequest {
        subject: &plan.subject,
        profile: plan.profile.as_deref(),
        env,
        cwd,
    };
    // A package naming skills other publishers own binds them from the
    // registry it is published to.
    let closure = match &target {
        RegistryTarget::Remote { registry_url } => materialize_publish_closure(
            &remote_registry_client(registry_url, env)?,
            registry_url,
            request,
        )
        .map_err(|error| usage_error(error.to_string()))?,
        RegistryTarget::Local { .. } => None,
    };
    let env = closure.as_ref().map_or(env, |closure| &closure.env);
    let package =
        prepare_registry_publish_package(RegistryPublishPackageRequest { env, ..request })
            .map_err(|error| internal_error(error.to_string()))?;
    let orchestrator = crate::runtime::local_orchestrator(env).map_err(|error| {
        internal_error(format!("failed to initialize runtime effects: {error}"))
    })?;
    let harness = package
        .run_harness(&orchestrator, env)
        .map_err(|error| internal_error(error.to_string()))?;
    match target {
        RegistryTarget::Remote { registry_url } => {
            let result = remote_publish::publish_remote_skill_package(
                &registry_url,
                &plan,
                &package,
                &harness,
                env,
                cwd,
            )?;
            output::write_output(
                plan.json,
                &output::RegistryEnvelope {
                    status: "success",
                    registry: output::RegistryPayload::Publish {
                        publish: output::PublishPayload::Hosted(Box::new(result)),
                    },
                },
                || "\n  registry publish success\n\n".to_owned(),
            )
        }
        RegistryTarget::Local {
            registry_path,
            registry_url,
            ..
        } => {
            let package = package.into_parts();
            let result = publish_skill_markdown(
                &FileRegistryStore::new(registry_path),
                &package.markdown,
                PublishSkillMarkdownOptions {
                    ingest: IngestSkillOptions {
                        owner: plan.publisher,
                        version: plan.version,
                        profile_document: package.profile_document,
                        package_files: package.package_files,
                        trust_tier: plan.trust_tier,
                        upsert: plan.upsert,
                        ..IngestSkillOptions::default()
                    },
                    registry_url,
                    harness,
                },
            )?;
            output::write_output(
                plan.json,
                &output::RegistryEnvelope {
                    status: "success",
                    registry: output::RegistryPayload::Publish {
                        publish: output::PublishPayload::Local(Box::new(result)),
                    },
                },
                || "\n  registry publish success\n\n".to_owned(),
            )
        }
    }
}

/// The skill `plan` names and how it was acquired, or none when the
/// registry publishes no skill under that name and version.
pub(crate) fn install_candidate(
    plan: &RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<
    Option<(
        InstallCandidate,
        Option<runx_runtime::registry::AcquiredRegistrySkill>,
    )>,
    RegistryCliError,
> {
    let source_authority = target.manifest_source_authority();
    match target {
        RegistryTarget::Remote { registry_url } => {
            let client = remote_registry_client(&registry_url, env)?;
            // One segment reaches the registry only as a root name, which it
            // resolves to the canonical `<owner>/<name>` it installs.
            let skill_id = if plan.subject.contains('/') {
                plan.subject.clone()
            } else {
                match client.read_root(&plan.subject, plan.version.as_deref())? {
                    Some(detail) => detail.skill_id,
                    None => return Ok(None),
                }
            };
            let installation_id = remote_installation_id(env, cwd)?;
            let acquired = match client.acquire(
                &skill_id,
                AcquireOptions {
                    installation_id: &installation_id,
                    version: plan.version.as_deref(),
                    channel: Some("cli"),
                },
            ) {
                Err(runx_runtime::registry::RegistryClientError::HttpStatus {
                    status: 404,
                    ..
                }) => return Ok(None),
                acquired => acquired?,
            };
            Ok(Some((
                candidate_from_acquired(&skill_id, &acquired, source_authority),
                Some(acquired),
            )))
        }
        RegistryTarget::Local {
            registry_path,
            registry_url,
            ..
        } => {
            let Some(resolution) = resolve_registry_skill(
                &FileRegistryStore::new(registry_path.clone()),
                &plan.subject,
                RegistryResolveOptions {
                    version: plan.version.clone(),
                    registry_url,
                },
            )?
            else {
                return Ok(None);
            };
            Ok(Some((
                candidate_from_resolution(&plan.subject, resolution, source_authority),
                None,
            )))
        }
    }
}

/// The canonical `<owner>/<name>` a root name addresses in the registry
/// `plan` targets; `None` where no skill holds it, or the registry is a
/// directory, which holds no root names.
pub(crate) fn resolve_root_name(
    plan: &RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Option<String>, RegistryCliError> {
    let RegistryTarget::Remote { registry_url } = resolve_registry_target(plan, env, cwd) else {
        return Ok(None);
    };
    Ok(remote_registry_client(&registry_url, env)?
        .read_root(&plan.subject, plan.version.as_deref())?
        .map(|detail| detail.skill_id))
}

fn remote_registry_client(
    registry_url: &str,
    env: &BTreeMap<String, String>,
) -> Result<RegistryClient, RegistryCliError> {
    let transport = runx_runtime::hosted_api_transport(
        runx_runtime::hosted_private_network_allowed(false, env),
    )
    .map_err(|error| internal_error(error.to_string()))?;
    RegistryClient::with_transport(registry_url, transport).map_err(RegistryCliError::from)
}

fn remote_installation_id(
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<String, RegistryCliError> {
    if let Some(installation_id) = env
        .get("RUNX_INSTALLATION_ID")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(installation_id.to_owned());
    }

    runx_runtime::installation_id(&runx_runtime::RunxPaths::resolve(env, cwd))
        .map_err(|error| usage_error(error.to_string()))
}

fn candidate_from_resolution(
    registry_ref: &str,
    resolution: RegistrySkillResolution,
    source_authority: RegistryManifestSourceAuthority,
) -> InstallCandidate {
    InstallCandidate {
        markdown: resolution.markdown,
        profile_document: resolution.profile_document,
        package_files: resolution.package_files,
        package_digest: resolution.package_digest,
        source: resolution.source,
        source_label: resolution.source_label,
        r#ref: registry_ref.to_owned(),
        skill_id: Some(resolution.skill_id),
        version: Some(resolution.version),
        signed_manifest: resolution.signed_manifest,
        profile_digest: resolution.profile_digest,
        runner_names: resolution.runner_names,
        trust_tier: Some(resolution.trust_tier),
        manifest_source_authority: Some(source_authority),
    }
}

fn candidate_from_acquired(
    registry_ref: &str,
    acquired: &runx_runtime::registry::AcquiredRegistrySkill,
    source_authority: RegistryManifestSourceAuthority,
) -> InstallCandidate {
    InstallCandidate {
        markdown: acquired.markdown.clone(),
        profile_document: acquired.profile_document.clone(),
        package_files: acquired.package_files.clone(),
        package_digest: acquired.package_digest.clone(),
        source: "runx-registry".to_owned(),
        source_label: "runx registry".to_owned(),
        r#ref: registry_ref.to_owned(),
        skill_id: Some(acquired.skill_id.clone()),
        version: Some(acquired.version.clone()),
        signed_manifest: acquired.signed_manifest.clone(),
        profile_digest: acquired.profile_digest.clone(),
        runner_names: acquired.runner_names.clone(),
        trust_tier: Some(acquired.trust_tier.clone()),
        manifest_source_authority: Some(source_authority),
    }
}

pub(crate) fn trusted_manifest_keys_from_env_for_source(
    env: &BTreeMap<String, String>,
    source_authority: RegistryManifestSourceAuthority,
) -> Result<Vec<TrustedRegistryManifestKey>, RegistryCliError> {
    runx_runtime::registry::trusted_registry_manifest_keys_from_env_with_source(
        env,
        Some(source_authority),
    )
    .map_err(trust_env_error)
}

fn trust_env_error(
    error: runx_runtime::registry::RegistryManifestTrustEnvError,
) -> RegistryCliError {
    match error {
        runx_runtime::registry::RegistryManifestTrustEnvError::InvalidKey => {
            internal_error(error.to_string())
        }
        runx_runtime::registry::RegistryManifestTrustEnvError::MissingKeyId => {
            usage_error(error.to_string())
        }
        runx_runtime::registry::RegistryManifestTrustEnvError::MissingOwner => {
            usage_error(error.to_string())
        }
        runx_runtime::registry::RegistryManifestTrustEnvError::MissingSource => {
            usage_error(error.to_string())
        }
    }
}

pub(crate) struct RegistryCliError {
    message: String,
    exit_code: u8,
}

impl std::fmt::Display for RegistryCliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::fmt::Debug for RegistryCliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegistryCliError")
            .field("message", &self.message)
            .field("exit_code", &self.exit_code)
            .finish()
    }
}

impl std::error::Error for RegistryCliError {}

impl RegistryCliError {
    pub(crate) fn into_message(self) -> String {
        self.message
    }

    fn code(&self) -> &'static str {
        if self.exit_code == 64 {
            "invalid_args"
        } else {
            "registry_error"
        }
    }
}

fn usage_error(message: impl Into<String>) -> RegistryCliError {
    RegistryCliError {
        message: message.into(),
        exit_code: 64,
    }
}

fn internal_error(message: impl Into<String>) -> RegistryCliError {
    RegistryCliError {
        message: message.into(),
        exit_code: 1,
    }
}

fn not_found_local(registry_ref: &str, registry_path: &Path) -> RegistryCliError {
    RegistryCliError {
        message: format!(
            "registry skill not found: {registry_ref} in the local registry at {}; for the hosted catalog pass --registry https://api.runx.ai",
            registry_path.display()
        ),
        exit_code: 1,
    }
}

impl From<runx_runtime::registry::RegistryClientError> for RegistryCliError {
    fn from(error: runx_runtime::registry::RegistryClientError) -> Self {
        internal_error(error.to_string())
    }
}

impl From<runx_runtime::registry::LocalRegistryError> for RegistryCliError {
    fn from(error: runx_runtime::registry::LocalRegistryError) -> Self {
        internal_error(error.to_string())
    }
}

impl From<runx_runtime::registry::InstallError> for RegistryCliError {
    fn from(error: runx_runtime::registry::InstallError) -> Self {
        let error_kind = match &error {
            runx_runtime::registry::InstallError::UnsignedManifest(_) => Some("unsigned_manifest"),
            runx_runtime::registry::InstallError::UnknownManifestKey { .. } => Some("unknown_key"),
            runx_runtime::registry::InstallError::InvalidManifestSignature { .. } => {
                Some("invalid_signature")
            }
            runx_runtime::registry::InstallError::DigestMismatch { .. } => Some("digest_mismatch"),
            _ => None,
        };
        match error_kind {
            Some(kind) => internal_error(format!("registry install {kind}: {error}")),
            None => internal_error(error.to_string()),
        }
    }
}

impl From<ConfigError> for RegistryCliError {
    fn from(error: ConfigError) -> Self {
        internal_error(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runx_runtime::registry::{InstallLocalSkillResult, InstallStatus, TrustTier};

    #[test]
    fn registry_install_render_shows_direct_skill_run_command() {
        let rendered = output::render_install(
            "local",
            "acme/echo@1.2.3",
            &InstallLocalSkillResult {
                status: InstallStatus::Installed,
                destination: PathBuf::from("/tmp/runx/skills/acme/echo/SKILL.md"),
                skill_name: "echo".to_owned(),
                source: "local".to_owned(),
                source_label: "local registry".to_owned(),
                skill_id: Some("acme/echo".to_owned()),
                version: Some("1.2.3".to_owned()),
                digest: "sha256:abc".to_owned(),
                profile_digest: None,
                runner_names: Vec::new(),
                trust_tier: Some(TrustTier::Community),
            },
            None,
        );

        assert!(rendered.contains("next             runx skill acme/echo@1.2.3"));
    }
}
