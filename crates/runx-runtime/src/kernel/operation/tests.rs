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
use crate::kernel::standing::{StandingApproval, StandingApprovals};
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
"#;

/// How the fake provider answers a call.
#[derive(Clone, Copy, PartialEq)]
enum Answer {
    Succeed,
    TimeOut,
    Reject,
    Unbound,
    WrongProvider,
}

#[derive(Clone)]
struct Provider {
    answer: Arc<Mutex<Answer>>,
    calls: Arc<Mutex<Vec<JsonObject>>>,
}

impl Provider {
    fn new(answer: Answer) -> Self {
        Self {
            answer: Arc::new(Mutex::new(answer)),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
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
            return Ok(RuntimeHttpResponse::new(
                200,
                format!(
                    r#"{{"status":"success","principal":{{"principal_id":"{PRINCIPAL_ID}"}}}}"#
                ),
            ));
        }
        let body: JsonObject =
            serde_json::from_str(request.body.as_deref().unwrap_or("{}")).unwrap();
        self.calls.lock().unwrap().push(body.clone());
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
            Answer::Unbound => Ok(RuntimeHttpResponse::new(
                403,
                r#"{"status":"error","error":{"code":"binding_unavailable","detail":"unbound"}}"#,
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
        let root = tempfile::tempdir().unwrap();
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
                crate::receipts::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
                root.path().join("receipts").to_string_lossy().into_owned(),
            ),
            (
                crate::INJECTED_GRANT_ID_ENV.to_owned(),
                "grant_slack".to_owned(),
            ),
            (
                crate::INJECTED_GRANTED_SCOPES_ENV.to_owned(),
                crate::encode_injected_scopes(&scopes).unwrap(),
            ),
            (
                crate::INJECTED_PRINCIPAL_REF_ENV.to_owned(),
                PRINCIPAL_REF.to_owned(),
            ),
        ]);
        let provider = Provider::new(Answer::Succeed);
        let effect = OperationEffect::with_http_transport(provider.clone());
        Self {
            root,
            steps,
            env,
            run_id: "run-1",
            provider,
            effect,
        }
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
        let admission = self
            .admit(step, inputs)
            .map_err(|error| error.to_string())?;
        let mut host = Approver::new(actor);
        let EffectPreparationOutcome::Ready(admission) = self
            .prepare(step, admission, &mut host)
            .map_err(|error| error.to_string())?
        else {
            return Err("pending".to_owned());
        };
        let context = admission.context::<OperationAdmission>().unwrap();
        let outcome = execute(context, step, &self.env, self.root.path(), &NoTools)
            .map_err(|error| error.to_string())?;
        Ok((*admission, outcome))
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
    let (admission, outcome) = fixture
        .run(
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
    assert_eq!(outcome.boundary, ExecutionBoundaryKind::RemoteProvider);
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
    };
    assert!(fixture.admit_as("read", &read, Some(&authority)).is_err());

    authority.delegations.push(AttenuationRequest {
        principal_ref: actor_ref,
        resource_ref: Reference::with_uri(
            ReferenceType::ConnectorInstance,
            "runx:connector_instance:injected.slack",
        ),
        resource_family: AuthorityResourceFamily::Connector,
        verbs: vec![AuthorityVerb::Read],
        capabilities: Vec::new(),
        bounds: AuthorityBounds {
            operations: vec!["slack/channel.post.read".into()],
            scopes: vec!["channel.post.read".into()],
            targets: vec!["slack://T123/C456".into()],
            ..AuthorityBounds::default()
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
fn a_standing_approval_covers_only_the_pinned_post() {
    use runx_contracts::connector::{ConnectorId, InstanceId, OperationName};

    let mut fixture = Fixture::new(&["channel.post"]);
    let delegate = format!("sha256:{}", "a".repeat(64));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    StandingApprovals::new(KernelStore::for_workspace(&fixture.env, fixture.root.path()).unwrap())
        .grant(StandingApproval {
            approval_id: "notify".to_owned(),
            connector: ConnectorId::new("slack").unwrap(),
            operation: OperationName::new("channel.post").unwrap(),
            instance_id: InstanceId::new("injected.slack").unwrap(),
            target: "slack://T123/C456".to_owned(),
            delegate_digest: delegate.clone(),
            expires_at_unix_seconds: now + 3_600,
            max_total: 1,
            max_per_day: 1,
            max_text_bytes: 200,
            allowed_mention: None,
        })
        .expect("grant");
    fixture
        .env
        .insert(STANDING_APPROVAL_ID_ENV.to_owned(), "notify".to_owned());
    fixture
        .env
        .insert(STANDING_DELEGATE_DIGEST_ENV.to_owned(), delegate);
    let (admission, _) = fixture
        .run("post", &post("deploy done"), None)
        .expect("covered");
    let plan = admission
        .context::<OperationAdmission>()
        .unwrap()
        .attempt
        .clone()
        .unwrap();
    assert_eq!(plan.approval.unwrap().actor, ApprovalActor::Standing);

    let mut elsewhere = post("hi");
    elsewhere.insert(
        "input".to_owned(),
        serde_json::from_value(
            serde_json::json!({"channel_locator": "slack://T123/C999", "text": "hi"}),
        )
        .unwrap(),
    );
    elsewhere.insert(
        "idempotency_key".to_owned(),
        JsonValue::String("post-2".to_owned()),
    );
    fixture.run_id = "run-2";
    let error = fixture
        .run("post", &elsewhere, None)
        .expect_err("another target");
    assert!(error.contains("differs from the approval"), "{error}");

    let mut again = post("again");
    again.insert(
        "idempotency_key".to_owned(),
        JsonValue::String("post-3".to_owned()),
    );
    let error = fixture.run("post", &again, None).expect_err("quota spent");
    assert!(error.contains("quota"), "{error}");
    assert_eq!(fixture.provider.calls(), 1);
}

#[test]
fn the_host_guard_shows_exact_input_and_refuses_protected_material() {
    let mut fixture = Fixture::new(&["channel.post"]);
    fixture.env.insert(
        egress::REQUIRE_MUTATION_APPROVAL_ENV.to_owned(),
        "required".to_owned(),
    );
    fixture.env.insert(
        egress::CONFIDENTIAL_TERMS_ENV.to_owned(),
        r#"["project-x"]"#.to_owned(),
    );
    let admission = fixture.admit("post", &post("ship it")).expect("admits");
    let mut host = Approver::new(None);
    fixture
        .prepare("post", admission, &mut host)
        .expect("pending");
    let request = serde_json::to_string(&host.requests).unwrap();
    assert!(request.contains("exact_input") && request.contains("ship it"));
    let error = fixture
        .admit("post", &post("about project-x"))
        .expect_err("refused")
        .to_string();
    assert!(error.contains("protected"), "{error}");
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

#[test]
fn paid_job_authority_cannot_satisfy_an_approval_floor() {
    let mut fixture = Fixture::new(&["channel.post"]);
    fixture
        .env
        .insert(PAID_EXTERNAL_JOB_AUTHORITY_ENV.to_owned(), "{}".to_owned());
    let error = fixture.run("post", &post("hi"), None).expect_err("refused");
    assert!(error.contains("not paid job authority"), "{error}");
}

#[test]
fn paid_job_authority_cannot_satisfy_an_explicit_call_gate() {
    let mut fixture = Fixture::new(&["aws.transcribe.execute"]);
    fixture
        .env
        .insert(PAID_EXTERNAL_JOB_AUTHORITY_ENV.to_owned(), "{}".to_owned());
    let call: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {"target": "s3://media/interview.mp3"},
        "idempotency_key": "transcribe-1",
        "approval": {"reason": "review this transcription job"}
    }))
    .unwrap();
    let error = fixture.run("transcribe", &call, None).expect_err("refused");
    assert!(error.contains("not paid job authority"), "{error}");
}

#[test]
fn an_enforced_operation_resends_after_an_unknown_outcome() {
    let fixture = Fixture::new(&["aws.transcribe.execute"]);
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

#[test]
fn a_later_refusal_does_not_settle_an_earlier_unknown_attempt() {
    let fixture = Fixture::new(&["aws.transcribe.execute"]);
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
fn a_standing_use_is_released_when_the_provider_refuses_before_executing() {
    use runx_contracts::connector::{ConnectorId, InstanceId, OperationName};

    let mut fixture = Fixture::new(&["channel.post"]);
    let delegate = format!("sha256:{}", "a".repeat(64));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let approvals = StandingApprovals::new(
        KernelStore::for_workspace(&fixture.env, fixture.root.path()).unwrap(),
    );
    approvals
        .grant(StandingApproval {
            approval_id: "notify".to_owned(),
            connector: ConnectorId::new("slack").unwrap(),
            operation: OperationName::new("channel.post").unwrap(),
            instance_id: InstanceId::new("injected.slack").unwrap(),
            target: "slack://T123/C456".to_owned(),
            delegate_digest: delegate.clone(),
            expires_at_unix_seconds: now + 3_600,
            max_total: 1,
            max_per_day: 1,
            max_text_bytes: 200,
            allowed_mention: None,
        })
        .expect("grant");
    fixture
        .env
        .insert(STANDING_APPROVAL_ID_ENV.to_owned(), "notify".to_owned());
    fixture
        .env
        .insert(STANDING_DELEGATE_DIGEST_ENV.to_owned(), delegate);
    fixture.provider.answer(Answer::Unbound);
    fixture
        .run("post", &post("hi"), None)
        .expect_err("refused before executing");
    assert_eq!(approvals.status("notify").unwrap().unwrap().used_total, 0);
    fixture.provider.answer(Answer::Succeed);
    fixture
        .run("post", &post("hi"), None)
        .expect("the released use is spent once");
    assert_eq!(approvals.status("notify").unwrap().unwrap().used_total, 1);
}

#[test]
fn a_hosted_declared_write_sends_each_request_through_connect_and_reads_it_back() {
    let fixture = Fixture::new(&["repo.write"]);
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
    let fixture = Fixture::new(&["repo.read"]);
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
fn an_unauthenticated_write_is_acknowledged_once_behind_exact_approval() {
    // The injected grant is Slack's; n8n takes no authority and runs public.
    let fixture = Fixture::new(&["channel.post"]);
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({
        "input": {
            "webhook_host": "acme.app.n8n.cloud",
            "workflow_slug": "runx-effect",
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
    let (_, outcome) = fixture
        .run("deliver", &inputs, Some(ResolutionResponseActor::Human))
        .expect("runs");
    let packet = packet(&outcome);
    assert_eq!(
        packet["finality"],
        JsonValue::String("acknowledged".to_owned())
    );
    assert!(
        packet["operation_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("http:sha256:")),
        "an acknowledgement without a provider id is identified by its answer"
    );
    assert_eq!(
        serde_json::to_value(&packet["result"]).unwrap()["acknowledgement"]["message"],
        "Workflow was started"
    );
    assert_eq!(fixture.provider.calls(), 1, "no readback is sent");
}

/// A plugin connector whose MCP server is a process the runtime starts over
/// stdio: the instance's stored credential reaches it in the variable its
/// declaration names, beside the base host environment every host process
/// gets, and nothing else of the operator's. The server echoes what it holds,
/// and the echo of the credential is redacted before the answer is kept.
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
    let connector_dir = root.path().join("connectors/probe-mcp");
    std::fs::create_dir_all(&connector_dir).unwrap();
    std::fs::write(
        connector_dir.join("connector.yaml"),
        format!(
            r#"schema: runx.connector.v1
id: probe-mcp
name: Probe
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
            crate::receipts::paths::RUNX_CWD_ENV.to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            crate::receipts::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
            root.path().join("receipts").to_string_lossy().into_owned(),
        ),
        (
            crate::execution::runner::RUNX_RUN_ID_ENV.to_owned(),
            "run-1".to_owned(),
        ),
    ]);
    let connector = crate::ConnectorScope::for_skill(&skill_dir)
        .unwrap()
        .connector("probe-mcp")
        .unwrap();
    crate::kernel::setup::set_credential(
        &crate::Instances::new(crate::KernelStore::for_workspace(&env, &skill_dir).unwrap()),
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
        let outcome = execute(context, "read", &env, &skill_dir, &NoTools).unwrap();
        serde_json::to_value(&packet(&outcome)["result"]).unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(read("PROBE_KEY"), super::credentials::REDACTED);
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
