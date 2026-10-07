//! Local sessions that print a token: authority the person already holds in a
//! command-line tool they are signed into, such as `gh`.
//!
//! The token is read from the tool when an instance is confirmed and before
//! every call, delivered as the credential it stands in for, and never
//! stored. The principal it belongs to is read with it, and a call refuses
//! when that is no longer the principal the instance pinned.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use runx_contracts::JsonValue;
use runx_contracts::connector::{ConnectorManifest, CredentialName, SessionAuth, SessionPrincipal};
use runx_contracts::schema::NonEmptyString;

use crate::http::{HttpMethod, RuntimeHttpHeader, RuntimeHttpRequest, RuntimeHttpTransport};
use crate::process::{ProcessSpec, run_process};

/// A tool that prints more than this is not printing a token.
const TOKEN_OUTPUT_BYTES: usize = 64 * 1024;
const TOKEN_TIMEOUT: Duration = Duration::from_secs(20);

/// A connector's command session, as its manifest declares it.
pub(crate) struct CommandSession<'a> {
    connector: &'a str,
    command: &'a [NonEmptyString],
    pub credential: &'a CredentialName,
    principal: &'a SessionPrincipal,
}

/// Who a token belongs to.
pub(crate) struct Principal {
    pub id: String,
    pub login: String,
}

impl Principal {
    pub(crate) fn principal_ref(&self, connector: &str) -> String {
        format!("runx:principal:{connector}:{}", self.id)
    }
}

impl<'a> CommandSession<'a> {
    pub(crate) fn of(manifest: &'a ConnectorManifest) -> Option<Self> {
        match manifest.auth.session.as_ref()? {
            SessionAuth::Command {
                command,
                credential,
                principal,
            } => Some(Self {
                connector: manifest.id.as_str(),
                command,
                credential,
                principal,
            }),
            _ => None,
        }
    }

    /// Run the tool for its token.
    pub(crate) fn token(
        &self,
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<String, String> {
        let (program, args) = self.command.split_first().ok_or_else(|| {
            format!(
                "connector {} declares an empty session command",
                self.connector
            )
        })?;
        let program = program.as_str();
        let outcome = run_process(
            ProcessSpec::new("session token", program, TOKEN_OUTPUT_BYTES)
                .args(args.iter().map(|arg| arg.as_str().to_owned()).collect())
                .cwd(crate::config::resolve_runx_workspace_base(env, cwd))
                .env(
                    crate::process_invocation::process_base_environment(env)
                        .map_err(|error| error.to_string())?,
                )
                .timeout(Some(TOKEN_TIMEOUT)),
        )
        .map_err(|error| format!("{program} could not run: {error}"))?;
        if outcome.timed_out {
            return Err(format!(
                "{program} did not print a token within {}s",
                TOKEN_TIMEOUT.as_secs()
            ));
        }
        if matches!(outcome.status.code(), Some(126 | 127)) {
            return Err(format!("{program} is not installed or not on PATH"));
        }
        if !outcome.status.success() || outcome.stdout.truncated {
            return Err(format!(
                "{program} printed no token ({}); sign in to it and retry",
                outcome.status
            ));
        }
        let token = String::from_utf8(outcome.stdout.bytes)
            .map_err(|_| format!("{program} printed a token that is not text"))?;
        let token = token.trim();
        if token.is_empty() || token.contains(char::is_whitespace) {
            return Err(format!(
                "{program} printed no single token; sign in to it and retry"
            ));
        }
        Ok(token.to_owned())
    }

    /// Read who `token` belongs to.
    pub(crate) fn principal(
        &self,
        token: &str,
        transport: &dyn RuntimeHttpTransport,
    ) -> Result<Principal, String> {
        let response = transport
            .send_idempotent(RuntimeHttpRequest {
                method: HttpMethod::Get,
                url: self.principal.url.as_str().to_owned(),
                headers: vec![
                    RuntimeHttpHeader::new("accept", "application/json"),
                    RuntimeHttpHeader::new("authorization", format!("Bearer {token}")),
                ],
                body: None,
            })
            .map_err(|error| format!("reading the session's account failed: {error}"))?;
        if !(200..300).contains(&response.status) {
            return Err(format!(
                "{} answered HTTP {} for the session's token; sign in again",
                self.principal.url, response.status
            ));
        }
        let answer = serde_json::from_str::<JsonValue>(&response.body)
            .map_err(|_| "the session's account answer is not JSON".to_owned())?;
        let field = |path: &str| {
            super::operation::confirm::at_path(&answer, path)
                .and_then(|value| match value {
                    JsonValue::String(value) if !value.is_empty() => Some(value.clone()),
                    JsonValue::Number(value) => Some(value.to_string()),
                    _ => None,
                })
                .ok_or_else(|| format!("the session's account answer has no {path}"))
        };
        Ok(Principal {
            id: field(self.principal.id.as_str())?,
            login: field(self.principal.login.as_str())?,
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::os::unix::fs::PermissionsExt;

    use runx_contracts::connector::ConnectorManifest;

    use super::*;
    use crate::http::{RuntimeHttpError, RuntimeHttpResponse};

    const MANIFEST: &str = r#"
schema: runx.connector.v1
id: forge
name: Forge
description: A forge reached with the token its CLI prints.
auth:
  credentials:
    token:
      kind: api_key
      fields: [{name: token}]
      delivery: {env: FORGE_TOKEN}
      audience: [api.forge.test]
  session:
    kind: command
    command: [forge-cli, auth, token]
    credential: token
    principal: {url: "https://api.forge.test/user", id: id, login: login}
placements:
  local: {auth: [session, credentials]}
operations:
  user.read:
    description: Read the signed-in user.
    verb: read
    scopes: [user.read]
    credentials: [token]
    target: {template: "https://api.forge.test/user"}
"#;

    struct Account;

    impl RuntimeHttpTransport for Account {
        fn send(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
            let signed = request
                .headers
                .iter()
                .any(|header| header.name == "authorization" && header.value == "Bearer tok-123");
            Ok(match signed {
                true => RuntimeHttpResponse::new(200, r#"{"id": 7, "login": "octo"}"#),
                false => RuntimeHttpResponse::new(401, "{}"),
            })
        }
    }

    fn manifest() -> ConnectorManifest {
        runx_parser::parse_connector_manifest(MANIFEST).expect("manifest parses")
    }

    fn tool(dir: &Path, script: &str) -> BTreeMap<String, String> {
        let path = dir.join("forge-cli");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        BTreeMap::from([
            (
                "PATH".to_owned(),
                format!("{}:/usr/bin:/bin", dir.display()),
            ),
            ("RUNX_CWD".to_owned(), dir.display().to_string()),
        ])
    }

    #[test]
    fn a_signed_in_tool_yields_its_token_and_principal() {
        let dir = tempfile::tempdir().unwrap();
        let env = tool(dir.path(), "echo tok-123");
        let manifest = manifest();
        let session = CommandSession::of(&manifest).expect("a command session");
        let token = session.token(&env, dir.path()).expect("token");
        assert_eq!(token, "tok-123");
        let principal = session.principal(&token, &Account).expect("principal");
        assert_eq!(
            (principal.id.as_str(), principal.login.as_str()),
            ("7", "octo")
        );
        assert_eq!(principal.principal_ref("forge"), "runx:principal:forge:7");
        assert!(session.principal("other", &Account).is_err());
    }

    #[test]
    fn a_tool_that_is_signed_out_or_absent_yields_no_token() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = manifest();
        let session = CommandSession::of(&manifest).expect("a command session");
        let signed_out = tool(dir.path(), "echo 'not logged in' >&2; exit 1");
        assert!(
            session
                .token(&signed_out, dir.path())
                .unwrap_err()
                .contains("sign in")
        );
        let chatty = tool(dir.path(), "echo two tokens");
        assert!(session.token(&chatty, dir.path()).is_err());
        let absent = BTreeMap::from([
            ("PATH".to_owned(), "/nonexistent".to_owned()),
            ("RUNX_CWD".to_owned(), dir.path().display().to_string()),
        ]);
        assert!(session.token(&absent, dir.path()).is_err());
    }
}
