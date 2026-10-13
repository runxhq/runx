use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_runtime::{
    ConfigError, ConfigKey, ManagedAgentAuthMode, ManagedAgentConfig, ModelDialect,
    RunxAgentConfig, RunxConfigFile, RunxPaths, SecretString, load_local_agent_api_key,
    load_local_public_api_token, load_managed_agent_config, load_runx_config_file,
    lookup_runx_config_value, mask_runx_config_file, update_runx_config_value,
    write_runx_config_file,
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
fn config_round_trips_encrypted_secrets() -> Result<(), Box<dyn std::error::Error>> {
    type SecretRef = fn(&RunxConfigFile) -> Option<String>;
    type LoadSecret = fn(&RunxPaths, &str) -> Result<String, ConfigError>;
    let cases: [(ConfigKey, &str, &str, SecretRef, LoadSecret); 2] = [
        (
            ConfigKey::AgentApiKey,
            "sk-test-secret",
            "local_agent_key_",
            |config| {
                config
                    .agent
                    .as_ref()
                    .and_then(|agent| agent.api_key_ref.clone())
            },
            load_local_agent_api_key,
        ),
        (
            ConfigKey::PublicApiToken,
            "rxk_test_secret",
            "local_public_api_token_",
            |config| {
                config
                    .public
                    .as_ref()
                    .and_then(|public| public.api_token_ref.clone())
            },
            load_local_public_api_token,
        ),
    ];

    for (key, secret, ref_prefix, secret_ref, load_secret) in cases {
        let temp = tempdir()?;
        let config_dir = temp.path();
        let paths = home_at(config_dir);
        let config = update_runx_config_value(RunxConfigFile::default(), key, secret, &paths)?;
        let stored_ref = secret_ref(&config).ok_or("missing secret ref")?;

        assert!(stored_ref.starts_with(ref_prefix));
        assert_eq!(load_secret(&paths, &stored_ref)?, secret);
        assert_eq!(
            lookup_runx_config_value(&config, key),
            Some("[encrypted]".to_owned())
        );
        assert_eq!(
            secret_ref(&mask_runx_config_file(&config)),
            Some("[encrypted]".to_owned())
        );
        let config_json = serde_json::to_string(&config)?;
        assert!(!config_json.contains(secret));
        let key_file = config_dir.join("keys").join(format!("{stored_ref}.json"));
        assert!(config_dir.join("keys/local-config-secret").exists());
        assert!(key_file.exists());
        assert_private_file(&config_dir.join("keys/local-config-secret"))?;
        assert_private_file(&key_file)?;
    }
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
            api_key_ref: None,
            ..RunxAgentConfig::default()
        }),
        public: None,
        credentials: None,
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
fn config_reports_corrupt_local_agent_key_with_stable_prefix()
-> Result<(), Box<dyn std::error::Error>> {
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
        ("local_agent_key_corrupt.json", "{not-json"),
    ] {
        let path = keys_dir.join(name);
        fs::write(&path, contents)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
    }

    let error = match load_local_agent_api_key(&home_at(temp.path()), "local_agent_key_corrupt") {
        Ok(_) => return Err("corrupt key should fail".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("runx local agent key corrupted or unreadable at")
    );
    assert!(error.to_string().contains("local_agent_key_corrupt.json"));
    Ok(())
}

#[test]
fn config_loads_managed_agent_env_precedence_and_local_key_fallback()
-> Result<(), Box<dyn std::error::Error>> {
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
            dialect: ModelDialect::ChatCompletions,
            model: "gpt-test".to_owned(),
            api_key: Some(SecretString::new("sk-explicit")),
            endpoint_url: "https://api.openai.com/v1/chat/completions".to_owned(),
            auth_mode: runx_runtime::ManagedAgentAuthMode::ApiKey,
        })
    );

    let local_config = update_runx_config_value(
        RunxConfigFile {
            agent: Some(RunxAgentConfig {
                provider: Some("anthropic".to_owned()),
                model: Some("claude-test".to_owned()),
                api_key_ref: None,
                ..RunxAgentConfig::default()
            }),
            public: None,
            credentials: None,
        },
        ConfigKey::AgentApiKey,
        "local-secret",
        &home_at(temp.path()),
    )?;
    write_runx_config_file(&temp.path().join("config.json"), &local_config)?;
    let local_env = env_map([("RUNX_HOME", temp.path().to_str().unwrap_or_default())]);
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

    let provider_env = env_map([
        ("RUNX_HOME", temp.path().to_str().unwrap_or_default()),
        ("RUNX_AGENT_PROVIDER", "anthropic"),
        ("RUNX_AGENT_MODEL", "claude-env"),
        ("ANTHROPIC_API_KEY", "anthropic-env-secret"),
    ]);
    let provider_config = load_managed_agent_config(&provider_env, temp.path())?;
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
    let local_config = update_runx_config_value(
        RunxConfigFile {
            agent: Some(RunxAgentConfig {
                provider: Some("anthropic".to_owned()),
                model: Some("claude-file".to_owned()),
                api_key_ref: None,
                ..RunxAgentConfig::default()
            }),
            public: None,
            credentials: None,
        },
        ConfigKey::AgentApiKey,
        "local-secret",
        &home_at(temp.path()),
    )?;
    write_runx_config_file(&temp.path().join("config.json"), &local_config)?;
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

    // A blank model is no model: the provider's default applies.
    let blank_model = env_map([
        ("RUNX_HOME", runx_home),
        ("RUNX_AGENT_PROVIDER", "anthropic"),
        ("RUNX_AGENT_MODEL", ""),
        ("RUNX_AGENT_API_KEY", "sk-explicit"),
    ]);
    assert_eq!(
        load_managed_agent_config(&blank_model, temp.path())?.map(|config| config.model),
        Some("claude-opus-5-5".to_owned())
    );

    let blank_generic_key = env_map([
        ("RUNX_HOME", runx_home),
        ("RUNX_AGENT_PROVIDER", "anthropic"),
        ("RUNX_AGENT_MODEL", "claude-env"),
        ("RUNX_AGENT_API_KEY", ""),
        ("ANTHROPIC_API_KEY", "provider-secret"),
    ]);
    assert_eq!(
        load_managed_agent_config(&blank_generic_key, temp.path())?
            .and_then(|config| config.api_key),
        Some(SecretString::new("local-secret"))
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
            provider: Some("openai".to_owned()),
            model: Some("qwen2.5".to_owned()),
            endpoint_url: Some(endpoint.to_owned()),
            auth_mode: Some("local_none".to_owned()),
            ..RunxAgentConfig::default()
        }),
        ..RunxConfigFile::default()
    };
    write_runx_config_file(&config_path, &config)?;
    let env = env_map([("RUNX_HOME", temp.path().to_str().unwrap_or_default())]);
    assert_eq!(
        load_managed_agent_config(&env, temp.path())?,
        Some(ManagedAgentConfig {
            provider: "openai".into(),
            dialect: ModelDialect::ChatCompletions,
            model: "qwen2.5".to_owned(),
            api_key: None,
            endpoint_url: endpoint.to_owned(),
            auth_mode: ManagedAgentAuthMode::LocalNone,
        })
    );

    let with_stored_key = update_runx_config_value(
        config.clone(),
        ConfigKey::AgentApiKey,
        "stored-api-key",
        &home_at(temp.path()),
    )?;
    write_runx_config_file(&config_path, &with_stored_key)?;
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
        update_runx_config_value(
            invalid.clone(),
            ConfigKey::AgentAuthMode,
            "typo",
            &home_at(temp.path())
        ),
        Err(ConfigError::InvalidAgentSetting {
            key: "agent.auth_mode",
            ..
        })
    ));
    let repaired = update_runx_config_value(
        invalid,
        ConfigKey::AgentAuthMode,
        "local_none",
        &home_at(temp.path()),
    )?;
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

fn home_at(home: &Path) -> RunxPaths {
    RunxPaths::resolve(
        &env_map([("RUNX_HOME", home.to_str().unwrap_or_default())]),
        home,
    )
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
