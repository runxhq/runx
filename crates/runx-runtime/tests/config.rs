use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::connector::{SecretName, SecretReference};
use runx_runtime::{
    ConfigError, ConfigKey, ManagedAgentAuthMode, ManagedAgentConfig, ModelDialect,
    RunxAgentConfig, RunxConfigFile, RunxPaths, RunxSecretRecord, SecretString,
    load_managed_agent_config, load_runx_config_file, lookup_runx_config_value,
    mask_runx_config_file, secrets, update_runx_config_value, write_runx_config_file,
};
use tempfile::tempdir;

#[test]
fn config_home_path_anchors_relative_runx_home_to_workspace_base()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path().join("workspace");
    let run_dir = temp.path().join("run");
    let cwd = workspace.join("packages/demo");
    fs::create_dir_all(workspace.join("home"))?;
    fs::create_dir_all(&cwd)?;
    fs::write(
        workspace.join("pnpm-workspace.yaml"),
        "packages:\n  - packages/*\n",
    )?;

    let env = env_map([
        ("RUNX_CWD", run_dir.to_str().unwrap_or_default()),
        ("RUNX_HOME", "home"),
    ]);

    assert_eq!(RunxPaths::resolve(&env, &cwd).home(), run_dir.join("home"));
    Ok(())
}

#[test]
fn config_home_path_uses_the_admitted_environment_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let home = temp.path().join("operator-home");
    let cwd = temp.path().join("workspace");
    let env = env_map([("HOME", home.to_str().unwrap_or_default())]);

    assert_eq!(RunxPaths::resolve(&env, &cwd).home(), home.join(".runx"));
    Ok(())
}

#[test]
fn a_secret_is_sealed_in_the_home_and_the_agent_names_it() -> Result<(), Box<dyn std::error::Error>>
{
    let secret = "sk-test-secret";
    let temp = tempdir()?;
    let home = temp.path();
    let config = agent_key(home, RunxConfigFile::default(), secret)?;

    let Some(RunxSecretRecord::Stored { material_id }) = config.secrets.get(&name("agent")?) else {
        return Err("the secret is sealed in the home".into());
    };
    assert!(material_id.starts_with("local_secret_"));
    assert_eq!(
        secrets::reveal(&home_env(home), home, &name("agent")?)?,
        secret
    );
    assert_eq!(
        lookup_runx_config_value(&config, ConfigKey::AgentSecret),
        Some("agent".to_owned())
    );
    assert_eq!(
        mask_runx_config_file(&config).secrets.get(&name("agent")?),
        Some(&RunxSecretRecord::Stored {
            material_id: "[encrypted]".to_owned()
        })
    );
    let written = fs::read_to_string(home.join("config.json"))?;
    assert!(!written.contains(secret));
    let key_file = home.join("keys").join(format!("{material_id}.json"));
    assert!(key_file.exists());
    assert_private_file(&home.join("keys/local-config-secret"))?;
    assert_private_file(&key_file)?;

    // A secret's value is never empty, and the agent names only a set one.
    assert!(matches!(
        secrets::set_value(&home_at(home), &name("blank")?, "   "),
        Err(secrets::SecretError::Empty)
    ));
    assert!(matches!(
        update_runx_config_value(config, ConfigKey::AgentSecret, "unset"),
        Err(ConfigError::UnknownSecret { .. })
    ));
    Ok(())
}

#[test]
fn config_loads_and_writes_supported_keys_only() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let config_path = temp.path().join("config.json");
    let config = RunxConfigFile {
        agent: Some(RunxAgentConfig {
            provider: Some("openai".to_owned()),
            model: Some("gpt-test".to_owned()),
            ..RunxAgentConfig::default()
        }),
        ..RunxConfigFile::default()
    };
    write_runx_config_file(&config_path, &config)?;
    assert_private_file(&config_path)?;

    assert_eq!(load_runx_config_file(&config_path)?, config);
    assert_eq!(
        lookup_runx_config_value(&config, ConfigKey::AgentProvider),
        Some("openai".to_owned())
    );
    assert!(matches!(
        runx_runtime::parse_config_key("agent.unknown"),
        Err(ConfigError::UnsupportedKey { .. })
    ));
    // A runx account credential is stored by login, bound to its principal,
    // never set as config.
    assert!(matches!(
        runx_runtime::parse_config_key("public.api_token"),
        Err(ConfigError::UnsupportedKey { .. })
    ));
    Ok(())
}

#[test]
fn config_loads_missing_and_rejects_malformed_or_non_object_json()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let missing_path = temp.path().join("missing.json");
    assert_eq!(
        load_runx_config_file(&missing_path)?,
        RunxConfigFile::default()
    );

    let malformed_path = temp.path().join("malformed.json");
    fs::write(&malformed_path, "{not-json")?;
    assert!(matches!(
        load_runx_config_file(&malformed_path),
        Err(ConfigError::InvalidJson { .. })
    ));

    let non_object_path = temp.path().join("array.json");
    fs::write(&non_object_path, "[]")?;
    assert!(matches!(
        load_runx_config_file(&non_object_path),
        Err(ConfigError::NonObjectJson { .. })
    ));
    Ok(())
}

#[test]
fn a_corrupt_sealed_secret_is_reported_with_its_file() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let keys_dir = temp.path().join("keys");
    fs::create_dir_all(&keys_dir)?;
    // Kept as runx keeps keys, closed to others; only its contents are bad.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&keys_dir, fs::Permissions::from_mode(0o700))?;
    }
    for (name, contents) in [
        ("local-config-secret", "test-secret"),
        ("local_secret_corrupt.json", "{not-json"),
    ] {
        let path = keys_dir.join(name);
        fs::write(&path, contents)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
    }
    write_runx_config_file(
        &temp.path().join("config.json"),
        &RunxConfigFile {
            secrets: BTreeMap::from([(
                name("agent")?,
                RunxSecretRecord::Stored {
                    material_id: "local_secret_corrupt".to_owned(),
                },
            )]),
            ..RunxConfigFile::default()
        },
    )?;

    let error = secrets::reveal(&home_env(temp.path()), temp.path(), &name("agent")?)
        .err()
        .ok_or("a corrupt secret should fail")?
        .to_string();
    assert!(error.contains("is corrupted or unreadable"), "{error}");
    assert!(error.contains("local_secret_corrupt.json"), "{error}");
    Ok(())
}

#[test]
fn config_loads_the_managed_agent_key_from_its_one_source() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let explicit_env = env_map([
        ("RUNX_HOME", temp.path().to_str().unwrap_or_default()),
        ("RUNX_AGENT_PROVIDER", "openai"),
        ("RUNX_AGENT_MODEL", "gpt-test"),
        ("RUNX_AGENT_API_KEY", "sk-explicit"),
    ]);
    assert_eq!(
        load_managed_agent_config(&explicit_env, temp.path())?,
        Some(ManagedAgentConfig {
            provider: "openai".into(),
            dialect: ModelDialect::OpenAiResponses,
            model: "gpt-test".to_owned(),
            api_key: Some(SecretString::new("sk-explicit")),
            endpoint_url: "https://api.openai.com/v1/responses".to_owned(),
            auth_mode: runx_runtime::ManagedAgentAuthMode::ApiKey,
        })
    );

    agent_key(
        temp.path(),
        RunxConfigFile {
            agent: Some(RunxAgentConfig {
                provider: Some("anthropic".to_owned()),
                model: Some("claude-test".to_owned()),
                ..RunxAgentConfig::default()
            }),
            ..RunxConfigFile::default()
        },
        "local-secret",
    )?;
    let local_env = home_env(temp.path());
    assert_eq!(
        load_managed_agent_config(&local_env, temp.path())?,
        Some(ManagedAgentConfig {
            provider: "anthropic".into(),
            dialect: ModelDialect::AnthropicMessages,
            model: "claude-test".to_owned(),
            api_key: Some(SecretString::new("local-secret")),
            endpoint_url: "https://api.anthropic.com/v1/messages".to_owned(),
            auth_mode: runx_runtime::ManagedAgentAuthMode::ApiKey,
        })
    );

    // The provider's variable and the agent's secret are two places for one
    // key: when they differ runx keeps neither.
    let differing = env_map([
        ("RUNX_HOME", temp.path().to_str().unwrap_or_default()),
        ("ANTHROPIC_API_KEY", "anthropic-env-secret"),
    ]);
    let refused = load_managed_agent_config(&differing, temp.path())
        .err()
        .ok_or("two differing keys were accepted")?;
    assert_eq!(
        refused.to_string(),
        "the model key is set by ANTHROPIC_API_KEY and the secret agent with different values; keep one"
    );
    let agreeing = env_map([
        ("RUNX_HOME", temp.path().to_str().unwrap_or_default()),
        ("ANTHROPIC_API_KEY", "local-secret"),
    ]);
    assert_eq!(
        load_managed_agent_config(&agreeing, temp.path())?.and_then(|config| config.api_key),
        Some(SecretString::new("local-secret"))
    );

    // A secret that cannot be read cannot be shown to agree: refused, even
    // with a key in the environment.
    let unreadable_home = tempdir()?;
    let mut unreadable = RunxConfigFile {
        secrets: BTreeMap::from([(
            name("agent")?,
            RunxSecretRecord::Stored {
                material_id: "local_secret_missing".to_owned(),
            },
        )]),
        ..RunxConfigFile::default()
    };
    unreadable.agent = Some(RunxAgentConfig {
        secret: Some(name("agent")?),
        ..RunxAgentConfig::default()
    });
    write_runx_config_file(&unreadable_home.path().join("config.json"), &unreadable)?;
    let over_unreadable = env_map([
        (
            "RUNX_HOME",
            unreadable_home.path().to_str().unwrap_or_default(),
        ),
        ("RUNX_AGENT_PROVIDER", "anthropic"),
        ("RUNX_AGENT_MODEL", "claude-env"),
        ("ANTHROPIC_API_KEY", "anthropic-env-secret"),
    ]);
    let refused = load_managed_agent_config(&over_unreadable, unreadable_home.path())
        .err()
        .ok_or("an unreadable secret was ignored")?
        .to_string();
    assert!(refused.contains("is corrupted or unreadable"), "{refused}");

    let keyless_home = tempdir()?;
    let provider_env = env_map([
        (
            "RUNX_HOME",
            keyless_home.path().to_str().unwrap_or_default(),
        ),
        ("RUNX_AGENT_PROVIDER", "anthropic"),
        ("RUNX_AGENT_MODEL", "claude-env"),
        ("ANTHROPIC_API_KEY", "anthropic-env-secret"),
    ]);
    let provider_config = load_managed_agent_config(&provider_env, keyless_home.path())?;
    let mut shadowed = provider_env.clone();
    shadowed.insert(
        "RUNX_ENV_FILE_DIFFERS".to_owned(),
        "ANTHROPIC_API_KEY".to_owned(),
    );
    assert_eq!(
        load_managed_agent_config(&shadowed, keyless_home.path())
            .err()
            .ok_or("a key the environment and .env disagree on was accepted")?
            .to_string(),
        "the model key is set in the environment and in the workspace .env as ANTHROPIC_API_KEY with different values; keep one"
    );
    let debug = format!("{provider_config:?}");
    assert!(provider_config.is_some());
    assert!(!debug.contains("anthropic-env-secret"));
    assert!(debug.contains("[redacted-credential]"));
    Ok(())
}

#[test]
fn providers_are_profiles_and_any_other_names_its_dialect_and_endpoint()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let home = temp.path().to_str().unwrap_or_default();
    let gemini = load_managed_agent_config(
        &env_map([
            ("RUNX_HOME", home),
            ("RUNX_AGENT_PROVIDER", "gemini"),
            ("RUNX_AGENT_MODEL", "gemini-test"),
            ("GEMINI_API_KEY", "gemini-secret"),
            ("OPENAI_API_KEY", "unrelated-secret"),
        ]),
        temp.path(),
    )?
    .ok_or("gemini is a built-in profile")?;
    assert_eq!(gemini.dialect, ModelDialect::ChatCompletions);
    assert_eq!(gemini.api_key, Some(SecretString::new("gemini-secret")));
    assert!(
        gemini
            .endpoint_url
            .starts_with("https://generativelanguage.googleapis.com/")
    );

    let custom = |extra: &[(&str, &str)]| {
        let mut env = env_map([
            ("RUNX_HOME", home),
            ("RUNX_AGENT_PROVIDER", "gateway"),
            ("RUNX_AGENT_MODEL", "any-model"),
            ("RUNX_AGENT_API_KEY", "gateway-secret"),
            ("GATEWAY_API_KEY", "ambient-secret"),
        ]);
        env.extend(
            extra
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        );
        load_managed_agent_config(&env, temp.path())
    };
    assert!(matches!(
        custom(&[]),
        Err(ConfigError::InvalidAgentSetting {
            key: "agent.dialect",
            ..
        })
    ));
    let gateway = custom(&[
        ("RUNX_AGENT_DIALECT", "anthropic_messages"),
        (
            "RUNX_AGENT_ENDPOINT_URL",
            "https://gateway.example/v1/messages",
        ),
    ])?
    .ok_or("a configured provider resolves")?;
    assert_eq!(gateway.dialect, ModelDialect::AnthropicMessages);
    assert_eq!(gateway.api_key, Some(SecretString::new("gateway-secret")));
    assert!(matches!(
        load_managed_agent_config(
            &env_map([
                ("RUNX_HOME", home),
                ("RUNX_AGENT_PROVIDER", "openai"),
                ("RUNX_AGENT_DIALECT", "anthropic_messages"),
                ("RUNX_AGENT_MODEL", "gpt-test"),
                ("RUNX_AGENT_API_KEY", "sk"),
            ]),
            temp.path(),
        ),
        Err(ConfigError::InvalidAgentSetting {
            key: "agent.dialect",
            ..
        })
    ));
    Ok(())
}

#[test]
fn config_matches_blank_env_overlay_edges() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    agent_key(
        temp.path(),
        RunxConfigFile {
            agent: Some(RunxAgentConfig {
                provider: Some("anthropic".to_owned()),
                model: Some("claude-file".to_owned()),
                ..RunxAgentConfig::default()
            }),
            ..RunxConfigFile::default()
        },
        "local-secret",
    )?;
    let runx_home = temp.path().to_str().unwrap_or_default();

    let blank_provider = env_map([
        ("RUNX_HOME", runx_home),
        ("RUNX_AGENT_PROVIDER", ""),
        ("RUNX_AGENT_MODEL", "claude-env"),
        ("RUNX_AGENT_API_KEY", "sk-explicit"),
    ]);
    assert_eq!(
        load_managed_agent_config(&blank_provider, temp.path())?,
        None
    );

    // A blank model is no model, as a blank provider is no provider, and
    // runx names none of its own.
    let blank_model = env_map([
        ("RUNX_HOME", runx_home),
        ("RUNX_AGENT_PROVIDER", "anthropic"),
        ("RUNX_AGENT_MODEL", ""),
    ]);
    assert!(matches!(
        load_managed_agent_config(&blank_model, temp.path()),
        Err(ConfigError::InvalidAgentSetting {
            key: "agent.model",
            ..
        })
    ));

    let blank_generic_key = env_map([
        ("RUNX_HOME", runx_home),
        ("RUNX_AGENT_PROVIDER", "anthropic"),
        ("RUNX_AGENT_MODEL", "claude-env"),
        ("RUNX_AGENT_API_KEY", ""),
        ("ANTHROPIC_API_KEY", "provider-secret"),
    ]);
    // A blank variable is no key; the provider's own variable and the
    // agent's secret are then two that differ.
    assert_eq!(
        load_managed_agent_config(&blank_generic_key, temp.path())
            .err()
            .ok_or("two differing keys were accepted")?
            .to_string(),
        "the model key is set by ANTHROPIC_API_KEY and the secret agent with different values; keep one"
    );
    Ok(())
}

#[test]
fn explicit_keyless_mode_requires_exact_loopback_and_remains_repairable()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let config_path = temp.path().join("config.json");
    let endpoint = "http://127.0.0.1:18081/v1/chat/completions";
    let config = RunxConfigFile {
        agent: Some(RunxAgentConfig {
            provider: Some("local".to_owned()),
            dialect: Some("chat_completions".to_owned()),
            model: Some("qwen2.5".to_owned()),
            endpoint_url: Some(endpoint.to_owned()),
            auth_mode: Some("local_none".to_owned()),
            ..RunxAgentConfig::default()
        }),
        ..RunxConfigFile::default()
    };
    write_runx_config_file(&config_path, &config)?;
    let env = home_env(temp.path());
    assert_eq!(
        load_managed_agent_config(&env, temp.path())?,
        Some(ManagedAgentConfig {
            provider: "local".into(),
            dialect: ModelDialect::ChatCompletions,
            model: "qwen2.5".to_owned(),
            api_key: None,
            endpoint_url: endpoint.to_owned(),
            auth_mode: ManagedAgentAuthMode::LocalNone,
        })
    );

    // The keyless mode reads no key, whatever secret the agent names.
    agent_key(temp.path(), config.clone(), "stored-api-key")?;
    let local_override = env_map([
        ("RUNX_HOME", temp.path().to_str().unwrap_or_default()),
        ("RUNX_AGENT_AUTH_MODE", "local_none"),
    ]);
    assert_eq!(
        load_managed_agent_config(&local_override, temp.path())?
            .ok_or("missing local model")?
            .api_key,
        None
    );
    write_runx_config_file(&config_path, &config)?;

    for (field, expected_key) in [("provider", "agent.provider"), ("model", "agent.model")] {
        let mut incomplete = config.clone();
        let agent = incomplete.agent.as_mut().ok_or("missing agent")?;
        if field == "provider" {
            agent.provider = None;
        } else {
            agent.model = None;
        }
        write_runx_config_file(&config_path, &incomplete)?;
        assert!(matches!(
            load_managed_agent_config(&env, temp.path()),
            Err(ConfigError::InvalidAgentSetting { key, .. }) if key == expected_key
        ));
    }
    write_runx_config_file(&config_path, &config)?;

    let mut invalid = config.clone();
    invalid.agent.as_mut().ok_or("missing agent")?.auth_mode = Some("typo".to_owned());
    write_runx_config_file(&config_path, &invalid)?;
    assert_eq!(
        lookup_runx_config_value(
            &load_runx_config_file(&config_path)?,
            ConfigKey::AgentAuthMode
        ),
        Some("typo".to_owned())
    );
    assert!(matches!(
        load_managed_agent_config(&env, temp.path()),
        Err(ConfigError::InvalidAgentSetting {
            key: "agent.auth_mode",
            ..
        })
    ));
    assert!(matches!(
        update_runx_config_value(invalid.clone(), ConfigKey::AgentAuthMode, "typo"),
        Err(ConfigError::InvalidAgentSetting {
            key: "agent.auth_mode",
            ..
        })
    ));
    let repaired = update_runx_config_value(invalid, ConfigKey::AgentAuthMode, "local_none")?;
    write_runx_config_file(&config_path, &repaired)?;
    assert!(load_managed_agent_config(&env, temp.path())?.is_some());

    for forbidden in [
        "http://localhost:18081/v1/chat/completions",
        "http://127.0.0.2:18081/v1/chat/completions",
        "http://127.0.0.1:18081/other",
        "http://127.0.0.1:18081/v1/chat/completions?next=evil",
        "https://127.0.0.1:18081/v1/chat/completions",
        "http://127.0.0.1:18081@evil.example/v1/chat/completions",
    ] {
        let env = env_map([
            ("RUNX_HOME", temp.path().to_str().unwrap_or_default()),
            ("RUNX_AGENT_ENDPOINT_URL", forbidden),
        ]);
        assert!(
            matches!(
                load_managed_agent_config(&env, temp.path()),
                Err(ConfigError::InvalidAgentSetting {
                    key: "agent.endpoint_url",
                    ..
                })
            ),
            "endpoint must be rejected: {forbidden}"
        );
    }
    Ok(())
}

#[test]
fn a_model_key_held_by_a_manager_is_one_source_among_the_others()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let held = temp.path().join("anthropic.key");
    fs::write(&held, "key-from-the-manager\n")?;
    let reference = format!("file://{}", held.display());
    let home = temp.path().to_str().unwrap_or_default();
    secrets::set_reference(
        &home_env(temp.path()),
        temp.path(),
        &name("agent")?,
        &SecretReference::new(reference.clone()).ok_or("a reference")?,
    )?;
    let config = update_runx_config_value(
        load_runx_config_file(&temp.path().join("config.json"))?,
        ConfigKey::AgentSecret,
        "agent",
    )?;
    write_runx_config_file(&temp.path().join("config.json"), &config)?;
    let with = |key: &str| {
        env_map([
            ("RUNX_HOME", home),
            ("RUNX_AGENT_PROVIDER", "anthropic"),
            ("RUNX_AGENT_MODEL", "claude-test"),
            ("ANTHROPIC_API_KEY", key),
        ])
    };

    assert_eq!(
        load_managed_agent_config(&with(""), temp.path())?.and_then(|config| config.api_key),
        Some(SecretString::new("key-from-the-manager"))
    );
    assert_eq!(
        load_managed_agent_config(&with("key-from-the-manager"), temp.path())?
            .and_then(|config| config.api_key),
        Some(SecretString::new("key-from-the-manager"))
    );
    assert_eq!(
        load_managed_agent_config(&with("another-key"), temp.path())
            .err()
            .ok_or("two differing keys were accepted")?
            .to_string(),
        "the model key is set by ANTHROPIC_API_KEY and the secret agent with different values; keep one"
    );
    fs::remove_file(&held)?;
    assert!(
        load_managed_agent_config(&with(""), temp.path())
            .err()
            .is_some_and(|error| error
                .to_string()
                .starts_with(&format!("model key: {reference} cannot be read")))
    );
    Ok(())
}

#[test]
fn a_model_key_variable_holding_a_reference_is_refused_not_sent()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let refused = load_managed_agent_config(
        &env_map([
            ("RUNX_HOME", temp.path().to_str().unwrap_or_default()),
            ("RUNX_AGENT_PROVIDER", "anthropic"),
            ("RUNX_AGENT_MODEL", "claude-test"),
            ("ANTHROPIC_API_KEY", "op://dev/anthropic/credential"),
        ]),
        temp.path(),
    )
    .err()
    .ok_or("a reference was taken as the key")?;
    assert!(
        refused
            .to_string()
            .contains("ANTHROPIC_API_KEY holds a reference to a secret, not the key"),
        "{refused}"
    );
    Ok(())
}

/// Write `config` to the home at `home`, keep `value` as the secret `agent`
/// there, and name it as the managed agent's key.
fn agent_key(
    home: &Path,
    config: RunxConfigFile,
    value: &str,
) -> Result<RunxConfigFile, Box<dyn std::error::Error>> {
    let paths = home_at(home);
    write_runx_config_file(&paths.config_file(), &config)?;
    secrets::set_value(&paths, &name("agent")?, value)?;
    let config = update_runx_config_value(
        load_runx_config_file(&paths.config_file())?,
        ConfigKey::AgentSecret,
        "agent",
    )?;
    write_runx_config_file(&paths.config_file(), &config)?;
    Ok(config)
}

fn name(value: &str) -> Result<SecretName, Box<dyn std::error::Error>> {
    Ok(SecretName::new(value).ok_or("a secret name")?)
}

fn home_env(home: &Path) -> BTreeMap<String, String> {
    env_map([("RUNX_HOME", home.to_str().unwrap_or_default())])
}

fn home_at(home: &Path) -> RunxPaths {
    RunxPaths::resolve(&home_env(home), home)
}

fn env_map<const N: usize>(entries: [(&str, &str); N]) -> BTreeMap<String, String> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

#[cfg(unix)]
fn assert_private_file(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "{} mode was {mode:o}", path.display());
    Ok(())
}

#[cfg(not(unix))]
fn assert_private_file(_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
