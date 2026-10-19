// Module rationale: native registry CLI keeps local and hosted
// search/install/package/publish command wiring together, for skills and
// connectors alike, so the command matrix and output envelope stay auditable.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::connector::ConnectorName;
use runx_runtime::ConfigError;
use runx_runtime::WorkspaceEnv;
use runx_runtime::registry::resolve::{
    RegistryTarget, ResolveError, install_candidate, registry_client, resolve_registry_target,
    trusted_manifest_keys_for_source,
};
use runx_runtime::registry::{
    FileRegistryStore, IngestSkillOptions, InstallLocalSkillOptions, PublishSkillMarkdownOptions,
    RegistryPublishPackageRequest, TrustTier, install_local_skill, materialize_publish_closure,
    prepare_registry_publish_package, publish_skill_markdown,
};

mod connector;
mod output;
mod remote_publish;

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

    /// The registry the plan targets.
    pub(crate) fn target(&self, env: &BTreeMap<String, String>, cwd: &Path) -> RegistryTarget {
        resolve_registry_target(self.registry.as_deref(), env, cwd)
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
    let target = plan.target(env, cwd);
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
    let query = plan.subject;
    let found = runx_runtime::registry::resolve::search(&query, plan.limit, target, env)?;
    let human = output::render_search(&query, found.source, &found.results, &found.connectors);
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::Search {
                source: found.source,
                query: query.clone(),
                results: found.results,
                connectors: found.connectors,
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
    let Some((candidate, acquisition)) =
        install_candidate(&plan.subject, plan.version.as_deref(), target, env, cwd)?
    else {
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
            destination_root: runx_runtime::RunxPaths::resolve(env, cwd).added_skills(),
            connectors_root: runx_runtime::RunxPaths::resolve(env, cwd).added_connectors(),
            install_records: runx_runtime::RunxPaths::resolve(env, cwd).install_records(),
            expected_digest: plan.expected_digest.clone(),
            trusted_manifest_keys: trusted_manifest_keys_for_source(env, source_authority)?,
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
        RegistryTarget::Remote { registry_url } => {
            materialize_publish_closure(&registry_client(registry_url, env)?, registry_url, request)
                .map_err(|error| usage_error(error.to_string()))?
        }
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
        ResolveError::from(error).into()
    }
}

impl From<ResolveError> for RegistryCliError {
    fn from(error: ResolveError) -> Self {
        match error {
            ResolveError::Usage(message) => usage_error(message),
            ResolveError::Failed(message) => internal_error(message),
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
