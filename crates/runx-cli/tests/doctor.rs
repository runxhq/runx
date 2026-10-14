use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn doctor_empty_workspace_json_matches_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = doctor_fixture("empty-success")?;
    let output = runx_command()
        .args(["doctor", "--json"])
        .env("RUNX_CWD", fixture.join("workspace"))
        .output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout)?,
        expected_report(&fixture)?
    );
    Ok(())
}

#[test]
fn doctor_failure_json_exits_one_and_matches_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = doctor_fixture("tool-fixture-missing")?;
    let workspace = fixture.join("workspace");
    let output = runx_command()
        .args(["doctor", workspace.to_str().unwrap_or_default(), "--json"])
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout)?,
        expected_report(&fixture)?
    );
    Ok(())
}

#[test]
fn doctor_reports_explicit_keyless_local_model_and_invalid_endpoint()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = doctor_fixture("empty-success")?;
    let workspace = fixture.join("workspace");
    let mut command = runx_command();
    command
        .args(["doctor", "--json"])
        .env("RUNX_CWD", &workspace)
        .env("RUNX_AGENT_PROVIDER", "local")
        .env("RUNX_AGENT_DIALECT", "chat_completions")
        .env("RUNX_AGENT_MODEL", "fixture-model")
        .env("RUNX_AGENT_AUTH_MODE", "local_none")
        .env(
            "RUNX_AGENT_ENDPOINT_URL",
            "http://127.0.0.1:18081/v1/chat/completions",
        );
    let output = command.output()?;
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let diagnostic = report["diagnostics"]
        .as_array()
        .and_then(|diagnostics| {
            diagnostics
                .iter()
                .find(|item| item["id"] == "runx.agent.config")
        })
        .ok_or("missing managed-agent diagnostic")?;
    assert_eq!(diagnostic["severity"], "info");
    assert_eq!(diagnostic["evidence"]["auth_mode"], "local_none");
    assert_eq!(diagnostic["evidence"]["api_key_set"], false);

    command.env(
        "RUNX_AGENT_ENDPOINT_URL",
        "http://localhost:18081/v1/chat/completions",
    );
    let output = command.output()?;
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let diagnostic = report["diagnostics"]
        .as_array()
        .and_then(|diagnostics| {
            diagnostics
                .iter()
                .find(|item| item["id"] == "runx.agent.config")
        })
        .ok_or("missing managed-agent diagnostic")?;
    assert_eq!(diagnostic["severity"], "warning");
    assert!(
        diagnostic["evidence"]["semantic_error"]
            .as_str()
            .is_some_and(|message| message.contains("agent.endpoint_url"))
    );
    Ok(())
}

#[test]
fn doctor_says_keep_one_model_key_when_two_differ() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = doctor_fixture("empty-success")?;
    let output = runx_command()
        .args(["doctor", "--json"])
        .env("RUNX_CWD", fixture.join("workspace"))
        .env("RUNX_AGENT_PROVIDER", "anthropic")
        .env("RUNX_AGENT_MODEL", "claude-fixture")
        .env("RUNX_AGENT_API_KEY", "sk-one")
        .env("ANTHROPIC_API_KEY", "sk-two")
        .output()?;
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let diagnostic = report["diagnostics"]
        .as_array()
        .and_then(|diagnostics| {
            diagnostics
                .iter()
                .find(|item| item["id"] == "runx.agent.config")
        })
        .ok_or("missing managed-agent diagnostic")?;
    assert!(
        diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains(
                "the model key is set by RUNX_AGENT_API_KEY and ANTHROPIC_API_KEY with different values"
            )),
        "{diagnostic}"
    );
    assert_eq!(diagnostic["repairs"][0]["id"], "runx.agent.config.one_key");
    assert_eq!(diagnostic["repairs"].as_array().map(Vec::len), Some(1));
    Ok(())
}

#[test]
fn config_cli_switches_from_local_model_to_anthropic_and_doctor_accepts_it()
-> Result<(), Box<dyn std::error::Error>> {
    let home = temp_root("runx-config-provider-switch");
    fs::create_dir_all(&home)?;
    let fixture = doctor_fixture("empty-success")?;
    let workspace = fixture.join("workspace");
    for (key, value) in [
        ("agent.provider", "local"),
        ("agent.dialect", "chat_completions"),
        ("agent.model", "fixture-local"),
        ("agent.auth_mode", "local_none"),
        (
            "agent.endpoint_url",
            "http://127.0.0.1:18081/v1/chat/completions",
        ),
    ] {
        assert!(
            runx_command()
                .args(["config", "set", key, value])
                .env("RUNX_HOME", &home)
                .output()?
                .status
                .success()
        );
    }
    for key in ["agent.endpoint_url", "agent.dialect"] {
        assert!(
            runx_command()
                .args(["config", "unset", key])
                .env("RUNX_HOME", &home)
                .output()?
                .status
                .success()
        );
    }
    for (key, value) in [
        ("agent.provider", "anthropic"),
        ("agent.model", "fixture-claude"),
        ("agent.auth_mode", "api_key"),
    ] {
        assert!(
            runx_command()
                .args(["config", "set", key, value])
                .env("RUNX_HOME", &home)
                .output()?
                .status
                .success()
        );
    }
    let mut set_key = runx_command();
    let mut child = set_key
        .args(["config", "set", "agent.api_key", "--from-stdin"])
        .env("RUNX_HOME", &home)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing stdin")?
        .write_all(b"fixture-secret")?;
    assert!(child.wait()?.success());

    let config_env =
        BTreeMap::from([("RUNX_HOME".to_owned(), home.to_string_lossy().into_owned())]);
    let loaded = runx_runtime::load_managed_agent_config(&config_env, &workspace)?
        .ok_or("managed configuration should load")?;
    assert_eq!(loaded.provider.as_str(), "anthropic");
    assert_eq!(loaded.endpoint_url, "https://api.anthropic.com/v1/messages");
    assert!(loaded.api_key.is_some());

    let doctor = runx_command()
        .args(["doctor", "--json"])
        .env("RUNX_HOME", &home)
        .env("RUNX_CWD", workspace)
        .output()?;
    assert!(doctor.status.success());
    let report: serde_json::Value = serde_json::from_slice(&doctor.stdout)?;
    let diagnostic = report["diagnostics"]
        .as_array()
        .ok_or("missing diagnostics")?
        .iter()
        .find(|item| item["id"] == "runx.agent.config")
        .ok_or("missing managed-agent diagnostic")?;
    assert_eq!(diagnostic["severity"], "info");
    assert_eq!(diagnostic["evidence"]["endpoint_set"], false);
    assert_eq!(diagnostic["evidence"]["auth_mode"], "api_key");
    fs::remove_dir_all(home)?;
    Ok(())
}

#[test]
fn doctor_authority_json_reports_missing_env_names() -> Result<(), Box<dyn std::error::Error>> {
    let output = authority_doctor_command()
        .args(["doctor", "authority", "--json"])
        .output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(report["status"], "success");
    // The signer is ready (the first run creates this home's key); nothing is
    // trusted yet, and no provider grant is configured.
    assert_eq!(report["summary"]["warnings"], 2);
    let rendered = serde_json::to_string(&report)?;
    for env_name in AUTHORITY_ENV_NAMES {
        assert!(
            rendered.contains(env_name),
            "authority doctor should name missing env var {env_name}"
        );
    }
    Ok(())
}

#[test]
fn doctor_authority_json_redacts_secret_values() -> Result<(), Box<dyn std::error::Error>> {
    let output = authority_doctor_command()
        .args(["doctor", "authority", "--json"])
        .env("RUNX_RECEIPT_SIGN_KID", "kid_prod")
        .env(
            "RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64",
            "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=",
        )
        .env("RUNX_RECEIPT_SIGN_ISSUER_TYPE", "hosted")
        .env("RUNX_PUBLIC_API_BASE_URL", "https://api.runx.test")
        .env("RUNX_PUBLIC_API_TOKEN", "rxk-secret-token")
        .output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(report["summary"]["infos"], 3);
    let rendered = serde_json::to_string(&report)?;
    assert!(rendered.contains("kid_prod"));
    assert!(rendered.contains("configured_signer"));
    assert!(!rendered.contains("QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI="));
    assert!(!rendered.contains("rxk-secret-token"));
    Ok(())
}

#[test]
fn doctor_authority_reports_connect_grant_discovery_without_exposing_token()
-> Result<(), Box<dyn std::error::Error>> {
    let output = authority_doctor_command()
        .args(["doctor", "authority", "--json"])
        .env("RUNX_PUBLIC_API_BASE_URL", "https://api.runx.test")
        .env("RUNX_PUBLIC_API_TOKEN", "rxk-secret-token")
        .output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    let provider = report["diagnostics"]
        .as_array()
        .and_then(|diagnostics| {
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic["id"] == "runx.authority.provider_grant")
        })
        .ok_or_else(|| std::io::Error::other("provider grant diagnostic is missing"))?;
    assert_eq!(provider["severity"], "info");
    assert_eq!(provider["evidence"]["connect_discovery"], true);
    assert!(
        provider["message"]
            .as_str()
            .is_some_and(|message| message.contains("Runx Connect is configured"))
    );
    assert!(!serde_json::to_string(&report)?.contains("rxk-secret-token"));
    Ok(())
}

#[test]
fn doctor_registry_json_reports_readiness_without_key_material()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("doctor-registry");
    let output = registry_doctor_command()
        .args(["doctor", "registry", "--json"])
        .env("RUNX_HOME", root.to_str().unwrap_or_default())
        .env("RUNX_REGISTRY_URL", "https://registry.runx.test/api")
        .env("RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID", "operator-key-1")
        .env(
            "RUNX_REGISTRY_MANIFEST_TRUST_KEY_BASE64",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .env("RUNX_REGISTRY_MANIFEST_TRUST_OWNER", "acme")
        .output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(report["status"], "success");
    assert_eq!(report["summary"]["warnings"], 0);
    let rendered = serde_json::to_string(&report)?;
    assert!(rendered.contains("https://registry.runx.test/api"));
    assert!(rendered.contains("runx.project.skills"));
    assert!(rendered.contains("operator-key-1"));
    assert!(rendered.contains("acme/*"));
    assert!(rendered.contains("RUNX_INSTALLATION_ID"));
    assert!(!rendered.contains("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="));
    assert!(!diagnostic_has_repair(
        &report,
        "runx.registry.installation_id"
    ));
    Ok(())
}

#[test]
fn doctor_registry_json_reports_trust_policy_scope_without_key_material()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("doctor-registry-trust-policy");
    let output = registry_doctor_command()
        .args(["doctor", "registry", "--json"])
        .env("RUNX_HOME", root.to_str().unwrap_or_default())
        .output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    let rendered = serde_json::to_string(&report)?;
    assert!(rendered.contains("trust_policy"));
    assert!(rendered.contains("official_runx"));
    assert!(rendered.contains("runx/*"));
    assert!(rendered.contains("can_grant_first_party"));
    assert!(!rendered.contains("RUNX_REGISTRY_MANIFEST_PUBLIC_KEY"));
    Ok(())
}

#[test]
fn doctor_registry_json_warns_on_partial_trust_key_config() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_root("doctor-registry-partial");
    let output = registry_doctor_command()
        .args(["doctor", "registry", "--json"])
        .env("RUNX_HOME", root.to_str().unwrap_or_default())
        .env(
            "RUNX_REGISTRY_MANIFEST_TRUST_KEY_BASE64",
            "raw-public-key-material",
        )
        .output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(report["status"], "success");
    assert_eq!(report["summary"]["warnings"], 1);
    let rendered = serde_json::to_string(&report)?;
    assert!(rendered.contains("partial_operator_key_config"));
    assert!(rendered.contains("RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID"));
    assert!(rendered.contains("RUNX_REGISTRY_MANIFEST_TRUST_OWNER"));
    assert!(!rendered.contains("raw-public-key-material"));
    assert!(diagnostic_has_repair(&report, "runx.registry.trust_keys"));
    Ok(())
}

fn diagnostic_has_repair(report: &serde_json::Value, id: &str) -> bool {
    report["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|diagnostic| {
            diagnostic["id"] == id
                && diagnostic["repairs"]
                    .as_array()
                    .is_some_and(|repairs| !repairs.is_empty())
        })
}

fn runx_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_runx"));
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.env("NO_COLOR", "1");
    command.env("RUNX_HOME", temp_root("runx-doctor-home"));
    crate::support::isolate_project(&mut command);
    command
}

fn authority_doctor_command() -> Command {
    let mut command = runx_command();
    for env_name in AUTHORITY_ENV_NAMES {
        command.env_remove(env_name);
    }
    command
}

fn registry_doctor_command() -> Command {
    let mut command = runx_command();
    for env_name in REGISTRY_ENV_NAMES {
        command.env_remove(env_name);
    }
    command
}

const AUTHORITY_ENV_NAMES: &[&str] = &[
    "RUNX_RECEIPT_SIGN_KID",
    "RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64",
    "RUNX_RECEIPT_SIGN_ISSUER_TYPE",
    "RUNX_RECEIPT_VERIFY_ISSUER_TYPE",
    "RUNX_RECEIPT_VERIFY_KID",
    "RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64",
    "RUNX_PUBLIC_API_TOKEN",
];

const REGISTRY_ENV_NAMES: &[&str] = &[
    "RUNX_HOME",
    "RUNX_REGISTRY_URL",
    "RUNX_REGISTRY_DIR",
    "RUNX_INSTALLATION_ID",
    "RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID",
    "RUNX_REGISTRY_MANIFEST_TRUST_KEY_BASE64",
    "RUNX_REGISTRY_MANIFEST_TRUST_OWNER",
];

fn temp_root(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()))
}

fn expected_report(fixture: &Path) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let expected_json = fs::read_to_string(fixture.join("expected.json"))?;
    Ok(serde_json::from_str(&expected_json)?)
}

fn doctor_fixture(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(repo_root()?.join("fixtures").join("doctor").join(name))
}

fn repo_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}
