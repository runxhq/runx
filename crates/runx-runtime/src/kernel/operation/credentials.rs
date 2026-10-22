//! Local connector credentials: the secret an instance names (`runx
//! secret`), or the variable the connector's declaration delivers it in,
//! bound to a connector credential by an instance.
//!
//! A credential with one field is delivered as that field's value; a
//! credential with several as a JSON object of its fields. The declaration
//! fixes the field names, so a secret or a variable that does not hold them
//! is refused at binding and again at delivery.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

use crate::kernel::connectors::LoadedConnector;
use runx_contracts::connector::{
    ConnectorManifest, CredentialDeclaration, CredentialName, EnvName, SecretName,
};
use runx_contracts::{ConnectorInstance, InstanceAuthority, JsonValue};

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
    if crate::secrets::reference::looks_like_reference(value) {
        return Err(format!(
            "{variable} holds a reference to a secret, not the secret; set a secret to it with `runx secret set <name> --from <reference>` and bind that with `runx connector use {} --secret <name>`, or run under the tool that resolves it",
            connector.name
        ));
    }
    let fields = fields(declaration, value).map_err(|reason| format!("{variable} {reason}"))?;
    Ok(CredentialSecret::new(credential.clone(), fields))
}

/// Stands in for the proof while the proof is made: it covers the whole
/// binding except itself.
pub(crate) const UNPROVEN: &str = "unproven";

/// The proof a secret binding carries: an HMAC, keyed by this runx home,
/// over the whole binding as its binding digest sees it (connector revision,
/// placement, principal, secret, scopes, parameters, operations), with the
/// proof itself left out. Only a home that bound it can make it, so a
/// project store can neither name the operator's secret nor widen a binding.
pub(crate) fn binding_proof(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    instance: &ConnectorInstance,
) -> Result<String, String> {
    // A home the project could supply would let it supply the key too.
    if ["RUNX_HOME", "HOME", "USERPROFILE"]
        .iter()
        .all(|name| env.get(*name).is_none_or(|value| value.trim().is_empty()))
    {
        return Err(
            "a secret binding needs a runx home the operator owns; set RUNX_HOME or HOME"
                .to_owned(),
        );
    }
    let mut covered = instance.clone();
    let InstanceAuthority::Secret { proof, .. } = &mut covered.authority else {
        return Err(format!("{} binds no secret", instance.instance_id));
    };
    *proof = runx_contracts::schema::NonEmptyString::new(UNPROVEN).ok_or("an empty proof")?;
    let digest = covered
        .binding_digest()
        .map_err(|error| error.to_string())?;
    crate::secrets::prove(
        &RunxPaths::resolve(env, cwd),
        format!("runx.secret_binding.v1:{digest}").as_bytes(),
    )
    .map_err(|error| error.to_string())
}

/// Whether this runx home made `instance`'s secret binding, exactly as it
/// stands, and the secret it names is set. Admission checks this and reads
/// no value; every call checks it again before it reads one.
pub(crate) fn check_secret(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    instance: &ConnectorInstance,
) -> Result<(), String> {
    let InstanceAuthority::Secret { secret, proof, .. } = &instance.authority else {
        return Ok(());
    };
    let connector = &instance.connector;
    if binding_proof(env, cwd, instance)? != proof.as_str() {
        return Err(format!(
            "{connector} names the secret {secret} through a binding this runx home did not make, or that changed since; bind it with `runx connector use {connector} --secret {secret}`"
        ));
    }
    if !crate::secrets::is_set(&RunxPaths::resolve(env, cwd), secret)
        .map_err(|error| error.to_string())?
    {
        return Err(format!(
            "{connector} names the secret {secret}, which is not set; set it with `runx secret set {secret}`"
        ));
    }
    Ok(())
}

/// The credential a secret instance holds, read now, once its proof shows
/// this runx home bound it as it stands. Nothing is kept: every call reads
/// again, so a secret set anew serves from the next call.
pub(crate) fn from_secret(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    connector: &LoadedConnector,
    instance: &ConnectorInstance,
) -> Result<CredentialSecret, String> {
    check_secret(env, cwd, instance)?;
    let InstanceAuthority::Secret {
        credential, secret, ..
    } = &instance.authority
    else {
        return Err(format!("{} binds no secret", instance.instance_id));
    };
    let declaration = declaration(&connector.manifest, credential)?;
    let value = crate::secrets::read(env, cwd, secret, crate::secrets::Purpose::Call)
        .map_err(|error| error.to_string())?;
    let fields = secret_fields(declaration, secret, value.expose())?;
    Ok(CredentialSecret::new(credential.clone(), fields))
}

/// The fields the secret `secret`'s `value` fills. A whole JSON object
/// holding a one-field credential's field is where a manager keeps that
/// field, refused rather than sent whole as the key.
pub(crate) fn secret_fields(
    declaration: &CredentialDeclaration,
    secret: &SecretName,
    value: &str,
) -> Result<BTreeMap<String, String>, String> {
    if let [only] = declaration.fields.as_slice()
        && serde_json::from_str::<serde_json::Value>(value)
            .ok()
            .is_some_and(|value| value.get(only.name.as_str()).is_some())
    {
        return Err(format!(
            "the secret {secret} holds a JSON object with {name}; set it to that field alone, or to a reference ending #{name}",
            name = only.name
        ));
    }
    fields(declaration, value).map_err(|reason| format!("the secret {secret} {reason}"))
}

/// The refusal of a binding recorded before `runx secret`, naming the
/// command that binds it again.
pub(crate) fn bound_before_secrets(instance: &ConnectorInstance) -> String {
    let connector = &instance.connector;
    match &instance.authority {
        InstanceAuthority::Credential { profile, .. } => format!(
            "{connector} was bound to the credential profile {profile} before `runx secret`, which keeps it as the secret {profile}; bind it again: `runx connector use {connector} --secret {profile}`"
        ),
        InstanceAuthority::Reference { reference, .. } => format!(
            "{connector} was bound to {reference} before `runx secret`; set a secret to it and bind that: `runx secret set <name> --from {reference}`, then `runx connector use {connector} --secret <name>`"
        ),
        authority => format!(
            "{connector}'s {} binding was not recorded before `runx secret`",
            authority.kind()
        ),
    }
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
    use runx_contracts::connector::{CredentialName, SecretName};
    use runx_contracts::schema::NonEmptyString;

    /// An instance of fixture/search bound to the secret `exa`, with the
    /// proof the home at `env` makes for it.
    fn bound_to_exa(
        connector: &crate::LoadedConnector,
        env: &BTreeMap<String, String>,
        cwd: &std::path::Path,
    ) -> runx_contracts::ConnectorInstance {
        let mut instance = crate::kernel::instances::pinned(
            connector,
            runx_contracts::connector::InstanceId::new("secret.exa").unwrap(),
            runx_contracts::connector::Placement::Local,
            "runx:principal:local-secret:exa".to_owned(),
            None,
            InstanceAuthority::Secret {
                credential: CredentialName::new("api_key").unwrap(),
                secret: SecretName::new("exa").unwrap(),
                scopes: vec![NonEmptyString::new("search.read").unwrap()],
                proof: NonEmptyString::new(super::UNPROVEN).unwrap(),
            },
        );
        let made = super::binding_proof(env, cwd, &instance).unwrap();
        if let InstanceAuthority::Secret { proof, .. } = &mut instance.authority {
            *proof = NonEmptyString::new(made).unwrap();
        }
        instance
    }

    #[test]
    fn a_secret_this_home_bound_is_read_on_every_call_and_a_planted_one_never() {
        let connector = crate::kernel::connectors::fixture_scope()
            .connector("fixture/search")
            .unwrap();
        let home_env = |home: &std::path::Path| {
            BTreeMap::from([("RUNX_HOME".to_owned(), home.display().to_string())])
        };
        let home = tempfile::tempdir().unwrap();
        let env = home_env(home.path());
        let paths = crate::RunxPaths::resolve(&env, home.path());
        let exa = SecretName::new("exa").unwrap();
        crate::secrets::set_value(&paths, &exa, "first-key").unwrap();
        let call = |instance: &runx_contracts::ConnectorInstance| {
            super::from_secret(&env, home.path(), &connector, instance)
        };
        let bound = bound_to_exa(&connector, &env, home.path());
        assert_eq!(call(&bound).unwrap().field("api_key").unwrap(), "first-key");
        // A secret set again serves every binding that names it, without
        // binding again.
        crate::secrets::set_value(&paths, &exa, "rotated").unwrap();
        assert_eq!(call(&bound).unwrap().field("api_key").unwrap(), "rotated");

        // A project store carrying a binding another home made is refused.
        let other = tempfile::tempdir().unwrap();
        let planted = bound_to_exa(&connector, &home_env(other.path()), other.path());
        let refused = call(&planted).unwrap_err();
        assert!(
            refused.contains("through a binding this runx home did not make"),
            "{refused}"
        );
        // So is this home's binding with its grant widened in the store.
        let mut widened = bound.clone();
        if let InstanceAuthority::Secret { scopes, .. } = &mut widened.authority {
            scopes.push(NonEmptyString::new("crawl.write").unwrap());
        }
        assert!(
            call(&widened)
                .unwrap_err()
                .contains("or that changed since")
        );
        // With no home the operator owns, there is no key to prove with.
        assert!(
            super::binding_proof(&BTreeMap::new(), home.path(), &bound)
                .unwrap_err()
                .contains("needs a runx home the operator owns")
        );

        // A whole JSON object for a one-field credential is refused.
        crate::secrets::set_value(&paths, &exa, r#"{"api_key":"inside"}"#).unwrap();
        assert!(
            call(&bound)
                .unwrap_err()
                .contains("holds a JSON object with api_key")
        );

        // A secret removed since refuses before anything is read.
        crate::secrets::remove(&paths, &exa).unwrap();
        let removed = super::check_secret(&env, home.path(), &bound).unwrap_err();
        assert!(removed.contains("which is not set"), "{removed}");
    }

    #[test]
    fn a_secret_held_by_a_manager_is_read_there_on_every_call() {
        let connector = crate::kernel::connectors::fixture_scope()
            .connector("fixture/search")
            .unwrap();
        let held = tempfile::tempdir().unwrap();
        let file = held.path().join("exa.key");
        std::fs::write(&file, "key-from-the-manager\n").unwrap();
        let reference =
            runx_contracts::connector::SecretReference::new(format!("file://{}", file.display()))
                .unwrap();
        let home = tempfile::tempdir().unwrap();
        let env = BTreeMap::from([("RUNX_HOME".to_owned(), home.path().display().to_string())]);
        crate::secrets::set_reference(
            &env,
            home.path(),
            &SecretName::new("exa").unwrap(),
            &reference,
        )
        .unwrap();
        let bound = bound_to_exa(&connector, &env, home.path());
        let call = || super::from_secret(&env, home.path(), &connector, &bound);
        assert_eq!(
            call().unwrap().field("api_key").unwrap(),
            "key-from-the-manager"
        );
        // A secret rotated in the manager serves without setting it again.
        std::fs::write(&file, "rotated\n").unwrap();
        assert_eq!(call().unwrap().field("api_key").unwrap(), "rotated");
        // A failed read at call time refuses the call.
        std::fs::remove_file(&file).unwrap();
        assert!(call().unwrap_err().contains("cannot be read"));
    }

    #[test]
    fn a_binding_made_before_secrets_names_the_command_that_binds_it_again() {
        let connector = crate::kernel::connectors::fixture_scope()
            .connector("fixture/forum")
            .unwrap();
        let instance = crate::kernel::instances::pinned(
            &connector,
            runx_contracts::connector::InstanceId::new("credential.forum").unwrap(),
            runx_contracts::connector::Placement::Local,
            "runx:principal:local-credential:fixture/forum:forum".to_owned(),
            None,
            InstanceAuthority::Credential {
                profile: NonEmptyString::new("forum").unwrap(),
                credential: CredentialName::new("api_key").unwrap(),
                scopes: Vec::new(),
                material_ref: NonEmptyString::new("local_credential_1").unwrap(),
            },
        );
        assert_eq!(
            super::bound_before_secrets(&instance),
            "fixture/forum was bound to the credential profile forum before `runx secret`, which keeps it as the secret forum; bind it again: `runx connector use fixture/forum --secret forum`"
        );
    }

    #[test]
    fn an_environment_credential_is_read_from_its_variable_on_every_call() {
        let connector = crate::kernel::connectors::fixture_scope()
            .connector("fixture/search")
            .unwrap();
        let credential = CredentialName::new("api_key").unwrap();
        let variable = runx_contracts::connector::EnvName::new("SEARCH_API_KEY").unwrap();
        let read = |value: Option<&str>| {
            let env = value
                .map(|value| BTreeMap::from([("SEARCH_API_KEY".to_owned(), value.to_owned())]))
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
            refused.contains("reads its api_key credential from SEARCH_API_KEY, which is not set"),
            "{refused}"
        );
    }

    #[test]
    fn a_variable_the_shell_and_the_env_file_disagree_on_is_refused() {
        let connector = crate::kernel::connectors::fixture_scope()
            .connector("fixture/search")
            .unwrap();
        let credential = CredentialName::new("api_key").unwrap();
        let variable = runx_contracts::connector::EnvName::new("SEARCH_API_KEY").unwrap();
        let env = BTreeMap::from([
            ("SEARCH_API_KEY".to_owned(), "from-shell".to_owned()),
            (
                "RUNX_ENV_FILE_DIFFERS".to_owned(),
                "OTHER,SEARCH_API_KEY".to_owned(),
            ),
        ]);

        let refused = super::from_environment(&env, &connector, &credential, &variable)
            .err()
            .unwrap();

        assert_eq!(
            refused,
            "SEARCH_API_KEY is set in the environment and in the workspace .env with different values; unset one so fixture/search reads one api_key"
        );
    }
}
