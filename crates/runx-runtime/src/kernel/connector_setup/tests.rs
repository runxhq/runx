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
        Self::with(&[])
    }

    /// A project whose environment also sets `variables`.
    fn with(variables: &[(&str, &str)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut env = BTreeMap::from([
            (
                crate::HOSTED_API_BASE_URL_ENV.to_owned(),
                BASE_URL.to_owned(),
            ),
            (
                crate::HOSTED_API_TOKEN_ENV.to_owned(),
                "rxk_test".to_owned(),
            ),
            (
                crate::paths::RUNX_HOME_ENV.to_owned(),
                root.path().join("home").to_string_lossy().into_owned(),
            ),
        ]);
        env.extend(
            variables
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
        );
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
                target: None,
                from: None,
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
    assert_eq!(
        started[0]["connector_binding"],
        serde_json::to_value(slack.binding()).unwrap(),
        "Connect is asked for the exact connector the kernel loaded"
    );
    assert!(started[0].get("provider").is_none());
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
                target: Some("C0123"),
                from: None,
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
fn a_new_grant_shows_its_link_once_before_it_waits() {
    let slack = source_scope().connector("runx/slack").unwrap();
    let connect = Connect::new(awaiting_authorization(), &[pending(), pending()]);
    let created = connect.issue("grant_slack_1", &slack);
    connect.statuses.lock().unwrap().push_back(created);
    let project = Project::new();
    let shown = std::cell::RefCell::new(Vec::new());
    let show = |connector: &str, url: &str| {
        // The person is shown the link before anything waits on them: no
        // status has been asked for, and nothing is bound.
        assert!(connect.sent("/v1/connect/sessions/sess_1").is_empty());
        assert!(project.instances.list().unwrap().is_empty());
        shown.borrow_mut().push(format!("{connector} {url}"));
    };

    let outcome = project
        .setup(&connect)
        .showing_links(&show)
        .use_connector(
            &slack,
            UseRequest::Hosted {
                target: None,
                from: None,
            },
            &options(Duration::from_secs(30)),
        )
        .unwrap();

    assert_eq!(grant_of(outcome).unwrap(), "grant_slack_1");
    assert_eq!(
        *shown.borrow(),
        vec!["runx/slack https://connect.runx.invalid/authorize/sess_1".to_owned()]
    );
}

#[test]
fn a_hosted_key_the_environment_and_env_file_disagree_on_never_reaches_connect() {
    let aws = source_scope().connector("runx/aws").unwrap();
    let connect = Connect::new(
        json!({"status": "credential_required", "session_id": "sess_1"}),
        &[],
    );
    let project = Project::with(&[
        ("AWS_CREDENTIALS", "{}"),
        ("RUNX_ENV_FILE_DIFFERS", "AWS_CREDENTIALS"),
    ]);

    let refused = project
        .setup(&connect)
        .reading_secrets(&never_asked)
        .use_connector(
            &aws,
            UseRequest::Hosted {
                target: None,
                from: None,
            },
            &options(Duration::ZERO),
        )
        .err()
        .unwrap();

    assert!(
        refused.starts_with("AWS_CREDENTIALS is set in the environment and in the workspace .env"),
        "{refused}"
    );
    assert!(connect.sent("/v1/connect/sessions").is_empty());
}

#[test]
fn a_plain_use_refuses_a_variable_the_env_file_disagrees_on_even_when_blank() {
    let exa = source_scope().connector("runx/exa").unwrap();
    let connect = Connect::new(Value::Null, &[]);
    // A blank export over a `.env` value is still two values.
    let project = Project::with(&[
        ("EXA_API_KEY", ""),
        ("RUNX_ENV_FILE_DIFFERS", "EXA_API_KEY"),
    ]);

    let refused = project
        .setup(&connect)
        .reading_secrets(&never_asked)
        .use_connector(&exa, UseRequest::Default, &options(Duration::ZERO))
        .err()
        .unwrap();

    assert!(
        refused.starts_with("EXA_API_KEY is set in the environment and in the workspace .env"),
        "{refused}"
    );
    assert!(project.instances.list().unwrap().is_empty());
}

#[test]
fn two_runx_tokens_that_differ_are_refused_not_read_as_signed_out() {
    let aws = source_scope().connector("runx/aws").unwrap();
    let connect = Connect::new(Value::Null, &[]);
    let project = Project::new();
    crate::hosted_api::store_authenticated_hosted_environment(
        project.workspace.env(),
        project.workspace.cwd(),
        crate::hosted_api::HostedApiCredentialPurpose::Default,
        BASE_URL,
        "operator-1",
        "rxk_stored_other",
    )
    .unwrap();

    let refused = project
        .setup(&connect)
        .reading_secrets(&never_asked)
        .use_connector(&aws, UseRequest::Default, &options(Duration::ZERO))
        .err()
        .unwrap();

    assert!(refused.contains("differ"), "{refused}");
    assert!(!refused.contains("runx login` first"), "{refused}");
    assert!(connect.sent("/v1/connect/sessions").is_empty());
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

    let asked = |wanted: &str| {
        assert_eq!(
            wanted,
            "runx/aws access_key, a JSON object of region, account_id, access_key_id, secret_access_key, session_token"
        );
        Ok(secret.clone())
    };
    let outcome = project
        .setup(&connect)
        .reading_secrets(&asked)
        .use_connector(
            &aws,
            UseRequest::Hosted {
                target: None,
                from: None,
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
fn a_connector_whose_credential_only_connect_holds_takes_its_hosted_placement() {
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
        .reading_secrets(&|_: &str| Ok(secret.clone()))
        .use_connector(&aws, UseRequest::Default, &options(Duration::ZERO))
        .unwrap();

    assert_eq!(grant_of(outcome).unwrap(), "grant_aws_1");
    assert_eq!(
        connect.sent("/v1/connect/sessions")[0]["connector_binding"],
        serde_json::to_value(aws.binding()).unwrap()
    );
    assert_eq!(
        connect.sent("/v1/connect/sessions/sess_1/credentials")[0]["credentials"]["access_key_id"],
        "AKIAFIXTURE"
    );
}

#[test]
fn a_connect_session_waiting_for_a_credential_names_the_form_that_sends_one() {
    let aws = source_scope().connector("runx/aws").unwrap();
    let connect = Connect::new(
        awaiting_authorization(),
        &[json!({"status": "credential_required", "session_id": "sess_1"})],
    );
    let project = Project::new();

    let refused = project
        .setup(&connect)
        .use_connector(
            &aws,
            UseRequest::Existing("sess_1"),
            &options(Duration::ZERO),
        )
        .err()
        .unwrap();

    assert!(
        refused.contains(
            "run `runx connector use runx/aws --hosted` (reads AWS_CREDENTIALS, else asks for it)"
        ),
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
                target: None,
                from: None,
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

fn selected(outcome: UseOutcome) -> Result<runx_contracts::ConnectorInstance, String> {
    match outcome {
        UseOutcome::Selected(instance) => Ok(*instance),
        UseOutcome::Pending { finish, .. } => Err(format!("still pending: {finish}")),
    }
}

fn never_asked(wanted: &str) -> Result<String, String> {
    Err(format!("asked for {wanted}"))
}

#[test]
fn a_credential_whose_variable_is_set_is_bound_to_the_variable() -> Result<(), String> {
    let exa = source_scope().connector("runx/exa").unwrap();
    let project = Project::with(&[("EXA_API_KEY", "exa-fixture-key")]);
    let connect = Connect::new(Value::Null, &[]);

    let instance = selected(
        project
            .setup(&connect)
            .reading_secrets(&never_asked)
            .use_connector(&exa, UseRequest::Default, &options(Duration::ZERO))?,
    )?;

    assert_eq!(
        instance.authority,
        InstanceAuthority::Environment {
            credential: CredentialName::new("api_key").unwrap(),
            variable: runx_contracts::connector::EnvName::new("EXA_API_KEY").unwrap(),
            scopes: vec!["search.read".into()],
        }
    );
    // The instance names the variable; the key is never copied anywhere.
    assert!(
        !serde_json::to_string(&instance)
            .unwrap()
            .contains("exa-fixture-key")
    );
    assert!(
        crate::credential_profiles::list_local_credential_profiles(&project.workspace)
            .unwrap()
            .is_empty()
    );
    Ok(())
}

#[test]
fn a_connector_with_one_credential_asks_for_it_when_its_variable_is_not_set() -> Result<(), String>
{
    let exa = source_scope().connector("runx/exa").unwrap();
    let project = Project::new();
    let connect = Connect::new(Value::Null, &[]);
    let asked = |wanted: &str| {
        assert_eq!(wanted, "runx/exa api_key");
        Ok("exa-typed-key".to_owned())
    };

    let instance = selected(
        project
            .setup(&connect)
            .reading_secrets(&asked)
            .use_connector(&exa, UseRequest::Default, &options(Duration::ZERO))?,
    )?;

    let InstanceAuthority::Credential { profile, .. } = &instance.authority else {
        return Err(format!(
            "a typed credential is stored: {:?}",
            instance.authority
        ));
    };
    assert_eq!(profile.as_str(), "runx/exa");
    Ok(())
}

#[test]
fn a_credential_with_a_local_home_stays_local_though_connect_could_hold_it() -> Result<(), String> {
    let apollo = source_scope().connector("runx/apollo").unwrap();
    let project = Project::new();
    let connect = Connect::new(Value::Null, &[]);

    let instance = selected(
        project
            .setup(&connect)
            .reading_secrets(&|_: &str| Ok("apollo-typed-key".to_owned()))
            .use_connector(&apollo, UseRequest::Default, &options(Duration::ZERO))?,
    )?;

    assert!(
        matches!(instance.authority, InstanceAuthority::Credential { .. }),
        "{:?}",
        instance.authority
    );
    assert!(connect.sent("/v1/connect/sessions").is_empty());
    Ok(())
}

#[test]
fn a_credential_that_cannot_be_asked_for_names_its_variable() {
    let exa = source_scope().connector("runx/exa").unwrap();
    let project = Project::new();
    let connect = Connect::new(Value::Null, &[]);

    let refused = project
        .setup(&connect)
        .use_connector(&exa, UseRequest::Default, &options(Duration::ZERO))
        .err()
        .unwrap();

    assert!(refused.contains("set EXA_API_KEY"), "{refused}");
}

#[test]
fn a_connector_with_several_credentials_names_each_and_its_variable() {
    let twitter = source_scope().connector("runx/twitter").unwrap();
    let project = Project::new();
    let connect = Connect::new(Value::Null, &[]);

    let refused = project
        .setup(&connect)
        .reading_secrets(&never_asked)
        .use_connector(&twitter, UseRequest::Default, &options(Duration::ZERO))
        .err()
        .unwrap();

    assert!(
        refused.contains(
            "`runx connector use runx/twitter --credential app` (reads TWITTER_BEARER_TOKEN, else asks for it)"
        ),
        "{refused}"
    );
    assert!(
        refused.contains(
            "`runx connector use runx/twitter --credential user` (reads TWITTER_USER_AUTH, else asks for it)"
        ),
        "{refused}"
    );
}

#[test]
fn a_profile_is_not_stored_for_a_credential_its_variable_holds() {
    let exa = source_scope().connector("runx/exa").unwrap();
    let project = Project::with(&[("EXA_API_KEY", "exa-fixture-key")]);
    let connect = Connect::new(Value::Null, &[]);

    let refused = project
        .setup(&connect)
        .reading_secrets(&never_asked)
        .use_connector(
            &exa,
            UseRequest::Credential {
                credential: &CredentialName::new("api_key").unwrap(),
                profile: Some("work"),
            },
            &options(Duration::ZERO),
        )
        .err()
        .unwrap();

    assert!(
        refused.contains("is read from EXA_API_KEY, which is set"),
        "{refused}"
    );
}

#[test]
fn a_hosted_credential_is_seeded_from_one_reference_holding_every_field() {
    let aws = source_scope().connector("runx/aws").unwrap();
    let connect = Connect::new(
        json!({"status": "credential_required", "session_id": "sess_1"}),
        &[],
    );
    let created = connect.issue("grant_aws_1", &aws);
    connect.statuses.lock().unwrap().push_back(created);
    let project = Project::new();
    let held = tempfile::tempdir().unwrap();
    let secret = held.path().join("aws.json");
    std::fs::write(
        &secret,
        json!({
            "region": "us-east-1",
            "account_id": "123456789012",
            "access_key_id": "AKIAFROMVAULT",
            "secret_access_key": "secret-from-vault",
        })
        .to_string(),
    )
    .unwrap();
    let reference =
        runx_contracts::connector::SecretReference::new(format!("file://{}", secret.display()))
            .unwrap();

    let outcome = project
        .setup(&connect)
        .reading_secrets(&|_: &str| Err("asked for what the reference holds".to_owned()))
        .use_connector(
            &aws,
            UseRequest::Hosted {
                target: None,
                from: Some(&reference),
            },
            &options(Duration::ZERO),
        )
        .unwrap();

    assert_eq!(grant_of(outcome).unwrap(), "grant_aws_1");
    let sent = &connect.sent("/v1/connect/sessions/sess_1/credentials")[0]["credentials"];
    assert_eq!(sent["access_key_id"], "AKIAFROMVAULT");
    assert_eq!(sent["secret_access_key"], "secret-from-vault");
}

#[test]
fn a_reference_cannot_seed_a_delegated_sign_in() {
    let github = source_scope().connector("runx/github").unwrap();
    let connect = Connect::new(awaiting_authorization(), &[]);
    let project = Project::new();
    let reference =
        runx_contracts::connector::SecretReference::new("aws-sm://team/github").unwrap();

    let refused = project
        .setup(&connect)
        .use_connector(
            &github,
            UseRequest::Hosted {
                target: None,
                from: Some(&reference),
            },
            &options(Duration::ZERO),
        )
        .err()
        .unwrap();

    assert!(
        refused.contains("--from seeds only a hosted credential"),
        "{refused}"
    );
    assert!(connect.sent("/v1/connect/sessions").is_empty());
}

#[test]
fn a_reference_that_cannot_be_read_sends_nothing_to_connect() {
    let aws = source_scope().connector("runx/aws").unwrap();
    let connect = Connect::new(
        json!({"status": "credential_required", "session_id": "sess_1"}),
        &[],
    );
    let project = Project::new();
    let reference =
        runx_contracts::connector::SecretReference::new("file:///nonexistent/runx/aws.json")
            .unwrap();

    let refused = project
        .setup(&connect)
        .use_connector(
            &aws,
            UseRequest::Hosted {
                target: None,
                from: Some(&reference),
            },
            &options(Duration::ZERO),
        )
        .err()
        .unwrap();

    assert!(refused.contains("cannot be read"), "{refused}");
    assert!(connect.sent("/v1/connect/sessions").is_empty());
}
