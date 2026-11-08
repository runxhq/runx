// Module rationale: native registry CLI keeps search/install/package/publish
// command wiring together, for skills and connectors alike, so the command
// matrix and output envelope stay auditable.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::connector::ConnectorName;
use runx_runtime::ConfigError;
use runx_runtime::WorkspaceEnv;
use runx_runtime::registry::resolve::{
    Installed, RegistryTarget, ResolveError, install_skill_ref, registry_client,
    resolve_registry_target, trusted_manifest_keys_for_source,
};
use runx_runtime::registry::{
    InstallLocalSkillOptions, InstallLocalSkillResult, RecordInstaller,
    RegistryPublishPackageRequest, RegistrySkillVersion, install_local_skill,
    materialize_publish_closure, prepare_registry_publish_package,
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
    /// The hosted registry `--registry` names by its URL, if one does.
    pub registry: Option<String>,
    /// The version to install, from `<ref>@<version>`.
    pub version: Option<String>,
    pub expected_digest: Option<String>,
    /// The namespace a publish goes into.
    pub publisher: Option<String>,
    pub limit: Option<usize>,
    /// A JSON array of signed version records to install in place of a ref:
    /// a file, or stdin.
    pub records: Option<crate::document_input::DocumentInputSource>,
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
    pub(crate) fn target(
        &self,
        env: &BTreeMap<String, String>,
    ) -> Result<RegistryTarget, RegistryCliError> {
        Ok(resolve_registry_target(self.registry.as_deref(), env)?)
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
        // A connector names itself; a skill's publisher names nothing here.
        if plan.publisher.is_some() {
            return Err(usage_error(
                "a connector names itself: package accepts only --json, publish only --registry and --json",
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
    let target = plan.target(env)?;
    match plan.action {
        RegistryAction::Search => run_search(plan, target, env),
        RegistryAction::Install if plan.records.is_some() => {
            run_record_install(plan, target, env, cwd)
        }
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
    let package_digest = package
        .package_digest()
        .map_err(|error| internal_error(error.to_string()))?;
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::Package {
                package: Box::new(package),
                content_digest,
                package_digest,
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
    let found = runx_runtime::registry::resolve::search(&query, plan.limit, &target, env)?;
    let human = output::render_search(&query, &found.source, &found.results, &found.connectors);
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
    let source = target.source_uri();
    let source_authority = target.manifest_source_authority();
    let registry_url = target.registry_url.clone();
    let paths = runx_runtime::RunxPaths::resolve(env, cwd);
    let options = InstallLocalSkillOptions {
        destination_root: paths.added_skills(),
        connectors_root: paths.added_connectors(),
        install_records: paths.install_records(),
        expected_digest: plan.expected_digest.clone(),
        trusted_manifest_keys: trusted_manifest_keys_for_source(env, source_authority)?,
    };
    let Some(Installed {
        package: (candidate, acquisition),
        installed: install,
        newest_unread,
    }) = install_skill_ref(
        &plan.subject,
        plan.version.as_deref(),
        target,
        env,
        cwd,
        |candidate| install_local_skill(candidate, &options),
    )?
    else {
        // No skill is published under the name; a connector may be.
        return match ConnectorName::new(&plan.subject) {
            Some(connector_ref) => {
                connector::install(&plan, &connector_ref, &registry_url, env, cwd)
            }
            None => Err(connector::not_found(&plan.requested_ref())),
        };
    };
    let install = InstallLocalSkillResult {
        newest_unread,
        ..install
    };
    let requested = plan.requested_ref();
    let human = output::render_install(
        &source,
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
                package: requested,
                install: Box::new(install),
                bundle_digest: candidate.bundle_digest.clone(),
                acquisition: acquisition.map(|acquired| output::InstallAcquisition {
                    publisher: acquired.publisher,
                    install_count: acquired.install_count,
                }),
            },
        },
        || human,
    )
}

/// Installs each signed version record a host already holds, one JSON array
/// read from a file or stdin as `--inputs` reads its document, as the
/// registry `target` names signed it, into the project: the records verify
/// and install exactly as a closure's records do when read from that
/// registry. The first that does not verify stops the install with its
/// typed error.
fn run_record_install(
    plan: RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let Some(document) = &plan.records else {
        return Err(usage_error(
            "runx registry install --records names a file or -",
        ));
    };
    let text = crate::document_input::read_document_input(document, env, cwd)
        .map_err(|error| usage_error(format!("runx registry install --records: {error}")))?;
    let records: Vec<RegistrySkillVersion> = serde_json::from_str(&text).map_err(|error| {
        usage_error(format!(
            "runx registry install --records is one JSON array of registry version records: {error}"
        ))
    })?;
    if records.is_empty() {
        return Err(usage_error(
            "runx registry install --records holds no record: the array is empty",
        ));
    }
    let installer =
        RecordInstaller::new(&target.registry_url, env, cwd).map_err(ResolveError::from)?;
    let source = target.source_uri();
    let mut installs = Vec::with_capacity(records.len());
    let mut human = String::new();
    for record in records {
        let package = format!("{}@{}", record.skill_id, record.version);
        let bundle_digest = record.bundle_digest.clone();
        let signed_manifest = record.signed_manifest.clone();
        let install = installer.install(record)?;
        human.push_str(&output::render_install(
            &source,
            &package,
            &install,
            signed_manifest.as_ref(),
        ));
        installs.push(output::RecordInstall {
            package,
            install: Box::new(install),
            bundle_digest,
        });
    }
    output::write_output(
        plan.json,
        &output::RegistryEnvelope {
            status: "success",
            registry: output::RegistryPayload::RecordInstall { source, installs },
        },
        || human,
    )
}

// Function rationale: a publish binds its closure, runs the package harness
// and sends the package, in that order, to the one hosted registry it targets.
fn run_publish(
    plan: RegistryPlan,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let request = RegistryPublishPackageRequest {
        subject: &plan.subject,
        env,
        cwd,
    };
    // A package naming skills other publishers own binds them from the
    // registry it is published to, installed into the publish's own project.
    let closure = materialize_publish_closure(
        &registry_client(&target.registry_url, env)?,
        &target.registry_url,
        request,
    )
    .map_err(|error| usage_error(error.to_string()))?;
    let env = &closure.env;
    let package =
        prepare_registry_publish_package(RegistryPublishPackageRequest { env, ..request })
            .map_err(|error| internal_error(error.to_string()))?;
    let orchestrator = crate::runtime::local_orchestrator(env).map_err(|error| {
        internal_error(format!("failed to initialize runtime effects: {error}"))
    })?;
    let harness = package
        .run_harness(&orchestrator, env)
        .map_err(|error| internal_error(error.to_string()))?;
    let result = remote_publish::publish_remote_skill_package(
        &target.registry_url,
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
                publish: Box::new(result),
            },
        },
        || "\n  registry publish success\n\n".to_owned(),
    )
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

impl From<runx_runtime::registry::RegistryClientError> for RegistryCliError {
    fn from(error: runx_runtime::registry::RegistryClientError) -> Self {
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
    use runx_runtime::registry::{InstallStatus, TrustTier, UnreadVersion};

    #[test]
    fn registry_install_render_shows_direct_skill_run_command() {
        let rendered = output::render_install(
            "remote:https://api.runx.ai",
            "acme/echo@1.2.3",
            &InstallLocalSkillResult {
                status: InstallStatus::Installed,
                destination: PathBuf::from("/tmp/runx/skills/acme/echo/SKILL.md"),
                skill_name: "echo".to_owned(),
                source: "runx-registry".to_owned(),
                source_label: "runx registry".to_owned(),
                skill_id: Some("acme/echo".to_owned()),
                version: Some("1.2.3".to_owned()),
                digest: "sha256:abc".to_owned(),
                profile_digest: None,
                runner_names: Vec::new(),
                trust_tier: Some(TrustTier::Community),
                newest_unread: None,
            },
            None,
        );

        assert!(rendered.contains("next             runx skill acme/echo@1.2.3"));
        assert!(!rendered.contains("newest"));
    }

    #[test]
    fn registry_install_render_names_the_newest_version_it_does_not_read() {
        let rendered = output::render_install(
            "remote:https://api.runx.ai",
            "acme/echo",
            &InstallLocalSkillResult {
                status: InstallStatus::Installed,
                destination: PathBuf::from("/tmp/runx/skills/acme/echo/SKILL.md"),
                skill_name: "echo".to_owned(),
                source: "runx-registry".to_owned(),
                source_label: "runx registry".to_owned(),
                skill_id: Some("acme/echo".to_owned()),
                version: Some("1.2.3".to_owned()),
                digest: "sha256:abc".to_owned(),
                profile_digest: None,
                runner_names: Vec::new(),
                trust_tier: Some(TrustTier::Community),
                newest_unread: Some(UnreadVersion {
                    version: "2.0.0".to_owned(),
                    reason: "run.yaml: runners.echo.colour is not supported".to_owned(),
                }),
            },
            None,
        );

        assert!(rendered.contains(
            "newest           acme/echo@2.0.0 is not read by this runx: run.yaml: runners.echo.colour is not supported"
        ));
    }

    #[test]
    fn registry_search_render_installs_a_connector_by_its_ref() -> Result<(), String> {
        let rendered = output::render_search(
            "github",
            "remote:https://api.runx.ai",
            &[],
            &[runx_runtime::registry::RegistryConnectorSearchResult {
                connector: runx_contracts::connector::ConnectorName::new("github")
                    .ok_or("github is a connector name")?,
                version: "sha-f92dd1307b53".to_owned(),
                name: "GitHub".to_owned(),
                description: "Read and update GitHub repositories.".to_owned(),
                runtime_capability: false,
                trust_tier: TrustTier::Verified,
            }],
        );

        assert!(rendered.contains("  - github@sha-f92dd1307b53  connector\n    GitHub\n"));
        assert!(rendered.contains("install  runx registry install github@sha-f92dd1307b53"));
        Ok(())
    }
}
