//! Local connector credentials: a named secret in the operator's credential
//! profiles, or the variable the connector's declaration delivers it in,
//! bound to a connector credential by an instance.
//!
//! A credential with one field is stored and delivered as that field's
//! value; a credential with several is stored and delivered as a JSON object
//! of its fields. The declaration fixes the field names, so a profile or a
//! variable that does not hold them is refused at binding and again at
//! delivery.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

use crate::kernel::connectors::LoadedConnector;
use runx_contracts::JsonValue;
use runx_contracts::connector::credential_declaration_digest;
use runx_contracts::connector::{
    ConnectorManifest, CredentialDeclaration, CredentialName, EnvName,
};

use crate::config::{load_local_credential_secret, load_runx_config_file};
use crate::credentials::{SecretString, SecretTaint};
use crate::paths::RunxPaths;

/// A delivered credential: which of the connector's credentials it is, and
/// its fields. Its `Debug` never shows the fields.
pub(crate) struct CredentialSecret {
    pub name: CredentialName,
    pub fields: BTreeMap<String, SecretString>,
    taint: RefCell<SecretTaint>,
}

impl std::fmt::Debug for CredentialSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialSecret")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl CredentialSecret {
    pub(crate) fn new(name: CredentialName, fields: BTreeMap<String, String>) -> Self {
        let taint = SecretTaint::from_values(fields.values().map(String::as_str));
        Self {
            name,
            fields: fields
                .into_iter()
                .map(|(name, value)| (name, SecretString::new(value)))
                .collect(),
            taint: RefCell::new(taint),
        }
    }

    pub(crate) fn field(&self, name: &str) -> Result<&str, String> {
        self.fields
            .get(name)
            .map(SecretString::expose)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("the credential has no {name}"))
    }

    pub(crate) fn presented(&self, value: &str) {
        self.taint.borrow_mut().include(value);
    }

    /// The same escape-safe boundary used for trusted process output.
    pub(crate) fn scrub(&self, value: JsonValue) -> JsonValue {
        self.taint.borrow().redact_value(value)
    }

    pub(crate) fn scrub_text(&self, text: impl Into<String>) -> String {
        self.taint.borrow().redact_output_text(text)
    }
}

/// The profile exists, was stored for this connector, and still holds the
/// material the instance was bound to.
pub(crate) fn check_profile(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    connector: &LoadedConnector,
    binding: &ProfileBinding<'_>,
) -> Result<(), String> {
    declaration(&connector.manifest, binding.credential)?;
    stored_profile(env, cwd, connector, binding).map(|_| ())
}

/// Load and check the secret an instance's credential delivers.
pub(crate) fn deliver(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    connector: &LoadedConnector,
    binding: &ProfileBinding<'_>,
) -> Result<CredentialSecret, String> {
    let declaration = declaration(&connector.manifest, binding.credential)?;
    let (paths, stored) = stored_profile(env, cwd, connector, binding)?;
    let profile = binding.profile;
    let value = SecretString::new(
        load_local_credential_secret(&paths, &stored.secret_ref)
            .map_err(|error| format!("credential profile {profile} cannot be read: {error}"))?,
    );
    let fields = fields(declaration, value.expose())
        .map_err(|reason| format!("credential profile {profile} {reason}"))?;
    Ok(CredentialSecret::new(binding.credential.clone(), fields))
}

/// Refuse `variable` when the environment and the workspace `.env` set it to
/// different values: either could be another account, and runx never picks
/// one. Every read of a connector credential's variable goes through here.
pub(crate) fn one_value(
    env: &BTreeMap<String, String>,
    connector: &LoadedConnector,
    credential: &CredentialName,
    variable: &EnvName,
) -> Result<(), String> {
    if crate::services::env_file_differs(env, variable.as_str()) {
        return Err(format!(
            "{variable} is set in the environment and in the workspace .env with different values; unset one so {} reads one {credential}",
            connector.name
        ));
    }
    Ok(())
}

/// The credential an environment instance holds, read now from `variable`
/// in the command's environment: the process's, filled from the workspace
/// `.env`. The instance pins the connector, so the variable is still the one
/// its declaration delivers the credential in. A variable the environment
/// and the `.env` set to different values is refused: either could be another
/// account, and runx never picks one.
pub(crate) fn from_environment(
    env: &BTreeMap<String, String>,
    connector: &LoadedConnector,
    credential: &CredentialName,
    variable: &EnvName,
) -> Result<CredentialSecret, String> {
    let declaration = declaration(&connector.manifest, credential)?;
    one_value(env, connector, credential, variable)?;
    let value = env
        .get(variable.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "{} reads its {credential} credential from {variable}, which is not set in the environment or the workspace .env",
                connector.name
            )
        })?;
    if crate::secret_refs::looks_like_reference(value) {
        return Err(format!(
            "{variable} holds a reference to a secret, not the secret; bind the reference with `runx connector use {} --from <reference>`, or run under the tool that resolves it",
            connector.name
        ));
    }
    let fields = fields(declaration, value).map_err(|reason| format!("{variable} {reason}"))?;
    Ok(CredentialSecret::new(credential.clone(), fields))
}

/// Stands in for the proof while the proof is made: it covers the whole
/// binding except itself.
const UNPROVEN: &str = "unproven";

/// The proof a reference instance carries: an HMAC, keyed by this runx home,
/// over the whole binding as its binding digest sees it (connector revision,
/// placement, principal, reference, pin, scopes, parameters, operations),
/// with the proof itself left out. Only a home that bound it can make it, so
/// a project store cannot supply or widen one.
pub(crate) fn reference_proof(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    instance: &runx_contracts::ConnectorInstance,
) -> Result<String, String> {
    // A home the project could supply would let it supply the key too.
    if ["RUNX_HOME", "HOME", "USERPROFILE"]
        .iter()
        .all(|name| env.get(*name).is_none_or(|value| value.trim().is_empty()))
    {
        return Err(
            "secret references need a runx home the operator owns; set RUNX_HOME or HOME"
                .to_owned(),
        );
    }
    let mut covered = instance.clone();
    let runx_contracts::InstanceAuthority::Reference { proof, .. } = &mut covered.authority else {
        return Err(format!(
            "{} is not a reference binding",
            instance.instance_id
        ));
    };
    *proof = runx_contracts::schema::NonEmptyString::new(UNPROVEN).ok_or("an empty proof")?;
    let digest = covered
        .binding_digest()
        .map_err(|error| error.to_string())?;
    crate::config::local_binding_proof(
        &RunxPaths::resolve(env, cwd),
        format!("runx.reference_binding.v1:{digest}").as_bytes(),
    )
    .map_err(|error| error.to_string())
}

/// Whether this runx home made `instance`'s reference binding, exactly as it
/// stands. Checked at admission and again before every read.
pub(crate) fn check_reference_proof(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    instance: &runx_contracts::ConnectorInstance,
) -> Result<(), String> {
    let runx_contracts::InstanceAuthority::Reference {
        reference, proof, ..
    } = &instance.authority
    else {
        return Ok(());
    };
    if reference_proof(env, cwd, instance)? == proof.as_str() {
        return Ok(());
    }
    Err(format!(
        "{} reads {reference} through a binding this runx home did not make, or that changed since; bind it with `runx connector use {} --from {reference}`",
        instance.connector, instance.connector
    ))
}

/// The credential a reference instance holds, read now from its secrets
/// manager at what binding pinned, once its proof shows this runx home bound
/// it as it stands. Nothing is kept: every call reads again.
pub(crate) fn from_reference(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    connector: &LoadedConnector,
    instance: &runx_contracts::ConnectorInstance,
) -> Result<CredentialSecret, String> {
    check_reference_proof(env, cwd, instance)?;
    let runx_contracts::InstanceAuthority::Reference {
        credential,
        reference,
        pinned,
        ..
    } = &instance.authority
    else {
        return Err(format!(
            "{} is not a reference binding",
            instance.instance_id
        ));
    };
    let declaration = declaration(&connector.manifest, credential)?;
    let read = crate::secret_refs::read(
        env,
        cwd,
        reference,
        pinned.as_ref().map(|pinned| pinned.as_str()),
        crate::secret_refs::Purpose::Call,
    )?;
    let fields = reference_fields(declaration, reference, &read.value)?;
    Ok(CredentialSecret::new(credential.clone(), fields))
}

pub(crate) const UNPROVEN_REFERENCE: &str = UNPROVEN;

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
    connector: &LoadedConnector,
    binding: &ProfileBinding<'_>,
) -> Result<(RunxPaths, crate::config::RunxCredentialProfile), String> {
    let profile = binding.profile;
    let paths = RunxPaths::resolve(env, cwd);
    let config = load_runx_config_file(&paths.config_file()).map_err(|error| error.to_string())?;
    let stored = config
        .credentials
        .unwrap_or_default()
        .profiles
        .remove(profile)
        .ok_or_else(|| format!("credential profile {profile} is not configured"))?;
    if stored.connector != connector.name
        || stored.credential != binding.credential.as_str()
    {
        return Err(format!(
            "credential profile {profile} holds {}'s {} credential, not {}'s {}",
            stored.connector, stored.credential, connector.name, binding.credential
        ));
    }
    let declaration = declaration(&connector.manifest, binding.credential)?;
    if stored.credential_declaration_digest
        != credential_declaration_digest(declaration).map_err(|error| error.to_string())?
    {
        return Err(format!(
            "credential profile {profile} was stored for a different credential declaration; set it again"
        ));
    }
    // The profile was set again since this instance was bound, here or in
    // another workspace: it may now hold another account.
    if stored.secret_ref != binding.material_ref {
        return Err(format!(
            "credential profile {profile} was replaced after {} was bound to it; bind it again here with `runx connector use {} {profile}`",
            connector.name, connector.name
        ));
    }
    Ok((paths, stored))
}

/// The stored value as the declaration's fields.
/// The fields a secret read through `reference` fills. A whole JSON object
/// bound to a one-field credential that names that field is a missing
/// `#field`, refused rather than sent whole as the key.
pub(crate) fn reference_fields(
    declaration: &CredentialDeclaration,
    reference: &runx_contracts::connector::SecretReference,
    value: &str,
) -> Result<BTreeMap<String, String>, String> {
    if let ([only], None) = (declaration.fields.as_slice(), reference.field())
        && serde_json::from_str::<serde_json::Value>(value)
            .ok()
            .is_some_and(|value| value.get(only.name.as_str()).is_some())
    {
        return Err(format!(
            "{reference} holds a JSON object with {name}; bind {reference}#{name}",
            name = only.name
        ));
    }
    fields(declaration, value).map_err(|reason| format!("{reference} {reason}"))
}

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

    #[test]
    fn connector_output_uses_the_native_escape_safe_secret_boundary() {
        let secret = super::CredentialSecret::new(
            runx_contracts::connector::CredentialName::new("key").unwrap(),
            BTreeMap::from([("api_key".to_owned(), "private-token".to_owned())]),
        );
        let text = r#"{"escaped":"private-\u0074oken","nested":"{\"token\":\"private-token\"}","base64":"cHJpdmF0ZS10b2tlbg=="}"#;
        let value: serde_json::Value = serde_json::from_str(&secret.scrub_text(text)).unwrap();
        assert_eq!(value["escaped"], "[redacted-credential]");
        assert_eq!(value["base64"], "[redacted-credential]");
        let nested: serde_json::Value =
            serde_json::from_str(value["nested"].as_str().unwrap()).unwrap();
        assert_eq!(nested["token"], "[redacted-credential]");
    }

    use runx_contracts::InstanceAuthority;
    use runx_contracts::connector::CredentialName;

    use super::{ProfileBinding, deliver};

    #[test]
    fn a_reference_this_home_bound_is_read_on_every_call_and_a_planted_one_never() {
        use runx_contracts::schema::NonEmptyString;
        let connector = crate::kernel::connectors::source_scope()
            .connector("exa")
            .unwrap();
        let held = tempfile::tempdir().unwrap();
        let secret = held.path().join("exa.key");
        std::fs::write(&secret, "key-from-the-manager\n").unwrap();
        let reference =
            runx_contracts::connector::SecretReference::new(format!("file://{}", secret.display()))
                .unwrap();
        let home_env = |home: &std::path::Path| {
            BTreeMap::from([("RUNX_HOME".to_owned(), home.display().to_string())])
        };
        let unproven = || {
            crate::kernel::instances::pinned(
                &connector,
                runx_contracts::connector::InstanceId::new("reference.exa").unwrap(),
                runx_contracts::connector::Placement::Local,
                "runx:principal:local-reference:exa".to_owned(),
                None,
                InstanceAuthority::Reference {
                    credential: CredentialName::new("api_key").unwrap(),
                    reference: reference.clone(),
                    pinned: None,
                    proof: NonEmptyString::new(super::UNPROVEN_REFERENCE).unwrap(),
                    scopes: vec![NonEmptyString::new("search.read").unwrap()],
                },
            )
        };
        let proven = |env: &BTreeMap<String, String>, cwd: &std::path::Path| {
            let mut instance = unproven();
            let made = super::reference_proof(env, cwd, &instance).unwrap();
            if let InstanceAuthority::Reference { proof, .. } = &mut instance.authority {
                *proof = NonEmptyString::new(made).unwrap();
            }
            instance
        };
        let home = tempfile::tempdir().unwrap();
        let env = home_env(home.path());
        let call = |instance: &runx_contracts::ConnectorInstance| {
            super::from_reference(&env, home.path(), &connector, instance)
        };
        let bound = proven(&env, home.path());
        assert_eq!(
            call(&bound).unwrap().field("api_key").unwrap(),
            "key-from-the-manager"
        );
        // A secret rotated in the manager serves without binding again.
        std::fs::write(&secret, "rotated\n").unwrap();
        assert_eq!(call(&bound).unwrap().field("api_key").unwrap(), "rotated");

        // A project store carrying a binding another home made is refused.
        let other = tempfile::tempdir().unwrap();
        let planted = proven(&home_env(other.path()), other.path());
        assert!(
            call(&planted)
                .unwrap_err()
                .contains("through a binding this runx home did not make")
        );
        // So is this home's binding with its grant widened in the store.
        let mut widened = bound.clone();
        if let InstanceAuthority::Reference { scopes, .. } = &mut widened.authority {
            scopes.push(NonEmptyString::new("crawl.write").unwrap());
        }
        assert!(
            call(&widened)
                .unwrap_err()
                .contains("or that changed since")
        );
        // With no home the operator owns, there is no key to prove with.
        assert!(
            super::reference_proof(&BTreeMap::new(), home.path(), &unproven())
                .unwrap_err()
                .contains("need a runx home the operator owns")
        );

        // A whole JSON object for a one-field credential is a missing #field.
        std::fs::write(&secret, r#"{"api_key":"inside"}"#).unwrap();
        assert!(
            call(&bound)
                .unwrap_err()
                .contains("holds a JSON object with api_key; bind file://")
        );

        // A failed read at call time refuses the call.
        std::fs::remove_file(&secret).unwrap();
        assert!(call(&bound).unwrap_err().contains("cannot be read"));
    }

    #[test]
    fn an_environment_credential_is_read_from_its_variable_on_every_call() {
        let connector = crate::kernel::connectors::source_scope()
            .connector("exa")
            .unwrap();
        let credential = CredentialName::new("api_key").unwrap();
        let variable = runx_contracts::connector::EnvName::new("EXA_API_KEY").unwrap();
        let read = |value: Option<&str>| {
            let env = value
                .map(|value| BTreeMap::from([("EXA_API_KEY".to_owned(), value.to_owned())]))
                .unwrap_or_default();
            super::from_environment(&env, &connector, &credential, &variable)
        };
        assert_eq!(
            read(Some("first")).unwrap().field("api_key").unwrap(),
            "first"
        );
        // A key rotated in the environment serves without binding again.
        assert_eq!(
            read(Some("second")).unwrap().field("api_key").unwrap(),
            "second"
        );
        let refused = read(None).unwrap_err();
        assert!(
            refused.contains("reads its api_key credential from EXA_API_KEY, which is not set"),
            "{refused}"
        );
    }

    #[test]
    fn a_variable_the_shell_and_the_env_file_disagree_on_is_refused() {
        let connector = crate::kernel::connectors::source_scope()
            .connector("exa")
            .unwrap();
        let credential = CredentialName::new("api_key").unwrap();
        let variable = runx_contracts::connector::EnvName::new("EXA_API_KEY").unwrap();
        let env = BTreeMap::from([
            ("EXA_API_KEY".to_owned(), "from-shell".to_owned()),
            (
                "RUNX_ENV_FILE_DIFFERS".to_owned(),
                "OTHER,EXA_API_KEY".to_owned(),
            ),
        ]);

        let refused = super::from_environment(&env, &connector, &credential, &variable)
            .err()
            .unwrap();

        assert_eq!(
            refused,
            "EXA_API_KEY is set in the environment and in the workspace .env with different values; unset one so exa reads one api_key"
        );
    }

    /// Two workspaces share one runx home. Binding a profile name in the
    /// second must not move the first workspace's instance to its account.
    #[test]
    fn a_profile_replaced_elsewhere_no_longer_serves_the_first_binding() {
        let root = tempfile::tempdir().unwrap();
        let connector = crate::kernel::connectors::source_scope()
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
                    crate::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
                    root.path().join(workspace).to_string_lossy().into_owned(),
                ),
            ]);
            let instance = crate::kernel::setup::set_credential(
                &crate::Instances::new(
                    crate::KernelStore::for_project(&crate::paths::RunxPaths::resolve(
                        &env,
                        root.path(),
                    ))
                    .unwrap(),
                ),
                &connector,
                "moltbook",
                &credential,
                secret,
                &[],
                runx_contracts::JsonObject::new(),
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
        let secret = deliver(&first, root.path(), &connector, &binding).unwrap();
        assert_eq!(secret.field("api_key").unwrap(), "first-account-key");

        let (_, second_ref) = bind("b", "second-account-key");
        let refused = deliver(&first, root.path(), &connector, &binding).unwrap_err();
        assert!(refused.contains("was replaced"), "{refused}");
        assert_ne!(first_ref, second_ref);
    }
}
