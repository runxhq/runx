#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use runx_contracts::connector::{CredentialName, SecretName};
use runx_contracts::{InstanceAuthority, JsonObject};
use serde_json::{Value, json};

use super::{ConnectorSetup, UseOptions, UseOutcome, UseRequest};
use crate::http::{HttpMethod, RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse};
use crate::kernel::connectors::{LoadedConnector, fixture_scope};
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
    let chat = fixture_scope().connector("fixture/chat").unwrap();
    let connect = Connect::new(awaiting_authorization(), &[pending()]);
    let created = connect.issue("grant_chat_1", &chat);
    connect.statuses.lock().unwrap().push_back(created);
    let project = Project::new();

    let outcome = project
        .setup(&connect)
        .use_connector(
            &chat,
            UseRequest::Hosted {
                target: None,
                secret: None,
            },
            &options(Duration::from_secs(30)),
        )
        .unwrap();

    assert_eq!(grant_of(outcome).unwrap(), "grant_chat_1");
    // A new grant asks for every scope the connector's operations declare.
    let started = connect.sent("/v1/connect/sessions");
    let mut declared = chat
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
        serde_json::to_value(chat.binding()).unwrap(),
        "Connect is asked for the exact connector the kernel loaded"
    );
    assert!(started[0].get("provider").is_none());
    assert_eq!(started[0]["scopes"], json!(declared));
    assert_eq!(connect.sent("/v1/connect/sessions/sess_1").len(), 2);
}

#[test]
fn an_unauthorized_grant_names_the_command_that_finishes_it() -> Result<(), String> {
    let chat = fixture_scope().connector("fixture/chat").unwrap();
    let connect = Connect::new(awaiting_authorization(), &[]);
    let project = Project::new();

    let outcome = project
        .setup(&connect)
        .use_connector(
            &chat,
            UseRequest::Hosted {
                target: Some("C0123"),
                secret: None,
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
    assert_eq!(finish, "runx connector use fixture/chat sess_1");
    // The target goes as one locator; Connect reads it per provider.
    let started = connect.sent("/v1/connect/sessions");
    assert_eq!(started[0]["target_locator"], "C0123");
    assert!(started[0].get("target_repo").is_none());

    // Once authorized, the printed command binds the grant the session made.
    let created = connect.issue("grant_chat_1", &chat);
    connect.statuses.lock().unwrap().push_back(created);
    let finished = project
        .setup(&connect)
        .use_connector(
            &chat,
            UseRequest::Existing("sess_1"),
            &options(Duration::ZERO),
        )
        .unwrap();
    assert_eq!(grant_of(finished)?, "grant_chat_1");
    Ok(())
}

#[test]
fn a_new_grant_shows_its_link_once_before_it_waits() {
    let chat = fixture_scope().connector("fixture/chat").unwrap();
    let connect = Connect::new(awaiting_authorization(), &[pending(), pending()]);
    let created = connect.issue("grant_chat_1", &chat);
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
            &chat,
            UseRequest::Hosted {
                target: None,
                secret: None,
            },
            &options(Duration::from_secs(30)),
        )
        .unwrap();

    assert_eq!(grant_of(outcome).unwrap(), "grant_chat_1");
    assert_eq!(
        *shown.borrow(),
        vec!["fixture/chat https://connect.runx.invalid/authorize/sess_1".to_owned()]
    );
}

#[test]
fn a_hosted_key_the_environment_and_env_file_disagree_on_never_reaches_connect() {
    let compute = fixture_scope().connector("fixture/compute").unwrap();
    let connect = Connect::new(
        json!({"status": "credential_required", "session_id": "sess_1"}),
        &[],
    );
    let project = Project::with(&[
        ("COMPUTE_CREDENTIALS", "{}"),
        ("RUNX_ENV_FILE_DIFFERS", "COMPUTE_CREDENTIALS"),
    ]);

    let refused = project
        .setup(&connect)
        .reading_secrets(&never_asked)
        .use_connector(
            &compute,
            UseRequest::Hosted {
                target: None,
                secret: None,
            },
            &options(Duration::ZERO),
        )
        .err()
        .unwrap();

    assert!(
        refused
            .starts_with("COMPUTE_CREDENTIALS is set in the environment and in the workspace .env"),
        "{refused}"
    );
    assert!(connect.sent("/v1/connect/sessions").is_empty());
}

#[test]
fn a_plain_use_refuses_a_variable_the_env_file_disagrees_on_even_when_blank() {
    let search = fixture_scope().connector("fixture/search").unwrap();
    let connect = Connect::new(Value::Null, &[]);
    // A blank export over a `.env` value is still two values.
    let project = Project::with(&[
        ("SEARCH_API_KEY", ""),
        ("RUNX_ENV_FILE_DIFFERS", "SEARCH_API_KEY"),
    ]);

    let refused = project
        .setup(&connect)
        .reading_secrets(&never_asked)
        .use_connector(&search, UseRequest::Default, &options(Duration::ZERO))
        .err()
        .unwrap();

    assert!(
        refused.starts_with("SEARCH_API_KEY is set in the environment and in the workspace .env"),
        "{refused}"
    );
    assert!(project.instances.list().unwrap().is_empty());
}

#[test]
fn two_runx_tokens_that_differ_are_refused_not_read_as_signed_out() {
    let compute = fixture_scope().connector("fixture/compute").unwrap();
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
        .use_connector(&compute, UseRequest::Default, &options(Duration::ZERO))
        .err()
        .unwrap();

    assert!(refused.contains("differ"), "{refused}");
    assert!(!refused.contains("runx login` first"), "{refused}");
    assert!(connect.sent("/v1/connect/sessions").is_empty());
}

#[test]
fn a_hosted_credential_is_submitted_to_the_session_it_opens() {
    let compute = fixture_scope().connector("fixture/compute").unwrap();
    let connect = Connect::new(
        json!({"status": "credential_required", "session_id": "sess_1"}),
        &[],
    );
    let created = connect.issue("grant_compute_1", &compute);
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
            "fixture/compute access_key, a JSON object of region, account_id, access_key_id, secret_access_key, session_token"
        );
        Ok(secret.clone())
    };
    let outcome = project
        .setup(&connect)
        .reading_secrets(&asked)
        .use_connector(
            &compute,
            UseRequest::Hosted {
                target: None,
                secret: None,
            },
            &options(Duration::ZERO),
        )
        .unwrap();

    assert_eq!(grant_of(outcome).unwrap(), "grant_compute_1");
    let submitted = connect.sent("/v1/connect/sessions/sess_1/credentials");
    assert_eq!(submitted[0]["credentials"]["region"], "us-east-1");
    assert_eq!(
        submitted[0]["credentials"]["secret_access_key"],
        "fixture-secret"
    );
}

#[test]
fn a_connector_whose_credential_only_connect_holds_takes_its_hosted_placement() {
    let compute = fixture_scope().connector("fixture/compute").unwrap();
    let connect = Connect::new(
        json!({"status": "credential_required", "session_id": "sess_1"}),
        &[],
    );
    let created = connect.issue("grant_compute_1", &compute);
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
        .use_connector(&compute, UseRequest::Default, &options(Duration::ZERO))
        .unwrap();

    assert_eq!(grant_of(outcome).unwrap(), "grant_compute_1");
    assert_eq!(
        connect.sent("/v1/connect/sessions")[0]["connector_binding"],
        serde_json::to_value(compute.binding()).unwrap()
    );
    assert_eq!(
        connect.sent("/v1/connect/sessions/sess_1/credentials")[0]["credentials"]["access_key_id"],
        "AKIAFIXTURE"
    );
}

#[test]
fn a_connect_session_waiting_for_a_credential_names_the_form_that_sends_one() {
    let compute = fixture_scope().connector("fixture/compute").unwrap();
    let connect = Connect::new(
        awaiting_authorization(),
        &[json!({"status": "credential_required", "session_id": "sess_1"})],
    );
    let project = Project::new();

    let refused = project
        .setup(&connect)
        .use_connector(
            &compute,
            UseRequest::Existing("sess_1"),
            &options(Duration::ZERO),
        )
        .err()
        .unwrap();

    assert!(
        refused.contains(
            "run `runx connector use fixture/compute --hosted` (reads COMPUTE_CREDENTIALS, else asks for it)"
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
    let chat = fixture_scope().connector("fixture/chat").unwrap();
    let connect = Connect::new(
        awaiting_authorization(),
        &[json!({"status": "failed", "session_id": "sess_1", "error": "access denied"})],
    );
    let project = Project::new();

    let refused = project
        .setup(&connect)
        .use_connector(
            &chat,
            UseRequest::Hosted {
                target: None,
                secret: None,
            },
            &options(Duration::from_secs(30)),
        )
        .err()
        .unwrap();

    assert_eq!(
        refused,
        "runx Connect could not grant fixture/chat: access denied"
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
    let search = fixture_scope().connector("fixture/search").unwrap();
    let project = Project::with(&[("SEARCH_API_KEY", "search-fixture-key")]);
    let connect = Connect::new(Value::Null, &[]);

    let instance = selected(
        project
            .setup(&connect)
            .reading_secrets(&never_asked)
            .use_connector(&search, UseRequest::Default, &options(Duration::ZERO))?,
    )?;

    assert_eq!(
        instance.authority,
        InstanceAuthority::Environment {
            credential: CredentialName::new("api_key").unwrap(),
            variable: runx_contracts::connector::EnvName::new("SEARCH_API_KEY").unwrap(),
            scopes: vec!["search.read".into()],
        }
    );
    // The instance names the variable; the key is never copied anywhere.
    assert!(
        !serde_json::to_string(&instance)
            .unwrap()
            .contains("search-fixture-key")
    );
    assert!(
        crate::secrets::list(project.workspace.paths())
            .unwrap()
            .is_empty()
    );
    Ok(())
}

#[test]
fn a_connector_with_one_credential_asks_for_it_when_its_variable_is_not_set() -> Result<(), String>
{
    let search = fixture_scope().connector("fixture/search").unwrap();
    let project = Project::new();
    let connect = Connect::new(Value::Null, &[]);
    let asked = |wanted: &str| {
        assert_eq!(wanted, "fixture/search api_key");
        Ok("search-typed-key".to_owned())
    };

    let instance = selected(
        project
            .setup(&connect)
            .reading_secrets(&asked)
            .use_connector(&search, UseRequest::Default, &options(Duration::ZERO))?,
    )?;

    let InstanceAuthority::Secret { secret, .. } = &instance.authority else {
        return Err(format!(
            "a typed credential is kept as a secret: {:?}",
            instance.authority
        ));
    };
    assert_eq!(secret.as_str(), "fixture/search");
    let value = crate::secrets::read(
        project.workspace.env(),
        project.workspace.cwd(),
        secret,
        crate::secrets::Purpose::Call,
    )
    .map_err(|error| error.to_string())?;
    assert_eq!(value.expose(), "search-typed-key");
    Ok(())
}

#[test]
fn a_credential_with_a_local_home_stays_local_though_connect_could_hold_it() -> Result<(), String> {
    let refs = fixture_scope().connector("fixture/refs").unwrap();
    let project = Project::new();
    let connect = Connect::new(Value::Null, &[]);

    let instance = selected(
        project
            .setup(&connect)
            .reading_secrets(&|_: &str| Ok("refs-typed-key".to_owned()))
            .use_connector(&refs, UseRequest::Default, &options(Duration::ZERO))?,
    )?;

    assert!(
        matches!(instance.authority, InstanceAuthority::Secret { .. }),
        "{:?}",
        instance.authority
    );
    assert!(connect.sent("/v1/connect/sessions").is_empty());
    Ok(())
}

#[test]
fn a_credential_that_cannot_be_asked_for_names_its_variable() {
    let search = fixture_scope().connector("fixture/search").unwrap();
    let project = Project::new();
    let connect = Connect::new(Value::Null, &[]);

    let refused = project
        .setup(&connect)
        .use_connector(&search, UseRequest::Default, &options(Duration::ZERO))
        .err()
        .unwrap();

    assert!(refused.contains("set SEARCH_API_KEY"), "{refused}");
}

#[test]
fn a_connector_with_several_credentials_names_each_and_its_variable() {
    let social = fixture_scope().connector("fixture/social").unwrap();
    let project = Project::new();
    let connect = Connect::new(Value::Null, &[]);

    let refused = project
        .setup(&connect)
        .reading_secrets(&never_asked)
        .use_connector(&social, UseRequest::Default, &options(Duration::ZERO))
        .err()
        .unwrap();

    assert!(
        refused.contains(
            "`runx connector use fixture/social --credential app` (reads SOCIAL_BEARER_TOKEN, else asks for it)"
        ),
        "{refused}"
    );
    assert!(
        refused.contains(
            "`runx connector use fixture/social --credential user` (reads SOCIAL_USER_AUTH, else asks for it)"
        ),
        "{refused}"
    );
}

#[test]
fn a_prompted_value_never_takes_a_secret_already_set_by_that_name() {
    let search = fixture_scope().connector("fixture/search").unwrap();
    let project = Project::new();
    let connect = Connect::new(Value::Null, &[]);
    let name = SecretName::new("fixture/search").unwrap();
    crate::secrets::set_value(project.workspace.paths(), &name, "kept-key").unwrap();

    let refused = project
        .setup(&connect)
        .reading_secrets(&never_asked)
        .use_connector(&search, UseRequest::Default, &options(Duration::ZERO))
        .err()
        .unwrap();

    assert_eq!(
        refused,
        "a secret named fixture/search is set; bind it with `runx connector use fixture/search --secret fixture/search`, or set SEARCH_API_KEY"
    );
}

#[test]
fn a_named_secret_binds_without_asking_or_reading_the_variable() -> Result<(), String> {
    let search = fixture_scope().connector("fixture/search").unwrap();
    // The variable is set too: naming a secret never reads it.
    let project = Project::with(&[("SEARCH_API_KEY", "search-fixture-key")]);
    let connect = Connect::new(Value::Null, &[]);
    let name = SecretName::new("search-work").unwrap();
    crate::secrets::set_value(project.workspace.paths(), &name, "work-key")
        .map_err(|error| error.to_string())?;

    let instance = selected(
        project
            .setup(&connect)
            .reading_secrets(&never_asked)
            .use_connector(
                &search,
                UseRequest::Secret {
                    credential: None,
                    secret: &name,
                },
                &options(Duration::ZERO),
            )?,
    )?;

    assert!(
        matches!(&instance.authority, InstanceAuthority::Secret { secret, .. } if *secret == name),
        "{:?}",
        instance.authority
    );
    assert!(
        !serde_json::to_string(&instance)
            .unwrap()
            .contains("work-key")
    );
    Ok(())
}

/// The secret `name`, pointed at a file holding `contents`, as a secrets
/// manager would hold it.
fn reference_secret(
    project: &Project,
    held: &std::path::Path,
    name: &str,
    contents: &str,
) -> SecretName {
    let file = held.join(format!("{name}.json"));
    std::fs::write(&file, contents).unwrap();
    let reference =
        runx_contracts::connector::SecretReference::new(format!("file://{}", file.display()))
            .unwrap();
    let name = SecretName::new(name).unwrap();
    crate::secrets::set_reference(
        project.workspace.env(),
        project.workspace.cwd(),
        &name,
        &reference,
    )
    .unwrap();
    name
}

#[test]
fn a_hosted_credential_is_seeded_from_one_secret_holding_every_field() {
    let compute = fixture_scope().connector("fixture/compute").unwrap();
    let connect = Connect::new(
        json!({"status": "credential_required", "session_id": "sess_1"}),
        &[],
    );
    let created = connect.issue("grant_compute_1", &compute);
    connect.statuses.lock().unwrap().push_back(created);
    let project = Project::new();
    let held = tempfile::tempdir().unwrap();
    let name = reference_secret(
        &project,
        held.path(),
        "compute",
        &json!({
            "region": "us-east-1",
            "account_id": "123456789012",
            "access_key_id": "AKIAFROMVAULT",
            "secret_access_key": "secret-from-vault",
        })
        .to_string(),
    );

    let outcome = project
        .setup(&connect)
        .reading_secrets(&|_: &str| Err("asked for what the secret holds".to_owned()))
        .use_connector(
            &compute,
            UseRequest::Hosted {
                target: None,
                secret: Some(&name),
            },
            &options(Duration::ZERO),
        )
        .unwrap();

    assert_eq!(grant_of(outcome).unwrap(), "grant_compute_1");
    let sent = &connect.sent("/v1/connect/sessions/sess_1/credentials")[0]["credentials"];
    assert_eq!(sent["access_key_id"], "AKIAFROMVAULT");
    assert_eq!(sent["secret_access_key"], "secret-from-vault");
}

#[test]
fn a_secret_cannot_seed_a_delegated_sign_in() {
    let forge = fixture_scope().connector("fixture/forge").unwrap();
    let connect = Connect::new(awaiting_authorization(), &[]);
    let project = Project::new();
    let name = SecretName::new("forge").unwrap();

    let refused = project
        .setup(&connect)
        .use_connector(
            &forge,
            UseRequest::Hosted {
                target: None,
                secret: Some(&name),
            },
            &options(Duration::ZERO),
        )
        .err()
        .unwrap();

    assert!(
        refused.contains("--secret seeds only a hosted credential"),
        "{refused}"
    );
    assert!(connect.sent("/v1/connect/sessions").is_empty());
}

#[test]
fn a_secret_that_cannot_be_read_sends_nothing_to_connect() {
    let compute = fixture_scope().connector("fixture/compute").unwrap();
    let connect = Connect::new(
        json!({"status": "credential_required", "session_id": "sess_1"}),
        &[],
    );
    let project = Project::new();
    let held = tempfile::tempdir().unwrap();
    let name = reference_secret(&project, held.path(), "compute", "{}");
    std::fs::remove_file(held.path().join("compute.json")).unwrap();

    let refused = project
        .setup(&connect)
        .use_connector(
            &compute,
            UseRequest::Hosted {
                target: None,
                secret: Some(&name),
            },
            &options(Duration::ZERO),
        )
        .err()
        .unwrap();

    assert!(refused.contains("cannot be read"), "{refused}");
    assert!(connect.sent("/v1/connect/sessions").is_empty());
}
