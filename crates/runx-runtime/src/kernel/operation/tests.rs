#![allow(clippy::expect_used, clippy::unwrap_used)]

//! The operation path end to end through its effect hooks: admission,
//! approval lanes, custody, dispatch to a hosted transport and the packet.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use runx_contracts::{
    AuthorityBounds, AuthorityVerb, ExecutionEvent, JsonObject, JsonValue, Reference,
    ReferenceType, ResolutionRequest, ResolutionResponse, ResolutionResponseActor,
};
use runx_parser::WorkflowStep;

use super::*;
use crate::effects::{
    EffectPreparationOutcome, EffectStepRequest, ResolvedEffectTarget, RuntimeEffect,
};
use crate::http::{HttpMethod, RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse};
use crate::{Host, RuntimeError};

/// Keep `value` as the secret `name` in `workspace`'s runx home, then bind
/// `connector`'s `credential` to it and select that instance.
fn keep_and_bind(
    connector: &crate::LoadedConnector,
    name: &str,
    credential: &str,
    value: &str,
    workspace: &crate::services::WorkspaceEnv,
) {
    let name = runx_contracts::connector::SecretName::new(name).unwrap();
    crate::secrets::set_value(workspace.paths(), &name, value).unwrap();
    crate::kernel::setup::bind_secret(
        &Instances::new(KernelStore::for_project(workspace.paths()).unwrap()),
        connector,
        &runx_contracts::connector::CredentialName::new(credential).unwrap(),
        &name,
        &[],
        JsonObject::new(),
        workspace,
    )
    .unwrap();
}

const BASE_URL: &str = "https://provider-fixture.runx.invalid";
const PRINCIPAL_ID: &str = "operator-1";
const PRINCIPAL_REF: &str = "runx:principal:operator-1";
const SKILL: &str = r#"
minimum_runx: 0.12.0
methods:
  run:
    result_from: [post]
    steps:
      - id: post
        operation: fixture/chat/channel.post
      - id: read
        operation: fixture/chat/channel.post.read
      - id: job
        operation: fixture/compute/jobs.start
      - id: advisories
        operation: fixture/advisories/vulnerabilities.read
      - id: update
        operation: fixture/forge/issues.update
      - id: issues
        operation: fixture/forge/issues.read
      - id: deliver
        operation: fixture/hooks/handoff.deliver
      - id: allocate
        operation: artifact/artifact.allocate
"#;

/// How the fake provider answers a call.
#[derive(Clone, Copy, PartialEq)]
enum Answer {
    Succeed,
    TimeOut,
    Reject,
    RejectAfterOne,
    WrongProvider,
    /// Succeeds with a locator the connector's references do not declare.
    OffPattern,
}

#[derive(Clone)]
struct Provider {
    answer: Arc<Mutex<Answer>>,
    calls: Arc<Mutex<Vec<JsonObject>>>,
    /// The grants runx Connect lists for the principal.
    grants: Arc<Mutex<Vec<serde_json::Value>>>,
    /// The principal runx Connect authenticates.
    principal: Arc<Mutex<String>>,
    /// The principal the artifact space states it stored for, when not the
    /// authenticated one.
    artifact_owner: Arc<Mutex<Option<String>>>,
    /// The principal the artifact space states reading an artifact back,
    /// when not the one it stored for.
    inspected_owner: Arc<Mutex<Option<String>>>,
    /// What the artifact space holds, by reference.
    artifacts: Arc<Mutex<BTreeMap<String, serde_json::Value>>>,
}

impl Provider {
    fn new(answer: Answer) -> Self {
        Self {
            answer: Arc::new(Mutex::new(answer)),
            calls: Arc::new(Mutex::new(Vec::new())),
            grants: Arc::new(Mutex::new(Vec::new())),
            principal: Arc::new(Mutex::new(PRINCIPAL_ID.to_owned())),
            artifact_owner: Arc::new(Mutex::new(None)),
            inspected_owner: Arc::new(Mutex::new(None)),
            artifacts: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// runx's artifact space: an allocation is keyed by its idempotency key
    /// and an inspection reads what is held under a reference.
    fn artifact(&self, body: &serde_json::Value) -> serde_json::Value {
        let operation = body["operation"].as_str().unwrap_or_default();
        let input = &body["input"];
        let (reference, result) = match operation {
            "artifact.allocate" => {
                use base64::Engine as _;
                let key = input["idempotency_key"].as_str().unwrap();
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(input["data_base64"].as_str().unwrap())
                    .unwrap();
                let reference = format!(
                    "runx:artifact:{}",
                    runx_contracts::sha256_prefixed(key.as_bytes())
                );
                let result = serde_json::json!({
                    "ref": reference,
                    "digest": input["digest"],
                    "media_type": input["media_type"],
                    "size_bytes": bytes.len(),
                    "created_at": "2026-10-13T00:00:00.000Z",
                });
                self.artifacts
                    .lock()
                    .unwrap()
                    .insert(reference.clone(), result.clone());
                (reference, result)
            }
            _ => {
                let reference = input["ref"].as_str().unwrap().to_owned();
                let result = self.artifacts.lock().unwrap()[&reference].clone();
                (reference, result)
            }
        };
        let inspected = match operation {
            "artifact.allocate" => None,
            _ => self.inspected_owner.lock().unwrap().clone(),
        };
        let owner = inspected
            .or_else(|| self.artifact_owner.lock().unwrap().clone())
            .unwrap_or_else(|| self.principal.lock().unwrap().clone());
        let mut packet = serde_json::json!({
            "schema": "runx.provider.operation.v1",
            "status": "success",
            "provider": "runx-artifact",
            "operation": operation,
            "target": reference,
            "result": result,
            "transport": "hosted_loopback",
            "readback_uri": "runx:artifact-readback:1",
            "access": if operation == "artifact.allocate" { "mutate" } else { "read" },
            "principal_uri": format!("runx:principal:{owner}"),
            "finality": "verified",
        });
        if operation == "artifact.allocate" {
            packet["idempotency_key"] = input["idempotency_key"].clone();
        }
        packet
    }

    /// List an active grant of `scopes` for `connector`'s exact binding.
    fn grant(
        &self,
        grant_id: &str,
        connector: &super::super::connectors::LoadedConnector,
        scopes: &[String],
    ) {
        self.grants.lock().unwrap().push(serde_json::json!({
            "grant_id": grant_id,
            "provider": connector.manifest.name.as_str(),
            "connector_binding": connector.binding(),
            "status": "active",
            "credential_material_bound": true,
            "scopes": scopes,
        }));
    }

    fn answer(&self, answer: Answer) {
        *self.answer.lock().unwrap() = answer;
    }

    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl crate::http::RuntimeHttpTransport for Provider {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        if request.method == HttpMethod::Get && request.url == format!("{BASE_URL}/v1/me") {
            let principal = self.principal.lock().unwrap().clone();
            return Ok(RuntimeHttpResponse::new(
                200,
                serde_json::json!({"status": "success", "principal": {"principal_id": principal}})
                    .to_string(),
            ));
        }
        if request.method == HttpMethod::Get && request.url == format!("{BASE_URL}/v1/grants") {
            let grants = self.grants.lock().unwrap().clone();
            return Ok(RuntimeHttpResponse::new(
                200,
                serde_json::json!({"status": "success", "grants": grants}).to_string(),
            ));
        }
        if request.url == "https://api.forge.fixture.invalid/oauth/token" {
            self.calls.lock().unwrap().push(
                serde_json::from_value(
                    serde_json::json!({"http_method": "POST", "url": request.url}),
                )
                .unwrap(),
            );
            return Ok(RuntimeHttpResponse::new(
                200,
                r#"{"access_token":"oauth-access-must-not-persist","token_type":"Bearer","expires_in":3600}"#,
            ));
        }
        if request.url == "https://api.forge.fixture.invalid/repos/acme/app/issues/7" {
            self.calls.lock().unwrap().push(
                serde_json::from_value(serde_json::json!({
                    "method": request.method.as_str(), "url": request.url,
                }))
                .unwrap(),
            );
            return Ok(
                if request.method == HttpMethod::Get
                    && *self.answer.lock().unwrap() == Answer::TimeOut
                {
                    RuntimeHttpResponse::new(503, "{}")
                } else {
                    RuntimeHttpResponse::new(
                        200,
                        serde_json::json!({
                            "number": 7, "title": "ready", "body": null, "state": "open",
                            "client_secret": "provider-private-value-must-not-persist",
                        })
                        .to_string(),
                    )
                },
            );
        }
        let body: JsonObject =
            serde_json::from_str(request.body.as_deref().unwrap_or("{}")).unwrap();
        self.calls.lock().unwrap().push(body.clone());
        if request.url == format!("{BASE_URL}/v1/artifact-operations") {
            let sent: serde_json::Value =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            return Ok(RuntimeHttpResponse::new(
                200,
                self.artifact(&sent).to_string(),
            ));
        }
        if request.url == format!("{BASE_URL}/v1/provider-requests") {
            // The issue as the forge answers both the write and its readback.
            let sent: serde_json::Value =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let number = sent["input"]["updates"][0]["number"].as_u64().unwrap_or(7);
            let issue = serde_json::json!({"number": number, "title": "ready", "body": null, "state": "open"});
            let answer = serde_json::json!({"status": 200, "body": issue.to_string()});
            return Ok(RuntimeHttpResponse::new(200, answer.to_string()));
        }
        if let Some(id) = request
            .url
            .strip_prefix("https://advisories.fixture.invalid/v1/vulns/")
        {
            let answer = *self.answer.lock().unwrap();
            if answer == Answer::TimeOut {
                return Err(RuntimeHttpError::Transport {
                    message: "advisory request timed out after sending".to_owned(),
                });
            }
            if answer == Answer::Reject || (answer == Answer::RejectAfterOne && self.calls() > 1) {
                return Ok(RuntimeHttpResponse::new(
                    404,
                    r#"{"code":5,"message":"Bug not found"}"#,
                ));
            }
            return Ok(RuntimeHttpResponse::new(
                200,
                serde_json::json!({"id": id, "summary": "advisory"}).to_string(),
            ));
        }
        if request
            .url
            .starts_with("https://acme.hooks.fixture.invalid/")
        {
            // A workflow webhook acknowledges with whatever the workflow answers.
            return Ok(RuntimeHttpResponse::new(
                200,
                r#"{"message":"Workflow was started"}"#,
            ));
        }
        let field = |name: &str| body.get(name).cloned().unwrap_or(JsonValue::Null);
        let key = body
            .get("input")
            .and_then(JsonValue::as_object)
            .and_then(|input| input.get("idempotency_key"))
            .cloned()
            .unwrap_or(JsonValue::Null);
        match *self.answer.lock().unwrap() {
            Answer::TimeOut => Err(RuntimeHttpError::Transport {
                message: "timed out after sending".to_owned(),
            }),
            Answer::Reject => Ok(RuntimeHttpResponse::new(
                400,
                r#"{"status":"error","error":{"code":"invalid_input","detail":"refused"}}"#,
            )),
            answer => {
                let operation = field("operation");
                let named = operation
                    .as_str()
                    .unwrap_or_default()
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let provider = match (answer, named.as_str()) {
                    (Answer::WrongProvider, _) => "fixture/forge",
                    (_, "jobs") => "fixture/compute",
                    _ => "fixture/chat",
                };
                let result = match provider {
                    "fixture/compute" => serde_json::json!({
                        "job_ref": "job-1",
                        "status": "queued",
                        "output_available": false,
                        "failure_reason": null,
                        "recovered": false,
                    }),
                    _ if answer == Answer::OffPattern => {
                        serde_json::json!({"message_locator": "https://example.com/m/1", "text": "hi"})
                    }
                    _ => {
                        serde_json::json!({"message_locator": "chat://T123/C456/1.0", "text": "hi"})
                    }
                };
                let response = serde_json::json!({
                    "status": "success",
                    "provider": provider,
                    "operation": field("operation"),
                    "target": field("target"),
                    "access": field("access"),
                    "result": result,
                    "readback_uri": "runx:provider-readback:1",
                    "operation_id": "m-1",
                    "idempotency_key": key,
                });
                Ok(RuntimeHttpResponse::new(200, response.to_string()))
            }
        }
    }
}

struct Approver {
    actor: Option<ResolutionResponseActor>,
    requests: Vec<ResolutionRequest>,
}

impl Approver {
    fn new(actor: Option<ResolutionResponseActor>) -> Self {
        Self {
            actor,
            requests: Vec::new(),
        }
    }
}

impl Host for Approver {
    fn report(&mut self, _event: ExecutionEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError> {
        self.requests.push(request);
        Ok(self.actor.clone().map(|actor| ResolutionResponse {
            actor,
            payload: JsonValue::Bool(true),
        }))
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }
}

struct Fixture {
    root: tempfile::TempDir,
    steps: BTreeMap<String, WorkflowStep>,
    env: BTreeMap<String, String>,
    run_id: &'static str,
    provider: Provider,
    effect: OperationEffect,
}

impl Fixture {
    fn new(scopes: &[&str]) -> Self {
        Self::for_connector("fixture/chat", scopes)
    }

    fn for_connector(connector: &str, scopes: &[&str]) -> Self {
        let root = tempfile::tempdir().unwrap();
        crate::kernel::connectors::plugin_with_fixture_connectors(
            root.path(),
            &[
                "fixture/chat",
                "fixture/compute",
                "fixture/advisories",
                "fixture/forge",
                "fixture/hooks",
            ],
        );
        let manifest = runx_parser::validate_method_manifest(
            "fixture",
            runx_parser::parse_method_manifest_yaml(SKILL).unwrap(),
        )
        .unwrap();
        let steps = manifest.methods["run"]
            .workflow
            .steps
            .iter()
            .map(|step| (step.id.clone(), step.clone()))
            .collect();
        let scopes = scopes
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect::<Vec<_>>();
        let env = BTreeMap::from([
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
            (
                crate::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
                root.path().join("receipts").to_string_lossy().into_owned(),
            ),
        ]);
        let provider = Provider::new(Answer::Succeed);
        let effect = OperationEffect::with_http_transport(provider.clone());
        let fixture = Self {
            root,
            steps,
            env,
            run_id: "run-1",
            provider,
            effect,
        };
        fixture.confirm_grant("grant_chat", connector, &scopes);
        fixture
    }

    /// List a grant of `scopes` for `connector` and confirm it as the
    /// connector's instance, as `runx connector use <connector> <grant>` would.
    fn confirm_grant(&self, grant_id: &str, connector: &str, scopes: &[String]) {
        self.list_grant(grant_id, connector, scopes);
        self.select_grant(grant_id, connector);
    }

    /// List a grant runx Connect holds without confirming it yet.
    fn list_grant(&self, grant_id: &str, connector: &str, scopes: &[String]) {
        let connector = crate::kernel::connectors::fixture_scope()
            .connector(connector)
            .unwrap();
        self.provider.grant(grant_id, &connector, scopes);
    }

    /// Confirm a listed grant as the connector's selected instance.
    fn select_grant(&self, grant_id: &str, connector: &str) {
        self.try_select_grant(grant_id, connector).unwrap();
    }

    fn try_select_grant(&self, grant_id: &str, connector: &str) -> Result<(), String> {
        let connector = crate::kernel::connectors::fixture_scope()
            .connector(connector)
            .unwrap();
        let instances = Instances::new(
            KernelStore::for_project(&crate::paths::RunxPaths::resolve(
                &self.env,
                self.root.path(),
            ))
            .unwrap(),
        );
        crate::kernel::setup::confirm_grant_over(
            &instances,
            &connector,
            Arc::new(self.provider.clone()),
            grant_id,
            &self.env,
            self.root.path(),
        )
        .map(|_| ())
    }

    /// Keep `value` as the secret `default` and bind `connector`'s
    /// `credential` to it, as `runx secret set` and `runx connector use
    /// --secret` would.
    fn set_credential(&self, connector: &str, credential: &str, value: &str) {
        let connector = crate::kernel::connectors::ConnectorScope::at(self.root.path())
            .unwrap()
            .connector(connector)
            .unwrap();
        keep_and_bind(
            &connector,
            "default",
            credential,
            value,
            &crate::services::WorkspaceEnv::new(self.env.clone(), self.root.path().to_path_buf())
                .unwrap(),
        );
    }

    fn admit(
        &self,
        step: &str,
        inputs: &JsonObject,
    ) -> Result<EffectAdmission, RuntimeEffectError> {
        self.admit_as(step, inputs, None)
    }

    fn admit_as(
        &self,
        step: &str,
        inputs: &JsonObject,
        run_authority: Option<&crate::RunAuthority>,
    ) -> Result<EffectAdmission, RuntimeEffectError> {
        self.admit_held(step, inputs, run_authority, false)
    }

    /// [`Self::admit_as`], with whether the floor holds the step's write
    /// under `effects: ask`.
    fn admit_held(
        &self,
        step: &str,
        inputs: &JsonObject,
        run_authority: Option<&crate::RunAuthority>,
        held_by_effects: bool,
    ) -> Result<EffectAdmission, RuntimeEffectError> {
        self.admit_from(step, inputs, run_authority, held_by_effects, &[])
    }

    /// [`Self::admit_held`], for a call made from the packages of `chain`.
    fn admit_from(
        &self,
        step: &str,
        inputs: &JsonObject,
        run_authority: Option<&crate::RunAuthority>,
        held_by_effects: bool,
        chain: &[crate::identity::ChainLink],
    ) -> Result<EffectAdmission, RuntimeEffectError> {
        let step = &self.steps[step];
        self.effect
            .admit(EffectStepRequest {
                step,
                target: ResolvedEffectTarget::default(),
                inputs,
                env: &self.env,
                workflow_dir: self.root.path(),
                run_id: self.run_id,
                run_authority,
                held_by_effects,
                chain,
                clock: &crate::RunClock::System,
            })
            .map(|admission| admission.expect("an operation step is owned by the effect"))
    }

    /// Authorize the call [`Self::admit_as`] would admit, as reuse asks of
    /// a read a source recorded.
    fn authorize_as(
        &self,
        step: &str,
        inputs: &JsonObject,
        run_authority: Option<&crate::RunAuthority>,
    ) -> Result<admission::Authorized, AdmitError> {
        let step = &self.steps[step];
        let target = step.operation().unwrap();
        let call = OperationCall::decode(inputs).unwrap();
        let paths = crate::paths::RunxPaths::resolve(&self.env, self.root.path());
        let store = KernelStore::for_project(&paths).unwrap();
        let scope = ConnectorScope::of_operation(target, self.root.path(), &paths).unwrap();
        let instances = Instances::new(Arc::clone(&store));
        admission::authorize(&Evaluation {
            step: target,
            call: &call,
            retry_unknown_outcomes: step.retry_unknown_outcomes,
            fixture_stand_in: self.effect.fixture_stand_in,
            run_authority,
            held_by_effects: false,
            store: &store,
            run_id: self.run_id,
            chain: &[],
            context: Context {
                scope: &scope,
                instances: &instances,
                hosted: &self.effect.placements.hosted,
                env: &self.env,
                cwd: self.root.path(),
            },
            clock: &crate::RunClock::System,
        })
    }

    fn prepare(
        &self,
        step: &str,
        admission: EffectAdmission,
        host: &mut dyn Host,
    ) -> Result<EffectPreparationOutcome, RuntimeEffectError> {
        self.effect
            .prepare_execution(&self.steps[step], admission, host)
    }

    /// Admit, approve as `actor`, and execute.
    fn run(
        &self,
        step: &str,
        inputs: &JsonObject,
        actor: Option<ResolutionResponseActor>,
    ) -> Result<(EffectAdmission, OperationOutcome), String> {
        self.run_dispatched(step, inputs, actor)
            .map(|(admission, outcome, _)| (admission, outcome))
    }

    /// [`Self::run`], with what the kernel handed the implementation.
    fn run_dispatched(
        &self,
        step: &str,
        inputs: &JsonObject,
        actor: Option<ResolutionResponseActor>,
    ) -> Result<(EffectAdmission, OperationOutcome, Option<Dispatched>), String> {
        self.run_under(step, inputs, actor, None)
    }

    /// [`Self::run_dispatched`] under the run's authority.
    fn run_under(
        &self,
        step: &str,
        inputs: &JsonObject,
        actor: Option<ResolutionResponseActor>,
        run_authority: Option<&crate::RunAuthority>,
    ) -> Result<(EffectAdmission, OperationOutcome, Option<Dispatched>), String> {
        let admission = self
            .admit_as(step, inputs, run_authority)
            .map_err(|error| error.to_string())?;
        let mut host = Approver::new(actor);
        let EffectPreparationOutcome::Ready(admission) = self
            .prepare(step, admission, &mut host)
            .map_err(|error| error.to_string())?
        else {
            return Err("pending".to_owned());
        };
        let context = admission.context::<OperationAdmission>().unwrap();
        let mut dispatched = None;
        let outcome = execute(
            context,
            step,
            &self.env,
            self.root.path(),
            &NoTools,
            &mut dispatched,
        )
        .map_err(|error| error.to_string())?;
        Ok((*admission, outcome, dispatched))
    }
}

#[test]
fn a_projected_acknowledgement_survives_readback_recovery_without_persisting_the_private_response()
{
    assert_projected_acknowledgement_recovers(false);
}

#[test]
fn a_financial_call_cannot_understate_its_provider_inputs_to_bypass_the_runs_limit() {
    let fixture = Fixture::for_connector("fixture/forge", &["repo.write"]);
    let path = fixture
        .root
        .path()
        .join("connectors/fixture/forge/connector.yaml");
    let mut manifest = serde_json::to_value(
        runx_parser::parse_connector_manifest(&std::fs::read_to_string(&path).unwrap()).unwrap(),
    )
    .unwrap();
    let operation = &mut manifest["operations"]["issues.update"];
    // This synthetic provider charges no call fee; its bound transfer amount
    // must be the reason the money limit rejects the operation.
    operation["cost"] = serde_json::json!({"usd_micro":0,"per":"request","source":"docs"});
    operation["approval"] = serde_json::json!("required");
    operation["amount"] = serde_json::json!({"units":{"$argument":"/amount_minor"},"currency":{"$argument":"/currency"}});
    operation["input_schema"]["properties"]["amount_minor"] =
        serde_json::json!({"type":"integer","minimum":1});
    operation["input_schema"]["properties"]["currency"] =
        serde_json::json!({"type":"string","pattern":"^[a-z]{3}$"});
    std::fs::write(path, serde_json::to_string(&manifest).unwrap()).unwrap();
    fixture.set_credential("fixture/forge", "token", "forge-local-token");
    let mut inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs":{"repository":"acme/app","updates":[{"number":7,"patch":{"title":"ready"}}],"amount_minor":100,"currency":"usd"},
        "idempotency_key":"bound-payment-1", "amount":{"units":1,"unit":"USD"},
    })).unwrap();
    let error = fixture
        .admit_as("update", &inputs, Some(&limited(10_000)))
        .unwrap_err();
    assert!(
        error.to_string().contains("bound provider inputs"),
        "{error}"
    );
    inputs.insert(
        "amount".to_owned(),
        serde_json::from_value(serde_json::json!({"units":100,"unit":"EUR"})).unwrap(),
    );
    let error = fixture.admit("update", &inputs).unwrap_err();
    assert!(
        error.to_string().contains("bound provider inputs"),
        "{error}"
    );
    inputs.insert(
        "amount".to_owned(),
        serde_json::from_value(serde_json::json!({"units":100,"unit":"USD"})).unwrap(),
    );
    let error = fixture
        .admit_as("update", &inputs, Some(&limited(10_000)))
        .unwrap_err();
    assert!(
        matches!(error, RuntimeEffectError::CostLimitExceeded { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("1000000"), "{error}");
    let admission = fixture
        .admit("update", &inputs)
        .expect("matching terms admit for exact approval");
    let context = admission.context::<OperationAdmission>().unwrap();
    let explicit_intent = context.admitted.intent_digest().clone();
    assert_eq!(
        context.admitted.admission.decision,
        AdmissionDecision::NeedsResolution,
        "the connector floor requires a person even without step approval text"
    );
    inputs.remove("amount");
    let derived = fixture
        .admit("update", &inputs)
        .expect("the provider input binding supplies the native amount");
    assert_eq!(
        derived
            .context::<OperationAdmission>()
            .unwrap()
            .admitted
            .intent_digest(),
        &explicit_intent,
        "an optional caller assertion cannot change the approved financial intent"
    );
    let error = fixture
        .admit_as("update", &inputs, Some(&limited(10_000)))
        .unwrap_err();
    assert!(
        matches!(error, RuntimeEffectError::CostLimitExceeded { .. })
            && error.to_string().contains("1000000"),
        "omitting a redundant amount must not omit its budget charge: {error}"
    );
    assert_eq!(
        context.admitted.intent().amount.as_ref().unwrap().units,
        100
    );
    assert_eq!(
        context
            .admitted
            .intent()
            .amount
            .as_ref()
            .unwrap()
            .unit
            .as_str(),
        "USD"
    );
    assert_eq!(
        fixture.provider.calls(),
        0,
        "no provider request leaves during amount admission"
    );
}

#[test]
fn an_oauth2_payment_transport_recovers_from_sqlite_without_repeating_the_write_or_retaining_tokens()
 {
    assert_projected_acknowledgement_recovers(true);
}

fn assert_projected_acknowledgement_recovers(oauth: bool) {
    let mut fixture = Fixture::for_connector("fixture/forge", &["repo.write"]);
    let path = fixture
        .root
        .path()
        .join("connectors/fixture/forge/connector.yaml");
    let mut manifest: serde_json::Value = serde_json::to_value(
        runx_parser::parse_connector_manifest(&std::fs::read_to_string(&path).unwrap()).unwrap(),
    )
    .unwrap();
    manifest["operations"]["issues.update"]["implementation"]["http"]["decode"] = serde_json::json!([
        {"op": "auto_json"},
        {"op": "project", "fields": {"number": "number", "title": "title", "body": "body", "state": "state"}},
    ]);
    if oauth {
        manifest["auth"].as_object_mut().unwrap().remove("session");
        manifest["auth"]
            .as_object_mut()
            .unwrap()
            .remove("delegated");
        manifest["placements"] = serde_json::json!({"local":{"auth":["credentials"]},"hosted":{"auth":"credential","credential":"token"}});
        // A credential placement carries no provider scopes.
        manifest["operations"]["issues.update"]
            .as_object_mut()
            .unwrap()
            .remove("provider_scopes");
        manifest["auth"]["credentials"]["token"]["kind"] = serde_json::json!("client_credentials");
        manifest["auth"]["credentials"]["token"]["fields"] =
            serde_json::json!([{"name":"client_id"},{"name":"client_secret"}]);
        manifest["operations"]["issues.update"]["implementation"]["http"]["auth"]["token"] = serde_json::json!({"type":"oauth2_client_credentials", "token_url":"https://api.forge.fixture.invalid/oauth/token", "client_id":"client_id", "client_secret":"client_secret"});
        // Keep this credential change scoped to the operation under test.
        manifest["operations"]
            .as_object_mut()
            .unwrap()
            .retain(|name, _| name == "issues.update");
    }
    std::fs::write(path, serde_json::to_string(&manifest).unwrap()).unwrap();
    fixture.set_credential(
        "fixture/forge",
        "token",
        if oauth {
            r#"{"client_id":"fixture-app","client_secret":"oauth-app-secret-must-not-persist"}"#
        } else {
            "forge-local-token"
        },
    );
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"repository": "acme/app", "updates": [{"number": 7, "patch": {"title": "ready"}}]},
        "idempotency_key": "projection-recovery-1",
    })).unwrap();
    fixture.provider.answer(Answer::TimeOut);
    fixture
        .run("update", &inputs, Some(ResolutionResponseActor::Human))
        .expect_err("readback is not available yet");
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(
        &fixture.env,
        fixture.root.path(),
    ))
    .unwrap();
    let connection = rusqlite::Connection::open_with_flags(
        store.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut statement = connection
        .prepare("SELECT data FROM kernel_events UNION ALL SELECT data FROM kernel_blobs")
        .unwrap();
    let records = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        records
            .iter()
            .any(|record| String::from_utf8_lossy(record).contains("acknowledged")),
        "the acknowledged write is durably recorded"
    );
    for record in records {
        let text = String::from_utf8_lossy(&record);
        assert!(!text.contains("provider-private-value-must-not-persist"));
        assert!(!text.contains("client_secret"));
        assert!(!text.contains("oauth-access-must-not-persist"));
        assert!(!text.contains("oauth-app-secret-must-not-persist"));
    }
    // A new effect service reads the existing acknowledgement from SQLite.
    fixture.effect = OperationEffect::with_http_transport(fixture.provider.clone());
    fixture.provider.answer(Answer::Succeed);
    let (_, outcome) = fixture
        .run("update", &inputs, Some(ResolutionResponseActor::Human))
        .expect("the original write reads back");
    assert_eq!(packet(&outcome)["finality"].as_str(), Some("confirmed"));
    let calls = sent(&fixture);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["method"] == "PATCH")
            .count(),
        1,
        "the effect was submitted once"
    );
    assert_eq!(
        calls.iter().filter(|call| call["method"] == "GET").count(),
        2,
        "recovery reads back without submitting again"
    );
    if oauth {
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["url"] == "https://api.forge.fixture.invalid/oauth/token")
                .count(),
            2,
            "a restarted operation reacquires auth without replaying its acknowledged effect"
        );
    }
    assert!(
        !serde_json::to_string(packet(&outcome))
            .unwrap()
            .contains("provider-private-value-must-not-persist")
    );
}

fn post(text: &str) -> JsonObject {
    serde_json::from_value(serde_json::json!({
        "inputs": {"channel_locator": "chat://T123/C456", "text": text},
        "idempotency_key": "post-1",
    }))
    .unwrap()
}

fn packet(outcome: &OperationOutcome) -> &JsonObject {
    outcome.value.as_object().unwrap()["provider_operation"]
        .as_object()
        .unwrap()
}

#[test]
fn an_approved_post_binds_its_intent_term_and_readback() {
    let fixture = Fixture::new(&["channel.post", "channel.post.read"]);
    let (admission, outcome, dispatched) = fixture
        .run_dispatched(
            "post",
            &post("deploy done"),
            Some(ResolutionResponseActor::Human),
        )
        .expect("runs");
    let context = admission.context::<OperationAdmission>().unwrap();
    let packet = packet(&outcome);
    assert_eq!(
        packet["finality"],
        JsonValue::String("confirmed".to_owned())
    );
    assert_eq!(
        packet["plan_digest"].as_str(),
        Some(context.admitted.intent_digest().as_str())
    );
    assert_eq!(
        packet["idempotency_key"].as_str(),
        context.provider_key().as_deref()
    );
    assert!(
        packet["idempotency_key"].as_str().unwrap().contains('-'),
        "chat keys are UUIDs"
    );
    assert_eq!(
        packet["operation_digest"].as_str(),
        Some(context.admitted.intent().operation_digest.as_str())
    );
    let admitted = &context.admitted.admission;
    assert_eq!(
        admitted.decision,
        crate::kernel::admission::AdmissionDecision::Admit
    );
    let term = admitted.execution_term.as_ref().expect("execution term");
    assert_eq!(term.approvals.len(), 1);
    let derivation = admitted.derivation.as_ref().expect("derivation");
    assert_eq!(derivation.proofs.len(), 1);
    let dispatched = dispatched.expect("the call reached its implementation");
    assert_eq!(dispatched.boundary, ExecutionBoundaryKind::RemoteProvider);
    assert!(
        dispatched.credential.is_none(),
        "a hosted call hands no process a credential"
    );
    let sent = fixture.provider.calls.lock().unwrap()[0].clone();
    assert!(sent.contains_key("operation_digest"));
}

#[test]
fn the_floor_cannot_be_skipped_by_omitting_approval_text() {
    let fixture = Fixture::new(&["channel.post"]);
    let admission = fixture.admit("post", &post("hi")).expect("admits");
    let context = admission.context::<OperationAdmission>().unwrap();
    assert_eq!(
        context.admitted.admission.decision,
        crate::kernel::admission::AdmissionDecision::NeedsResolution
    );
    let mut host = Approver::new(None);
    assert!(matches!(
        fixture
            .prepare("post", admission, &mut host)
            .expect("prepares"),
        EffectPreparationOutcome::Pending { .. }
    ));
    assert_eq!(host.requests.len(), 1);
    assert_eq!(fixture.provider.calls(), 0);
}

#[test]
fn an_agent_cannot_stand_in_for_a_person() {
    let fixture = Fixture::new(&["channel.post"]);
    let error = fixture
        .run("post", &post("hi"), Some(ResolutionResponseActor::Agent))
        .expect_err("refused");
    assert!(error.contains("host-attested human"), "{error}");
    assert_eq!(fixture.provider.calls(), 0);
}

#[test]
fn a_read_never_asks_and_never_takes_an_approval() {
    let fixture = Fixture::new(&["channel.post.read"]);
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"conversation_locator": "chat://T123/C456"},
    }))
    .unwrap();
    let (_, outcome) = fixture.run("read", &inputs, None).expect("reads");
    assert_eq!(
        packet(&outcome)["access"],
        JsonValue::String("read".to_owned())
    );
    let mut gated = inputs.clone();
    gated.insert(
        "approval".to_owned(),
        serde_json::from_value(serde_json::json!({"reason": "read it"})).unwrap(),
    );
    assert!(fixture.admit("read", &gated).is_err());
}

/// A run's `packages` covers only calls made from a package: a call a
/// workflow makes from no package is refused under it, never let through by
/// an empty chain (D31).
#[test]
fn a_runs_packages_refuse_a_call_made_from_no_package() {
    let fixture = Fixture::new(&["channel.post.read"]);
    let read: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"conversation_locator": "chat://T123/C456"}
    }))
    .unwrap();
    fixture
        .admit_as("read", &read, None)
        .expect("no packages, no bound");
    let bounded = crate::RunAuthority {
        packages: Some(vec![
            runx_contracts::PackagePattern::new("project:work").unwrap(),
        ]),
        ..crate::RunAuthority::default()
    };
    let refused = fixture.admit_as("read", &read, Some(&bounded)).unwrap_err();
    assert!(
        refused.to_string().contains("made from no package"),
        "{refused}"
    );
}

/// A run's `packages` narrows whichever instance a call resolves, one taken
/// from an environment variable included, refusing before any attempt and
/// naming the package it doesn't cover (D31, the 0state-a3 probe's table).
#[test]
fn a_runs_packages_bound_an_instance_taken_from_the_environment() {
    let mut fixture = Fixture::new(&["channel.post.read"]);
    fixture
        .env
        .insert("FORGE_TOKEN".to_owned(), "forge-token".to_owned());
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"repository": "acme/app", "numbers": [1]}
    }))
    .unwrap();
    let link = |package: &str| crate::identity::ChainLink {
        package: runx_contracts::PackagePattern::new(package).unwrap(),
        owner: package.to_owned(),
        root: std::path::PathBuf::from("/packages").join(package),
        digest: format!("sha256:{}", "0".repeat(64)),
    };
    let chain = [link("project:work"), link("community-pub/forge-pusher")];
    let bounded = |packages: &[&str]| crate::RunAuthority {
        packages: Some(
            packages
                .iter()
                .map(|package| runx_contracts::PackagePattern::new(*package).unwrap())
                .collect(),
        ),
        ..crate::RunAuthority::default()
    };
    let refused = fixture
        .admit_from(
            "issues",
            &inputs,
            Some(&bounded(&["project:work"])),
            false,
            &chain,
        )
        .unwrap_err();
    assert!(
        refused.to_string().contains("community-pub/forge-pusher"),
        "{refused}"
    );
    assert_eq!(fixture.provider.calls(), 0);
    for covering in [
        &["project:work", "community-pub/forge-pusher"][..],
        &["project:*", "community-pub/*"],
    ] {
        let admission = fixture
            .admit_from("issues", &inputs, Some(&bounded(covering)), false, &chain)
            .expect("the chain is covered");
        let context = admission.context::<OperationAdmission>().unwrap();
        assert!(matches!(
            context.admitted.instance.authority,
            runx_contracts::InstanceAuthority::Environment { .. }
        ));
        let term = context.admitted.admission.execution_term.as_ref().unwrap();
        assert_eq!(
            term.bounds
                .packages
                .iter()
                .map(runx_contracts::PackagePattern::as_str)
                .collect::<Vec<_>>(),
            ["community-pub/forge-pusher", "project:work"],
            "the execution term names the chain"
        );
    }
    assert!(
        fixture
            .admit_from("issues", &inputs, Some(&bounded(&[])), false, &chain)
            .is_err(),
        "an empty list covers nothing"
    );
}

#[test]
fn acting_for_another_principal_requires_a_bounded_delegation() {
    let mut fixture = Fixture::new(&["channel.post", "channel.post.read"]);
    let read: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"conversation_locator": "chat://T123/C456"}
    }))
    .unwrap();
    let actor_ref = Reference::with_uri(ReferenceType::Principal, "runx:principal:assistant:test");
    let mut authority = crate::RunAuthority {
        grants: std::collections::BTreeMap::new(),
        actor_ref: Some(actor_ref.clone()),
        mandate_ref: Some(Reference::runx(ReferenceType::Mandate, "test-mandate")),
        delegations: Vec::new(),
        max_fanout: None,
        max_child_depth: None,
        standing_approvals: Vec::new(),
        max_spend_micro_usd: None,
        ..crate::RunAuthority::default()
    };
    assert!(fixture.admit_as("read", &read, Some(&authority)).is_err());

    authority.delegations.push(delegation(
        "fixture/chat",
        Reference::runx(
            ReferenceType::ConnectorInstance,
            crate::kernel::instances::instance_id(&["hosted", "fixture/chat", "grant_chat"])
                .unwrap()
                .as_str(),
        ),
        actor_ref,
        vec![AuthorityVerb::Read],
        AuthorityBounds {
            operations: vec!["fixture/chat/channel.post.read".into()],
            scopes: vec!["channel.post.read".into()],
            targets: vec!["chat://T123/C456".into()],
            uses: None,
            packages: Vec::new(),
        },
    ));
    let mandate_ref = authority.mandate_ref.take();
    assert!(fixture.admit_as("read", &read, Some(&authority)).is_err());
    authority.mandate_ref = mandate_ref;
    let admitted = fixture
        .admit_as("read", &read, Some(&authority))
        .expect("the bounded read is delegated");
    let context = admitted.context::<OperationAdmission>().unwrap();
    assert_eq!(context.admitted.delegations.len(), 1);
    assert_eq!(
        context
            .admitted
            .admission
            .derivation
            .as_ref()
            .unwrap()
            .evidence[1]
            .reference,
        authority.mandate_ref.clone().unwrap()
    );
    assert_eq!(
        context
            .admitted
            .admission
            .execution_term
            .as_ref()
            .unwrap()
            .principal_ref,
        authority.actor_ref.clone().unwrap()
    );
    assert!(
        fixture
            .admit_as("post", &post("hi"), Some(&authority))
            .is_err()
    );
    with_uses(
        &mut authority.delegations[0],
        Some(runx_contracts::AuthorityUseLimit {
            total: Some(1),
            per_day: None,
        }),
    );
    let admission = fixture
        .admit_as("read", &read, Some(&authority))
        .expect("the bounded use has capacity");
    let mut host = Approver::new(None);
    let prepared = fixture.prepare("read", admission, &mut host).unwrap();
    assert!(
        matches!(prepared, EffectPreparationOutcome::Ready(_)),
        "read needs no approval"
    );
    let EffectPreparationOutcome::Ready(prepared) = prepared else {
        return;
    };
    execute(
        prepared.context::<OperationAdmission>().unwrap(),
        "read",
        &fixture.env,
        fixture.root.path(),
        &NoTools,
        &mut None,
    )
    .expect("the first call reserves the use");
    authority.mandate_ref = Some(Reference::runx(ReferenceType::Mandate, "another-mandate"));
    assert!(
        fixture.admit_as("read", &read, Some(&authority)).is_err(),
        "an unsettled attempt cannot change its mandate evidence"
    );
    authority.mandate_ref = Some(Reference::runx(ReferenceType::Mandate, "test-mandate"));
    fixture.run_id = "run-2";
    let error = fixture
        .admit_as("read", &read, Some(&authority))
        .expect_err("another run exceeds the delegation's use cap");
    assert!(error.to_string().contains("limit is exhausted"), "{error}");
    assert_eq!(fixture.provider.calls(), 1);
}

/// A plain narrowing names no actor, mandate or principal (authority.md): the
/// instance's own principal makes the call under its term, and the run is
/// what stated it. Naming another actor takes the run's actor as well.
#[test]
fn a_plain_narrowing_names_nobody_and_another_actor_takes_the_runs_actor() {
    let fixture = Fixture::new(&["channel.post", "channel.post.read"]);
    let read: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"conversation_locator": "chat://T123/C456"}
    }))
    .unwrap();
    let instance = Reference::runx(
        ReferenceType::ConnectorInstance,
        crate::kernel::instances::instance_id(&["hosted", "fixture/chat", "grant_chat"])
            .unwrap()
            .as_str(),
    );
    let assistant = Reference::with_uri(ReferenceType::Principal, "runx:principal:assistant:test");
    let mut plain = delegation(
        "fixture/chat",
        instance,
        assistant.clone(),
        vec![AuthorityVerb::Read],
        AuthorityBounds {
            operations: vec!["fixture/chat/channel.post.read".into()],
            scopes: vec!["channel.post.read".into()],
            targets: vec!["chat://T123/*".into()],
            uses: None,
            packages: Vec::new(),
        },
    );
    plain.principal_ref = None;
    let authority = crate::RunAuthority {
        delegations: vec![plain.clone()],
        ..crate::RunAuthority::default()
    };
    let admitted = fixture
        .admit_as("read", &read, Some(&authority))
        .expect("a plain narrowing needs no actor or mandate");
    let context = admitted.context::<OperationAdmission>().unwrap();
    let derivation = context.admitted.admission.derivation.as_ref().unwrap();
    assert_eq!(
        derivation.evidence[1].reference,
        Reference::runx(ReferenceType::Run, fixture.run_id),
        "the run stated it"
    );
    assert_eq!(
        context
            .admitted
            .admission
            .execution_term
            .as_ref()
            .unwrap()
            .principal_ref,
        context.admitted.capability.term.principal_ref,
        "the instance's principal makes the call"
    );
    // Its term still bounds the call.
    let elsewhere: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"conversation_locator": "chat://T999/C1"}
    }))
    .unwrap();
    assert!(
        fixture
            .admit_as("read", &elsewhere, Some(&authority))
            .is_err()
    );

    // A delegation to another actor the run doesn't name as who acts is
    // refused, and so is another actor acting under a plain narrowing.
    let mut to_assistant = plain.clone();
    to_assistant.principal_ref = Some(assistant.clone());
    for authority in [
        crate::RunAuthority {
            delegations: vec![to_assistant],
            mandate_ref: Some(Reference::runx(ReferenceType::Mandate, "test-mandate")),
            ..crate::RunAuthority::default()
        },
        crate::RunAuthority {
            actor_ref: Some(assistant.clone()),
            mandate_ref: Some(Reference::runx(ReferenceType::Mandate, "test-mandate")),
            delegations: vec![plain.clone()],
            ..crate::RunAuthority::default()
        },
    ] {
        assert!(fixture.admit_as("read", &read, Some(&authority)).is_err());
    }
}

/// A grant is confirmed only for the exact connector Connect executes it
/// under: another revision, or none, is refused.
#[test]
fn a_grant_bound_to_another_revision_or_none_is_never_confirmed() {
    let fixture = Fixture::new(&["channel.post"]);
    let chat = crate::kernel::connectors::fixture_scope()
        .connector("fixture/chat")
        .unwrap();
    let mut binding = chat.binding();
    binding.connector_digest =
        runx_contracts::connector::Digest::new(format!("sha256:{}", "0".repeat(64))).unwrap();
    fixture.provider.grants.lock().unwrap().extend([
        serde_json::json!({
            "grant_id": "grant_chat_other",
            "provider": "fixture/chat",
            "connector_binding": binding,
            "status": "active",
            "scopes": ["channel.post"],
        }),
        serde_json::json!({
            "grant_id": "grant_chat_unbound",
            "provider": "fixture/chat",
            "status": "active",
            "scopes": ["channel.post"],
        }),
    ]);

    let other = fixture
        .try_select_grant("grant_chat_other", "fixture/chat")
        .unwrap_err();
    assert!(
        other.contains("does not bind connector fixture/chat"),
        "{other}"
    );
    let unbound = fixture
        .try_select_grant("grant_chat_unbound", "fixture/chat")
        .unwrap_err();
    assert!(unbound.contains("names no connector"), "{unbound}");
}

#[test]
fn unusable_hosted_material_refuses_before_a_provider_call() {
    let fixture = Fixture::new(&["channel.post"]);
    for grant in fixture.provider.grants.lock().unwrap().iter_mut() {
        grant["credential_material_bound"] = serde_json::json!(false);
    }
    let error = fixture
        .admit("post", &post("hi"))
        .expect_err("unusable material")
        .to_string();
    assert!(error.contains("no usable credential"), "{error}");
    assert!(sent(&fixture).is_empty());
}

#[test]
fn a_grant_that_lacks_the_scope_refuses_before_anything_is_sent() {
    let fixture = Fixture::new(&["channel.post.read"]);
    let error = fixture
        .admit("post", &post("hi"))
        .expect_err("refused")
        .to_string();
    assert!(error.contains("grant_chat does not cover"), "{error}");
}

#[test]
fn a_target_outside_the_declared_pattern_is_refused_before_anything_is_sent() {
    let fixture = Fixture::new(&["channel.post"]);
    let call: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"channel_locator": "chat://T123/C456/extra", "text": "hi"},
        "idempotency_key": "post-1",
    }))
    .unwrap();
    let error = fixture
        .admit("post", &call)
        .expect_err("refused")
        .to_string();
    assert!(error.contains("does not have the shape"), "{error}");
    assert_eq!(fixture.provider.calls(), 0);
}

#[test]
fn input_outside_the_operation_schema_is_refused_before_anything_is_sent() {
    let fixture = Fixture::new(&["channel.post"]);
    let ids = |count: usize| {
        let ids = (0..count)
            .map(|index| format!("ADV-{index}"))
            .collect::<Vec<_>>();
        serde_json::from_value::<JsonObject>(serde_json::json!({"inputs": {"ids": ids}})).unwrap()
    };
    fixture
        .admit("advisories", &ids(50))
        .expect("the schema admits 50 ids");
    let error = fixture
        .admit("advisories", &ids(51))
        .expect_err("refused")
        .to_string();
    assert!(
        error.contains("does not satisfy the operation's schema"),
        "{error}"
    );
    assert!(
        !error.contains("ADV-50"),
        "the refusal names the path, never the value: {error}"
    );
    assert_eq!(fixture.provider.calls(), 0);
}

/// Price the advisory read in the fixture's plugin, as a provider that
/// bills per request declares it.
fn price_advisories(fixture: &Fixture, usd_micro: u64) {
    price_advisories_per(fixture, usd_micro, "request");
}

fn price_advisories_per(fixture: &Fixture, usd_micro: u64, per: &str) {
    advisory_cost(
        fixture,
        &format!("usd_micro: {usd_micro}, per: {per}, source: docs"),
    );
}

fn advisory_cost(fixture: &Fixture, declaration: &str) {
    let path = fixture
        .root
        .path()
        .join("connectors/fixture/advisories/connector.yaml");
    let yaml = std::fs::read_to_string(&path).unwrap().replace(
        "    result_fields: [advisories]\n",
        &format!("    result_fields: [advisories]\n    cost: {{{declaration}}}\n"),
    );
    std::fs::write(path, yaml).unwrap();
}

fn advisories(id: &str) -> JsonObject {
    serde_json::from_value(serde_json::json!({"inputs": {"ids": [id]}})).unwrap()
}

/// A run bounded only by what it may spend.
fn limited(max_spend_micro_usd: u64) -> crate::RunAuthority {
    crate::RunAuthority {
        grants: std::collections::BTreeMap::new(),
        actor_ref: None,
        mandate_ref: None,
        delegations: Vec::new(),
        max_fanout: None,
        max_child_depth: None,
        standing_approvals: Vec::new(),
        max_spend_micro_usd: Some(max_spend_micro_usd),
        ..crate::RunAuthority::default()
    }
}

#[test]
fn a_priced_call_spends_from_the_runs_limit_and_one_over_it_sends_nothing() {
    let fixture = Fixture::new(&["channel.post"]);
    price_advisories(&fixture, 5_000);
    let authority = limited(8_000);
    let (admission, outcome, _) = fixture
        .run_under("advisories", &advisories("ADV-1"), None, Some(&authority))
        .unwrap();
    let context = admission.context::<OperationAdmission>().unwrap();
    assert!(
        context
            .admitted
            .admission
            .reasons
            .iter()
            .any(
                |reason| reason.check == runx_contracts::AdmissionCheck::Cost
                    && reason.outcome == runx_contracts::CheckOutcome::Pass
            )
    );
    assert_eq!(
        packet(&outcome)["cost"],
        serde_json::from_value::<JsonValue>(serde_json::json!({
            "quoted_usd_micro": 5000,
            "settled_usd_micro": 5000,
            "payer": "provider_account",
        }))
        .unwrap()
    );
    let sent = fixture.provider.calls();
    assert_eq!(sent, 1);

    // 3,000 is left: another 5,000 call is refused before anything is sent.
    let refused = fixture
        .admit_as("advisories", &advisories("ADV-2"), Some(&authority))
        .expect_err("over the limit");
    assert!(
        matches!(refused, RuntimeEffectError::CostLimitExceeded { .. }),
        "{refused}"
    );
    assert!(
        refused.to_string().contains("cost_limit_exceeded"),
        "{refused}"
    );
    assert_eq!(fixture.provider.calls(), sent);
}

#[test]
fn a_call_billed_on_success_gives_its_spend_back_when_refused() {
    let fixture = Fixture::new(&["channel.post"]);
    price_advisories_per(&fixture, 5_000, "success");
    let authority = limited(5_000);
    fixture.provider.answer(Answer::Reject);
    assert!(
        fixture
            .run_under("advisories", &advisories("ADV-1"), None, Some(&authority))
            .is_err()
    );
    // Nothing was billable, so the whole limit is left for the next call.
    fixture.provider.answer(Answer::Succeed);
    fixture
        .run_under("advisories", &advisories("ADV-2"), None, Some(&authority))
        .expect("the refused call spent nothing");
}

#[test]
fn a_run_without_a_limit_spends_as_it_always_has() {
    let fixture = Fixture::new(&["channel.post"]);
    price_advisories(&fixture, 5_000);
    let (admission, outcome, _) = fixture
        .run_under("advisories", &advisories("ADV-1"), None, None)
        .unwrap();
    let context = admission.context::<OperationAdmission>().unwrap();
    assert!(
        !context
            .admitted
            .admission
            .reasons
            .iter()
            .any(|reason| reason.check == runx_contracts::AdmissionCheck::Cost),
        "an unlimited run records no cost check"
    );
    assert!(context.admitted.spend.is_none());
    // The packet still says what the call cost.
    assert_eq!(
        packet(&outcome)["cost"].as_object().unwrap()["quoted_usd_micro"],
        JsonValue::Number(runx_contracts::JsonNumber::U64(5000))
    );
}

#[test]
fn readback_pending_preserves_both_finality_and_charge_retry_safety() {
    for declaration in [
        None,
        Some("usd_micro: 0, per: request, source: docs"),
        Some("usd_micro: 5000, per: request, source: docs"),
    ] {
        let metered = declaration.is_some_and(|cost| cost.starts_with("usd_micro: 5000"));
        for answer in [Answer::Reject, Answer::TimeOut] {
            let fixture = Fixture::new(&["channel.post"]);
            if let Some(cost) = declaration {
                advisory_cost(&fixture, cost);
            }
            fixture.provider.answer(answer);
            let mut inputs = advisories("ADV-1");
            inputs.insert("confirms".to_owned(), JsonValue::Object(JsonObject::new()));
            let admission = fixture.admit("advisories", &inputs).unwrap();
            let mut host = Approver::new(None);
            let admission = match fixture.prepare("advisories", admission, &mut host).unwrap() {
                EffectPreparationOutcome::Ready(admission) => Some(admission),
                _ => None,
            }
            .expect("read admits");
            let context = admission.context::<OperationAdmission>().unwrap();
            let error = execute(
                context,
                "advisories",
                &fixture.env,
                fixture.root.path(),
                &NoTools,
                &mut None,
            )
            .unwrap_err();
            let expected = if metered {
                runx_contracts::RetryResend::Unsafe
            } else {
                runx_contracts::RetryResend::Safe
            };
            assert!(
                matches!(&error, RuntimeError::ProviderReadbackPending { resend, .. } if *resend == expected),
                "{error}"
            );
            assert_eq!(fixture.provider.calls(), 1);
            if metered && answer == Answer::TimeOut {
                fixture.provider.answer(Answer::Succeed);
                assert!(fixture.run("advisories", &inputs, None).is_err());
                assert_eq!(
                    fixture.provider.calls(),
                    1,
                    "readback finality cannot authorize another uncertain charge"
                );
            }
        }
    }
}

#[test]
fn readiness_reports_the_same_unpriced_usage_gate_and_money_refusal_as_admission() {
    let fixture = Fixture::new(&["channel.post"]);
    advisory_cost(&fixture, "per: request, source: docs");
    let scope = crate::ConnectorScope::at(fixture.root.path()).unwrap();
    let connector = scope.connector("fixture/advisories").unwrap();
    let readiness =
        admission::connector_readiness(&connector, &fixture.env, fixture.root.path()).unwrap();
    assert!(readiness.iter().any(|item| {
        item.reasons.iter().any(|reason| {
            reason.check == AdmissionCheck::Approval
                && reason.outcome == runx_contracts::CheckOutcome::Required
                && reason.detail.as_str().contains("provider-account credits")
        })
    }));
    let manifest = runx_parser::validate_method_manifest(
        "fixture",
        runx_parser::parse_method_manifest_yaml(SKILL).unwrap(),
    )
    .unwrap();
    let readiness = admission::skill_readiness(
        fixture.root.path(),
        &manifest.connectors,
        &[],
        Some(&limited(10000)),
        &[],
        &fixture.env,
        fixture.root.path(),
    )
    .unwrap();
    assert!(
        readiness
            .iter()
            .filter(|item| item.requirement.as_str() == "fixture/advisories")
            .any(|item| item
                .reasons
                .iter()
                .any(|reason| reason.check == AdmissionCheck::Cost
                    && reason.outcome == runx_contracts::CheckOutcome::Fail))
    );
    assert_eq!(fixture.provider.calls(), 0);
}

#[test]
fn an_unpriced_metered_read_asks_once_and_never_invents_a_dollar_price() {
    let fixture = Fixture::new(&["channel.post"]);
    advisory_cost(&fixture, "per: request, source: docs");
    assert_eq!(
        fixture
            .run("advisories", &advisories("ADV-1"), None)
            .unwrap_err(),
        "pending"
    );
    assert_eq!(fixture.provider.calls(), 0);
    assert!(
        fixture
            .run(
                "advisories",
                &advisories("ADV-1"),
                Some(ResolutionResponseActor::Agent)
            )
            .is_err()
    );
    assert_eq!(fixture.provider.calls(), 0);
    let (_, outcome) = fixture
        .run(
            "advisories",
            &advisories("ADV-1"),
            Some(ResolutionResponseActor::Human),
        )
        .unwrap();
    assert_eq!(
        packet(&outcome)["cost"],
        JsonValue::from_serialize(&serde_json::json!({"payer":"provider_account"})).unwrap()
    );
    assert_eq!(fixture.provider.calls(), 1);
}

#[test]
fn bounded_provider_usage_authorizes_an_unpriced_read_without_an_extra_approval() {
    let fixture = Fixture::new(&["channel.post"]);
    advisory_cost(&fixture, "per: request, source: docs");
    let inspection = fixture.admit("advisories", &advisories("ADV-1")).unwrap();
    let context = inspection.context::<OperationAdmission>().unwrap();
    let mut authority = limited(u64::MAX);
    authority.max_spend_micro_usd = None;
    authority.actor_ref = Some(context.admitted.actor_ref.clone());
    authority.mandate_ref = Some(Reference::runx(
        ReferenceType::Mandate,
        "one-provider-lookup",
    ));
    authority.delegations.push(delegation(
        "fixture/advisories",
        context.admitted.capability.term.resource_ref.clone(),
        context.admitted.actor_ref.clone(),
        vec![AuthorityVerb::Read],
        AuthorityBounds {
            operations: vec!["fixture/advisories/vulnerabilities.read".into()],
            scopes: vec!["vulnerabilities.read".into()],
            targets: vec![context.admitted.target.clone().into()],
            uses: Some(runx_contracts::AuthorityUseLimit {
                total: Some(1),
                per_day: None,
            }),
            packages: Vec::new(),
        },
    ));
    with_uses(
        &mut authority.delegations[0],
        Some(runx_contracts::AuthorityUseLimit::default()),
    );
    assert_eq!(
        fixture
            .run_under("advisories", &advisories("ADV-1"), None, Some(&authority))
            .unwrap_err(),
        "pending"
    );
    assert_eq!(fixture.provider.calls(), 0);
    with_uses(
        &mut authority.delegations[0],
        Some(runx_contracts::AuthorityUseLimit {
            total: Some(1),
            per_day: None,
        }),
    );
    fixture
        .run_under("advisories", &advisories("ADV-1"), None, Some(&authority))
        .unwrap();
    assert!(
        fixture
            .admit_as("advisories", &advisories("ADV-2"), Some(&authority))
            .is_err()
    );
    assert_eq!(fixture.provider.calls(), 1);
}

#[test]
fn a_money_limit_refuses_unknown_and_unpriced_quotes_before_dispatch() {
    for declared in [false, true] {
        let fixture = Fixture::new(&["channel.post"]);
        if declared {
            advisory_cost(&fixture, "per: request, source: docs");
        }
        let error = fixture
            .admit_as("advisories", &advisories("ADV-1"), Some(&limited(5_000)))
            .unwrap_err();
        assert!(
            matches!(error, RuntimeEffectError::CostLimitExceeded { .. }),
            "{error}"
        );
        assert!(error.to_string().contains("no USD quote"), "{error}");
        assert_eq!(fixture.provider.calls(), 0);
    }
}

/// Authorization is admission's checks of the run's terms and evidence with
/// nothing charged: a paid read a zero-spend run can't make is still one it
/// may make, so a result reused by reference doesn't fail that run, and
/// what the run's terms refuse, authorization refuses as admission does.
#[test]
fn authorization_charges_nothing_and_refuses_what_the_terms_refuse() {
    let fixture = Fixture::new(&["channel.post"]);
    price_advisories(&fixture, 5_000);
    let zero_spend = limited(0);
    let refused = fixture
        .admit_as("advisories", &advisories("ADV-1"), Some(&zero_spend))
        .expect_err("a zero-spend run can't make a paid read");
    assert!(
        matches!(refused, RuntimeEffectError::CostLimitExceeded { .. }),
        "{refused}"
    );
    fixture
        .authorize_as("advisories", &advisories("ADV-1"), Some(&zero_spend))
        .expect("authorization charges nothing");

    let packaged = crate::RunAuthority {
        packages: Some(vec![
            runx_contracts::PackagePattern::new("project:work").unwrap(),
        ]),
        ..limited(0)
    };
    let refused = fixture
        .authorize_as("advisories", &advisories("ADV-1"), Some(&packaged))
        .err()
        .expect("the run's packages cover no package the call is made from");
    assert!(
        matches!(
            &refused,
            AdmitError::Refused(refusal) if refusal.check == AdmissionCheck::Delegation
                && refusal.message().contains("made from no package")
        ),
        "{refused:?}"
    );
    assert_eq!(fixture.provider.calls(), 0);
}

#[test]
fn a_metered_read_timeout_keeps_its_spend_and_refuses_a_blind_resend() {
    let mut fixture = Fixture::new(&["channel.post"]);
    price_advisories_per(&fixture, 5_000, "success");
    let authority = limited(5_000);
    fixture.provider.answer(Answer::TimeOut);
    let error = fixture
        .run_under("advisories", &advisories("ADV-1"), None, Some(&authority))
        .unwrap_err();
    assert!(error.contains("unknown"), "{error}");
    // A fresh effect reopens the same kernel store instead of relying on
    // a transport's memory of the first dispatch.
    fixture.effect = OperationEffect::with_http_transport(fixture.provider.clone());
    fixture.provider.answer(Answer::Succeed);
    let error = fixture
        .run_under("advisories", &advisories("ADV-1"), None, Some(&authority))
        .unwrap_err();
    assert!(error.contains("cannot be resent safely"), "{error}");
    assert!(
        fixture
            .admit_as("advisories", &advisories("ADV-2"), Some(&authority))
            .is_err()
    );
    assert_eq!(fixture.provider.calls(), 1);
}

/// Prices the advisory read at 5,000 and gives it a checking read first.
fn price_advisories_with_a_precondition(fixture: &Fixture) -> std::path::PathBuf {
    price_advisories(fixture, 5_000);
    let path = fixture
        .root
        .path()
        .join("connectors/fixture/advisories/connector.yaml");
    let yaml = std::fs::read_to_string(&path).unwrap().replace(
        "        allowed_hosts: [advisories.fixture.invalid]\n",
        "        allowed_hosts: [advisories.fixture.invalid]\n        preconditions:\n          - request: {id: exists, http_method: GET, url: \"https://advisories.fixture.invalid/v1/vulns/exists\"}\n            expected_status: 200\n",
    );
    std::fs::write(&path, yaml).unwrap();
    path
}

#[test]
fn a_priced_call_keeps_its_spend_when_its_precondition_times_out() {
    let mut fixture = Fixture::new(&["channel.post"]);
    price_advisories_with_a_precondition(&fixture);
    let authority = limited(5_000);
    fixture.provider.answer(Answer::TimeOut);
    assert!(
        fixture
            .run_under("advisories", &advisories("ADV-1"), None, Some(&authority))
            .is_err()
    );
    // The read may have been charged, so the limit is spent and nothing else goes.
    fixture.effect = OperationEffect::with_http_transport(fixture.provider.clone());
    fixture.provider.answer(Answer::Succeed);
    assert!(
        fixture
            .run_under("advisories", &advisories("ADV-2"), None, Some(&authority))
            .is_err()
    );
    assert_eq!(fixture.provider.calls(), 1);
}

#[test]
fn a_priced_call_keeps_its_spend_when_its_request_cannot_be_built_after_its_precondition() {
    let fixture = Fixture::new(&["channel.post"]);
    let path = price_advisories_with_a_precondition(&fixture);
    // A header takes a string, so this request cannot be rendered from a list of ids.
    let yaml = std::fs::read_to_string(&path).unwrap().replace(
        "          - each: {\"$argument\": \"/ids\"}\n            http_method: GET\n",
        "          - each: {\"$argument\": \"/ids\"}\n            http_method: GET\n            headers: {x-test: {\"$argument\": \"/ids\"}}\n",
    );
    std::fs::write(path, yaml).unwrap();
    let authority = limited(5_000);
    assert!(
        fixture
            .run_under("advisories", &advisories("ADV-1"), None, Some(&authority))
            .is_err()
    );
    // The precondition went out, so the second call is refused before sending anything.
    assert_eq!(fixture.provider.calls(), 1);
    assert!(
        fixture
            .run_under("advisories", &advisories("ADV-2"), None, Some(&authority))
            .is_err()
    );
    assert_eq!(fixture.provider.calls(), 1);
}

#[test]
fn a_read_with_no_declared_charge_can_be_read_again_after_a_timeout() {
    let mut fixture = Fixture::new(&["channel.post"]);
    fixture.provider.answer(Answer::TimeOut);
    let error = fixture
        .run("advisories", &advisories("ADV-1"), None)
        .unwrap_err();
    assert!(error.contains("transport"), "{error}");
    fixture.effect = OperationEffect::with_http_transport(fixture.provider.clone());
    fixture.provider.answer(Answer::Succeed);
    fixture
        .run("advisories", &advisories("ADV-1"), None)
        .unwrap();
    assert_eq!(fixture.provider.calls(), 2);
}

#[test]
fn a_request_billed_read_does_not_refund_a_sent_refusal() {
    let fixture = Fixture::new(&["channel.post"]);
    price_advisories(&fixture, 5_000);
    let authority = limited(5_000);
    fixture.provider.answer(Answer::Reject);
    assert!(
        fixture
            .run_under("advisories", &advisories("ADV-1"), None, Some(&authority))
            .is_err()
    );
    fixture.provider.answer(Answer::Succeed);
    assert!(
        fixture
            .admit_as("advisories", &advisories("ADV-2"), Some(&authority))
            .is_err()
    );
    assert_eq!(fixture.provider.calls(), 1);
}

#[test]
fn a_partial_read_keeps_its_possible_charge_even_when_the_later_request_is_refused() {
    let fixture = Fixture::new(&["channel.post"]);
    price_advisories_per(&fixture, 5_000, "success");
    fixture.provider.answer(Answer::RejectAfterOne);
    let input: JsonObject =
        serde_json::from_value(serde_json::json!({"inputs":{"ids":["ADV-1","ADV-2"]}})).unwrap();
    let authority = limited(5_000);
    let error = fixture
        .run_under("advisories", &input, None, Some(&authority))
        .unwrap_err();
    assert!(error.contains("unknown"), "{error}");
    assert_eq!(fixture.provider.calls(), 2);
    assert!(
        fixture
            .admit_as("advisories", &advisories("ADV-3"), Some(&authority))
            .is_err()
    );
}

#[test]
fn a_readback_naming_another_provider_settles_failed_and_names_it() {
    let fixture = Fixture::new(&["channel.post"]);
    fixture.provider.answer(Answer::WrongProvider);
    let error = fixture
        .run("post", &post("hi"), Some(ResolutionResponseActor::Human))
        .expect_err("contradicted");
    assert!(
        error.contains(r#"provider holds "fixture/forge" where "fixture/chat" was asked"#),
        "{error}"
    );
    assert!(!error.contains("unknown"), "{error}");
    // Settled: nothing is held for a retry, so the next attempt starts fresh.
    fixture.provider.answer(Answer::Succeed);
    fixture
        .run("post", &post("hi"), Some(ResolutionResponseActor::Human))
        .expect("starts fresh");
    assert_eq!(fixture.provider.calls(), 2);
}

#[test]
fn an_unknown_outcome_is_held_and_never_resent_blind() {
    let fixture = Fixture::new(&["channel.post"]);
    fixture.provider.answer(Answer::TimeOut);
    let error = fixture
        .run("post", &post("hi"), Some(ResolutionResponseActor::Human))
        .expect_err("unknown");
    assert!(error.contains("unknown"), "{error}");
    fixture.provider.answer(Answer::Succeed);
    let error = fixture
        .run("post", &post("hi"), Some(ResolutionResponseActor::Human))
        .expect_err("held");
    assert!(error.contains("cannot be resent safely"), "{error}");
    assert_eq!(fixture.provider.calls(), 1);
}

#[test]
fn a_definite_rejection_settles_and_the_next_attempt_starts_fresh() {
    let fixture = Fixture::new(&["channel.post"]);
    fixture.provider.answer(Answer::Reject);
    let error = fixture
        .run("post", &post("hi"), Some(ResolutionResponseActor::Human))
        .expect_err("rejected");
    assert!(error.contains("refused"), "{error}");
    fixture.provider.answer(Answer::Succeed);
    fixture
        .run("post", &post("hi"), Some(ResolutionResponseActor::Human))
        .expect("retries");
    assert_eq!(fixture.provider.calls(), 2);
}

#[test]
fn the_approval_carries_what_executes_bound_by_its_digest() {
    let fixture = Fixture::new(&["channel.post"]);
    let call = post("ship it");
    let admission = fixture.admit("post", &call).expect("admits");
    let mut host = Approver::new(None);
    fixture
        .prepare("post", admission, &mut host)
        .expect("pending");
    let gate = host
        .requests
        .last()
        .and_then(|request| match request {
            ResolutionRequest::Approval { gate, .. } => Some(gate),
            _ => None,
        })
        .expect("the operation asks for its approval");
    // A person sees exactly the input that executes, and a host recomputes
    // the digest the summary names before it offers Approve.
    let input = call["inputs"].clone();
    assert_eq!(gate.payload.as_ref(), Some(&input));
    assert_eq!(
        gate.summary
            .as_ref()
            .and_then(|summary| summary.get("payload_digest")),
        Some(&JsonValue::String(
            runx_contracts::canonical_digest(&input).unwrap()
        ))
    );
}

#[test]
fn a_provider_effect_key_survives_a_fresh_native_run_and_kernel_store() {
    let mut first = Fixture::for_connector("fixture/compute", &["jobs.execute"]);
    first.run_id = "external-stage-attempt-1";
    let mut second = Fixture::for_connector("fixture/compute", &["jobs.execute"]);
    second.run_id = "external-stage-attempt-2";
    let start: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"target": "objects://media/interview.mp3"},
        "idempotency_key": "one-durable-provider-effect",
    }))
    .unwrap();
    first.provider.answer(Answer::TimeOut);
    first
        .run("job", &start, None)
        .expect_err("uncertain provider outcome");
    second
        .run("job", &start, None)
        .expect("the new native attempt resends under provider deduplication");
    let original = first.provider.calls.lock().unwrap();
    let retried = second.provider.calls.lock().unwrap();
    assert_eq!(original.len(), 1);
    assert_eq!(retried.len(), 1);
    let key = |call: &JsonObject| call["input"].as_object().unwrap()["idempotency_key"].clone();
    assert_eq!(key(&original[0]), key(&retried[0]));
}

#[test]
fn an_enforced_operation_resends_after_an_unknown_outcome() {
    let fixture = Fixture::for_connector("fixture/compute", &["jobs.execute"]);
    let start: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"target": "objects://media/interview.mp3"},
        "idempotency_key": "job-1",
    }))
    .unwrap();
    fixture.provider.answer(Answer::TimeOut);
    fixture.run("job", &start, None).expect_err("unknown");
    fixture.provider.answer(Answer::Succeed);
    fixture
        .run("job", &start, None)
        .expect("the provider deduplicates by key");
    let calls = fixture.provider.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    let key = |call: &JsonObject| call["input"].as_object().unwrap()["idempotency_key"].clone();
    assert_eq!(
        key(&calls[0]),
        key(&calls[1]),
        "a resend carries the same provider key"
    );
}

/// A write left unknown recovers only under the instance it was attempted
/// with: confirming the instance again with another account or credential
/// holds it, even where resending would be safe, and nothing is sent.
#[test]
fn an_unknown_write_is_held_when_its_instance_was_rebound() {
    let mut fixture = Fixture::for_connector("fixture/compute", &["jobs.execute"]);
    let start: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"target": "objects://media/interview.mp3"},
        "idempotency_key": "job-1",
    }))
    .unwrap();
    fixture.provider.answer(Answer::TimeOut);
    fixture.run("job", &start, None).expect_err("unknown");
    // The same grant is confirmed again, now held by another account, and
    // the run resumes in a new process.
    *fixture.provider.principal.lock().unwrap() = "operator-2".to_owned();
    fixture.select_grant("grant_chat", "fixture/compute");
    fixture.effect = OperationEffect::with_http_transport(fixture.provider.clone());
    fixture.provider.answer(Answer::Succeed);
    let error = fixture.run("job", &start, None).expect_err("held");
    assert!(error.contains("reconcile it before resuming"), "{error}");
    assert_eq!(fixture.provider.calls(), 1, "nothing was sent again");
}

#[test]
fn a_later_refusal_does_not_settle_an_earlier_unknown_attempt() {
    let fixture = Fixture::for_connector("fixture/compute", &["jobs.execute"]);
    let call: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"target": "objects://media/interview.mp3"},
        "idempotency_key": "job-1",
    }))
    .unwrap();
    fixture.provider.answer(Answer::TimeOut);
    fixture.run("job", &call, None).expect_err("unknown");
    fixture.provider.answer(Answer::Reject);
    let error = fixture
        .run("job", &call, None)
        .expect_err("the earlier outcome stays unknown");
    assert!(error.contains("prior attempt may have executed"), "{error}");
    let admission = fixture.admit("job", &call).unwrap();
    assert!(
        admission
            .context::<OperationAdmission>()
            .unwrap()
            .admitted
            .pending
            .is_some()
    );
}

#[test]
fn a_hosted_declared_write_sends_each_request_through_connect_and_reads_it_back() {
    let fixture = Fixture::for_connector("fixture/forge", &["repo.write"]);
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"repository": "acme/app", "updates": [{"number": 7, "patch": {"title": "ready"}}]},
        "idempotency_key": "update-1",
    }))
    .unwrap();
    let (admission, outcome) = fixture
        .run("update", &inputs, Some(ResolutionResponseActor::Human))
        .expect("runs");
    let context = admission.context::<OperationAdmission>().unwrap();
    assert_eq!(
        packet(&outcome)["finality"],
        JsonValue::String("confirmed".to_owned())
    );
    let calls = sent(&fixture);
    let requests = calls
        .iter()
        .map(|call| (call["request"].as_str().unwrap(), call["item"].as_u64()))
        .collect::<Vec<_>>();
    assert_eq!(requests, [("update", Some(0)), ("issue", Some(0))]);
    for call in &calls {
        assert_eq!(
            call["operation_digest"].as_str(),
            Some(context.admitted.intent().operation_digest.as_str())
        );
        assert_eq!(
            call["implementation_digest"].as_str(),
            context.implementation_digest().as_deref()
        );
        assert_eq!(
            call["idempotency_key"].as_str(),
            context.provider_key().as_deref()
        );
        assert!(call.get("target").is_none() && call.get("scopes").is_none());
    }
    // The write carries no earlier answers; its checking read carries the
    // write's decoded answer, which Connect renders the read from.
    assert!(calls[0].get("responses").is_none());
    assert_eq!(calls[1]["responses"]["write"]["json"]["number"], 7);
}

#[test]
fn a_hosted_declared_read_sends_one_request_per_item() {
    let fixture = Fixture::for_connector("fixture/forge", &["repo.read"]);
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"repository": "acme/app", "numbers": [7, 8]},
    }))
    .unwrap();
    fixture.run("issues", &inputs, None).expect("runs");
    let items = sent(&fixture)
        .iter()
        .map(|call| {
            (
                call["request"].as_str().unwrap().to_owned(),
                call["item"].as_u64(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        items,
        [("issue".to_owned(), Some(0)), ("issue".to_owned(), Some(1))]
    );
}

/// What admission checked is what is sent. The forge fixture's grammar admits
/// a dot segment, so `acme/..` passes its schema and a term on `acme/*`, and
/// parsing its URL would resolve `/repos/acme/../issues/7` to
/// `/repos/issues/7`. The request is refused before it leaves, through runx
/// Connect and sent directly alike, while `acme/app` is sent both ways.
#[test]
fn a_request_whose_path_changes_when_parsed_is_never_sent() {
    for direct in [false, true] {
        let fixture = Fixture::for_connector("fixture/forge", &["repo.read"]);
        let instance = if direct {
            fixture.set_credential("fixture/forge", "token", "forge-local-token");
            crate::kernel::instances::instance_id(&["secret", "fixture/forge", "token", "default"])
        } else {
            crate::kernel::instances::instance_id(&["hosted", "fixture/forge", "grant_chat"])
        }
        .unwrap();
        let actor_ref =
            Reference::with_uri(ReferenceType::Principal, "runx:principal:assistant:test");
        let authority = crate::RunAuthority {
            grants: std::collections::BTreeMap::new(),
            actor_ref: Some(actor_ref.clone()),
            mandate_ref: Some(Reference::runx(ReferenceType::Mandate, "test-mandate")),
            delegations: vec![delegation(
                "fixture/forge",
                Reference::runx(ReferenceType::ConnectorInstance, instance.as_str()),
                actor_ref,
                vec![AuthorityVerb::Read],
                AuthorityBounds {
                    operations: vec!["fixture/forge/issues.read".into()],
                    scopes: vec!["repo.read".into()],
                    targets: vec!["acme/*".into()],
                    uses: None,
                    packages: Vec::new(),
                },
            )],
            max_fanout: None,
            max_child_depth: None,
            standing_approvals: Vec::new(),
            max_spend_micro_usd: None,
            ..crate::RunAuthority::default()
        };
        let read = |repository: &str| -> JsonObject {
            serde_json::from_value(serde_json::json!({
                "inputs": {"repository": repository, "numbers": [7]},
            }))
            .unwrap()
        };
        fixture
            .admit_as("issues", &read("acme/.."), Some(&authority))
            .expect("the schema and the term both admit acme/..");
        let refused = fixture.run_under("issues", &read("acme/.."), None, Some(&authority));
        let reason = match &refused {
            Err(reason) => reason.clone(),
            Ok((_, outcome, _)) => serde_json::to_string(packet(outcome)).unwrap(),
        };
        assert!(
            reason.contains("changes when parsed"),
            "direct: {direct}: {reason}"
        );
        assert_eq!(fixture.provider.calls(), 0, "direct: {direct}");
        fixture
            .run_under("issues", &read("acme/app"), None, Some(&authority))
            .expect("acme/app is sent");
        assert_eq!(fixture.provider.calls(), 1, "direct: {direct}");
    }
}

#[test]
fn a_webhook_write_is_acknowledged_once_behind_its_credential_and_exact_approval() {
    // The webhook's URL is its credential: the operator binds it, and the
    // call reaches only that URL.
    let fixture = Fixture::new(&["channel.post"]);
    fixture.set_credential(
        "fixture/hooks",
        "webhook",
        "https://acme.hooks.fixture.invalid/webhook/runx-effect",
    );
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {
            "webhook_host": "acme.hooks.fixture.invalid",
            "delivery": {
                "event_id": "evt-1",
                "handoff_scope": "orchestrator.hooks.workflow.invoke",
                "handoff_audience": "hooks:workflow:runx-effect",
                "idempotency_key": "evt-1",
                "payload": {"hello": "workflow"},
            },
        },
        "idempotency_key": "evt-1",
    }))
    .unwrap();
    assert_eq!(
        fixture.run("deliver", &inputs, None).expect_err("waits"),
        "pending"
    );
    assert_eq!(fixture.provider.calls(), 0);
    let (admission, outcome) = fixture
        .run("deliver", &inputs, Some(ResolutionResponseActor::Human))
        .expect("runs");
    let acknowledged = packet(&outcome);
    assert_eq!(
        acknowledged["finality"],
        JsonValue::String("acknowledged".to_owned())
    );
    // Its evidence is its recorded acknowledgement, never a confirming read,
    // and the receipt never seals it as the effect's finality.
    let first_evidence = acknowledged["readback_uri"].as_str().unwrap().to_owned();
    assert!(
        first_evidence.starts_with("runx:http-acknowledgement:sha256:"),
        "{first_evidence}"
    );
    let refs = sealed_refs(&fixture, "deliver", &admission, &outcome);
    assert!(
        refs.iter()
            .any(|reference| reference.uri.as_str() == first_evidence
                && reference.proof_kind == Some(ProofKind::EffectEvidence))
    );
    assert!(
        refs.iter()
            .all(|reference| reference.proof_kind != Some(ProofKind::EffectFinality))
    );
    let first = acknowledged["operation_id"].as_str().unwrap().to_owned();
    assert!(
        first.starts_with("http:runx:sha256:"),
        "an acknowledgement without a provider id is identified by the key it was sent with"
    );
    assert_eq!(
        serde_json::to_value(&acknowledged["result"]).unwrap()["acknowledgement"]["message"],
        "Workflow was started"
    );
    assert_eq!(fixture.provider.calls(), 1, "no readback is sent");

    // Another write the provider answers alike is still another operation.
    let other: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {
            "webhook_host": "acme.hooks.fixture.invalid",
            "delivery": {
                "event_id": "evt-2",
                "handoff_scope": "orchestrator.hooks.workflow.invoke",
                "handoff_audience": "hooks:workflow:runx-effect",
                "idempotency_key": "evt-2",
                "payload": {"hello": "workflow"},
            },
        },
        "idempotency_key": "evt-2",
    }))
    .unwrap();
    let (_, outcome) = fixture
        .run("deliver", &other, Some(ResolutionResponseActor::Human))
        .expect("runs");
    let second = packet(&outcome)["operation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first, second);
    assert_ne!(
        first_evidence,
        packet(&outcome)["readback_uri"].as_str().unwrap(),
        "each acknowledgement is its own evidence"
    );
}

/// The verification references the receipt seals for a step's outcome.
fn sealed_refs(
    fixture: &Fixture,
    step: &str,
    admission: &EffectAdmission,
    outcome: &OperationOutcome,
) -> Vec<Reference> {
    let claim = JsonObject::from([(
        "provider_operation".to_owned(),
        JsonValue::Object(JsonObject::from([(
            "data".to_owned(),
            JsonValue::Object(packet(outcome).clone()),
        )])),
    )]);
    let mut output = crate::adapter::InvocationOutput::runtime_success(outcome.value.clone(), 0);
    fixture
        .effect
        .prepare_output(crate::effects::EffectOutputRequest {
            step: &fixture.steps[step],
            admission,
            claim: &claim,
            output: &mut output,
        })
        .unwrap();
    output.observations.effect_verification_refs
}

/// A plugin connector whose MCP server is a process the runtime starts over
/// stdio: the instance's stored credential reaches it in the variable its
/// declaration names, beside the base host environment every host process
/// gets, and nothing else of the operator's. The server echoes what it holds,
/// and the echo of the credential is redacted before the answer is kept. The
/// call seals the trusted host process boundary.
#[cfg(feature = "mcp")]
#[test]
fn a_stdio_mcp_server_receives_the_instance_credential_and_no_operator_secret() {
    let root = tempfile::tempdir().unwrap();
    let skill_dir = root.path().join("skills/probe");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let server = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/connectors/mcp-echo/stdio-server.mjs")
        .canonicalize()
        .unwrap();
    std::fs::write(root.path().join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
    let connector_dir = root.path().join("connectors/probe-mcp");
    std::fs::create_dir_all(&connector_dir).unwrap();
    std::fs::write(
        connector_dir.join("connector.yaml"),
        format!(
            r#"schema: runx.connector.v1
name: probe-mcp
title: Probe
description: Reads its own environment.
auth:
  credentials:
    key:
      kind: api_key
      fields: [{{name: key}}]
      delivery: {{env: PROBE_KEY}}
      audience: [probe.invalid]
placements:
  local:
    auth: [credentials]
    mcp_server: {{transport: stdio, command: node, args: ["{server}"]}}
operations:
  env.read:
    description: Read one variable of the server's environment.
    verb: read
    scopes: [env.read]
    credentials: [key]
    target: {{template: "probe://env"}}
    input_schema:
      type: object
      required: [name]
      additionalProperties: false
      properties:
        name: {{type: string}}
    implementation:
      mcp:
        tool: env
        tool_digest: sha256:aaae474ce25272c8d016ad85a657ae9bcbac104587e0837e79e7baaa1b504405
        arguments: {{name: {{"$argument": "/name"}}}}
"#,
            server = server.display()
        ),
    )
    .unwrap();
    let manifest = runx_parser::validate_method_manifest(
        "fixture",
        runx_parser::parse_method_manifest_yaml(
            r#"
minimum_runx: 0.12.0
methods:
  run:
    result_from: [read]
    steps:
      - id: read
        operation: probe-mcp/env.read
"#,
        )
        .unwrap(),
    )
    .unwrap();
    let step = manifest.methods["run"].workflow.steps[0].clone();
    let env = BTreeMap::from([
        ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
        ("HOME".to_owned(), "/operator/home".to_owned()),
        (
            "OPERATOR_SECRET".to_owned(),
            "not-for-the-server".to_owned(),
        ),
        (
            "RUNX_HOME".to_owned(),
            root.path().join("home").to_string_lossy().into_owned(),
        ),
        (
            crate::paths::RUNX_CWD_ENV.to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            crate::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
            root.path().join("receipts").to_string_lossy().into_owned(),
        ),
        (
            crate::execution::engine::RUNX_RUN_ID_ENV.to_owned(),
            "run-1".to_owned(),
        ),
    ]);
    let connector = crate::ConnectorScope::at(&skill_dir)
        .unwrap()
        .connector("probe-mcp")
        .unwrap();
    keep_and_bind(
        &connector,
        "probe",
        "key",
        "probe-secret",
        &crate::services::WorkspaceEnv::new(env.clone(), root.path().to_path_buf()).unwrap(),
    );
    let effect = OperationEffect::default();
    let read = |name: &str| {
        let inputs: JsonObject =
            serde_json::from_value(serde_json::json!({"inputs": {"name": name}})).unwrap();
        let admission = effect
            .admit(EffectStepRequest {
                step: &step,
                target: ResolvedEffectTarget::default(),
                inputs: &inputs,
                env: &env,
                workflow_dir: &skill_dir,
                run_id: "run-1",
                run_authority: None,
                held_by_effects: false,
                chain: &[],
                clock: &crate::RunClock::System,
            })
            .unwrap()
            .unwrap();
        let mut host = Approver::new(None);
        let outcome = effect
            .prepare_execution(&step, admission, &mut host)
            .unwrap();
        assert!(
            matches!(outcome, EffectPreparationOutcome::Ready(_)),
            "a read is never held for approval"
        );
        let EffectPreparationOutcome::Ready(admission) = outcome else {
            return String::new();
        };
        let context = admission.context::<OperationAdmission>().unwrap();
        let mut dispatched = None;
        let outcome =
            execute(context, "read", &env, &skill_dir, &NoTools, &mut dispatched).unwrap();
        let dispatched = dispatched.unwrap();
        assert_eq!(
            dispatched.boundary,
            ExecutionBoundaryKind::TrustedHostProcess,
            "a stdio server is a process the runtime starts, not contained"
        );
        let delivered = dispatched.credential.unwrap();
        assert_eq!(delivered.provider.as_str(), "probe-mcp");
        serde_json::to_value(&packet(&outcome)["result"]).unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(read("PROBE_KEY"), "[redacted-credential]");
    assert_eq!(read("OPERATOR_SECRET"), "");
}

/// What the kernel sent to runx Connect, as plain JSON.
fn sent(fixture: &Fixture) -> Vec<serde_json::Value> {
    fixture
        .provider
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| serde_json::to_value(call).unwrap())
        .collect()
}

fn acted(text: &str, act: serde_json::Value) -> JsonObject {
    let mut call = post(text);
    call.insert("act".to_owned(), JsonValue::from_serialize(&act).unwrap());
    call
}

#[test]
fn a_calls_act_is_bound_into_its_intent_and_shown_to_its_approver() {
    let fixture = Fixture::new(&["channel.post"]);
    let intent = |inputs: &JsonObject| {
        let admission = fixture.admit("post", inputs).expect("admits");
        let context = admission.context::<OperationAdmission>().unwrap();
        (
            context.admitted.intent().act_digest.clone(),
            context.admitted.intent_digest().clone(),
        )
    };
    let closing = acted("hi", serde_json::json!({"purpose": "Close the thread"}));
    let declining = acted("hi", serde_json::json!({"purpose": "Decline the thread"}));
    let (act, closing_digest) = intent(&closing);
    assert!(act.is_some(), "a stated act is bound into the intent");
    let (_, declining_digest) = intent(&declining);
    assert_ne!(
        closing_digest, declining_digest,
        "a changed act is another call"
    );
    let (none, _) = intent(&post("hi"));
    assert!(none.is_none());

    // The approver sees the act it approves, under the intent that binds it.
    let admission = fixture.admit("post", &closing).expect("admits");
    let mut host = Approver::new(None);
    fixture
        .prepare("post", admission, &mut host)
        .expect("pending");
    let request = serde_json::to_value(&host.requests).unwrap();
    let gate = &request[0]["gate"];
    assert_eq!(gate["summary"]["act"]["purpose"], "Close the thread");
    assert_eq!(
        gate["id"],
        serde_json::Value::String(format!("operation:{closing_digest}"))
    );
}

#[test]
fn a_changed_act_is_held_by_the_unsettled_attempt_of_another() {
    let fixture = Fixture::new(&["channel.post"]);
    fixture.provider.answer(Answer::TimeOut);
    let closing = acted("hi", serde_json::json!({"purpose": "Close the thread"}));
    fixture
        .run("post", &closing, Some(ResolutionResponseActor::Human))
        .expect_err("unknown");
    fixture.provider.answer(Answer::Succeed);
    let declining = acted("hi", serde_json::json!({"purpose": "Decline the thread"}));
    let error = fixture
        .run("post", &declining, Some(ResolutionResponseActor::Human))
        .expect_err("held");
    assert!(error.contains("carries another intent"), "{error}");
    assert_eq!(fixture.provider.calls(), 1);
}

#[test]
fn an_act_outside_its_shape_is_refused_before_anything_is_sent() {
    let fixture = Fixture::new(&["channel.post"]);
    for act in [
        serde_json::json!({"purpose": "Revise it", "form": "revision"}),
        serde_json::json!({"purpose": "Settle it", "decision": "paid"}),
        serde_json::json!({"reason": "no purpose"}),
        serde_json::json!({"purpose": "Close it", "legitimacy": "trust me"}),
    ] {
        let error = fixture
            .admit("post", &acted("hi", act.clone()))
            .expect_err("refused")
            .to_string();
        assert!(
            error.contains("operation step inputs are invalid"),
            "{act}: {error}"
        );
    }
    assert_eq!(fixture.provider.calls(), 0);
}

#[test]
fn a_call_carries_the_effect_its_operation_declares_and_admissions_authority() {
    let fixture = Fixture::new(&["channel.post"]);
    let (admission, outcome) = fixture
        .run("post", &post("hi"), Some(ResolutionResponseActor::Human))
        .expect("runs");
    assert_eq!(
        outcome.effects.len(),
        1,
        "one effect: {:?}",
        outcome.effects
    );
    let effect = &outcome.effects[0];
    assert_eq!(effect.reference_type, ReferenceType::Thread);
    assert_eq!(effect.uri.as_str(), "chat://T123/C456/1.0");
    let authority = fixture.effect.authority(&admission).expect("authority");
    assert_eq!(
        authority.actor_ref.as_ref().map(|actor| actor.uri.as_str()),
        Some(PRINCIPAL_REF)
    );
    assert_eq!(
        authority.mandate_ref, None,
        "a direct call acts under no mandate"
    );
    let context = admission.context::<OperationAdmission>().unwrap();
    let sealed = context
        .sealed_call(outcome.effects.clone(), None)
        .expect("sealed");
    let term = context.admitted.admission.execution_term.as_ref().unwrap();
    assert_eq!(sealed.legitimacy.term_id(), term.term_id.as_str());
}

#[test]
fn an_effect_the_connector_does_not_declare_holds_the_call() {
    let fixture = Fixture::new(&["channel.post"]);
    fixture.provider.answer(Answer::OffPattern);
    let error = fixture
        .run("post", &post("hi"), Some(ResolutionResponseActor::Human))
        .expect_err("held");
    assert!(
        error.contains("is not a thread fixture/chat declares"),
        "{error}"
    );
    assert!(
        error.contains("unknown"),
        "an unconfirmed effect holds: {error}"
    );
}

#[test]
fn an_effect_renders_from_the_result_and_is_checked_only_against_declared_patterns() {
    use runx_contracts::connector::TemplateInputs;

    let manifest = |references: &str, kind: &str, locator: &str| {
        let yaml = format!(
            r#"schema: runx.connector.v1
name: acme
title: Acme
description: A plugin-shaped connector.
auth: {{none: {{}}}}
placements: {{local: {{auth: [none]}}}}
{references}
operations:
  items.create:
    description: Create items.
    verb: write
    scopes: [items.write]
    target: {{template: "acme://items"}}
    result_fields: [items]
    effect:
      kind: {kind}
      each: {{"$result": "/items"}}
      locator: {locator}
"#
        );
        runx_parser::parse_connector_manifest(&yaml).unwrap()
    };
    let result = serde_json::json!({"items": [{"id": "a1"}, {"id": "b2"}]});
    let result = JsonValue::from_serialize(&result).unwrap();
    let inputs = TemplateInputs {
        result: Some(&result),
        ..TemplateInputs::default()
    };
    let locator = r#"{"$concat": ["acme://items/", {"$item": "/id"}]}"#;

    let effect_of = |manifest: &runx_contracts::connector::ConnectorManifest| {
        manifest
            .operations
            .values()
            .next()
            .unwrap()
            .effect
            .clone()
            .unwrap()
    };

    // A plugin declares no references: the effect is typed as declared.
    let plugin = manifest("", "tracking_item", locator);
    let effect = effect_of(&plugin);
    let refs = render_effects(&plugin, &effect, false, &inputs).expect("renders");
    assert_eq!(
        refs.iter().map(|r| r.uri.as_str()).collect::<Vec<_>>(),
        ["acme://items/a1", "acme://items/b2"]
    );
    assert!(
        refs.iter()
            .all(|r| r.reference_type == ReferenceType::TrackingItem)
    );

    // Declared references check every locator against the kind's pattern.
    let declared = manifest(
        "references:\n  - {kind: tracking_item, locator: \"acme://items/[a-z][0-9]\"}",
        "tracking_item",
        locator,
    );
    let effect = effect_of(&declared);
    assert!(render_effects(&declared, &effect, false, &inputs).is_ok());
    let mismatched = manifest(
        "references:\n  - {kind: tracking_item, locator: \"acme://items/[0-9]+\"}",
        "tracking_item",
        locator,
    );
    let effect = effect_of(&mismatched);
    assert!(render_effects(&mismatched, &effect, false, &inputs).is_err());

    // A repeated write's result is its items' outcomes: only what it applied
    // was produced.
    let outcomes = JsonValue::from_serialize(&serde_json::json!({"items": [
        {"status": "applied", "acknowledgement": {"id": "a1"}},
        {"status": "refused", "code": "provider_rejected", "reason": "no"},
        {"status": "applied", "acknowledgement": {"id": "b2"}},
        {"status": "unsent", "code": "rate_limited", "reason": "halted"},
    ]}))
    .unwrap();
    let refs = render_effects(
        &plugin,
        &effect_of(&plugin),
        true,
        &TemplateInputs {
            result: Some(&outcomes),
            ..TemplateInputs::default()
        },
    )
    .expect("renders the applied items");
    assert_eq!(
        refs.iter().map(|r| r.uri.as_str()).collect::<Vec<_>>(),
        ["acme://items/a1", "acme://items/b2"]
    );

    // A result without what the effect reads does not render.
    let empty = JsonValue::from_serialize(&serde_json::json!({})).unwrap();
    let error = render_effects(
        &plugin,
        &effect_of(&plugin),
        false,
        &TemplateInputs {
            result: Some(&empty),
            ..TemplateInputs::default()
        },
    )
    .expect_err("unresolved");
    assert!(error.contains("result array"), "{error}");

    // A locator names something: one that renders empty does not render.
    let bare = manifest("", "tracking_item", r#"{"$item": "/id"}"#);
    let blank = JsonValue::from_serialize(&serde_json::json!({"items": [{"id": " "}]})).unwrap();
    let error = render_effects(
        &bare,
        &effect_of(&bare),
        false,
        &TemplateInputs {
            result: Some(&blank),
            ..TemplateInputs::default()
        },
    )
    .expect_err("empty");
    assert!(error.contains("rendered empty"), "{error}");
}

fn allocation(value: serde_json::Value) -> JsonObject {
    serde_json::from_value(serde_json::json!({
        "inputs": {
            "purpose": "runx.test.result.v1",
            "media_type": "application/json",
            "value": value,
        },
        "idempotency_key": "job-1",
    }))
    .unwrap()
}

#[test]
fn an_artifact_is_stored_for_the_signed_in_account_and_read_back() {
    let mut fixture = Fixture::new(&[]);
    fixture.env.insert(
        crate::execution::engine::RUNX_RUN_ID_ENV.to_owned(),
        "run-1".to_owned(),
    );
    let (admission, outcome, dispatched) = fixture
        .run_dispatched(
            "allocate",
            &allocation(serde_json::json!({"answer": 42})),
            None,
        )
        .expect("a write to the account's own artifact space needs no approval");
    let context = admission.context::<OperationAdmission>().unwrap();
    let packet = packet(&outcome);
    assert_eq!(
        packet["finality"],
        JsonValue::String("confirmed".to_owned())
    );
    assert_eq!(
        packet["transport"],
        JsonValue::String("local_artifact".to_owned())
    );
    let result = packet["result"].as_object().unwrap();
    let artifact = result["artifact"].as_object().unwrap();
    assert_eq!(
        result["principal_uri"],
        JsonValue::String(PRINCIPAL_REF.to_owned())
    );
    assert_eq!(
        artifact["schema"],
        JsonValue::String("runx.artifact.v1".to_owned())
    );
    assert_eq!(
        artifact["digest"],
        JsonValue::String(runx_contracts::sha256_prefixed(br#"{"answer":42}"#))
    );
    assert_eq!(
        artifact["labels"],
        serde_json::from_value::<JsonValue>(serde_json::json!({"sharing": "configured_model"}))
            .unwrap()
    );
    assert_eq!(
        dispatched.expect("dispatched").boundary,
        ExecutionBoundaryKind::RemoteProvider
    );
    let sent = sent(&fixture);
    assert_eq!(
        sent.iter()
            .map(|body| body["operation"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["artifact.allocate", "artifact.inspect"],
        "the allocation is read back by its reference"
    );
    assert_eq!(sent[0]["run_id"], "run-1");
    assert_eq!(
        sent[0]["input"]["idempotency_key"].as_str(),
        context.provider_key().as_deref(),
        "the artifact is keyed by the call's intent"
    );
}

#[test]
fn public_storage_acts_as_the_runs_declared_actor() {
    let mut fixture = Fixture::new(&[]);
    fixture.env.insert(
        crate::execution::engine::RUNX_RUN_ID_ENV.to_owned(),
        "run-actor".to_owned(),
    );
    let actor = Reference::with_uri(ReferenceType::Principal, PRINCIPAL_REF);
    let mut authority = limited(u64::MAX);
    authority.max_spend_micro_usd = None;
    authority.actor_ref = Some(actor.clone());
    let (admission, outcome, _) = fixture
        .run_under(
            "allocate",
            &allocation(serde_json::json!({"answer": 42})),
            None,
            Some(&authority),
        )
        .expect("public storage admits the declared actor");
    let context = admission.context::<OperationAdmission>().unwrap();
    assert_eq!(context.admitted.capability.term.principal_ref, actor);
    assert_eq!(
        context
            .admitted
            .admission
            .execution_term
            .as_ref()
            .unwrap()
            .principal_ref,
        actor
    );
    assert_eq!(
        packet(&outcome)["result"].as_object().unwrap()["principal_uri"],
        JsonValue::String(PRINCIPAL_REF.to_owned())
    );
    assert_eq!(
        sent(&fixture).len(),
        2,
        "allocation has independent readback"
    );
}

#[test]
fn an_artifact_held_for_another_principal_is_an_unknown_outcome() {
    let mut fixture = Fixture::new(&[]);
    fixture.env.insert(
        crate::execution::engine::RUNX_RUN_ID_ENV.to_owned(),
        "run-1".to_owned(),
    );
    *fixture.provider.artifact_owner.lock().unwrap() = Some("someone-else".to_owned());
    let error = fixture
        .run(
            "allocate",
            &allocation(serde_json::json!({"answer": 42})),
            None,
        )
        .expect_err("held");
    assert!(error.contains("another principal"), "{error}");
}

#[test]
fn an_artifact_read_back_for_another_principal_settles_failed_and_names_it() {
    let mut fixture = Fixture::new(&[]);
    fixture.env.insert(
        crate::execution::runner::RUNX_RUN_ID_ENV.to_owned(),
        "run-1".to_owned(),
    );
    *fixture.provider.inspected_owner.lock().unwrap() = Some("someone-else".to_owned());
    let error = fixture
        .run(
            "allocate",
            &allocation(serde_json::json!({"answer": 42})),
            None,
        )
        .expect_err("contradicted");
    assert!(
        error.contains(&format!(
            r#"principal_uri holds "runx:principal:someone-else" where "{PRINCIPAL_REF}" was asked"#
        )),
        "{error}"
    );
    assert!(!error.contains("unknown"), "{error}");
}

#[test]
fn a_read_or_a_deduplicated_write_resends_and_any_other_write_does_not() {
    let manifest: runx_contracts::connector::ConnectorManifest =
        serde_json::from_value(serde_json::json!({
            "schema": "runx.connector.v1",
            "name": "notes",
            "title": "Notes",
            "description": "Write and read notes.",
            "auth": {"delegated": {"principal": {"url": "https://notes.example/api/me", "id": "id", "login": "login"}}},
            "placements": {"hosted": {"auth": "delegated"}},
            "operations": {
                "note.read": {
                    "description": "Read a note.",
                    "verb": "read",
                    "scopes": ["note.read"],
                    "target": {"template": {"$argument": "/note"}},
                    "provider_scopes": ["notes:read"]
                },
                "note.post": {
                    "description": "Post a note.",
                    "verb": "write",
                    "scopes": ["note.post"],
                    "target": {"template": {"$argument": "/note"}},
                    "approval": "required",
                    "result_fields": ["note"],
                    "provider_scopes": ["notes:write"]
                },
                "note.put": {
                    "description": "Put a note by its key.",
                    "verb": "write",
                    "scopes": ["note.put"],
                    "target": {"template": {"$argument": "/note"}},
                    "approval": "required",
                    "idempotency_key_format": "uuid",
                    "guarantees": {"provider_idempotency": "enforced"},
                    "result_fields": ["note"],
                    "provider_scopes": ["notes:write"]
                }
            }
        }))
        .expect("the manifest parses");
    let operation = |name: &str| {
        manifest
            .operations
            .iter()
            .find(|(operation, _)| operation.as_str() == name)
            .map(|(_, operation)| operation.clone())
            .expect("the operation")
    };
    let priced = |usd_micro: u64| {
        Some(
            serde_json::from_value(serde_json::json!({
                "usd_micro": usd_micro, "per": "request", "source": "docs",
            }))
            .unwrap(),
        )
    };
    let with_cost = |name: &str, usd_micro: u64| {
        let mut declared = operation(name);
        declared.cost = priced(usd_micro);
        declared
    };

    // With no declared charge, the verb and its guarantees decide, and a
    // zero quote changes nothing.
    assert!(resends_safely(&operation("note.read"), false));
    assert!(resends_safely(&with_cost("note.read", 0), false));
    let mut state_idempotent = operation("note.put");
    state_idempotent.guarantees.provider_idempotency = ProviderIdempotency::Inherent;
    assert!(resends_safely(&state_idempotent, false));
    assert!(resends_safely(&operation("note.put"), false));
    assert!(!resends_safely(&operation("note.post"), false));
    assert!(resends_safely(&operation("note.post"), true));
    assert!(resends_safely(&with_cost("note.post", 0), true));

    // A declared charge can land unseen, so only provider deduplication of
    // the whole call covers a resend. Read access, state idempotency and
    // tolerated duplicates all fail to.
    assert!(!resends_safely(&with_cost("note.read", 5_000), false));
    state_idempotent.cost = priced(5_000);
    assert!(!resends_safely(&state_idempotent, false));
    assert!(!resends_safely(&with_cost("note.post", 5_000), true));
    assert!(resends_safely(&with_cost("note.put", 5_000), false));
}

/// A safe read reruns by itself after a crash (D33): a read that costs
/// nothing and that runx carries out itself. A read a process carries out,
/// the connector's own or a stdio server's, rests on its author's promise,
/// and a priced one can bill unseen, so neither does, and nor does a write.
#[test]
fn only_a_free_read_runx_carries_out_is_a_safe_read() {
    let manifest = |mcp_server: serde_json::Value| -> runx_contracts::connector::ConnectorManifest {
        serde_json::from_value(serde_json::json!({
            "schema": "runx.connector.v1",
            "name": "notes",
            "title": "Notes",
            "description": "Read and write notes.",
            "auth": {"delegated": {"principal": {"url": "https://notes.example/api/me", "id": "id", "login": "login"}}},
            "placements": {
                "hosted": {"auth": "delegated"},
                "local": {"auth": ["none"], "mcp_server": mcp_server}
            },
            "operations": {
                "note.read": {
                    "description": "Read a note.",
                    "verb": "read",
                    "scopes": ["note.read"],
                    "target": {"template": {"$argument": "/note"}},
                    "provider_scopes": ["notes:read"]
                },
                "note.priced": {
                    "description": "Read a note at a price.",
                    "verb": "read",
                    "scopes": ["note.read"],
                    "target": {"template": {"$argument": "/note"}},
                    "cost": {"usd_micro": 5000, "per": "request", "source": "docs"},
                    "provider_scopes": ["notes:read"]
                },
                "note.shell": {
                    "description": "Read a note through a process.",
                    "verb": "read",
                    "scopes": ["note.read"],
                    "target": {"template": {"$argument": "/note"}},
                    "implementation": {"process": {"tool": "notes.read"}}
                },
                "note.served": {
                    "description": "Read a note through the connector's server.",
                    "verb": "read",
                    "scopes": ["note.read"],
                    "target": {"template": {"$argument": "/note"}},
                    "implementation": {"mcp": {
                        "tool": "read_note",
                        "arguments": {"note": {"$argument": "/note"}},
                        "tool_digest": format!("sha256:{}", "0".repeat(64))
                    }}
                },
                "note.post": {
                    "description": "Post a note.",
                    "verb": "write",
                    "scopes": ["note.post"],
                    "target": {"template": {"$argument": "/note"}},
                    "approval": "required",
                    "result_fields": ["note"],
                    "provider_scopes": ["notes:write"]
                }
            }
        }))
        .expect("the manifest parses")
    };
    let safe = |manifest: &runx_contracts::connector::ConnectorManifest, name: &str| {
        let operation = manifest
            .operations
            .iter()
            .find(|(operation, _)| operation.as_str() == name)
            .map(|(_, operation)| operation)
            .expect("the operation");
        safe_read(manifest, operation)
    };
    let served_over_http = manifest(serde_json::json!({
        "transport": "streamable_http", "url": "https://notes.example/mcp"
    }));
    assert!(safe(&served_over_http, "note.read"));
    assert!(safe(&served_over_http, "note.served"));
    assert!(!safe(&served_over_http, "note.priced"));
    assert!(!safe(&served_over_http, "note.shell"));
    assert!(!safe(&served_over_http, "note.post"));
    let served_by_a_process = manifest(serde_json::json!({
        "transport": "stdio", "command": "notes-server"
    }));
    assert!(safe(&served_by_a_process, "note.read"));
    assert!(!safe(&served_by_a_process, "note.served"));
}

#[cfg(unix)]
#[test]
fn a_managed_cli_session_is_pinned_and_checked_before_each_process_call() {
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::for_connector("fixture/forge", &["repo.read"]);
    let root = fixture.root.path();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let probe = bin.join("account-cli");
    std::fs::write(&probe, "#!/bin/sh\ncat \"$1\"\n").unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
    fixture.env.insert(
        "PATH".to_owned(),
        format!("{}:/usr/bin:/bin", bin.display()),
    );
    fixture
        .env
        .insert("RUNX_CWD".to_owned(), root.display().to_string());
    let account = root.join("account.json");
    std::fs::write(&account, r#"{"id":"account-one"}"#).unwrap();
    let directory = root.join("connectors/fixture/forge");
    let manifest = serde_json::json!({
        "schema":"runx.connector.v1", "name":"fixture/forge", "title":"Forge", "description":"A managed CLI account.",
        "auth":{"session":{"kind":"managed_command","command":["account-cli",account.to_str().unwrap()],"id":"id","login":"id"}},
        "placements":{"local":{"auth":["session"]}},
        "operations":{"issues.read":{"description":"Read this account.","verb":"read","scopes":["repo.read"],"target":{"template":"forge://current"},
            "input_schema":{"type":"object","properties":{},"additionalProperties":false},
            "result_fields":["account_id"],"implementation":{"process":{"tool":"fixture.account"}}}}
    });
    std::fs::write(directory.join("connector.yaml"), manifest.to_string()).unwrap();
    let tool = directory.join("tools/fixture/account");
    std::fs::create_dir_all(&tool).unwrap();
    std::fs::write(tool.join("account.sh"), "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::write(
        tool.join("manifest.json"),
        serde_json::json!({
            "schema": "runx.tool.manifest.v1",
            "name": "fixture.account",
            "version": "0.1.0",
            "description": "Read a managed session account.",
            "process": {"command": "sh", "args": ["./account.sh"], "inputs_via": "stdin"},
            "inputs": {
                "principal_uri": {"type": "string"}
            }
        })
        .to_string(),
    )
    .unwrap();
    let connector = crate::kernel::connectors::ConnectorScope::at(root)
        .unwrap()
        .connector("fixture/forge")
        .unwrap();
    let store =
        KernelStore::for_project(&crate::paths::RunxPaths::resolve(&fixture.env, root)).unwrap();
    let instances = Instances::new(store);
    let readers = crate::kernel::setup::AccountReaders {
        transport: &fixture.provider,
        tools: &crate::kernel::operation::NoTools,
    };
    assert!(
        crate::kernel::setup::confirm_session_over(
            &instances,
            &connector,
            &["unserved".to_owned()],
            JsonObject::new(),
            &readers,
            &fixture.env,
            root,
        )
        .is_err()
    );
    let instance = crate::kernel::setup::confirm_session_over(
        &instances,
        &connector,
        &["repo.read".to_owned()],
        JsonObject::new(),
        &readers,
        &fixture.env,
        root,
    )
    .unwrap();
    assert_eq!(
        instance.principal_uri.as_str(),
        "runx:principal:fixture/forge:account-one"
    );
    assert!(
        crate::kernel::instances::delivered_credential(&connector.manifest, &instance.authority)
            .is_none()
    );
    struct Reader(RefCell<Vec<JsonObject>>);
    impl Tools for Reader {
        fn run(
            &self,
            _: &str,
            _: &std::path::Path,
            inputs: JsonObject,
            _: &crate::credentials::CredentialDelivery,
        ) -> Result<JsonValue, super::confirm::Unperformed> {
            self.0.borrow_mut().push(inputs.clone());
            Ok(JsonValue::Object(JsonObject::from([(
                "account_id".to_owned(),
                inputs["principal_uri"].clone(),
            )])))
        }
    }
    let reader = Reader(RefCell::new(Vec::new()));
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({"inputs":{}})).unwrap();
    let run = || {
        let admission = fixture.admit("issues", &inputs).unwrap();
        let mut host = Approver::new(None);
        let admission = match fixture.prepare("issues", admission, &mut host).unwrap() {
            EffectPreparationOutcome::Ready(admission) => Some(admission),
            _ => None,
        }
        .expect("a read needs no approval");
        execute(
            admission.context::<OperationAdmission>().unwrap(),
            "issues",
            &fixture.env,
            root,
            &reader,
            &mut None,
        )
    };
    assert!(run().is_ok());
    assert_eq!(
        reader.0.borrow()[0]["principal_uri"].as_str(),
        Some("runx:principal:fixture/forge:account-one")
    );
    std::fs::write(
        &account,
        r#"{"id":"account-two","secret":"must-not-leave-probe"}"#,
    )
    .unwrap();
    let error = run().unwrap_err().to_string();
    assert!(error.contains("another account"), "{error}");
    assert!(!error.contains("must-not-leave-probe"));
    assert_eq!(
        reader.0.borrow().len(),
        1,
        "the changed session never reaches the process"
    );
}

/// A hosted connector with a write that may consume provider credits with
/// no quote, and one that may not.
const CREDITS: &str = r#"schema: runx.connector.v1
name: fixture/credits
title: Fixture Credits
description: Spend provider credits on a hosted fixture.
auth:
  delegated: {principal: {url: "https://credits.fixture.invalid/me", id: id, login: login}}
placements:
  hosted:
    auth: delegated
operations:
  credits.spend:
    description: Spend credits on one job.
    verb: write
    scopes: [credits.spend]
    target:
      template: {"$argument": "/job"}
    cost: {per: request, source: docs}
    result_fields: [job]
    provider_scopes: ["credits:write"]
  credits.note:
    description: Note one job.
    verb: write
    scopes: [credits.note]
    target:
      template: {"$argument": "/job"}
    result_fields: [job]
    provider_scopes: ["credits:write"]
"#;

impl Fixture {
    /// A project holding [`CREDITS`] under the grant `grant_a`, with a step
    /// for each of its operations.
    fn credits() -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("connectors/fixture/credits");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("connector.yaml"), CREDITS).unwrap();
        let declared = runx_parser::ConnectorRequirement {
            operations: ["credits.spend", "credits.note"]
                .into_iter()
                .filter_map(|name| runx_contracts::connector::OperationName::new(name.to_owned()))
                .collect(),
            manifest_digest: None,
            choose: None,
            from: Vec::new(),
        };
        let steps = ["spend", "note"]
            .into_iter()
            .map(|id| {
                let step = WorkflowStep {
                    id: id.to_owned(),
                    kind: runx_parser::StepKind::Operation(runx_parser::OperationStepTarget {
                        requirement: "fixture/credits".to_owned(),
                        operation: runx_contracts::connector::OperationName::new(format!(
                            "credits.{id}"
                        )),
                        from: Vec::new(),
                        declared: Some(declared.clone()),
                        loaded: None,
                    }),
                    artifacts: None,
                    outputs: None,
                    inputs: JsonObject::new(),
                    bindings: Vec::new(),
                    act: None,
                    context_skills: Vec::new(),
                    when: None,
                    guard: Vec::new(),
                    retry_unknown_outcomes: false,
                    effects: None,
                    reach: None,
                };
                (id.to_owned(), step)
            })
            .collect();
        let env = BTreeMap::from([
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
            (
                crate::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
                root.path().join("receipts").to_string_lossy().into_owned(),
            ),
        ]);
        let provider = Provider::new(Answer::Succeed);
        let effect = OperationEffect::with_http_transport(provider.clone());
        let fixture = Self {
            root,
            steps,
            env,
            run_id: "run-1",
            provider,
            effect,
        };
        let connector = crate::kernel::connectors::ConnectorScope::at(fixture.root.path())
            .unwrap()
            .connector("fixture/credits")
            .unwrap();
        fixture.provider.grant(
            "grant_a",
            &connector,
            &["credits.spend".to_owned(), "credits.note".to_owned()],
        );
        crate::kernel::setup::confirm_grant_over(
            &Instances::new(
                KernelStore::for_project(&crate::paths::RunxPaths::resolve(
                    &fixture.env,
                    fixture.root.path(),
                ))
                .unwrap(),
            ),
            &connector,
            Arc::new(fixture.provider.clone()),
            "grant_a",
            &fixture.env,
            fixture.root.path(),
        )
        .unwrap();
        fixture
    }

    /// Admit the step `step` and prepare it under a person who answers as
    /// `actor`: the gates the person was shown, and whether it may run.
    fn gated(
        &self,
        step: &str,
        run_authority: Option<&crate::RunAuthority>,
        held_by_effects: bool,
        actor: Option<ResolutionResponseActor>,
    ) -> (Vec<runx_contracts::ApprovalGate>, bool) {
        let inputs: JsonObject = serde_json::from_value(serde_json::json!({
            "idempotency_key": "job-1-call",
            "inputs": {"job": "job-1"},
        }))
        .unwrap();
        let admission = self
            .admit_held(step, &inputs, run_authority, held_by_effects)
            .unwrap();
        let mut host = Approver::new(actor);
        let ready = matches!(
            self.prepare(step, admission, &mut host).unwrap(),
            EffectPreparationOutcome::Ready(_)
        );
        let gates = host
            .requests
            .into_iter()
            .filter_map(|request| match request {
                ResolutionRequest::Approval { gate, .. } => Some(*gate),
                _ => None,
            })
            .collect();
        (gates, ready)
    }
}

/// The type of each reason a gate carries, when it carries more than one.
fn reasons(gate: &runx_contracts::ApprovalGate) -> Vec<String> {
    gate.summary
        .as_ref()
        .and_then(|summary| summary.get("reasons"))
        .and_then(JsonValue::as_array)
        .map(|reasons| {
            reasons
                .iter()
                .filter_map(|reason| reason.as_object()?.get("type")?.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_held_write_that_may_consume_credits_waits_once_on_a_gate_naming_both_reasons() {
    let fixture = Fixture::credits();

    let (gates, ready) = fixture.gated("spend", None, true, None);
    assert!(!ready);
    assert_eq!(gates.len(), 1, "one gate: {gates:?}");
    let gate = &gates[0];
    assert_eq!(gate.gate_type.as_deref(), Some("write"));
    assert_eq!(reasons(gate), ["write", "provider_usage"]);
    assert!(
        gate.reason.as_str().contains("effects ask a person"),
        "{gate:?}"
    );
    assert!(
        gate.reason.as_str().contains("provider-account credits"),
        "{gate:?}"
    );
    assert!(
        gate.id.as_str().starts_with("operation:sha256:"),
        "the gate is bound to the call's intent: {gate:?}"
    );

    // One approval answers each reason the gate carries.
    let (gates, ready) = fixture.gated("spend", None, true, Some(ResolutionResponseActor::Human));
    assert!(ready);
    assert_eq!(gates.len(), 1, "{gates:?}");
}

#[test]
fn a_write_with_only_the_floors_reason_or_only_admissions_still_waits() {
    let fixture = Fixture::credits();

    // The floor's reason alone: a write with no approval of its own.
    let (gates, ready) = fixture.gated("note", None, true, None);
    assert!(!ready);
    assert_eq!(gates.len(), 1, "one gate: {gates:?}");
    let gate = &gates[0];
    assert_eq!(gate.gate_type.as_deref(), Some("write"));
    assert!(
        reasons(gate).is_empty(),
        "one reason, named by the gate: {gate:?}"
    );

    // Admission's alone: the floor did not hold the call.
    let (gates, ready) = fixture.gated("spend", None, false, None);
    assert!(!ready);
    assert_eq!(gates.len(), 1, "one gate: {gates:?}");
    let gate = &gates[0];
    assert_eq!(gate.gate_type.as_deref(), Some("provider_usage"));
    assert!(reasons(gate).is_empty(), "{gate:?}");

    // Neither: nothing waits.
    let (gates, ready) = fixture.gated("note", None, false, None);
    assert!(ready);
    assert!(gates.is_empty(), "{gates:?}");
}

#[test]
fn a_use_bound_on_another_instance_leaves_one_gate_naming_both_reasons() {
    let fixture = Fixture::credits();
    let actor_ref = Reference::with_uri(ReferenceType::Principal, "runx:principal:assistant:test");
    let delegation = |grant: &str, uses: Option<u64>| {
        delegation(
            "fixture/credits",
            Reference::runx(
                ReferenceType::ConnectorInstance,
                crate::kernel::instances::instance_id(&["hosted", "fixture/credits", grant])
                    .unwrap()
                    .as_str(),
            ),
            actor_ref.clone(),
            vec![AuthorityVerb::Write],
            AuthorityBounds {
                operations: vec!["fixture/credits/credits.spend".into()],
                scopes: vec!["credits.spend".into()],
                targets: vec!["*".into()],
                uses: uses.map(|total| runx_contracts::AuthorityUseLimit {
                    total: Some(total),
                    per_day: None,
                }),
                packages: Vec::new(),
            },
        )
    };
    // The call runs on grant_a's instance, delegated without a use bound;
    // another instance's delegation bounds its uses.
    let authority = crate::RunAuthority {
        actor_ref: Some(actor_ref.clone()),
        mandate_ref: Some(Reference::runx(ReferenceType::Mandate, "test-mandate")),
        delegations: vec![delegation("grant_a", None), delegation("grant_b", Some(3))],
        ..crate::RunAuthority::default()
    };

    let (gates, ready) = fixture.gated("spend", Some(&authority), true, None);
    assert!(!ready);
    assert_eq!(gates.len(), 1, "one gate: {gates:?}");
    let gate = &gates[0];
    assert_eq!(reasons(gate), ["write", "provider_usage"]);

    let (gates, ready) = fixture.gated(
        "spend",
        Some(&authority),
        true,
        Some(ResolutionResponseActor::Human),
    );
    assert!(ready);
    assert_eq!(gates.len(), 1, "{gates:?}");
}

/// A host's delegation on `connector`'s instance at `resource_ref`, as the
/// root's admission records it, to `principal_ref`, holding one term of
/// `verbs` over `bounds`.
fn delegation(
    connector: &str,
    resource_ref: Reference,
    principal_ref: Reference,
    verbs: Vec<AuthorityVerb>,
    bounds: AuthorityBounds,
) -> crate::Delegation {
    crate::Delegation {
        connector: runx_contracts::connector::ConnectorName::new(connector).unwrap(),
        resource_ref: Some(resource_ref),
        principal_ref: Some(principal_ref),
        expires_at: None,
        terms: vec![crate::DelegationTerm { verbs, bounds }].into(),
    }
}

/// `delegation` with each of its terms' uses bounded by `uses`.
fn with_uses(delegation: &mut crate::Delegation, uses: Option<runx_contracts::AuthorityUseLimit>) {
    let mut terms = delegation.terms.clone().into_vec();
    for term in &mut terms {
        term.bounds.uses.clone_from(&uses);
    }
    delegation.terms = terms.into();
}

/// A read records the credential it presented as a fingerprint keyed by the
/// runx home, never the credential: the same token fingerprints the same, a
/// swapped token differently, and a hosted grant, whose credential runx
/// never holds, not at all, so reuse never takes it as the same (§9, revise).
#[test]
fn a_read_fingerprints_the_credential_it_presented() {
    let fixture = Fixture::for_connector("fixture/forge", &["repo.read"]);
    let read: JsonObject = serde_json::from_value(serde_json::json!({
        "inputs": {"repository": "acme/app", "numbers": [7]},
    }))
    .unwrap();
    let presented = |fixture: &Fixture| {
        fixture
            .run("issues", &read, None)
            .expect("the read runs")
            .1
            .credential
    };
    assert_eq!(presented(&fixture), None, "a hosted grant's credential");
    fixture.set_credential("fixture/forge", "token", "forge-local-token");
    let first = presented(&fixture).expect("a local token is fingerprinted");
    assert!(!first.contains("forge-local-token"), "{first}");
    assert_eq!(presented(&fixture), Some(first.clone()));
    fixture.set_credential("fixture/forge", "token", "a-rotated-token");
    assert_ne!(presented(&fixture), Some(first));
}
