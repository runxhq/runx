// Module rationale: native registry CLI keeps local and hosted
// search/read/resolve/install/publish command wiring together so the command
// matrix and output envelope stay auditable.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_runtime::ConfigError;
use runx_runtime::registry::{
    AcquireOptions, FileRegistryStore, IngestSkillOptions, InstallCandidate,
    InstallLocalSkillOptions, PublishSkillMarkdownOptions, RegistryClient,
    RegistryManifestSourceAuthority, RegistryPublishPackageRequest, RegistryResolveOptions,
    RegistrySearchOptions, RegistrySkillResolution, TrustTier, TrustedRegistryManifestKey,
    install_local_skill, prepare_registry_publish_package, publish_skill_markdown,
    read_registry_skill, resolve_registry_skill, search_registry_with_options,
};
use runx_runtime::{InitGeneratedValues, WorkspaceEnv, ensure_runx_install_state};

mod connector;
mod mark;
mod output;
mod remote_publish;
mod target;

pub use connector::ConnectorAction;
pub(crate) use connector::parse_connector_subject;

pub(crate) use target::{
    RegistryTarget, destination_root, registry_source_description, resolve_registry_target,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryAction {
    Search,
    Read,
    Resolve,
    Install,
    Package,
    Publish,
    /// `runx registry connector ...`.
    Connector(ConnectorAction),
    /// `runx registry mark check <file>`.
    MarkCheck,
}

#[derive(Debug, Eq, PartialEq)]
pub struct RegistryPlan {
    pub action: RegistryAction,
    pub subject: String,
    pub registry: Option<String>,
    pub registry_dir: Option<PathBuf>,
    pub version: Option<String>,
    pub expected_digest: Option<String>,
    pub destination: Option<PathBuf>,
    pub owner: Option<String>,
    pub profile: Option<PathBuf>,
    pub trust_tier: Option<TrustTier>,
    pub limit: Option<usize>,
    pub upsert: bool,
    pub json: bool,
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
    match plan.action {
        RegistryAction::Package => return run_package(plan, env, cwd),
        RegistryAction::Connector(action) => {
            return connector::run_connector(action, plan, env, cwd);
        }
        RegistryAction::MarkCheck => return mark::run_mark_check(plan, env, cwd),
        _ => {}
    }
    let target = resolve_registry_target(&plan, env, cwd);
    match plan.action {
        RegistryAction::Search => run_search(plan, target, env),
        RegistryAction::Read => run_read(plan, target, env),
        RegistryAction::Resolve => run_resolve(plan, target, env),
        RegistryAction::Install => run_install(plan, target, env, cwd),
        RegistryAction::Package | RegistryAction::Connector(_) | RegistryAction::MarkCheck => {
            unreachable!("packaging, connectors and marks are handled before registry resolution")
        }
        RegistryAction::Publish => run_publish(plan, target, env, cwd),
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
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::Package {
                package: Box::new(package),
            },
        },
        || "\n  registry package  ready\n\n".to_owned(),
    )
}

fn run_search(
    plan: RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let source = target.label();
    let query = plan.subject;
    let results = match target {
        RegistryTarget::Remote { registry_url } => remote_registry_client(&registry_url, env)?
            .search_with_limit(&query, plan.limit.unwrap_or(20))?,
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
    let human = output::render_search(&query, source, &results);
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::Search {
                source,
                query: query.clone(),
                results,
            },
        },
        || human,
    )
}

fn run_read(
    plan: RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let source = target.label();
    let skill = match target {
        RegistryTarget::Remote { registry_url } => remote_registry_client(&registry_url, env)?
            .read(&plan.subject, plan.version.as_deref())?
            .ok_or_else(|| not_found(&plan.subject))?,
        RegistryTarget::Local {
            registry_path,
            registry_url,
            ..
        } => read_registry_skill(
            &FileRegistryStore::new(registry_path.clone()),
            &plan.subject,
            plan.version.as_deref(),
            registry_url.as_deref(),
        )?
        .ok_or_else(|| not_found_local(&plan.subject, &registry_path))?,
    };
    let human = output::render_read(source, &plan.subject, &skill);
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::Read {
                source,
                r#ref: plan.subject,
                skill: Box::new(skill),
            },
        },
        || human,
    )
}

fn run_resolve(
    plan: RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let source = target.label();
    let resolution = match target {
        RegistryTarget::Remote { registry_url } => {
            let client = remote_registry_client(&registry_url, env)?;
            let resolved = client
                .resolve_ref(&plan.subject, plan.version.as_deref())?
                .ok_or_else(|| not_found(&plan.subject))?;
            let detail = client
                .read(&resolved.skill_id, resolved.version.as_deref())?
                .ok_or_else(|| not_found(&resolved.skill_id))?;
            output::RemoteOrLocalResolution::Remote(Box::new(detail))
        }
        RegistryTarget::Local {
            registry_path,
            registry_url,
            ..
        } => output::RemoteOrLocalResolution::Local(Box::new(
            resolve_registry_skill(
                &FileRegistryStore::new(registry_path.clone()),
                &plan.subject,
                RegistryResolveOptions {
                    version: plan.version,
                    registry_url,
                },
            )?
            .ok_or_else(|| not_found_local(&plan.subject, &registry_path))?,
        )),
    };
    let human = output::render_resolve(source, &plan.subject, &resolution);
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::Resolve {
                source,
                r#ref: plan.subject,
                resolution,
            },
        },
        || human,
    )
}

fn run_install(
    plan: RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let source = target.label();
    let source_authority = target.manifest_source_authority();
    let (candidate, acquisition) = install_candidate(&plan, target, env, cwd)?;
    let install = install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: destination_root(&plan, env, cwd),
            expected_digest: plan.expected_digest,
            trusted_manifest_keys: trusted_manifest_keys_from_env_for_source(
                env,
                source_authority,
            )?,
        },
    )?;
    let receipt_metadata = runx_runtime::registry_install_receipt_metadata(
        runx_runtime::RegistryInstallMetadataInput {
            candidate: &candidate,
            install: &install,
            acquisition: acquisition.as_ref(),
        },
    );
    let human = output::render_install(
        source,
        &plan.subject,
        &install,
        candidate.signed_manifest.as_ref(),
    );
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::Install {
                source,
                r#ref: plan.subject,
                install: Box::new(install),
                receipt_metadata,
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
    let package = prepare_registry_publish_package(RegistryPublishPackageRequest {
        subject: &plan.subject,
        profile: plan.profile.as_deref(),
        env,
        cwd,
    })
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
                || "\n  registry publish  success\n\n".to_owned(),
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
                        owner: plan.owner,
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
                || "\n  registry publish  success\n\n".to_owned(),
            )
        }
    }
}

pub(crate) fn install_candidate(
    plan: &RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<
    (
        InstallCandidate,
        Option<runx_runtime::registry::AcquiredRegistrySkill>,
    ),
    RegistryCliError,
> {
    let source_authority = target.manifest_source_authority();
    match target {
        RegistryTarget::Remote { registry_url } => {
            let installation_id = remote_installation_id(env, cwd)?;
            let acquired = remote_registry_client(&registry_url, env)?.acquire(
                &plan.subject,
                AcquireOptions {
                    installation_id: &installation_id,
                    version: plan.version.as_deref(),
                    channel: Some("cli"),
                },
            )?;
            Ok((
                candidate_from_acquired(&plan.subject, &acquired, source_authority),
                Some(acquired),
            ))
        }
        RegistryTarget::Local {
            registry_path,
            registry_url,
            ..
        } => {
            let resolution = resolve_registry_skill(
                &FileRegistryStore::new(registry_path.clone()),
                &plan.subject,
                RegistryResolveOptions {
                    version: plan.version.clone(),
                    registry_url,
                },
            )?
            .ok_or_else(|| not_found_local(&plan.subject, &registry_path))?;
            Ok((
                candidate_from_resolution(&plan.subject, resolution, source_authority),
                None,
            ))
        }
    }
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

    let generated = InitGeneratedValues::generate();
    let home = runx_runtime::RunxPaths::resolve(env, cwd);
    let state = ensure_runx_install_state(&home, &generated.installation_id, &generated.created_at)
        .map_err(|error| usage_error(error.to_string()))?;
    Ok(state.state.installation_id)
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

fn resolve_path(
    path: &Path,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    prefer_existing: bool,
) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let paths = runx_runtime::RunxPaths::resolve(env, cwd);
    if prefer_existing {
        paths.existing_input(path)
    } else {
        paths.in_workspace(path)
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

fn not_found(registry_ref: &str) -> RegistryCliError {
    RegistryCliError {
        message: format!("registry skill not found: {registry_ref}"),
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

impl From<runx_runtime::registry::RegistryResolveError> for RegistryCliError {
    fn from(error: runx_runtime::registry::RegistryResolveError) -> Self {
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
