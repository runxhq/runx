use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::{
    RunxCredentialProfile, load_runx_config_file, remove_local_credential_secret,
    resolve_runx_home_dir, store_local_credential_secret, write_runx_config_file,
};
use crate::credentials::credential_audience_host;
use crate::services::WorkspaceEnv;

use super::{CredentialBindingsFile, CredentialProfileSummary, SkillCredentialError};

const PROJECT_BINDINGS_PATH: &str = ".runx/credentials.json";

pub fn set_local_credential_profile(
    workspace: &WorkspaceEnv,
    name: &str,
    provider: &str,
    auth_mode: &str,
    audience: Option<&str>,
    secret: &str,
) -> Result<CredentialProfileSummary, SkillCredentialError> {
    let name = required(name, SkillCredentialError::EmptyProfileName)?;
    let provider = required(provider, SkillCredentialError::EmptyProvider)?;
    let auth_mode = required(auth_mode, SkillCredentialError::EmptyAuthMode)?;
    if secret.trim().is_empty() {
        return Err(SkillCredentialError::EmptySecret);
    }
    let requested_audience = audience
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            credential_audience_host(value)?;
            Ok::<_, SkillCredentialError>(value.to_owned())
        })
        .transpose()?;
    let config_dir = resolve_runx_home_dir(workspace.env(), workspace.cwd());
    let config_path = config_dir.join("config.json");
    let mut config = load_runx_config_file(&config_path)?;
    let credentials = config.credentials.get_or_insert_with(Default::default);
    let prior = credentials.profiles.get(name);
    let prior_ref = prior.map(|profile| profile.secret_ref.clone());
    let audience =
        requested_audience.or_else(|| prior.and_then(|profile| profile.audience.clone()));
    let secret_ref = store_local_credential_secret(&config_dir, secret)?;
    credentials.profiles.insert(
        name.to_owned(),
        RunxCredentialProfile {
            provider: provider.to_owned(),
            auth_mode: auth_mode.to_owned(),
            audience: audience.clone(),
            secret_ref,
        },
    );
    // Refresh provider-default pointers for this profile name. A pointer for
    // its previous provider would otherwise resolve to an incompatible profile.
    // Explicit project bindings and profile selection remain unchanged.
    credentials
        .defaults
        .retain(|default_provider, default_profile| {
            default_profile != name || default_provider == provider
        });
    credentials
        .defaults
        .insert(provider.to_owned(), name.to_owned());
    write_runx_config_file(&config_path, &config)?;
    if let Some(prior_ref) = prior_ref {
        remove_local_credential_secret(&config_dir, &prior_ref)?;
    }
    Ok(CredentialProfileSummary {
        name: name.to_owned(),
        provider: provider.to_owned(),
        auth_mode: auth_mode.to_owned(),
        audience,
        is_default: true,
    })
}

pub fn list_local_credential_profiles(
    workspace: &WorkspaceEnv,
) -> Result<Vec<CredentialProfileSummary>, SkillCredentialError> {
    let config_dir = resolve_runx_home_dir(workspace.env(), workspace.cwd());
    let config = load_runx_config_file(&config_dir.join("config.json"))?;
    let credentials = config.credentials.unwrap_or_default();
    Ok(credentials
        .profiles
        .into_iter()
        .map(|(name, profile)| CredentialProfileSummary {
            is_default: credentials.defaults.get(&profile.provider) == Some(&name),
            name,
            provider: profile.provider,
            auth_mode: profile.auth_mode,
            audience: profile.audience,
        })
        .collect())
}

pub fn remove_local_credential_profile(
    workspace: &WorkspaceEnv,
    name: &str,
) -> Result<bool, SkillCredentialError> {
    let name = required(name, SkillCredentialError::EmptyProfileName)?;
    let config_dir = resolve_runx_home_dir(workspace.env(), workspace.cwd());
    let config_path = config_dir.join("config.json");
    let mut config = load_runx_config_file(&config_path)?;
    let Some(credentials) = config.credentials.as_mut() else {
        return Ok(false);
    };
    let Some(profile) = credentials.profiles.remove(name) else {
        return Ok(false);
    };
    credentials.defaults.retain(|_, profile| profile != name);
    write_runx_config_file(&config_path, &config)?;
    remove_local_credential_secret(&config_dir, &profile.secret_ref)?;
    Ok(true)
}

pub fn bind_project_credential(
    workspace: &WorkspaceEnv,
    target: &str,
    profile: &str,
) -> Result<PathBuf, SkillCredentialError> {
    let target = required(target, SkillCredentialError::EmptyBindingTarget)?;
    let profile = required(profile, SkillCredentialError::EmptyProfileName)?;
    let config_dir = resolve_runx_home_dir(workspace.env(), workspace.cwd());
    let config = load_runx_config_file(&config_dir.join("config.json"))?;
    if !config
        .credentials
        .unwrap_or_default()
        .profiles
        .contains_key(profile)
    {
        return Err(SkillCredentialError::ProfileNotFound {
            profile: profile.to_owned(),
        });
    }
    let path = workspace.cwd().join(PROJECT_BINDINGS_PATH);
    let mut bindings = load_project_bindings(workspace.cwd())?;
    bindings
        .bindings
        .insert(target.to_owned(), profile.to_owned());
    write_project_bindings(path, &bindings)
}

pub fn bind_project_provider_transport(
    workspace: &WorkspaceEnv,
    provider: &str,
    transport: &str,
) -> Result<PathBuf, SkillCredentialError> {
    let provider = required(provider, SkillCredentialError::EmptyProvider)?;
    if provider.len() > 100
        || provider.chars().any(|character| {
            !character.is_ascii_lowercase()
                && !character.is_ascii_digit()
                && !matches!(character, '-' | '_' | '.')
        })
    {
        return Err(SkillCredentialError::InvalidProviderTransportBinding(
            "provider must be a bounded lowercase identifier".to_owned(),
        ));
    }
    let transport = required(transport, SkillCredentialError::EmptyBindingTarget)?;
    let transport = match transport {
        "auto" => "auto".to_owned(),
        "local" | "local:github" if provider == "github" => "local:github".to_owned(),
        "hosted" | "runx-connect" => "hosted".to_owned(),
        value if value.starts_with("hosted:") && value.len() > "hosted:".len() => {
            let grant_id = &value["hosted:".len()..];
            if !crate::path_util::is_safe_url_path_identifier(grant_id) {
                return Err(SkillCredentialError::InvalidProviderTransportBinding(
                    "hosted grant id must be a safe, non-empty identifier".to_owned(),
                ));
            }
            value.to_owned()
        }
        _ => {
            return Err(SkillCredentialError::InvalidProviderTransportBinding(
                "use auto, local for GitHub, hosted, or hosted:<grant-id>".to_owned(),
            ));
        }
    };
    let path = workspace.cwd().join(PROJECT_BINDINGS_PATH);
    let mut bindings = load_project_bindings(workspace.cwd())?;
    let key = format!("provider-transport:{provider}");
    if transport == "auto" {
        bindings.bindings.remove(&key);
    } else {
        bindings.bindings.insert(key, transport);
    }
    write_project_bindings(path, &bindings)
}

fn write_project_bindings(
    path: PathBuf,
    bindings: &CredentialBindingsFile,
) -> Result<PathBuf, SkillCredentialError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = serde_json::to_string_pretty(bindings).map_err(|error| {
        SkillCredentialError::InvalidBindings {
            path: path.clone(),
            message: error.to_string(),
        }
    })?;
    fs::write(&path, format!("{contents}\n"))?;
    Ok(path)
}

pub fn load_project_bindings(
    workspace_root: &Path,
) -> Result<CredentialBindingsFile, SkillCredentialError> {
    let path = workspace_root.join(PROJECT_BINDINGS_PATH);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CredentialBindingsFile {
                bindings: BTreeMap::new(),
            });
        }
        Err(error) => return Err(SkillCredentialError::Io(error)),
    };
    serde_json::from_str(&contents).map_err(|error| SkillCredentialError::InvalidBindings {
        path,
        message: error.to_string(),
    })
}

fn required(value: &str, error: SkillCredentialError) -> Result<&str, SkillCredentialError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(error);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use runx_parser::CredentialRequirement;

    use crate::credential_resolver::{
        SkillCredentialRequest, SkillCredentialResolution, SkillCredentialSource,
        resolve_skill_credential,
    };

    use super::{
        BTreeMap, WorkspaceEnv, load_runx_config_file, resolve_runx_home_dir,
        set_local_credential_profile,
    };

    fn github_request() -> SkillCredentialRequest {
        SkillCredentialRequest {
            skill_name: "audit-repo".to_owned(),
            requirement_name: "github".to_owned(),
            requirement: CredentialRequirement {
                provider: "github".to_owned(),
                audience: None,
                deliveries: BTreeMap::from([("token".to_owned(), "GITHUB_TOKEN".to_owned())]),
            },
            scopes: Vec::new(),
            explicit_profile: None,
        }
    }

    #[test]
    fn rebinding_a_profile_to_another_provider_drops_the_stale_default()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        let env = BTreeMap::from([
            ("RUNX_HOME".to_owned(), home.to_string_lossy().into_owned()),
            (
                "GITHUB_TOKEN".to_owned(),
                "github-environment-secret-sentinel".to_owned(),
            ),
        ]);
        let workspace = WorkspaceEnv::new(env, temp.path().to_path_buf())?;

        set_local_credential_profile(
            &workspace,
            "shared",
            "github",
            "token",
            None,
            "github-profile-secret-sentinel",
        )?;
        set_local_credential_profile(
            &workspace,
            "shared",
            "linear",
            "token",
            None,
            "linear-profile-secret-sentinel",
        )?;

        let config_dir = resolve_runx_home_dir(workspace.env(), workspace.cwd());
        let config = load_runx_config_file(&config_dir.join("config.json"))?;
        let defaults = config.credentials.unwrap_or_default().defaults;
        assert_eq!(defaults.get("linear").map(String::as_str), Some("shared"));
        assert_eq!(defaults.get("github"), None);

        let resolution = resolve_skill_credential(&github_request(), &workspace)?;
        let SkillCredentialResolution::Ready(resolved) = resolution else {
            return Err("github credential resolved to Missing".into());
        };
        assert_eq!(resolved.source, SkillCredentialSource::Environment);
        let descriptor = resolved
            .descriptor
            .ok_or("github environment credential has no descriptor")?;
        assert_eq!(descriptor.secret, "github-environment-secret-sentinel");

        let mut explicit = github_request();
        explicit.explicit_profile = Some("shared".to_owned());
        assert!(matches!(
            resolve_skill_credential(&explicit, &workspace),
            Err(crate::credential_resolver::SkillCredentialError::ProviderMismatch { .. })
        ));
        super::bind_project_credential(&workspace, "provider:github", "shared")?;
        assert!(matches!(
            resolve_skill_credential(&github_request(), &workspace),
            Err(crate::credential_resolver::SkillCredentialError::ProviderMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn profile_refresh_preserves_other_defaults_and_new_provider_resolution()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let workspace = WorkspaceEnv::new(
            BTreeMap::from([(
                "RUNX_HOME".to_owned(),
                temp.path().join("home").to_string_lossy().into_owned(),
            )]),
            temp.path().to_path_buf(),
        )?;
        set_local_credential_profile(
            &workspace,
            "shared",
            "github",
            "token",
            None,
            "first-sentinel",
        )?;
        set_local_credential_profile(
            &workspace,
            "shared",
            "github",
            "token",
            None,
            "rotated-sentinel",
        )?;
        let github = resolve_skill_credential(&github_request(), &workspace)?;
        assert!(
            matches!(github, SkillCredentialResolution::Ready(ref resolved)
            if resolved.source == SkillCredentialSource::GlobalDefault)
        );
        set_local_credential_profile(
            &workspace,
            "other",
            "github",
            "token",
            None,
            "other-sentinel",
        )?;
        set_local_credential_profile(
            &workspace,
            "shared",
            "linear",
            "token",
            None,
            "linear-sentinel",
        )?;
        let config_dir = resolve_runx_home_dir(workspace.env(), workspace.cwd());
        let config = load_runx_config_file(&config_dir.join("config.json"))?;
        let defaults = config.credentials.unwrap_or_default().defaults;
        assert_eq!(defaults.get("github").map(String::as_str), Some("other"));
        assert_eq!(defaults.get("linear").map(String::as_str), Some("shared"));
        let mut linear = github_request();
        linear.requirement.provider = "linear".to_owned();
        linear.requirement.deliveries =
            BTreeMap::from([("token".to_owned(), "LINEAR_TOKEN".to_owned())]);
        assert!(matches!(resolve_skill_credential(&linear, &workspace)?,
            SkillCredentialResolution::Ready(ref resolved)
                if resolved.source == SkillCredentialSource::GlobalDefault));
        Ok(())
    }

    #[test]
    fn rebound_profile_does_not_invent_a_missing_provider_credential()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let workspace = WorkspaceEnv::new(
            BTreeMap::from([(
                "RUNX_HOME".to_owned(),
                temp.path().join("home").to_string_lossy().into_owned(),
            )]),
            temp.path().to_path_buf(),
        )?;
        set_local_credential_profile(
            &workspace,
            "shared",
            "github",
            "token",
            None,
            "first-sentinel",
        )?;
        set_local_credential_profile(
            &workspace,
            "shared",
            "linear",
            "token",
            None,
            "second-sentinel",
        )?;
        assert!(matches!(
            resolve_skill_credential(&github_request(), &workspace)?,
            SkillCredentialResolution::Missing
        ));
        Ok(())
    }
}
