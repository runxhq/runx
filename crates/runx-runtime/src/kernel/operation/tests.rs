#![allow(clippy::expect_used, clippy::unwrap_used)]

//! The operation path end to end through its effect hooks: admission,
//! approval lanes, custody, dispatch to a hosted transport and the packet.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use runx_contracts::{
    AuthorityBounds, AuthorityResourceFamily, AuthorityVerb, ExecutionEvent, JsonObject, JsonValue,
    Reference, ReferenceType, ResolutionRequest, ResolutionResponse, ResolutionResponseActor,
};
use runx_core::policy::AttenuationRequest;
use runx_parser::GraphStep;

use super::*;
use crate::effects::{
    EffectPreparationOutcome, EffectStepRequest, ResolvedEffectTarget, RuntimeEffect,
};
use crate::http::{HttpMethod, RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse};
use crate::{Host, RuntimeError};

const BASE_URL: &str = "https://provider-fixture.runx.invalid";
const PRINCIPAL_ID: &str = "operator-1";
const PRINCIPAL_REF: &str = "runx:principal:operator-1";
const SKILL: &str = r#"
skill: operation-test
connectors:
  slack:
    operations: [channel.post, channel.post.read]
  aws:
    operations: [aws.transcribe.start]
  osv:
    operations: [vulnerabilities.read]
  github:
    operations: [issues.update, issues.read]
  n8n:
    operations: [workflow.invoke]
  artifact:
    operations: [artifact.allocate]
runners:
  run:
    result_from: [post]
    steps:
      - id: post
        operation: slack/channel.post
      - id: read
        operation: slack/channel.post.read
      - id: transcribe
        operation: aws/aws.transcribe.start
      - id: advisories
        operation: osv/vulnerabilities.read
      - id: update
        operation: github/issues.update
      - id: issues
        operation: github/issues.read
      - id: deliver
        operation: n8n/workflow.invoke
      - id: allocate
        operation: artifact/artifact.allocate
"#;

/// How the fake provider answers a call.
#[derive(Clone, Copy, PartialEq)]
enum Answer {
    Succeed,
    TimeOut,
    Reject,
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
            artifacts: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// runx's artifact space: an allocation is keyed by its idempotency key
    /// and an inspection reads what is held under a reference.
    fn artifact(&self, body: &serde_json::Value) -> serde_json::Value {
        let operation = body["operation"].as_str().unwrap_or_default();
        let input = &body["input"];
        let (artifact_ref, result) = match operation {
            "artifact.allocate" => {
                use base64::Engine as _;
                let key = input["idempotency_key"].as_str().unwrap();
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(input["data_base64"].as_str().unwrap())
                    .unwrap();
                let artifact_ref = format!(
                    "runx:artifact:{}",
                    runx_contracts::sha256_prefixed(key.as_bytes())
                );
                let result = serde_json::json!({
                    "artifact_ref": artifact_ref,
                    "content_digest": input["content_digest"],
                    "media_type": input["media_type"],
                    "size_bytes": bytes.len(),
                    "created_at": "2026-10-13T00:00:00.000Z",
                });
                self.artifacts
                    .lock()
                    .unwrap()
                    .insert(artifact_ref.clone(), result.clone());
                (artifact_ref, result)
            }
            _ => {
                let artifact_ref = input["artifact_ref"].as_str().unwrap().to_owned();
                let result = self.artifacts.lock().unwrap()[&artifact_ref].clone();
                (artifact_ref, result)
            }
        };
        let owner = self
            .artifact_owner
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| self.principal.lock().unwrap().clone());
        let mut packet = serde_json::json!({
            "schema": "runx.provider.operation.v1",
            "status": "success",
            "provider": "runx-artifact",
            "operation": operation,
            "target": artifact_ref,
            "result": result,
            "transport": "hosted_loopback",
            "readback_ref": "runx:artifact-readback:1",
            "access": if operation == "artifact.allocate" { "mutate" } else { "read" },
            "principal_ref": format!("runx:principal:{owner}"),
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
            "provider": connector.manifest.id.as_str(),
            "connector_binding": connector.binding(),
            "status": "active",
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
            // The issue as GitHub answers both the write and its readback.
            let sent: serde_json::Value =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let number = sent["input"]["updates"][0]["number"].as_u64().unwrap_or(7);
            let issue = serde_json::json!({"number": number, "title": "ready", "body": null, "state": "open"});
            let answer = serde_json::json!({"status": 200, "body": issue.to_string()});
            return Ok(RuntimeHttpResponse::new(200, answer.to_string()));
        }
        if request.url.starts_with("https://acme.app.n8n.cloud/") {
            // An n8n webhook acknowledges with whatever the workflow answers.
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
                    (Answer::WrongProvider, _) => "github",
                    (_, "aws") => "aws",
                    _ => "slack",
                };
                let result = match provider {
                    "aws" => serde_json::json!({
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
                        serde_json::json!({"message_locator": "slack://T123/C456/1.0", "text": "hi"})
                    }
                };
                let response = serde_json::json!({
                    "status": "success",
                    "provider": provider,
                    "operation": field("operation"),
                    "target": field("target"),
                    "access": field("access"),
                    "result": result,
                    "readback_ref": "runx:provider-readback:1",
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
    steps: BTreeMap<String, GraphStep>,
    env: BTreeMap<String, String>,
    run_id: &'static str,
    provider: Provider,
    effect: OperationEffect,
}

impl Fixture {
    fn new(scopes: &[&str]) -> Self {
        Self::for_connector("runx/slack", scopes)
    }

    fn for_connector(connector: &str, scopes: &[&str]) -> Self {
        let root = tempfile::tempdir().unwrap();
        crate::kernel::connectors::plugin_with_source_connectors(
            root.path(),
            &["slack", "aws", "osv", "github", "n8n"],
        );
        let manifest = runx_parser::validate_runner_manifest(
            runx_parser::parse_runner_manifest_yaml(SKILL).unwrap(),
        )
        .unwrap();
        let steps = manifest.runners["run"]
            .graph
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
        fixture.confirm_grant("grant_slack", connector, &scopes);
        fixture
    }

    /// List a grant of `scopes` for `connector` and confirm it as the
    /// connector's instance, as `runx connector use --grant` would.
    fn confirm_grant(&self, grant_id: &str, connector: &str, scopes: &[String]) {
        self.list_grant(grant_id, connector, scopes);
        self.select_grant(grant_id, connector);
    }

    /// List a grant runx Connect holds without confirming it yet.
    fn list_grant(&self, grant_id: &str, connector: &str, scopes: &[String]) {
        let connector = crate::kernel::connectors::source_scope()
            .connector(connector)
            .unwrap();
        self.provider.grant(grant_id, &connector, scopes);
    }

    /// Confirm a listed grant as the connector's selected instance.
    fn select_grant(&self, grant_id: &str, connector: &str) {
        self.try_select_grant(grant_id, connector).unwrap();
    }

    fn try_select_grant(&self, grant_id: &str, connector: &str) -> Result<(), String> {
        let connector = crate::kernel::connectors::source_scope()
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

    /// Store `value` as `connector`'s `credential` and select it, as
    /// `runx credential set` would.
    fn set_credential(&self, connector: &str, credential: &str, value: &str) {
        let connector = crate::kernel::connectors::source_scope()
            .connector(connector)
            .unwrap();
        let instances = Instances::new(
            KernelStore::for_project(&crate::paths::RunxPaths::resolve(
                &self.env,
                self.root.path(),
            ))
            .unwrap(),
        );
        crate::kernel::setup::set_credential(
            &instances,
            &connector,
            "default",
            &runx_contracts::connector::CredentialName::new(credential).unwrap(),
            value,
            &[],
            JsonObject::new(),
            &crate::services::WorkspaceEnv::new(self.env.clone(), self.root.path().to_path_buf())
                .unwrap(),
        )
        .unwrap();
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
        let step = &self.steps[step];
        self.effect
            .admit(EffectStepRequest {
                step,
                target: ResolvedEffectTarget::default(),
                inputs,
                env: &self.env,
                graph_dir: self.root.path(),
                run_id: self.run_id,
                run_authority,
            })
            .map(|admission| admission.expect("an operation step is owned by the effect"))
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

fn post(text: &str) -> JsonObject {
    serde_json::from_value(serde_json::json!({
        "input": {"channel_locator": "slack://T123/C456", "text": text},
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
        "slack keys are UUIDs"
    );
    assert_eq!(
        packet["operation_digest"].as_str(),
        Some(context.admitted.intent().operation_digest.as_str())
    );
    let admitted = &context.admitted.admission;
    assert_eq!(
        admitted.disposition,
        runx_contracts::AdmissionDisposition::Admit
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
        context.admitted.admission.disposition,
        runx_contracts::AdmissionDisposition::NeedsResolution
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
        "input": {"conversation_locator": "slack://T123/C456"},
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

#[test]
fn acting_for_another_principal_requires_a_bounded_delegation() {
    let mut fixture = Fixture::new(&["channel.post", "channel.post.read"]);
    let read: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {"conversation_locator": "slack://T123/C456"}
    }))
    .unwrap();
    let actor_ref = Reference::with_uri(ReferenceType::Principal, "runx:principal:assistant:test");
    let mut authority = crate::RunAuthority {
        actor_ref: actor_ref.clone(),
        mandate_ref: Some(Reference::runx(ReferenceType::Mandate, "test-mandate")),
        delegations: Vec::new(),
        max_fanout: None,
        max_child_depth: None,
        paid_job: None,
    };
    assert!(fixture.admit_as("read", &read, Some(&authority)).is_err());

    authority.delegations.push(AttenuationRequest {
        principal_ref: actor_ref,
        resource_ref: Reference::runx(
            ReferenceType::ConnectorInstance,
            crate::kernel::instances::instance_id(&["hosted", "runx/slack", "grant_slack"])
                .unwrap()
                .as_str(),
        ),
        resource_family: AuthorityResourceFamily::Connector,
        verbs: vec![AuthorityVerb::Read],
        bounds: AuthorityBounds {
            operations: vec!["runx/slack/channel.post.read".into()],
            scopes: vec!["channel.post.read".into()],
            targets: vec!["slack://T123/C456".into()],
            uses: None,
        },
        expires_at: None,
    });
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
        authority.actor_ref
    );
    assert!(
        fixture
            .admit_as("post", &post("hi"), Some(&authority))
            .is_err()
    );
    authority.delegations[0].bounds.uses = Some(runx_contracts::AuthorityUseLimit {
        total: Some(1),
        per_day: None,
    });
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

/// A grant is confirmed only for the exact connector Connect executes it
/// under: another revision, or none, is refused.
#[test]
fn a_grant_bound_to_another_revision_or_none_is_never_confirmed() {
    let fixture = Fixture::new(&["channel.post"]);
    let slack = crate::kernel::connectors::source_scope()
        .connector("runx/slack")
        .unwrap();
    let mut binding = slack.binding();
    binding.connector_digest =
        runx_contracts::connector::Digest::new(format!("sha256:{}", "0".repeat(64))).unwrap();
    fixture.provider.grants.lock().unwrap().extend([
        serde_json::json!({
            "grant_id": "grant_slack_other",
            "provider": "slack",
            "connector_binding": binding,
            "status": "active",
            "scopes": ["channel.post"],
        }),
        serde_json::json!({
            "grant_id": "grant_slack_unbound",
            "provider": "slack",
            "status": "active",
            "scopes": ["channel.post"],
        }),
    ]);

    let other = fixture
        .try_select_grant("grant_slack_other", "runx/slack")
        .unwrap_err();
    assert!(
        other.contains("does not bind connector runx/slack"),
        "{other}"
    );
    let unbound = fixture
        .try_select_grant("grant_slack_unbound", "runx/slack")
        .unwrap_err();
    assert!(unbound.contains("names no connector"), "{unbound}");
}

#[test]
fn a_grant_that_lacks_the_scope_refuses_before_anything_is_sent() {
    let fixture = Fixture::new(&["channel.post.read"]);
    let error = fixture
        .admit("post", &post("hi"))
        .expect_err("refused")
        .to_string();
    assert!(error.contains("grant_slack does not cover"), "{error}");
}

#[test]
fn a_target_outside_the_declared_pattern_is_refused_before_anything_is_sent() {
    let fixture = Fixture::new(&["channel.post"]);
    let call: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {"channel_locator": "slack://T123/C456/extra", "text": "hi"},
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
            .map(|index| format!("OSV-{index}"))
            .collect::<Vec<_>>();
        serde_json::from_value::<JsonObject>(serde_json::json!({"input": {"ids": ids}})).unwrap()
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
        !error.contains("OSV-50"),
        "the refusal names the path, never the value: {error}"
    );
    assert_eq!(fixture.provider.calls(), 0);
}

#[test]
fn a_readback_naming_another_provider_is_an_unknown_outcome() {
    let fixture = Fixture::new(&["channel.post"]);
    fixture.provider.answer(Answer::WrongProvider);
    let error = fixture
        .run("post", &post("hi"), Some(ResolutionResponseActor::Human))
        .expect_err("unknown");
    assert!(error.contains("names provider"), "{error}");
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
fn the_approval_summary_carries_digests_not_payloads() {
    let fixture = Fixture::new(&["channel.post"]);
    const SECRET: &str = "credential-material-must-never-cross";
    let admission = fixture.admit("post", &post(SECRET)).expect("admits");
    let mut host = Approver::new(None);
    fixture
        .prepare("post", admission, &mut host)
        .expect("pending");
    let request = serde_json::to_string(&host.requests).unwrap();
    assert!(!request.contains(SECRET));
    assert!(request.contains("payload_digest"));
}

/// A host's authority for a run that executes one stage of a paid job held
/// by `principal`.
fn paid_job(principal: &str) -> crate::RunAuthority {
    let digest = format!("sha256:{}", "e".repeat(64));
    let stage = serde_json::json!({
        "continuation": {
            "continuation_id": "job-1",
            "principal_ref": {"type": "principal", "uri": principal},
            "vendor_ref": {"type": "principal", "uri": "runx:principal:vendor"},
            "invocation_ref": {"type": "target", "uri": "runx:paid-invocation:paid-1"},
            "source_run_ref": {"type": "act", "uri": "runx:run:source-1"},
            "execution_binding": {
                "skill": "vendor/document-analysis",
                "runner": "continue",
                "package_digest": digest,
                "execution_closure_digest": digest,
            },
            "operation_identity": digest,
            "stage": "start",
            "status": "runnable",
            "attempts": 1,
            "max_attempts": 6,
            "deadline_at": "2026-08-27T01:00:00.000Z",
            "created_at": "2026-08-27T00:00:00.000Z",
            "updated_at": "2026-08-27T00:00:00.000Z",
        },
        "checkpoint": {"document": "input"},
        "operation_key": digest,
    });
    crate::RunAuthority {
        actor_ref: Reference::with_uri(ReferenceType::Principal, principal),
        mandate_ref: None,
        delegations: Vec::new(),
        max_fanout: None,
        max_child_depth: None,
        paid_job: Some(Box::new(serde_json::from_value(stage).unwrap())),
    }
}

#[test]
fn paid_job_authority_cannot_satisfy_an_approval_floor() {
    let fixture = Fixture::new(&["channel.post"]);
    let authority = paid_job(PRINCIPAL_REF);
    let error = fixture
        .run_under("post", &post("hi"), None, Some(&authority))
        .expect_err("refused");
    assert!(error.contains("not paid job authority"), "{error}");
}

#[test]
fn paid_job_authority_cannot_satisfy_an_explicit_call_gate() {
    let fixture = Fixture::for_connector("runx/aws", &["aws.transcribe.execute"]);
    let call: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {"target": "s3://media/interview.mp3"},
        "idempotency_key": "transcribe-1",
        "approval": {"reason": "review this transcription job"}
    }))
    .unwrap();
    let authority = paid_job(PRINCIPAL_REF);
    let error = fixture
        .run_under("transcribe", &call, None, Some(&authority))
        .expect_err("refused");
    assert!(error.contains("not paid job authority"), "{error}");
}

#[test]
fn an_enforced_operation_resends_after_an_unknown_outcome() {
    let fixture = Fixture::for_connector("runx/aws", &["aws.transcribe.execute"]);
    let start: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {"target": "s3://media/interview.mp3"},
        "idempotency_key": "transcribe-1",
    }))
    .unwrap();
    fixture.provider.answer(Answer::TimeOut);
    fixture
        .run("transcribe", &start, None)
        .expect_err("unknown");
    fixture.provider.answer(Answer::Succeed);
    fixture
        .run("transcribe", &start, None)
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
    let mut fixture = Fixture::for_connector("runx/aws", &["aws.transcribe.execute"]);
    let start: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {"target": "s3://media/interview.mp3"},
        "idempotency_key": "transcribe-1",
    }))
    .unwrap();
    fixture.provider.answer(Answer::TimeOut);
    fixture
        .run("transcribe", &start, None)
        .expect_err("unknown");
    // The same grant is confirmed again, now held by another account, and
    // the run resumes in a new process.
    *fixture.provider.principal.lock().unwrap() = "operator-2".to_owned();
    fixture.select_grant("grant_slack", "runx/aws");
    fixture.effect = OperationEffect::with_http_transport(fixture.provider.clone());
    fixture.provider.answer(Answer::Succeed);
    let error = fixture.run("transcribe", &start, None).expect_err("held");
    assert!(error.contains("reconcile it before resuming"), "{error}");
    assert_eq!(fixture.provider.calls(), 1, "nothing was sent again");
}

#[test]
fn a_later_refusal_does_not_settle_an_earlier_unknown_attempt() {
    let fixture = Fixture::for_connector("runx/aws", &["aws.transcribe.execute"]);
    let call: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {"target": "s3://media/interview.mp3"},
        "idempotency_key": "transcribe-1",
    }))
    .unwrap();
    fixture.provider.answer(Answer::TimeOut);
    fixture.run("transcribe", &call, None).expect_err("unknown");
    fixture.provider.answer(Answer::Reject);
    let error = fixture
        .run("transcribe", &call, None)
        .expect_err("the earlier outcome stays unknown");
    assert!(error.contains("prior attempt may have executed"), "{error}");
    let admission = fixture.admit("transcribe", &call).unwrap();
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
    let fixture = Fixture::for_connector("runx/github", &["repo.write"]);
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {"repository": "acme/app", "updates": [{"number": 7, "patch": {"title": "ready"}}]},
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
    let fixture = Fixture::for_connector("runx/github", &["repo.read"]);
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {"repository": "acme/app", "numbers": [7, 8]},
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

#[test]
fn a_webhook_write_is_acknowledged_once_behind_its_credential_and_exact_approval() {
    // The webhook's URL is its credential: the operator binds it, and the
    // call reaches only that URL.
    let fixture = Fixture::new(&["channel.post"]);
    fixture.set_credential(
        "runx/n8n",
        "webhook",
        "https://acme.app.n8n.cloud/webhook/runx-effect",
    );
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {
            "webhook_host": "acme.app.n8n.cloud",
            "delivery": {
                "event_id": "evt-1",
                "handoff_scope": "orchestrator.n8n.workflow.invoke",
                "handoff_audience": "n8n:workflow:runx-effect",
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
    let first_evidence = acknowledged["readback_ref"].as_str().unwrap().to_owned();
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
        "input": {
            "webhook_host": "acme.app.n8n.cloud",
            "delivery": {
                "event_id": "evt-2",
                "handoff_scope": "orchestrator.n8n.workflow.invoke",
                "handoff_audience": "n8n:workflow:runx-effect",
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
        packet(&outcome)["readback_ref"].as_str().unwrap(),
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
id: probe-mcp
name: Probe
description: Reads its own environment.
runtime_capability: false
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
    let manifest = runx_parser::validate_runner_manifest(
        runx_parser::parse_runner_manifest_yaml(
            r#"
skill: probe
connectors:
  probe-mcp:
    operations: [env.read]
runners:
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
    let step = manifest.runners["run"].graph.steps[0].clone();
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
            crate::execution::runner::RUNX_RUN_ID_ENV.to_owned(),
            "run-1".to_owned(),
        ),
    ]);
    let connector = crate::ConnectorScope::at(&skill_dir)
        .unwrap()
        .connector("probe-mcp")
        .unwrap();
    crate::kernel::setup::set_credential(
        &crate::Instances::new(
            crate::KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, &skill_dir))
                .unwrap(),
        ),
        &connector,
        "probe",
        &runx_contracts::connector::CredentialName::new("key").unwrap(),
        "probe-secret",
        &[],
        runx_contracts::JsonObject::new(),
        &crate::services::WorkspaceEnv::new(env.clone(), root.path().to_path_buf()).unwrap(),
    )
    .unwrap();
    let effect = OperationEffect::default();
    let read = |name: &str| {
        let inputs: JsonObject =
            serde_json::from_value(serde_json::json!({"input": {"name": name}})).unwrap();
        let admission = effect
            .admit(EffectStepRequest {
                step: &step,
                target: ResolvedEffectTarget::default(),
                inputs: &inputs,
                env: &env,
                graph_dir: &skill_dir,
                run_id: "run-1",
                run_authority: None,
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
    assert_eq!(effect.uri.as_str(), "slack://T123/C456/1.0");
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
    assert!(error.contains("is not a thread slack declares"), "{error}");
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
id: acme
name: Acme
description: A plugin-shaped connector.
runtime_capability: false
auth: {{none: {{}}}}
placements: {{local: {{auth: [none]}}}}
{references}
operations:
  items.create:
    description: Create items.
    verb: create
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
        "input": {
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
        crate::execution::runner::RUNX_RUN_ID_ENV.to_owned(),
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
    assert_eq!(
        result["principal_ref"],
        JsonValue::String(PRINCIPAL_REF.to_owned())
    );
    assert_eq!(
        result["content_digest"],
        JsonValue::String(runx_contracts::sha256_prefixed(br#"{"answer":42}"#))
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
fn an_artifact_held_for_another_principal_is_an_unknown_outcome() {
    let mut fixture = Fixture::new(&[]);
    fixture.env.insert(
        crate::execution::runner::RUNX_RUN_ID_ENV.to_owned(),
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
