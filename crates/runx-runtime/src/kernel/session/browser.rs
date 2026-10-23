//! A browser session's account, read through the connector's installed
//! session tool: the tool reports the account the signed-in profile
//! renders, setup pins it, and every operation checks it again before
//! dispatch. No cookie, token or browser lifecycle passes through runx.

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::connector::SessionAuth;
use runx_contracts::{JsonObject, JsonValue};

use super::Principal;
use crate::kernel::connectors::LoadedConnector;
use crate::kernel::operation::ToolRunner;

/// Who the browser profile `parameters` name is signed in as, by its
/// session tool run through `tools` with `operation: session`.
pub(crate) fn principal(
    connector: &LoadedConnector,
    parameters: &JsonObject,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    tools: &dyn ToolRunner,
) -> Result<Principal, String> {
    let Some(SessionAuth::HostBrowser { process }) = connector.manifest.auth.session.as_ref()
    else {
        return Err("the connector declares no browser session".to_owned());
    };
    let tool = process.tool.as_str();
    let manifest = connector
        .tools
        .get(tool)
        .ok_or_else(|| format!("the connector ships no session tool {tool}"))?
        .installed(&crate::RunxPaths::resolve(env, cwd), tool)?;
    let input = JsonObject::from([
        (
            "operation".to_owned(),
            JsonValue::String("session".to_owned()),
        ),
        (
            "parameters".to_owned(),
            JsonValue::Object(parameters.clone()),
        ),
    ]);
    let scopes = process
        .runtime_scopes
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let answer = tools.run(
        tool,
        &manifest,
        input,
        &scopes,
        &crate::credentials::CredentialDelivery::none(),
    )?;
    Principal::read(
        &answer,
        "id",
        "login",
        "the browser session's account answer",
    )
}

/// A browser operation uses the account the operator confirmed, never a
/// sibling profile signed in since.
pub(crate) fn verify(
    connector: &LoadedConnector,
    instance: &runx_contracts::ConnectorInstance,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    tools: &dyn ToolRunner,
) -> Result<(), String> {
    let principal = principal(connector, &instance.parameters, env, cwd, tools)?;
    if principal.principal_uri(connector.manifest.id.as_str()) != instance.principal_uri.as_str()
        || instance.account_id.as_ref().map(|value| value.as_str())
            != Some(principal.login.as_str())
    {
        return Err("the browser session is now signed in as another account; confirm the intended session again".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::KernelStore;
    use crate::credentials::CredentialDelivery;
    use crate::http::{
        RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
    };
    use crate::kernel::instances::Instances;
    use crate::kernel::setup::{AccountReaders, confirm_session_over};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A browser session exports no token and calls no identity API: any
    /// request is counted and refused.
    #[derive(Default)]
    struct NoNetwork {
        sent: Cell<usize>,
    }

    impl RuntimeHttpTransport for NoNetwork {
        fn send(&self, _: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
            self.sent.set(self.sent.get() + 1);
            Err(RuntimeHttpError::Transport {
                message: "a browser session sends no request".to_owned(),
            })
        }
    }

    /// The session tool, answering `operation: session` with `answer`.
    struct Proof(&'static str);

    impl ToolRunner for Proof {
        fn run(
            &self,
            tool: &str,
            _: &Path,
            input: JsonObject,
            _: &[String],
            _: &CredentialDelivery,
        ) -> Result<JsonValue, String> {
            if tool != "browser.probe"
                || input.get("operation").and_then(JsonValue::as_str) != Some("session")
            {
                return Err(format!("unexpected call to {tool}"));
            }
            serde_json::from_str(self.0).map_err(|error| error.to_string())
        }
    }

    #[test]
    fn browser_session_pins_and_rechecks_identity_and_rejects_unknown_scopes() -> TestResult {
        let root = tempfile::tempdir()?;
        let env = BTreeMap::from([
            (
                "RUNX_HOME".to_owned(),
                root.path().join("home").display().to_string(),
            ),
            (
                "RUNX_PROJECT_DIR".to_owned(),
                root.path().join("project").display().to_string(),
            ),
        ]);
        let connector = crate::kernel::connectors::fixture_scope().connector("fixture/browser")?;
        let instances = Instances::new(KernelStore::for_project(&crate::RunxPaths::resolve(
            &env,
            root.path(),
        ))?);
        let parameters: JsonObject = serde_json::from_str(r#"{"account":"alpha"}"#)?;
        let network = NoNetwork::default();
        let alpha = Proof(r#"{"id":"stable-alpha","login":"alpha"}"#);
        let confirm = |scopes: &[String], tools: &dyn ToolRunner| {
            confirm_session_over(
                &instances,
                &connector,
                scopes,
                parameters.clone(),
                &AccountReaders {
                    transport: &network,
                    tools,
                },
                &env,
                root.path(),
            )
        };

        let instance = confirm(&[], &alpha)?;
        assert_eq!(
            instance.account_id.as_ref().map(|value| value.as_str()),
            Some("alpha")
        );
        verify(&connector, &instance, &env, root.path(), &alpha)?;

        let beta = Proof(r#"{"id":"stable-beta","login":"beta"}"#);
        let switched = verify(&connector, &instance, &env, root.path(), &beta)
            .err()
            .ok_or("a sibling account passed")?;
        assert!(switched.contains("another account"), "{switched}");

        let unnamed = Proof(r#"{"login":"alpha"}"#);
        let refused = verify(&connector, &instance, &env, root.path(), &unnamed)
            .err()
            .ok_or("an answer naming no id passed")?;
        assert!(refused.contains("has no id"), "{refused}");

        let unserved = confirm(&["browser.write".to_owned()], &alpha)
            .err()
            .ok_or("an undeclared scope was granted")?;
        assert!(
            unserved.contains("serves no operation with scope browser.write"),
            "{unserved}"
        );
        assert_eq!(network.sent.get(), 0);
        Ok(())
    }
}
