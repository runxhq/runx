//! Local connector credentials: a named secret in the operator's credential
//! profiles, bound to a connector credential by an instance.
//!
//! A credential with one field is stored and delivered as that field's
//! value; a credential with several is stored and delivered as a JSON object
//! of its fields. The declaration fixes the field names, so a profile that
//! does not hold them is refused at binding and again at delivery.

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::JsonValue;
use runx_contracts::connector::{ConnectorManifest, CredentialDeclaration, CredentialName};

use crate::config::{load_local_credential_secret, load_runx_config_file, resolve_runx_home_dir};

/// A delivered credential: which of the connector's credentials it is, and
/// its fields. Its `Debug` never shows the fields.
pub(crate) struct CredentialSecret {
    pub name: CredentialName,
    pub fields: BTreeMap<String, String>,
}

impl std::fmt::Debug for CredentialSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialSecret")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// What replaces a credential field a provider answer repeats.
pub(crate) const REDACTED: &str = "[redacted-credential]";

impl CredentialSecret {
    /// `value` with every field of the credential it carries replaced, so a
    /// provider that echoes the secret in an answer never puts it in custody
    /// or a receipt.
    pub(crate) fn scrub(&self, value: JsonValue) -> JsonValue {
        let secrets = self
            .fields
            .values()
            .filter(|secret| secret.len() >= 4)
            .collect::<Vec<_>>();
        if secrets.is_empty() {
            return value;
        }
        scrub_value(value, &secrets)
    }
}

fn scrub_value(value: JsonValue, secrets: &[&String]) -> JsonValue {
    match value {
        JsonValue::String(text) => JsonValue::String(
            secrets
                .iter()
                .fold(text, |text, secret| text.replace(secret.as_str(), REDACTED)),
        ),
        JsonValue::Array(values) => JsonValue::Array(
            values
                .into_iter()
                .map(|value| scrub_value(value, secrets))
                .collect(),
        ),
        JsonValue::Object(object) => JsonValue::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, scrub_value(value, secrets)))
                .collect(),
        ),
        other => other,
    }
}

/// The profile exists, was stored for this connector, and still holds the
/// material the instance was bound to.
pub(crate) fn check_profile(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    connector: &ConnectorManifest,
    binding: &ProfileBinding<'_>,
) -> Result<(), String> {
    declaration(connector, binding.credential)?;
    stored_profile(env, cwd, connector, binding).map(|_| ())
}

/// Load and check the secret an instance's credential delivers.
pub(crate) fn deliver(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    connector: &ConnectorManifest,
    binding: &ProfileBinding<'_>,
) -> Result<CredentialSecret, String> {
    let declaration = declaration(connector, binding.credential)?;
    let (config_dir, stored) = stored_profile(env, cwd, connector, binding)?;
    let profile = binding.profile;
    let value = load_local_credential_secret(&config_dir, &stored.secret_ref)
        .map_err(|error| format!("credential profile {profile} cannot be read: {error}"))?;
    let fields = fields(declaration, &value)
        .map_err(|reason| format!("credential profile {profile} {reason}"))?;
    Ok(CredentialSecret {
        name: binding.credential.clone(),
        fields,
    })
}

/// What an instance's credential authority names: the profile, the
/// connector credential it holds, and the stored material it was bound to.
pub(crate) struct ProfileBinding<'a> {
    pub profile: &'a str,
    pub credential: &'a CredentialName,
    pub material_ref: &'a str,
}

fn stored_profile(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    connector: &ConnectorManifest,
    binding: &ProfileBinding<'_>,
) -> Result<(std::path::PathBuf, crate::config::RunxCredentialProfile), String> {
    let profile = binding.profile;
    let config_dir = resolve_runx_home_dir(env, cwd);
    let config = load_runx_config_file(&config_dir.join("config.json"))
        .map_err(|error| error.to_string())?;
    let stored = config
        .credentials
        .unwrap_or_default()
        .profiles
        .remove(profile)
        .ok_or_else(|| format!("credential profile {profile} is not configured"))?;
    if stored.connector != connector.id.as_str() || stored.credential != binding.credential.as_str()
    {
        return Err(format!(
            "credential profile {profile} holds {}'s {} credential, not {}'s {}",
            stored.connector, stored.credential, connector.id, binding.credential
        ));
    }
    // The profile was set again since this instance was bound, here or in
    // another workspace: it may now hold another account.
    if stored.secret_ref != binding.material_ref {
        return Err(format!(
            "credential profile {profile} was replaced after {} was bound to it; bind it again here with `runx connector use {} --profile {profile}`",
            connector.id, connector.id
        ));
    }
    Ok((config_dir, stored))
}

/// The stored value as the declaration's fields.
pub(crate) fn fields(
    declaration: &CredentialDeclaration,
    value: &str,
) -> Result<BTreeMap<String, String>, &'static str> {
    if value.trim().is_empty() {
        return Err("is empty");
    }
    let declared = declaration.fields.as_slice();
    if let [only] = declared {
        return Ok(BTreeMap::from([(
            only.name.as_str().to_owned(),
            value.to_owned(),
        )]));
    }
    let object: BTreeMap<String, String> =
        serde_json::from_str(value).map_err(|_| "must hold a JSON object of its fields")?;
    for field in declared {
        let present = object
            .get(field.name.as_str())
            .is_some_and(|value| !value.is_empty());
        if field.required && !present {
            return Err("is missing a required field");
        }
    }
    if object
        .keys()
        .any(|key| !declared.iter().any(|field| field.name.as_str() == key))
    {
        return Err("holds a field its declaration does not name");
    }
    Ok(object)
}

fn declaration<'a>(
    connector: &'a ConnectorManifest,
    credential: &CredentialName,
) -> Result<&'a CredentialDeclaration, String> {
    connector.auth.credentials.get(credential).ok_or_else(|| {
        format!(
            "connector {} declares no credential {credential}",
            connector.id
        )
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::collections::BTreeMap;

    use runx_contracts::InstanceAuthority;
    use runx_contracts::connector::CredentialName;

    use super::{ProfileBinding, deliver};

    /// Two workspaces share one runx home. Binding a profile name in the
    /// second must not move the first workspace's instance to its account.
    #[test]
    fn a_profile_replaced_elsewhere_no_longer_serves_the_first_binding() {
        let root = tempfile::tempdir().unwrap();
        let connector = crate::ConnectorScope::official_only()
            .unwrap()
            .connector("moltbook")
            .unwrap();
        let credential = CredentialName::new("api_key").unwrap();
        let bind = |workspace: &str, secret: &str| {
            let env = BTreeMap::from([
                (
                    "RUNX_HOME".to_owned(),
                    root.path().join("home").to_string_lossy().into_owned(),
                ),
                (
                    crate::receipts::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
                    root.path().join(workspace).to_string_lossy().into_owned(),
                ),
            ]);
            let instance = crate::kernel::setup::set_credential(
                &crate::Instances::new(
                    crate::KernelStore::for_workspace(&env, root.path()).unwrap(),
                ),
                &connector,
                "moltbook",
                &credential,
                secret,
                &[],
                &crate::services::WorkspaceEnv::new(env.clone(), root.path().to_path_buf())
                    .unwrap(),
            )
            .unwrap();
            let material_ref = match instance.authority {
                InstanceAuthority::Credential { material_ref, .. } => Some(material_ref),
                _ => None,
            };
            (env, material_ref.expect("a credential binding"))
        };
        let (first, first_ref) = bind("a", "first-account-key");
        let binding = ProfileBinding {
            profile: "moltbook",
            credential: &credential,
            material_ref: first_ref.as_str(),
        };
        let secret = deliver(&first, root.path(), &connector.manifest, &binding).unwrap();
        assert_eq!(secret.fields.get("api_key").unwrap(), "first-account-key");

        let (_, second_ref) = bind("b", "second-account-key");
        let refused = deliver(&first, root.path(), &connector.manifest, &binding).unwrap_err();
        assert!(refused.contains("was replaced"), "{refused}");
        assert_ne!(first_ref, second_ref);
    }
}
