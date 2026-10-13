#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use runx_contracts::connector::CredentialName;
use runx_contracts::{InstanceAuthority, JsonObject};
use serde_json::{Value, json};

use super::{ConnectorSetup, UseOptions, UseOutcome, UseRequest};
use crate::http::{HttpMethod, RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse};
use crate::kernel::connectors::{LoadedConnector, source_scope};
use crate::kernel::instances::Instances;
use crate::kernel::store::KernelStore;
use crate::services::WorkspaceEnv;

const BASE_URL: &str = "https://connect-fixture.runx.invalid";

/// A runx Connect that opens one session: `start` answers the opening,
/// `statuses` answer each status read in turn, and a credential submission is
/// answered with the session's grant. Every grant it issues is listed.
#[derive(Clone)]
struct Connect {
    start: Value,
    statuses: Arc<Mutex<VecDeque<Value>>>,
    grants: Arc<Mutex<Vec<Value>>>,
    sent: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Connect {
    fn new(start: Value, statuses: &[Value]) -> Self {
        Self {
            start,
            statuses: Arc::new(Mutex::new(statuses.iter().cloned().collect())),
            grants: Arc::new(Mutex::new(Vec::new())),
            sent: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// List `grant_id` as an active grant of every scope for `connector`.
    fn issue(&self, grant_id: &str, connector: &LoadedConnector) -> Value {
        self.grants.lock().unwrap().push(json!({
            "grant_id": grant_id,
            "provider": connector.manifest.id.as_str(),
            "connector_binding": connector.binding(),
            "status": "active",
            "scopes": connector
                .manifest
                .operations
                .values()
                .flat_map(|operation| operation.scopes.iter().map(|scope| scope.as_str()))
                .collect::<Vec<_>>(),
        }));
        json!({"status": "created", "grant": {"grant_id": grant_id}})
    }

    fn sent(&self, path: &str) -> Vec<Value> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(sent, _)| sent == path)
            .map(|(_, body)| body.clone())
            .collect()
    }
}

impl crate::http::RuntimeHttpTransport for Connect {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        let path = request
            .url
            .strip_prefix(BASE_URL)
            .unwrap_or_default()
            .to_owned();
        let body = request
            .body
            .as_deref()
            .map_or(Value::Null, |body| serde_json::from_str(body).unwrap());
        self.sent.lock().unwrap().push((path.clone(), body));
        let answer = match (request.method, path.as_str()) {
            (HttpMethod::Get, "/v1/me") => {
                json!({"status": "success", "principal": {"principal_id": "operator-1"}})
            }
            (HttpMethod::Get, "/v1/grants") => {
                json!({"status": "success", "grants": *self.grants.lock().unwrap()})
            }
            (HttpMethod::Post, "/v1/connect/sessions") => self.start.clone(),
            (HttpMethod::Post, "/v1/connect/sessions/sess_1/credentials") => {
                self.statuses.lock().unwrap().pop_front().unwrap()
            }
            (HttpMethod::Get, "/v1/connect/sessions/sess_1") => {
                self.statuses.lock().unwrap().pop_front().unwrap()
            }
            _ => return Ok(RuntimeHttpResponse::new(404, r#"{"status":"error"}"#)),
        };
        Ok(RuntimeHttpResponse::new(200, answer.to_string()))
    }
}

struct Project {
    /// Held so the project outlives the test.
    _root: tempfile::TempDir,
    workspace: WorkspaceEnv,
    instances: Instances,
}

impl Project {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let env = BTreeMap::from([
            (
                crate::HOSTED_API_BASE_URL_ENV.to_owned(),
                BASE_URL.to_owned(),
            ),
            (
                crate::HOSTED_API_TOKEN_ENV.to_owned(),
                "rxk_test".to_owned(),
            ),
        ]);
        let instances = Instances::new(
            KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, root.path())).unwrap(),
        );
        let workspace = WorkspaceEnv::new(env, root.path().to_path_buf()).unwrap();
        Self {
            _root: root,
            workspace,
            instances,
        }
    }

    fn setup(&self, connect: &Connect) -> ConnectorSetup<'_> {
        ConnectorSetup::with_transport(&self.instances, &self.workspace, Arc::new(connect.clone()))
    }
}

fn options(wait: Duration) -> UseOptions {
    UseOptions {
        scopes: Vec::new(),
        parameters: JsonObject::new(),
        wait,
    }
}

fn awaiting_authorization() -> Value {
    json!({
        "status": "pending",
        "session_id": "sess_1",
        "authorization_url": "https://connect.runx.invalid/authorize/sess_1",
    })
}

fn pending() -> Value {
    json!({"status": "pending", "session_id": "sess_1", "poll_after_ms": 1})
}

fn grant_of(outcome: UseOutcome) -> Result<String, String> {
    match outcome {
        UseOutcome::Selected(instance) => match instance.authority {
            InstanceAuthority::HostedGrant { grant_id } => Ok(grant_id.as_str().to_owned()),
            other => Err(format!("selected {other:?}, not a hosted grant")),
        },
        UseOutcome::Pending { finish, .. } => Err(format!("still pending: {finish}")),
    }
}

#[test]
fn a_hosted_grant_binds_once_the_account_holder_authorizes() {
    let slack = source_scope().connector("runx/slack").unwrap();
    let connect = Connect::new(awaiting_authorization(), &[pending()]);
    let created = connect.issue("grant_slack_1", &slack);
    connect.statuses.lock().unwrap().push_back(created);
    let project = Project::new();

    let outcome = project
        .setup(&connect)
        .use_connector(
            &slack,
            UseRequest::Hosted {
                credential: None,
                target: None,
            },
            &options(Duration::from_secs(30)),
        )
        .unwrap();

    assert_eq!(grant_of(outcome).unwrap(), "grant_slack_1");
    // A new grant asks for every scope the connector's operations declare.
    let started = connect.sent("/v1/connect/sessions");
    let mut declared = slack
        .manifest
        .operations
        .values()
        .flat_map(|operation| {
            operation
                .scopes
                .iter()
                .map(|scope| scope.as_str().to_owned())
        })
        .collect::<Vec<_>>();
    declared.sort();
    declared.dedup();
    assert_eq!(started[0]["provider"], "slack");
    assert_eq!(started[0]["scopes"], json!(declared));
    assert_eq!(connect.sent("/v1/connect/sessions/sess_1").len(), 2);
}

#[test]
fn an_unauthorized_grant_names_the_command_that_finishes_it() -> Result<(), String> {
    let slack = source_scope().connector("runx/slack").unwrap();
    let connect = Connect::new(awaiting_authorization(), &[]);
    let project = Project::new();

    let outcome = project
        .setup(&connect)
        .use_connector(
            &slack,
            UseRequest::Hosted {
                credential: None,
                target: Some("C0123"),
            },
            &options(Duration::ZERO),
        )
        .unwrap();
    let UseOutcome::Pending {
        session_id,
        authorization_url,
        finish,
    } = outcome
    else {
        return Err("a grant not yet authorized must be pending".to_owned());
    };
    assert_eq!(session_id, "sess_1");
    assert_eq!(
        authorization_url.as_deref(),
        Some("https://connect.runx.invalid/authorize/sess_1")
    );
    assert_eq!(finish, "runx connector use runx/slack sess_1");
    // The target goes as one locator; Connect reads it per provider.
    let started = connect.sent("/v1/connect/sessions");
    assert_eq!(started[0]["target_locator"], "C0123");
    assert!(started[0].get("target_repo").is_none());

    // Once authorized, the printed command binds the grant the session made.
    let created = connect.issue("grant_slack_1", &slack);
    connect.statuses.lock().unwrap().push_back(created);
    let finished = project
        .setup(&connect)
        .use_connector(
            &slack,
            UseRequest::Existing("sess_1"),
            &options(Duration::ZERO),
        )
        .unwrap();
    assert_eq!(grant_of(finished)?, "grant_slack_1");
    Ok(())
}

#[test]
fn a_hosted_credential_is_submitted_to_the_session_it_opens() {
    let aws = source_scope().connector("runx/aws").unwrap();
    let connect = Connect::new(
        json!({"status": "credential_required", "session_id": "sess_1"}),
        &[],
    );
    let created = connect.issue("grant_aws_1", &aws);
    connect.statuses.lock().unwrap().push_back(created);
    let project = Project::new();
    let secret = json!({
        "region": "us-east-1",
        "account_id": "123456789012",
        "access_key_id": "AKIAFIXTURE",
        "secret_access_key": "fixture-secret",
    })
    .to_string();

    let outcome = project
        .setup(&connect)
        .use_connector(
            &aws,
            UseRequest::Hosted {
                credential: Some((&CredentialName::new("access_key").unwrap(), &secret)),
                target: None,
            },
            &options(Duration::ZERO),
        )
        .unwrap();

    assert_eq!(grant_of(outcome).unwrap(), "grant_aws_1");
    let submitted = connect.sent("/v1/connect/sessions/sess_1/credentials");
    assert_eq!(submitted[0]["credentials"]["region"], "us-east-1");
    assert_eq!(
        submitted[0]["credentials"]["secret_access_key"],
        "fixture-secret"
    );
}

#[test]
fn a_hosted_credential_session_without_a_credential_names_the_form_that_sends_one() {
    let aws = source_scope().connector("runx/aws").unwrap();
    let connect = Connect::new(
        json!({"status": "credential_required", "session_id": "sess_1"}),
        &[],
    );
    let project = Project::new();

    let refused = project
        .setup(&connect)
        .use_connector(
            &aws,
            UseRequest::Hosted {
                credential: None,
                target: None,
            },
            &options(Duration::ZERO),
        )
        .err()
        .unwrap();

    assert!(
        refused.contains("runx connector use runx/aws --hosted --credential <name>"),
        "{refused}"
    );
    assert!(
        connect
            .sent("/v1/connect/sessions/sess_1/credentials")
            .is_empty()
    );
}

#[test]
fn a_failed_connect_session_is_refused_with_its_reason() {
    let slack = source_scope().connector("runx/slack").unwrap();
    let connect = Connect::new(
        awaiting_authorization(),
        &[json!({"status": "failed", "session_id": "sess_1", "error": "access denied"})],
    );
    let project = Project::new();

    let refused = project
        .setup(&connect)
        .use_connector(
            &slack,
            UseRequest::Hosted {
                credential: None,
                target: None,
            },
            &options(Duration::from_secs(30)),
        )
        .err()
        .unwrap();

    assert_eq!(
        refused,
        "runx Connect could not grant runx/slack: access denied"
    );
    assert!(project.instances.list().unwrap().is_empty());
}
