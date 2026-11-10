// Module rationale: doctor aggregates path, registry, and authority diagnostics until those surfaces split.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::{
    DoctorDiagnostic, DoctorDiagnosticSeverity, DoctorLocation, DoctorRepair,
    DoctorRepairConfidence, DoctorRepairKind, DoctorRepairRisk, DoctorReport, DoctorReportSchema,
    DoctorStatus, DoctorSummary, JsonObject, JsonValue,
};
use runx_runtime::{
    HOSTED_API_TOKEN_ENV, HostedApiEnvironment, RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV,
    RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV, RUNX_RECEIPT_SIGN_KID_ENV,
    RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV,
    RUNX_RECEIPT_VERIFY_KID_ENV, ReceiptTrust, ReceiptTrustSource, RuntimeError, RunxPaths,
    WorkspaceEnv, default_doctor_options, load_managed_agent_config, load_runx_config_file,
    receipt_signer, receipt_trust, run_doctor,
};

use crate::router::{DoctorMode, DoctorPlan};
use runx_runtime::registry::resolve::{RegistryTarget, resolve_registry_target};

pub fn run_native_doctor(mut plan: DoctorPlan, workspace: &WorkspaceEnv) -> ExitCode {
    // The path checked is named from where the operator typed it.
    if let Some(invoked) = plan
        .path
        .as_deref()
        .and_then(|path| workspace.invoked_path(path))
    {
        plan.path = Some(invoked);
    }
    match run_doctor_command(&plan, workspace.env(), workspace.cwd()) {
        Ok(output) => crate::cli_io::write_stdout_code(&output.stdout, output.exit_code),
        Err(error) => crate::cli_io::write_stderr_code(&format!("runx: {error}\n"), 1),
    }
}

struct DoctorCliOutput {
    stdout: String,
    exit_code: u8,
}

fn run_doctor_command(
    plan: &DoctorPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<DoctorCliOutput, DoctorCliError> {
    if plan.mode == DoctorMode::Authority || plan.mode == DoctorMode::Registry {
        let report = if plan.mode == DoctorMode::Authority {
            run_authority_doctor(env, cwd)
        } else {
            run_registry_doctor(env, cwd)
        };
        let stdout = if plan.json {
            json_line(&report)?
        } else {
            render_doctor_report(&report)
        };
        return Ok(DoctorCliOutput {
            stdout,
            exit_code: 0,
        });
    }

    let root = resolve_doctor_root(plan, env, cwd);
    let mut report = run_doctor(&root, &default_doctor_options())?;
    if let Some(diagnostic) = managed_agent_config_diagnostic(env, cwd) {
        report.diagnostics.push(diagnostic);
    }
    report.summary = summary(&report.diagnostics);
    if report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == DoctorDiagnosticSeverity::Error)
    {
        report.status = DoctorStatus::Failure;
    }
    let exit_code = match report.status {
        DoctorStatus::Success => 0,
        DoctorStatus::Failure => 1,
    };
    let stdout = if plan.json {
        json_line(&report)?
    } else {
        render_doctor_report(&report)
    };
    Ok(DoctorCliOutput { stdout, exit_code })
}

// Function rationale: this builds one structured diagnostic packet
// from env, config, and credential state so the evidence and repair stay together.
fn managed_agent_config_diagnostic(
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Option<DoctorDiagnostic> {
    let config_path = RunxPaths::resolve(env, cwd).config_file();
    let config = load_runx_config_file(&config_path);
    let mut evidence = JsonObject::new();
    evidence.insert(
        "config_path".to_owned(),
        JsonValue::String(config_path.display().to_string()),
    );

    let (
        config_provider,
        config_model,
        config_key_ref,
        config_endpoint,
        config_auth_mode,
        config_error,
    ) = match config {
        Ok(config) => (
            config
                .agent
                .as_ref()
                .and_then(|agent| agent.provider.as_deref())
                .map(str::to_owned),
            config
                .agent
                .as_ref()
                .and_then(|agent| agent.model.as_deref())
                .map(str::to_owned),
            // The secret the agent names as its key.
            config
                .agent
                .as_ref()
                .and_then(|agent| agent.secret.as_ref())
                .map(|secret| secret.as_str().to_owned()),
            config
                .agent
                .as_ref()
                .and_then(|agent| agent.endpoint_url.clone()),
            config
                .agent
                .as_ref()
                .and_then(|agent| agent.auth_mode.clone()),
            None,
        ),
        Err(error) => (None, None, None, None, None, Some(error.to_string())),
    };

    let provider = first_non_empty([
        env.get("RUNX_AGENT_PROVIDER").map(String::as_str),
        config_provider.as_deref(),
    ]);
    let model = first_non_empty([
        env.get("RUNX_AGENT_MODEL").map(String::as_str),
        config_model.as_deref(),
    ]);
    let endpoint = first_non_empty([
        env.get("RUNX_AGENT_ENDPOINT_URL").map(String::as_str),
        config_endpoint.as_deref(),
    ]);
    let auth_mode = first_non_empty([
        env.get("RUNX_AGENT_AUTH_MODE").map(String::as_str),
        config_auth_mode.as_deref(),
    ]);
    let provider_key_env = provider.and_then(provider_api_key_env);
    let api_key_configured = env_contains_non_empty(env, "RUNX_AGENT_API_KEY")
        || provider_key_env.is_some_and(|name| env_contains_non_empty(env, name))
        || config_key_ref
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());

    evidence.insert(
        "provider_set".to_owned(),
        JsonValue::Bool(provider.is_some()),
    );
    evidence.insert("model_set".to_owned(), JsonValue::Bool(model.is_some()));
    evidence.insert(
        "endpoint_set".to_owned(),
        JsonValue::Bool(endpoint.is_some()),
    );
    if let Some(auth_mode) = auth_mode {
        evidence.insert(
            "auth_mode".to_owned(),
            JsonValue::String(
                if matches!(auth_mode, "api_key" | "local_none") {
                    auth_mode
                } else {
                    "[invalid]"
                }
                .to_owned(),
            ),
        );
    }
    evidence.insert(
        "api_key_set".to_owned(),
        JsonValue::Bool(api_key_configured),
    );
    if let Some(provider) = provider {
        evidence.insert(
            "provider".to_owned(),
            JsonValue::String(provider.to_owned()),
        );
    }
    if let Some(model) = model {
        evidence.insert("model".to_owned(), JsonValue::String(model.to_owned()));
    }
    if let Some(name) = provider_key_env {
        evidence.insert(
            "provider_api_key_env".to_owned(),
            JsonValue::String(name.to_owned()),
        );
    }
    if let Some(error) = config_error.as_ref() {
        evidence.insert("config_error".to_owned(), JsonValue::String(error.clone()));
    }

    let semantic_result = (config_error.is_none()).then(|| load_managed_agent_config(env, cwd));
    let semantic_error = semantic_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .map(ToString::to_string);
    // Two places hold different model keys: the config is fine, and the
    // repair is keeping one.
    let conflicting_keys = semantic_result.as_ref().is_some_and(|result| {
        matches!(
            result,
            Err(runx_runtime::ConfigError::ConflictingAgentKeys { .. })
        )
    });
    if let Some(error) = semantic_error.as_ref() {
        evidence.insert(
            "semantic_error".to_owned(),
            JsonValue::String(error.clone()),
        );
    }
    let complete = semantic_result
        .as_ref()
        .is_some_and(|result| matches!(result, Ok(Some(_))));
    let partial = !complete
        && (provider.is_some()
            || model.is_some()
            || api_key_configured
            || endpoint.is_some()
            || auth_mode.is_some()
            || config_error.is_some()
            || semantic_error.is_some());
    if !complete && !partial {
        return None;
    }
    let severity = if partial {
        DoctorDiagnosticSeverity::Warning
    } else {
        DoctorDiagnosticSeverity::Info
    };
    let message = if let Some(error) = config_error {
        format!("Managed-agent config could not be read: {error}.")
    } else if let Some(error) = semantic_error {
        format!("Managed-agent config is invalid: {error}.")
    } else if complete {
        "Managed-agent config is complete; agent-task methods can execute in-process.".to_owned()
    } else {
        "Managed-agent config is partial; set provider, model, and API key (a provider without a built-in profile also needs a dialect and an endpoint), or select local_none with an exact loopback endpoint. Otherwise agent-task methods may yield to the host or fail later.".to_owned()
    };

    Some(DoctorDiagnostic {
        id: "runx.agent.config".to_owned(),
        instance_id: "runx:doctor:runx.agent.config".to_owned(),
        severity,
        title: "Managed-agent config".to_owned(),
        message,
        target: object([
            ("kind", string_value("config")),
            ("id", string_value("runx.agent.config")),
        ]),
        location: DoctorLocation {
            path: "runx config".to_owned(),
            json_pointer: Some("/agent".to_owned()),
        },
        evidence: Some(evidence),
        repairs: if conflicting_keys {
            vec![DoctorRepair {
                id: "runx.agent.config.one_key".to_owned(),
                kind: DoctorRepairKind::Manual,
                confidence: DoctorRepairConfidence::High,
                risk: DoctorRepairRisk::Low,
                path: Some("runx config".to_owned()),
                json_pointer: Some("/agent".to_owned()),
                contents: Some(
                    "Keep one model key: unset the variable runx should not read, or set the secret agent.secret names to the key you mean with runx secret set <name>.".to_owned(),
                ),
                patch: None,
                command: None,
                requires_human_review: false,
            }]
        } else if partial {
            vec![DoctorRepair {
                id: "runx.agent.config.configure".to_owned(),
                kind: DoctorRepairKind::Manual,
                confidence: DoctorRepairConfidence::High,
                risk: DoctorRepairRisk::Low,
                path: Some("runx config".to_owned()),
                json_pointer: Some("/agent".to_owned()),
                contents: Some(
                    "Set agent.provider, agent.model, and agent.secret, the secret holding the key; or set agent.auth_mode local_none and an exact loopback agent.endpoint_url. Clear a local endpoint and its dialect with runx config unset agent.endpoint_url and runx config unset agent.dialect before switching to a built-in provider. Correct other invalid settings through runx config set."
                        .to_owned(),
                ),
                patch: None,
                command: Some(
                    "runx config unset agent.endpoint_url && runx config unset agent.dialect && runx config set agent.auth_mode api_key && runx config set agent.provider anthropic && runx config set agent.model <model> && runx secret set anthropic && runx config set agent.secret anthropic".to_owned(),
                ),
                requires_human_review: false,
            }]
        } else {
            Vec::new()
        },
    })
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    values
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn provider_api_key_env(provider: &str) -> Option<&'static str> {
    runx_runtime::model_provider(provider.trim().to_ascii_lowercase().as_str())
        .map(|profile| profile.key_env)
}

fn run_registry_doctor(env: &BTreeMap<String, String>, cwd: &Path) -> DoctorReport {
    let paths = RunxPaths::resolve(env, cwd);
    let mut diagnostics = match resolve_registry_target(None, env) {
        Ok(target) => vec![
            registry_target_diagnostic(&target),
            registry_trust_key_diagnostic(env, &paths),
            registry_remote_install_diagnostic(env),
        ],
        Err(error) => vec![registry_target_error_diagnostic(&error.into_message())],
    };
    diagnostics.extend([
        path_diagnostic(
            "runx.project.skills",
            "Skills directory",
            paths.skills(),
            &["RUNX_CWD"],
        ),
        path_diagnostic(
            "runx.project.added_skills",
            "Added skills directory",
            paths.added_skills(),
            &["RUNX_PROJECT_DIR", "RUNX_CWD"],
        ),
    ]);
    DoctorReport {
        schema: DoctorReportSchema::V1,
        status: DoctorStatus::Success,
        summary: summary(&diagnostics),
        diagnostics,
    }
}

fn registry_target_diagnostic(target: &RegistryTarget) -> DoctorDiagnostic {
    let mut evidence = JsonObject::new();
    evidence.insert(
        "description".to_owned(),
        JsonValue::String(target.description()),
    );
    evidence.insert(
        "source_uri".to_owned(),
        JsonValue::String(target.source_uri()),
    );
    DoctorDiagnostic {
        id: "runx.registry.target".to_owned(),
        instance_id: "runx:doctor-registry:runx.registry.target".to_owned(),
        severity: DoctorDiagnosticSeverity::Info,
        title: "Registry target".to_owned(),
        message: format!("Registry target selected: {}.", target.description()),
        target: object([
            ("kind", string_value("registry")),
            ("id", string_value("runx.registry.target")),
        ]),
        location: DoctorLocation {
            path: "environment".to_owned(),
            json_pointer: None,
        },
        evidence: Some(evidence),
        repairs: Vec::new(),
    }
}

/// The registry the environment names is not one: a registry is a URL.
fn registry_target_error_diagnostic(message: &str) -> DoctorDiagnostic {
    DoctorDiagnostic {
        id: "runx.registry.target".to_owned(),
        instance_id: "runx:doctor-registry:runx.registry.target".to_owned(),
        severity: DoctorDiagnosticSeverity::Error,
        title: "Registry target".to_owned(),
        message: format!("{message}."),
        target: object([
            ("kind", string_value("registry")),
            ("id", string_value("runx.registry.target")),
        ]),
        location: DoctorLocation {
            path: "environment".to_owned(),
            json_pointer: None,
        },
        evidence: None,
        repairs: vec![manual_env_repair(
            "runx.registry.target.configure_env",
            &["RUNX_REGISTRY_URL"],
            "Set RUNX_REGISTRY_URL to a hosted registry's http(s) URL, or unset it for the runx catalog.",
            DoctorRepairRisk::Low,
        )],
    }
}

fn path_diagnostic(id: &str, title: &str, path: PathBuf, env_names: &[&str]) -> DoctorDiagnostic {
    let mut evidence = JsonObject::new();
    evidence.insert(
        "path".to_owned(),
        JsonValue::String(path.display().to_string()),
    );
    evidence.insert(
        "env_vars".to_owned(),
        JsonValue::Array(
            env_names
                .iter()
                .map(|name| JsonValue::String((*name).to_owned()))
                .collect(),
        ),
    );
    DoctorDiagnostic {
        id: id.to_owned(),
        instance_id: format!("runx:doctor-registry:{id}"),
        severity: DoctorDiagnosticSeverity::Info,
        title: title.to_owned(),
        message: format!("{title} resolves to {}.", path.display()),
        target: object([("kind", string_value("registry")), ("id", string_value(id))]),
        location: DoctorLocation {
            path: "environment".to_owned(),
            json_pointer: None,
        },
        evidence: Some(evidence),
        repairs: Vec::new(),
    }
}

// Function rationale: one diagnostic assembles the registry key set and its
// repair hint. An operator key signs only its publisher, from its registry.
fn registry_trust_key_diagnostic(
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
) -> DoctorDiagnostic {
    let keys_env = runx_runtime::registry::RUNX_REGISTRY_KEYS_ENV;
    let keys_file = paths.registry_keys_file();
    let (keys, refusal) = match runx_runtime::registry::registry_keys(env, paths) {
        Ok(keys) => (keys, None),
        Err(error) => (
            runx_runtime::registry::official_registry_keys(),
            Some(error.to_string()),
        ),
    };
    let operator_keys = keys
        .iter()
        .filter(|key| {
            !matches!(
                key.scope,
                runx_runtime::registry::RegistryKeyScope::Official
            )
        })
        .count();
    let mut evidence = JsonObject::new();
    evidence.insert(
        "key_ids".to_owned(),
        JsonValue::Array(
            keys.iter()
                .map(|key| JsonValue::String(key.key_id.clone()))
                .collect(),
        ),
    );
    evidence.insert(
        "trust_policy".to_owned(),
        JsonValue::Array(keys.iter().map(registry_trust_policy_evidence).collect()),
    );
    evidence.insert(
        "operator_keys".to_owned(),
        JsonValue::Number(runx_contracts::JsonNumber::U64(
            u64::try_from(operator_keys).unwrap_or(u64::MAX),
        )),
    );
    evidence.insert(
        "invalid_operator_keys".to_owned(),
        JsonValue::Bool(refusal.is_some()),
    );
    evidence.insert(
        "env_vars".to_owned(),
        JsonValue::Array(
            [keys_env, "RUNX_REGISTRY_URL"]
                .into_iter()
                .map(|name| JsonValue::String(name.to_owned()))
                .collect(),
        ),
    );
    evidence.insert(
        "keys_file".to_owned(),
        JsonValue::String(keys_file.display().to_string()),
    );
    DoctorDiagnostic {
        id: "runx.registry.trust_keys".to_owned(),
        instance_id: "runx:doctor-registry:runx.registry.trust_keys".to_owned(),
        severity: if refusal.is_some() {
            DoctorDiagnosticSeverity::Warning
        } else {
            DoctorDiagnosticSeverity::Info
        },
        title: "Registry trust keys".to_owned(),
        message: match &refusal {
            Some(refusal) => {
                format!("Registry keys are refused, so only the built-in keys verify: {refusal}")
            }
            None if operator_keys > 0 => {
                format!("Registry keys: the built-in keys and {operator_keys} operator key(s).")
            }
            None => format!(
                "Using built-in registry trust keys. Add an operator key as {{ key_id, public_key, publisher, source }} in {keys_env} or {}.",
                keys_file.display()
            ),
        },
        target: object([
            ("kind", string_value("registry")),
            ("id", string_value("runx.registry.trust_keys")),
        ]),
        location: DoctorLocation {
            path: "environment".to_owned(),
            json_pointer: None,
        },
        evidence: Some(evidence),
        repairs: if refusal.is_some() {
            vec![manual_env_repair(
                "runx.registry.trust_keys.configure_env",
                &[keys_env],
                "Give each operator key its key_id, base64 ed25519 public_key, publisher namespace and the registry URL it signs from; the official namespaces runx and project are never an operator key's.",
                DoctorRepairRisk::Sensitive,
            )]
        } else {
            Vec::new()
        },
    }
}

fn registry_trust_policy_evidence(key: &runx_runtime::registry::RegistryKey) -> JsonValue {
    // The official keys sign any publisher from the official registry and
    // state first_party for runx/* skills and for connectors whose tier
    // statement names publisher runx; an operator key signs its publisher
    // from its registry and never states first_party.
    let (scope, allowed_namespace, allowed_source, first_party_namespace) = match &key.scope {
        runx_runtime::registry::RegistryKeyScope::Official => (
            "official_runx",
            "*".to_owned(),
            "official_runx".to_owned(),
            Some("runx/*"),
        ),
        runx_runtime::registry::RegistryKeyScope::ThirdParty { publisher, source } => (
            "third_party",
            format!("{publisher}/*"),
            runx_runtime::registry::registry_manifest_source_key(source),
            None,
        ),
    };
    JsonValue::Object(object([
        ("key_id", JsonValue::String(key.key_id.clone())),
        ("scope", string_value(scope)),
        ("allowed_namespace", JsonValue::String(allowed_namespace)),
        ("allowed_source", JsonValue::String(allowed_source)),
        (
            "can_grant_first_party",
            JsonValue::Bool(first_party_namespace.is_some()),
        ),
        (
            "first_party_namespace",
            first_party_namespace.map_or(JsonValue::Null, string_value),
        ),
        // A connector's name doesn't say who published it, so its first
        // party is the publisher its signed tier statement names.
        (
            "first_party_connector_publisher",
            first_party_namespace
                .map(|_| "runx")
                .map_or(JsonValue::Null, string_value),
        ),
    ]))
}

fn registry_remote_install_diagnostic(env: &BTreeMap<String, String>) -> DoctorDiagnostic {
    let configured = env_contains_non_empty(env, "RUNX_INSTALLATION_ID");
    let severity = DoctorDiagnosticSeverity::Info;
    let message = if configured {
        "Remote registry install identity configured.".to_owned()
    } else {
        "Remote registry installs will use the local runx install identity; set RUNX_INSTALLATION_ID only when an explicit shared identity is needed.".to_owned()
    };
    let mut evidence = JsonObject::new();
    evidence.insert("configured".to_owned(), JsonValue::Bool(configured));
    evidence.insert(
        "env_vars".to_owned(),
        JsonValue::Array(vec![JsonValue::String("RUNX_INSTALLATION_ID".to_owned())]),
    );
    DoctorDiagnostic {
        id: "runx.registry.installation_id".to_owned(),
        instance_id: "runx:doctor-registry:runx.registry.installation_id".to_owned(),
        severity,
        title: "Registry install identity".to_owned(),
        message,
        target: object([
            ("kind", string_value("registry")),
            ("id", string_value("runx.registry.installation_id")),
        ]),
        location: DoctorLocation {
            path: "environment".to_owned(),
            json_pointer: None,
        },
        evidence: Some(evidence),
        repairs: Vec::new(),
    }
}

fn run_authority_doctor(env: &BTreeMap<String, String>, cwd: &Path) -> DoctorReport {
    let paths = RunxPaths::resolve(env, cwd);
    let diagnostics = vec![
        receipt_signer_diagnostic(env, &paths),
        receipt_verification_diagnostic(env, &paths),
        provider_grant_diagnostic(env, cwd),
    ];
    DoctorReport {
        schema: DoctorReportSchema::V1,
        status: DoctorStatus::Success,
        summary: summary(&diagnostics),
        diagnostics,
    }
}

/// The identity runs seal receipts under: a configured signer, else this
/// home's own key. Reading it never creates a key.
fn receipt_signer_diagnostic(
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
) -> DoctorDiagnostic {
    const ENV_NAMES: &[&str] = &[
        RUNX_RECEIPT_SIGN_KID_ENV,
        RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV,
        RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV,
    ];

    let resolution = receipt_signer(env, paths);
    let ready = resolution.is_ok();
    let message = match &resolution {
        Ok(Some(source)) => format!("Receipts are signed by {}.", trusted_key_label(source)),
        Ok(None) => format!(
            "Receipts will be signed by this home's local key, created at {} on the first run.",
            paths.local_receipt_key().display()
        ),
        Err(error) => format!("Receipt signer not ready: {error}."),
    };
    let mut evidence = authority_evidence(ENV_NAMES, ready, None);
    if let Ok(Some(source)) = &resolution {
        evidence.insert("signer".to_owned(), trusted_key_evidence(source));
    }

    DoctorDiagnostic {
        id: "runx.authority.signer".to_owned(),
        instance_id: "runx:doctor-authority:runx.authority.signer".to_owned(),
        severity: if ready {
            DoctorDiagnosticSeverity::Info
        } else {
            DoctorDiagnosticSeverity::Warning
        },
        title: "Receipt signer".to_owned(),
        message,
        target: object([
            ("kind", string_value("authority")),
            ("id", string_value("runx.authority.signer")),
        ]),
        location: DoctorLocation {
            path: "environment".to_owned(),
            json_pointer: None,
        },
        evidence: Some(evidence),
        repairs: if ready {
            Vec::new()
        } else {
            vec![manual_env_repair(
                "runx.authority.signer.configure_env",
                ENV_NAMES,
                "Configure one complete valid receipt signing identity, or unset it to sign with this home's local key.",
                DoctorRepairRisk::Sensitive,
            )]
        },
    }
}

/// The keys `runx verify` trusts here: this home's own key and every
/// configured key.
fn receipt_verification_diagnostic(
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
) -> DoctorDiagnostic {
    const VERIFY_ENV: &[&str] = &[
        RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV,
        RUNX_RECEIPT_VERIFY_KID_ENV,
        RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV,
    ];

    let resolution = receipt_trust(env, paths, ReceiptTrust::HomeAndConfigured);
    let configured = matches!(&resolution, Ok(sources) if !sources.is_empty());
    let message = match &resolution {
        Ok(sources) if sources.is_empty() => "No receipt key is trusted yet: this home has no local key and no verifier is configured.".to_owned(),
        Ok(sources) => format!(
            "Receipts verify against {}.",
            sources
                .iter()
                .map(trusted_key_label)
                .collect::<Vec<_>>()
                .join("; ")
        ),
        Err(error) => format!("Receipt verification configuration is invalid: {error}."),
    };
    let mut evidence = authority_evidence(VERIFY_ENV, configured, None);
    if let Ok(sources) = &resolution {
        evidence.insert(
            "trusted_keys".to_owned(),
            JsonValue::Array(sources.iter().map(trusted_key_evidence).collect()),
        );
    }

    DoctorDiagnostic {
        id: "runx.authority.verify_key".to_owned(),
        instance_id: "runx:doctor-authority:runx.authority.verify_key".to_owned(),
        severity: if configured {
            DoctorDiagnosticSeverity::Info
        } else {
            DoctorDiagnosticSeverity::Warning
        },
        title: "Receipt verification key".to_owned(),
        message,
        target: object([
            ("kind", string_value("authority")),
            ("id", string_value("runx.authority.verify_key")),
        ]),
        location: DoctorLocation {
            path: "environment".to_owned(),
            json_pointer: None,
        },
        evidence: Some(evidence),
        repairs: if configured {
            Vec::new()
        } else {
            vec![manual_env_repair(
                "runx.authority.verify_key.configure_env",
                VERIFY_ENV,
                "Run once to create this home's local key, or configure the public key of the issuer whose receipts this operator verifies.",
                DoctorRepairRisk::Sensitive,
            )]
        },
    }
}

fn trusted_key_label(source: &ReceiptTrustSource) -> String {
    let origin = match source.origin {
        runx_runtime::ReceiptTrustOrigin::ConfiguredVerifier => "the configured verifier",
        runx_runtime::ReceiptTrustOrigin::ConfiguredSigner => "the configured signer",
        runx_runtime::ReceiptTrustOrigin::Home => "this home's local key",
    };
    format!(
        "{origin} ({} key {})",
        source.key.issuer_type().label(),
        source.key.kid()
    )
}

fn trusted_key_evidence(source: &ReceiptTrustSource) -> JsonValue {
    JsonValue::Object(object([
        ("origin", string_value(source.origin.label())),
        (
            "issuer_type",
            string_value(source.key.issuer_type().label()),
        ),
        ("kid", string_value(source.key.kid())),
    ]))
}

fn provider_grant_diagnostic(env: &BTreeMap<String, String>, cwd: &Path) -> DoctorDiagnostic {
    // A hosted operation runs under the grant its connector instance was
    // confirmed with; runx Connect is where that grant is read.
    let configured = HostedApiEnvironment::resolve(None, env, cwd)
        .and_then(|environment| environment.require_token().map(|_| ()))
        .is_ok();
    let mut evidence = authority_evidence(&[HOSTED_API_TOKEN_ENV], configured, None);
    evidence.insert("connect_discovery".to_owned(), JsonValue::Bool(configured));
    DoctorDiagnostic {
        id: "runx.authority.provider_grant".to_owned(),
        instance_id: "runx:doctor-authority:runx.authority.provider_grant".to_owned(),
        severity: if configured {
            DoctorDiagnosticSeverity::Info
        } else {
            DoctorDiagnosticSeverity::Warning
        },
        title: "Provider permission grant".to_owned(),
        message: if configured {
            "Runx Connect is configured; a hosted operation runs under the grant its connector instance was confirmed with.".to_owned()
        } else {
            "Runx Connect is not configured; a hosted operation needs `runx login`, then a grant confirmed with `runx connector use`.".to_owned()
        },
        target: object([
            ("kind", string_value("authority")),
            ("id", string_value("runx.authority.provider_grant")),
        ]),
        location: DoctorLocation {
            path: "environment".to_owned(),
            json_pointer: None,
        },
        evidence: Some(evidence),
        repairs: if configured {
            Vec::new()
        } else {
            vec![manual_env_repair(
                "runx.authority.provider_grant.configure",
                &[HOSTED_API_TOKEN_ENV],
                "Run `runx login` for runx Connect, then set the connector up with `runx connector use <connector> --hosted`.",
                DoctorRepairRisk::Sensitive,
            )]
        },
    }
}

fn manual_env_repair(
    id: &str,
    env_names: &[&str],
    contents: &str,
    risk: DoctorRepairRisk,
) -> DoctorRepair {
    DoctorRepair {
        id: id.to_owned(),
        kind: DoctorRepairKind::Manual,
        confidence: DoctorRepairConfidence::High,
        risk,
        path: Some("environment".to_owned()),
        json_pointer: None,
        contents: Some(format!(
            "{contents} Required env vars: {}.",
            env_names.join(", ")
        )),
        patch: None,
        command: None,
        requires_human_review: true,
    }
}

fn authority_evidence(
    env_names: &[&str],
    configured: bool,
    key_id_env: Option<&str>,
) -> JsonObject {
    let mut evidence = JsonObject::new();
    evidence.insert(
        "env_vars".to_owned(),
        JsonValue::Array(
            env_names
                .iter()
                .map(|name| JsonValue::String((*name).to_owned()))
                .collect(),
        ),
    );
    evidence.insert("configured".to_owned(), JsonValue::Bool(configured));
    if let Some(name) = key_id_env {
        evidence.insert("key_id_env".to_owned(), JsonValue::String(name.to_owned()));
    }
    evidence
}

fn env_contains_non_empty(env: &BTreeMap<String, String>, name: &str) -> bool {
    env.get(name)
        .map(String::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn object(entries: impl IntoIterator<Item = (&'static str, JsonValue)>) -> JsonObject {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn string_value(value: &str) -> JsonValue {
    JsonValue::String(value.to_owned())
}

fn summary(diagnostics: &[DoctorDiagnostic]) -> DoctorSummary {
    let mut errors = 0;
    let mut warnings = 0;
    let mut infos = 0;
    for diagnostic in diagnostics {
        match diagnostic.severity {
            DoctorDiagnosticSeverity::Error => errors += 1,
            DoctorDiagnosticSeverity::Warning => warnings += 1,
            DoctorDiagnosticSeverity::Info => infos += 1,
        }
    }
    DoctorSummary {
        errors,
        warnings,
        infos,
    }
}

fn resolve_doctor_root(plan: &DoctorPlan, env: &BTreeMap<String, String>, cwd: &Path) -> PathBuf {
    let paths = RunxPaths::resolve(env, cwd);
    plan.path.as_deref().map_or_else(
        || paths.workspace().to_path_buf(),
        |path| paths.existing_input(path),
    )
}

fn json_line<T: serde::Serialize>(value: &T) -> Result<String, DoctorCliError> {
    serde_json::to_string_pretty(value)
        .map(|json| format!("{json}\n"))
        .map_err(DoctorCliError::Serialize)
}

fn render_doctor_report(report: &DoctorReport) -> String {
    let mut lines = vec![
        String::new(),
        format!(
            "  {}  doctor  {} error(s), {} warning(s)",
            status_icon(&report.status),
            report.summary.errors,
            report.summary.warnings
        ),
    ];
    for diagnostic in &report.diagnostics {
        lines.push(format!(
            "  {}  {}  {}",
            diagnostic_icon(&diagnostic.severity),
            diagnostic.id,
            diagnostic.location.path
        ));
        lines.push(format!("     {}", diagnostic.message));
        if let Some(repair) = diagnostic.repairs.first() {
            if let Some(command) = repair.command.as_ref() {
                lines.push(format!("     next: {command}"));
            } else if let Some(contents) = repair.contents.as_ref() {
                lines.push(format!("     next: {contents}"));
            }
        }
    }
    lines.push(String::new());
    lines.join("\n")
}

fn status_icon(status: &DoctorStatus) -> &'static str {
    match status {
        DoctorStatus::Success => "✓",
        DoctorStatus::Failure => "✗",
    }
}

fn diagnostic_icon(severity: &DoctorDiagnosticSeverity) -> &'static str {
    match severity {
        DoctorDiagnosticSeverity::Error => "✗",
        DoctorDiagnosticSeverity::Warning | DoctorDiagnosticSeverity::Info => "·",
    }
}

#[derive(Debug)]
enum DoctorCliError {
    Runtime(RuntimeError),
    Serialize(serde_json::Error),
}

impl std::fmt::Display for DoctorCliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(error) => write!(formatter, "{error}"),
            Self::Serialize(error) => {
                write!(formatter, "failed to serialize doctor report: {error}")
            }
        }
    }
}

impl From<RuntimeError> for DoctorCliError {
    fn from(value: RuntimeError) -> Self {
        Self::Runtime(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_render_surfaces_first_repair_next_action() {
        let report = DoctorReport {
            schema: DoctorReportSchema::V1,
            status: DoctorStatus::Success,
            summary: DoctorSummary {
                errors: 0,
                warnings: 1,
                infos: 0,
            },
            diagnostics: vec![DoctorDiagnostic {
                id: "runx.registry.installation_id".to_owned(),
                instance_id: "runx:doctor-registry:runx.registry.installation_id".to_owned(),
                severity: DoctorDiagnosticSeverity::Info,
                title: "Registry install identity".to_owned(),
                message: "Remote registry installs use an automatic local install identity."
                    .to_owned(),
                target: object([
                    ("kind", string_value("registry")),
                    ("id", string_value("runx.registry.installation_id")),
                ]),
                location: DoctorLocation {
                    path: "environment".to_owned(),
                    json_pointer: None,
                },
                evidence: None,
                repairs: vec![DoctorRepair {
                    id: "runx.registry.installation_id.configure_env".to_owned(),
                    kind: DoctorRepairKind::Manual,
                    confidence: DoctorRepairConfidence::High,
                    risk: DoctorRepairRisk::Low,
                    path: Some("environment".to_owned()),
                    json_pointer: None,
                    contents: Some(
                        "Set RUNX_INSTALLATION_ID only when you need a shared install identity."
                            .to_owned(),
                    ),
                    patch: None,
                    command: None,
                    requires_human_review: true,
                }],
            }],
        };

        let rendered = render_doctor_report(&report);

        assert!(rendered.contains(
            "next: Set RUNX_INSTALLATION_ID only when you need a shared install identity."
        ));
    }
}
