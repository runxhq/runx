//! Operation steps: the kernel's one path for every connector call.
//!
//! The graph engine admits an operation step through [`OperationEffect`]:
//! admission ([`super::admission`]) proves the call, approval evidence is
//! gathered when its floor requires it, custody records the attempt, the
//! instance's placement executes it, and the readback becomes the step's
//! `runx.provider.operation.v1` packet. The admission carries everything the
//! execution needs, so nothing between admission and execution can swap the
//! instance, the implementation or the transport.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::connector::{
    ApprovalFloor, IdempotencyKeyFormat, Implementation, OperationAccess, Placement,
    ProviderIdempotency, implementation_digest,
};
use runx_contracts::schema::IsoDateTime;
use runx_contracts::{
    AdmissionDisposition, ApprovalGate, AuthorityApproval, AuthorityVerb, ExecutionBoundaryKind,
    ExternalJobStageRequest, InstanceAuthority, JsonObject, JsonValue, ProofKind, Reference,
    ReferenceType, ResolutionRequest,
};
use runx_core::state_machine::AuthorityAdmissionWitness;
use runx_parser::{DuplicatePolicy, GraphStep};

use super::admission::{self, AdmitError, Admitted, Context, Evaluation};
use super::connectors::ConnectorScope;
use super::custody::{Acknowledgement, ApprovalActor, ApprovalRecord, Attempt};
use super::instances::Instances;
use super::store::KernelStore;
use crate::approval::ApprovalResolution;
use crate::drivers::http::Egress;
use crate::effects::{
    EffectAdmission, EffectOutputRequest, EffectPreparationOutcome, EffectReceiptRequest,
    EffectStepRequest, RuntimeEffect, RuntimeEffectError,
};
use crate::http::{ReqwestHttpTransport, RuntimeHttpTransport};
use crate::{ProviderOperationRequest, RuntimeError};

pub(crate) mod call;
pub(crate) mod confirm;
pub(crate) mod credentials;
pub(crate) mod hosted;
pub(crate) mod packet;

use call::{OperationCall, ResultShape};
use confirm::Checks;
use hosted::HostedClient;
use packet::{Expected, Readback, Seal};

pub const OPERATION_EFFECT_FAMILY: &str = "operation";

type Transport = Arc<dyn RuntimeHttpTransport + Send + Sync>;

/// How the kernel reaches each placement.
#[derive(Default)]
struct Placements {
    hosted: HostedClient,
    /// Injected beneath local implementations; the runtime's own transports
    /// otherwise.
    http: Option<Transport>,
    #[cfg(feature = "mcp")]
    mcp: crate::drivers::mcp::Sessions,
    /// The principal each command session token was read as, by token
    /// digest, so a token's account is read once per process.
    principals: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl Placements {
    /// The transport for implementations whose hosts the manifest declares:
    /// private networks only where the operator allows them.
    fn declared_hosts(&self, env: &BTreeMap<String, String>) -> Result<Transport, Failure> {
        if let Some(transport) = &self.http {
            return Ok(Arc::clone(transport));
        }
        ReqwestHttpTransport::for_provider_operation(crate::hosted_private_network_allowed(
            false, env,
        ))
        .map(|transport| Arc::new(transport) as Transport)
        .map_err(|error| Failure::Rejected {
            status: 0,
            code: "transport_unavailable".to_owned(),
            reason: error.to_string(),
            release: true,
        })
    }
}

/// The effect that owns every operation step.
#[derive(Default)]
pub struct OperationEffect {
    placements: Arc<Placements>,
    /// The harness's provider fixture stands in for the hosted implementation
    /// of a declared operation that has none, so a skill's fixtures run while
    /// readiness still reports the operation unavailable.
    fixture_stand_in: bool,
}

impl std::fmt::Debug for OperationEffect {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperationEffect")
            .field(
                "transport",
                &if self.placements.http.is_some() {
                    "injected"
                } else {
                    "runtime-owned"
                },
            )
            .finish()
    }
}

impl OperationEffect {
    /// Inject the transport beneath both placements. Admission, custody,
    /// sealing and the request shapes stay the production path; only the
    /// wire is replaced, for the harness and embedders.
    pub fn with_http_transport<T>(transport: T) -> Self
    where
        T: RuntimeHttpTransport + Send + Sync + 'static,
    {
        let transport: Transport = Arc::new(transport);
        Self {
            placements: Arc::new(Placements {
                hosted: HostedClient::with_transport(Arc::clone(&transport)),
                http: Some(transport),
                #[cfg(feature = "mcp")]
                mcp: crate::drivers::mcp::Sessions::default(),
                principals: std::sync::Mutex::default(),
            }),
            fixture_stand_in: false,
        }
    }

    /// The harness's effect: its provider fixture answers hosted calls and
    /// stands in for declared operations no placement implements yet, and
    /// its MCP calls answer in place of every MCP server.
    pub(crate) fn harness<T>(transport: T, mcp_calls: &[runx_parser::HarnessMcpCallFixture]) -> Self
    where
        T: RuntimeHttpTransport + Send + Sync + 'static,
    {
        let mut effect = Self::with_http_transport(transport);
        effect.fixture_stand_in = true;
        #[cfg(feature = "mcp")]
        if !mcp_calls.is_empty()
            && let Some(placements) = Arc::get_mut(&mut effect.placements)
        {
            placements.mcp = crate::drivers::mcp::Sessions::standing_in(
                mcp_calls
                    .iter()
                    .map(|call| crate::drivers::mcp::StandInCall {
                        tool: call.tool.clone(),
                        arguments: JsonValue::Object(call.arguments.clone()),
                        headers: call.headers.clone(),
                        result: call.result.clone(),
                    })
                    .collect(),
            );
        }
        #[cfg(not(feature = "mcp"))]
        let _ = mcp_calls;
        effect
    }
}

/// What a driver may know of an admitted call. Only the kernel builds one,
/// from an admission, so a driver performs nothing admission did not prove.
pub(crate) struct AdmittedCall<'a> {
    pub connector: &'a str,
    pub operation: &'a str,
    /// The implementation kind, which names the readback reference.
    pub implementation: &'static str,
    pub target: &'a str,
    pub arguments: &'a JsonValue,
    pub parameters: &'a JsonObject,
    pub idempotency_key: Option<&'a str>,
    pub credential: Option<&'a credentials::CredentialSecret>,
    pub env: &'a BTreeMap<String, String>,
    access: OperationAccess,
    _sealed: Sealed,
}

/// Only this module makes one, so only the kernel builds an [`AdmittedCall`].
struct Sealed;

#[cfg(test)]
impl<'a> AdmittedCall<'a> {
    /// A call as if admitted, for driver tests.
    pub(crate) fn for_tests(
        arguments: &'a JsonValue,
        parameters: &'a JsonObject,
        env: &'a BTreeMap<String, String>,
        access: OperationAccess,
    ) -> Self {
        Self {
            connector: "example",
            operation: "items.update",
            implementation: "http",
            target: "example://items",
            arguments,
            parameters,
            idempotency_key: Some("key-1"),
            credential: None,
            env,
            access,
            _sealed: Sealed,
        }
    }
}

impl AdmittedCall<'_> {
    pub(crate) fn mutation(&self) -> bool {
        self.access == OperationAccess::Mutate
    }
}

/// Runs the tool package a `process` implementation names, from the
/// installed copy of the manifest its connector ships, through the runtime's
/// adapters. The step handler provides it; the kernel decides which tool and
/// everything around the run.
pub(crate) trait ToolRunner {
    fn run(
        &self,
        tool: &str,
        manifest: &Path,
        inputs: JsonObject,
        scopes: &[String],
        delivery: &crate::credentials::CredentialDelivery,
    ) -> Result<JsonValue, String>;
}

/// The runner of a runtime built without tool packages: it runs none.
#[cfg(test)]
pub(crate) struct NoTools;

#[cfg(test)]
impl ToolRunner for NoTools {
    fn run(
        &self,
        tool: &str,
        _manifest: &Path,
        _inputs: JsonObject,
        _scopes: &[String],
        _delivery: &crate::credentials::CredentialDelivery,
    ) -> Result<JsonValue, String> {
        Err(format!(
            "tool {tool} needs a runtime built with tool packages"
        ))
    }
}

/// An admitted operation step, carried from admission to sealing.
#[derive(Clone)]
pub(crate) struct OperationAdmission {
    placements: Arc<Placements>,
    admitted: Admitted,
    call: OperationCall,
    shape: ResultShape,
    /// The resource the caller's act targets, classified at admission by the
    /// connectors the run can reach.
    target_ref: Option<Reference>,
    duplicates: Option<DuplicatePolicy>,
    /// What the host supplied for approval lanes, bound at admission.
    lanes: LaneInputs,
    /// Set when the step is prepared for dispatch.
    attempt: Option<AttemptPlan>,
}

#[derive(Clone, Debug, Default)]
struct LaneInputs {
    /// The paid job the run's authority names, if any.
    paid_job: Option<ExternalJobStageRequest>,
    run_id: String,
}

impl LaneInputs {
    fn of(authority: Option<&crate::RunAuthority>, run_id: &str) -> Self {
        Self {
            paid_job: authority.and_then(|authority| authority.paid_job.as_deref().cloned()),
            run_id: run_id.to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
struct AttemptPlan {
    number: u32,
    approval: Option<ApprovalRecord>,
    evidence_refs: Vec<Reference>,
}

impl std::fmt::Debug for OperationAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperationAdmission")
            .field("intent_digest", self.admitted.intent_digest())
            .field("instance", &self.admitted.instance.instance_id)
            .finish_non_exhaustive()
    }
}

impl OperationAdmission {
    /// The step's skill label in its run record.
    pub(crate) fn skill_label(&self) -> String {
        format!(
            "operation:{}/{}",
            self.admitted.connector.manifest.id, self.admitted.operation
        )
    }

    fn mutation(&self) -> bool {
        self.admitted.operation().access() == OperationAccess::Mutate
    }

    /// What the call's receipt records: the act its caller stated, the
    /// operation's declared description, the execution term it was admitted
    /// under, what it produced, and the context the run was prepared with.
    pub(crate) fn sealed_call(
        &self,
        effects: Vec<Reference>,
        context_ref: Option<Reference>,
    ) -> Result<crate::receipts::SealedCall, RuntimeError> {
        let term = self
            .admitted
            .admission
            .execution_term
            .as_ref()
            .ok_or_else(|| engine("the operation has no execution term"))?;
        Ok(crate::receipts::SealedCall {
            operation_intent: Some(self.admitted.intent().clone()),
            act: self.call.act.clone(),
            description: self.admitted.operation().description.clone(),
            legitimacy: runx_contracts::TermLegitimacy::for_term(term.term_id.as_str())
                .ok_or_else(|| engine("the execution term id cannot name a legitimacy"))?,
            target_ref: self.target_ref.clone(),
            effects,
            context_ref,
        })
    }

    /// The key the provider sees, stable for the intent.
    fn provider_key(&self) -> Option<String> {
        self.mutation().then(|| {
            idempotency_key(
                self.admitted.intent_digest().as_str(),
                self.admitted.operation().idempotency_key_format,
            )
        })
    }

    /// Whether a request that may have reached the provider can be sent
    /// again: the provider enforces the key, the operation sets a state, or
    /// the step accepts duplicates.
    fn resend_safe(&self) -> bool {
        matches!(
            self.admitted.operation().guarantees.provider_idempotency,
            ProviderIdempotency::Enforced | ProviderIdempotency::Inherent
        ) || self.duplicates == Some(DuplicatePolicy::Visible)
    }

    /// The implementation that runs: any kind locally; only declared
    /// requests on the hosted placement, whose private drivers are opaque.
    fn implementation_digest(&self) -> Option<String> {
        let operation = self.admitted.operation();
        let implementation = match self.admitted.instance.placement {
            Placement::Local => operation.implementation.as_ref()?,
            Placement::Hosted => operation
                .hosted_http()
                .and(operation.implementation.as_ref())?,
        };
        let server = self
            .admitted
            .connector
            .manifest
            .placements
            .local
            .as_ref()
            .and_then(|local| local.mcp_server.as_ref());
        let tool = match implementation {
            Implementation::Process(process) => self
                .admitted
                .connector
                .tools
                .get(process.tool.as_str())
                .map(|tool| &tool.digest),
            _ => None,
        };
        implementation_digest(implementation, server, tool)
            .ok()
            .map(|digest| digest.as_str().to_owned())
    }

    /// The installed copy of the tool a local process operation runs: the
    /// one its connector ships, at the digest it was loaded with.
    fn process_tool(
        &self,
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Option<PathBuf>, String> {
        let operation = self.admitted.operation();
        let (Placement::Local, Some(Implementation::Process(process))) = (
            self.admitted.instance.placement,
            operation.implementation.as_ref(),
        ) else {
            return Ok(None);
        };
        let connector = &self.admitted.connector;
        connector
            .tools
            .get(process.tool.as_str())
            .ok_or_else(|| {
                format!(
                    "connector {} ships no tool {}",
                    connector.manifest.id, process.tool
                )
            })?
            .installed(
                &crate::paths::RunxPaths::resolve(env, cwd),
                process.tool.as_str(),
            )
            .map(Some)
    }
}

impl RuntimeEffect for OperationEffect {
    fn family(&self) -> &'static str {
        OPERATION_EFFECT_FAMILY
    }

    fn matches_target(&self, request: EffectStepRequest<'_>) -> bool {
        request.step.operation().is_some()
    }

    fn admit(
        &self,
        request: EffectStepRequest<'_>,
    ) -> Result<Option<EffectAdmission>, RuntimeEffectError> {
        let Some(target) = request.step.operation() else {
            return Ok(None);
        };
        let call = OperationCall::decode(request.inputs).map_err(invalid)?;
        let shape = call.shape().map_err(invalid)?;
        let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(
            request.env,
            request.graph_dir,
        ))
        .map_err(state)?;
        let lanes = LaneInputs::of(request.run_authority, request.run_id);
        let scope = ConnectorScope::at(request.graph_dir)
            .map_err(|error| invalid(error.to_string()))?
            .in_project(
                crate::paths::RunxPaths::resolve(request.env, request.graph_dir).connectors(),
            );
        let instances = Instances::new(Arc::clone(&store));
        let admitted = admission::admit(&Evaluation {
            step: target,
            call: &call,
            duplicates: request.step.duplicates,
            fixture_stand_in: self.fixture_stand_in,
            run_authority: request.run_authority,
            store: &store,
            run_id: &lanes.run_id,
            context: Context {
                scope: &scope,
                instances: &instances,
                hosted: &self.placements.hosted,
                env: request.env,
                cwd: request.graph_dir,
            },
        })
        .map_err(|error| match error {
            AdmitError::Refused(refusal) => RuntimeEffectError::Denied {
                family: OPERATION_EFFECT_FAMILY.to_owned(),
                message: refusal.message(),
                // A requirement that names no operation has no verb of its
                // own; it is refused as the write it might have been.
                verb: refusal.verb.unwrap_or(AuthorityVerb::Write),
        let target_ref = match call.act.as_ref().and_then(|act| act.target.as_ref()) {
            Some(target) => {
                let reachable = scope
                    .reachable()
                    .map_err(|error| invalid(error.to_string()))?;
                Some(Reference::with_uri(
                    super::connectors::references::bound_reference_kind(
                        &admitted.connector.manifest,
                        &reachable,
                        target.as_str(),
                    ),
                    target.as_str().to_owned(),
                ))
            }
            None => None,
        };
            },
            AdmitError::State(message) => state(message),
        })?;
        let verb = admitted.operation().verb.authority_verb();
        let context = OperationAdmission {
            placements: Arc::clone(&self.placements),
            target_ref,
            admitted,
            call,
            shape,
            duplicates: request.step.duplicates,
            lanes,
            attempt: None,
        };
        let admitted = &context.admitted;
        let intent = admitted.intent();
        let witness = AuthorityAdmissionWitness {
            verb: verb.clone(),
            parent_term_id: admitted
                .delegations
                .last()
                .map_or(&admitted.capability.term, |term| term)
                .term_id
                .as_str()
                .to_owned(),
            child_term_id: format!("intent:{}", admitted.intent_digest()),
            idempotency_key: admitted.request_key.clone(),
            capability_ref: Some(Reference::with_uri(
                ReferenceType::ConnectorOperation,
                format!(
                    "runx:connector_operation:{}/{}@{}",
                    intent.connector, intent.operation, intent.operation_digest
                ),
            )),
        };
        Ok(Some(EffectAdmission::new(
            OPERATION_EFFECT_FAMILY,
            verb,
            witness,
            context,
        )))
    }

    fn prepare_execution(
        &self,
        step: &GraphStep,
        admission: EffectAdmission,
        host: &mut dyn crate::Host,
    ) -> Result<EffectPreparationOutcome, RuntimeEffectError> {
        let mut context = admission
            .context::<OperationAdmission>()
            .cloned()
            .ok_or_else(|| state("the operation admission is missing"))?;
        let mut plan = AttemptPlan {
            number: context
                .admitted
                .pending
                .as_ref()
                .map_or(1, |pending| pending.attempt + 1),
            approval: None,
            evidence_refs: Vec::new(),
        };
        if context.admitted.admission.disposition == AdmissionDisposition::NeedsResolution {
            match approve(&context, step, host)? {
                Lane::Approved(approved) => {
                    context
                        .admitted
                        .admit_with(vec![approved.evidence])
                        .map_err(denied)?;
                    plan.approval = Some(approved.record);
                    plan.evidence_refs = approved.refs;
                }
                Lane::Pending { reason, request } => {
                    return Ok(EffectPreparationOutcome::Pending { reason, request });
                }
            }
        }
        context.attempt = Some(plan);
        Ok(EffectPreparationOutcome::Ready(Box::new(
            admission.with_context(context),
        )))
    }

    fn prepare_output(&self, request: EffectOutputRequest<'_>) -> Result<(), RuntimeEffectError> {
        if !request.output.succeeded() {
            return Ok(());
        }
        let context = request
            .admission
            .context::<OperationAdmission>()
            .ok_or_else(|| state("the operation admission is missing"))?;
        let packet = request
            .claim
            .get("provider_operation")
            .and_then(crate::output_contract::artifact_data)
            .and_then(JsonValue::as_object)
            .ok_or_else(|| state("the provider_operation packet is missing"))?;
        let field = |name: &str| packet.get(name).and_then(JsonValue::as_str);
        let intent_digest = context.admitted.intent_digest().as_str();
        if field("finality") != Some(context.admitted.operation().finality())
            || field("plan_digest") != Some(intent_digest)
            || field("idempotency_key").map(str::to_owned) != context.provider_key()
        {
            return Err(state("the packet does not match the admitted intent"));
        }
        let provider = context.admitted.connector.manifest.id.as_str();
        let mut refs = vec![verification(
            format!("runx:operation_intent:{intent_digest}"),
            provider,
            ProofKind::EffectEvidence,
        )];
        if let Some(plan) = &context.attempt {
            if let Some(record) = &plan.approval {
                refs.push(verification(
                    record.uri(),
                    provider,
                    ProofKind::EffectEvidence,
                ));
            }
            refs.extend(plan.evidence_refs.iter().cloned());
        }
        for cover in &context.admitted.usage_covers {
            refs.push(verification(
                cover.receipt_ref(),
                provider,
                ProofKind::EffectEvidence,
            ));
        }
        if context.mutation() {
            let operation_id =
                field("operation_id").ok_or_else(|| state("the packet has no operation id"))?;
            refs.push(verification(
                format!("runx:provider_ack:{operation_id}"),
                provider,
                ProofKind::EffectEvidence,
            ));
        }
        // A write the provider cannot read back is acknowledged: its
        // evidence never stands as the effect's finality.
        let readback_ref =
            field("readback_ref").ok_or_else(|| state("the packet has no readback"))?;
        refs.push(verification(
            readback_ref.to_owned(),
            provider,
            if context.admitted.operation().finality() == "acknowledged" {
                ProofKind::EffectEvidence
            } else {
                ProofKind::EffectFinality
            },
        ));
        request
            .output
            .observations
            .effect_verification_refs
            .extend(refs);
        Ok(())
    }

    fn persist(
        &self,
        request: EffectReceiptRequest<'_>,
    ) -> Result<Option<super::custody::Settlement>, RuntimeEffectError> {
        if !request.output.succeeded() {
            return Ok(None);
        }
        let context = request
            .admission
            .context::<OperationAdmission>()
            .ok_or_else(|| state("the operation admission is missing"))?;
        let Some(custody) = &context.admitted.custody else {
            return Ok(None);
        };
        crate::receipts::store::LocalReceiptStore::new(receipt_root(
            request.env,
            request.graph_dir,
        ))
        .write_receipt(request.receipt, request.signature_policy)
        .map_err(|error| state(error.to_string()))?;
        Ok(Some(custody.settlement(
            request.receipt.id.as_str().to_owned(),
            &context.admitted.usage_covers,
        )))
    }

    fn authority(
        &self,
        admission: &EffectAdmission,
    ) -> Result<crate::receipts::SealAuthority, RuntimeEffectError> {
        let context = admission
            .context::<OperationAdmission>()
            .ok_or_else(|| state("the operation admission is missing"))?;
        let admitted = &context.admitted;
        Ok(crate::receipts::SealAuthority {
            actor_ref: Some(admitted.actor_ref.clone()),
            mandate_ref: admitted.mandate_ref.clone(),
            grant_refs: vec![admitted.capability.term.issued_by_ref.clone()],
            scope_refs: admitted
                .operation()
                .scopes
                .iter()
                .map(|scope| {
                    Reference::with_uri(
                        ReferenceType::ScopeAdmission,
                        format!("runx:scope_admission:{scope}"),
                    )
                })
                .collect(),
            terms: admitted.admission.execution_term.iter().cloned().collect(),
            derivations: admitted.admission.derivation.iter().cloned().collect(),
        })
    }
}

/// An operation step emits one artifact: its `runx.provider.operation.v1`
/// packet, as `<step>.provider_operation.data`.
pub(crate) fn packet_artifacts() -> runx_parser::SkillArtifactContract {
    runx_parser::SkillArtifactContract {
        emits: None,
        named_emits: Some(BTreeMap::from([(
            "provider_operation".to_owned(),
            "provider_operation".to_owned(),
        )])),
        packets: Some(BTreeMap::from([(
            "provider_operation".to_owned(),
            "runx.provider.operation.v1".to_owned(),
        )])),
        wrap_as: None,
        packet: None,
    }
}

/// The step's output: the packet, the caller-only overlay, and what the call
/// produced.
#[derive(Debug)]
pub(crate) struct OperationOutcome {
    pub value: JsonValue,
    pub ephemeral: Option<JsonValue>,
    pub effects: Vec<Reference>,
}

/// What the kernel handed an implementation it runs itself when it invoked
/// it: where the call ran, and the credential an MCP server it started
/// received. Recorded at invocation, so a call that then fails seals it too.
/// A process tool is not here: the process layer observes its own run.
#[derive(Debug)]
pub(crate) struct Dispatched {
    pub boundary: ExecutionBoundaryKind,
    pub credential: Option<runx_contracts::CredentialDeliveryObservation>,
}

impl Dispatched {
    /// The invocation of an implementation that received no credential.
    fn of(context: &OperationAdmission) -> Self {
        Self {
            boundary: placement(context).1,
            credential: None,
        }
    }
}

/// How a placement failed.
#[derive(Debug)]
pub(crate) enum Failure {
    /// A definitive response or local refusal. `release` records whether the
    /// implementation proved this attempt did not execute.
    Rejected {
        status: u16,
        code: String,
        reason: String,
        /// The implementation proves this attempt did not execute.
        release: bool,
    },
    /// The call may have executed.
    Unknown(String),
    /// The write was acknowledged and its readback did not complete.
    ReadbackPending(String),
}

/// Execute a prepared operation step. `dispatched` records the invocation
/// once the implementation is reached, whatever the call then returns.
pub(crate) fn execute(
    context: &OperationAdmission,
    step_id: &str,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    tools: &dyn ToolRunner,
    dispatched: &mut Option<Dispatched>,
) -> Result<OperationOutcome, RuntimeError> {
    let admitted = &context.admitted;
    let plan = context
        .attempt
        .as_ref()
        .ok_or_else(|| engine("the operation step was not prepared"))?;
    let intent_digest = admitted.intent_digest().as_str().to_owned();
    let provider_key = context.provider_key();
    let unknown = |reason: String| RuntimeError::ProviderEffectUnknown {
        plan_digest: intent_digest.clone(),
        idempotency_key: provider_key.clone().unwrap_or_default(),
        reason,
    };
    let implementation_digest = context.implementation_digest();
    let instance_binding = admitted
        .instance
        .binding_digest()
        .map_err(|error| engine(format!("digesting the instance binding: {error}")))?;
    let authority_term_id = admitted
        .admission
        .execution_term
        .as_ref()
        .ok_or_else(|| engine("the operation has no execution term"))?
        .term_id
        .as_str();

    if let Some(pending) = &context.admitted.pending
        && (pending.authority_term_id != authority_term_id
            || pending.mandate_ref != admitted.mandate_ref)
    {
        return Err(unknown(
            "the authority or mandate changed since the unsettled attempt; reconcile it before resuming"
                .to_owned(),
        ));
    }

    if let Some(pending) = &context.admitted.pending {
        if pending.instance_binding != instance_binding.as_str() {
            return Err(unknown(
                "the instance was confirmed again with another account or credential since the unsettled attempt; reconcile it before resuming".to_owned(),
            ));
        }
        if pending.implementation_digest != implementation_digest {
            return Err(unknown(
                "the implementation changed since the unsettled attempt; reconcile it before resuming".to_owned(),
            ));
        }
        // Readback is provider evidence, not permission to settle under a
        // different implementation or authority than the attempted call.
        if let Some(readback) = pending.readback.clone() {
            let produced = effects(context, &readback.result).map_err(unknown)?;
            return finish(
                context,
                readback,
                implementation_digest.as_deref(),
                produced,
            );
        }
        if pending.acknowledgement.is_none() && !context.resend_safe() {
            return Err(unknown(
                "an earlier attempt has an unknown outcome and this operation cannot be resent safely; reconcile the provider result before resuming".to_owned(),
            ));
        }
    }

    // Everything that can fail before the provider is reached runs before
    // the attempt is recorded, so a refusal here uses nothing.
    let preflight = |reason: String| match context.mutation() {
        true => RuntimeError::ProviderEffectRejected {
            plan_digest: intent_digest.clone(),
            idempotency_key: provider_key.clone().unwrap_or_default(),
            provider_code: "preflight_failed".to_owned(),
            http_status: 0,
            reason,
        },
        false => failed(context, reason),
    };
    let session = prepare(context, env, cwd).map_err(preflight)?;
    let tool = context.process_tool(env, cwd).map_err(preflight)?;
    if let Some(custody) = &context.admitted.custody {
        custody
            .attempt(
                &Attempt {
                    intent_digest: &intent_digest,
                    authority_term_id,
                    mandate_ref: admitted.mandate_ref.as_ref(),
                    idempotency_key: provider_key.as_deref().unwrap_or_default(),
                    instance_binding: instance_binding.as_str(),
                    attempt: plan.number,
                    approval: plan.approval.as_ref(),
                    implementation_digest: implementation_digest.as_deref(),
                    admitted_at: admitted.custody_head,
                },
                &admitted.usage_covers,
            )
            .map_err(|error| {
                RuntimeError::effect_state("recording the attempt", error.to_string())
            })?;
    }

    // An answer the connector cannot describe as it declares, its effect
    // included, is not trusted: the call holds for reconciliation.
    let outcome = dispatch(
        context,
        session,
        env,
        Local {
            tools,
            tool: tool.as_deref(),
        },
        dispatched,
    )
    .and_then(|readback| {
        readback
            .verify(&Expected {
                connector: admitted.connector.manifest.id.as_str(),
                operation: admitted.operation.as_str(),
                target: &admitted.target,
                idempotency_key: provider_key.as_deref(),
            })
            .map_err(Failure::Unknown)?;
        let produced = effects(context, &readback.result).map_err(Failure::Unknown)?;
        Ok((readback, produced))
    });
    match outcome {
        Ok((readback, produced)) => {
            if let Some(custody) = &context.admitted.custody {
                custody.read_back(readback.clone()).map_err(|error| {
                    RuntimeError::effect_state("recording the readback", error.to_string())
                })?;
            }
            finish(
                context,
                readback,
                implementation_digest.as_deref(),
                produced,
            )
        }
        Err(Failure::Rejected {
            status,
            code,
            reason,
            release,
        }) => {
            if let Some(custody) = &context.admitted.custody {
                if context.admitted.pending.is_some() {
                    let held =
                        "a prior attempt may have executed; a later refusal cannot settle it";
                    custody.unknown(held.to_owned()).map_err(|error| {
                        RuntimeError::effect_state("holding the earlier attempt", error.to_string())
                    })?;
                    return Err(unknown(held.to_owned()));
                }
                custody
                    .rejected(code.clone(), &admitted.usage_covers, release)
                    .map_err(|error| {
                        RuntimeError::effect_state("recording the rejection", error.to_string())
                    })?;
                return Err(RuntimeError::ProviderEffectRejected {
                    plan_digest: intent_digest,
                    idempotency_key: provider_key.unwrap_or_default(),
                    provider_code: code,
                    http_status: status,
                    reason,
                });
            }
            Err(read_failure(context, step_id, reason))
        }
        Err(Failure::Unknown(reason) | Failure::ReadbackPending(reason))
            if context.admitted.custody.is_some() =>
        {
            if let Some(custody) = &context.admitted.custody {
                custody.unknown(reason.clone()).map_err(|error| {
                    RuntimeError::effect_state("recording the unknown outcome", error.to_string())
                })?;
            }
            Err(unknown(reason))
        }
        Err(Failure::Unknown(reason) | Failure::ReadbackPending(reason)) => {
            Err(read_failure(context, step_id, reason))
        }
    }
}

/// Session material resolved before anything is sent.
enum Session {
    Hosted(hosted::HostedSession),
    Local(Option<credentials::CredentialSecret>),
    /// The runx account the runtime is signed in as, for an implementation
    /// that writes that account's own storage.
    Account(hosted::HostedSession),
}

fn prepare(
    context: &OperationAdmission,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Session, String> {
    let admitted = &context.admitted;
    let instance = &admitted.instance;
    match instance.placement {
        Placement::Hosted => {
            let session = context.placements.hosted.session(env, cwd)?;
            if session.principal_ref != instance.principal_ref.as_str() {
                return Err(
                    "the authenticated principal is not the instance's principal".to_owned(),
                );
            }
            Ok(Session::Hosted(session))
        }
        Placement::Local => match &instance.authority {
            InstanceAuthority::Credential {
                profile,
                credential,
                material_ref,
                ..
            } => credentials::deliver(
                env,
                cwd,
                &admitted.connector,
                &credentials::ProfileBinding {
                    profile,
                    credential,
                    material_ref,
                },
            )
            .map(|secret| Session::Local(Some(secret))),
            InstanceAuthority::Session {
                session: runx_contracts::connector::SessionKind::Command,
                ..
            } => signed_in(context, env, cwd).map(|secret| Session::Local(Some(secret))),
            _ => match admitted.operation().implementation {
                Some(Implementation::Artifact(_)) => context
                    .placements
                    .hosted
                    .session(env, cwd)
                    .map(Session::Account),
                _ => Ok(Session::Local(None)),
            },
        },
    }
}

/// The command session's token, read now and still held by the principal
/// the instance pinned, as the credential it stands in for.
fn signed_in(
    context: &OperationAdmission,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<credentials::CredentialSecret, String> {
    let manifest = &context.admitted.connector.manifest;
    let instance = &context.admitted.instance;
    let session = super::session::CommandSession::of(manifest)
        .ok_or("the connector no longer declares a command session")?;
    let token = session.token(env, cwd)?;
    let digest = runx_contracts::sha256_prefixed(token.as_bytes());
    let cached = context
        .placements
        .principals
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&digest)
        .cloned();
    let (principal_ref, login) = match cached {
        Some(principal_ref) => (principal_ref, None),
        None => {
            let transport =
                context
                    .placements
                    .declared_hosts(env)
                    .map_err(|failure| match failure {
                        Failure::Rejected { reason, .. }
                        | Failure::Unknown(reason)
                        | Failure::ReadbackPending(reason) => reason,
                    })?;
            let principal = session.principal(&token, transport.as_ref())?;
            let principal_ref = principal.principal_ref(manifest.id.as_str());
            context
                .placements
                .principals
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(digest, principal_ref.clone());
            (principal_ref, Some(principal.login))
        }
    };
    if principal_ref != instance.principal_ref.as_str() {
        return Err(format!(
            "{} is signed in as {}, not the account instance {} is pinned to; confirm it again with `runx connector use {} --session`",
            manifest.id,
            login.as_deref().unwrap_or("another account"),
            instance.instance_id,
            context.admitted.connector.connector_ref
        ));
    }
    let field = manifest
        .auth
        .credentials
        .get(session.credential)
        .and_then(|declaration| declaration.fields.first())
        .ok_or("the session's credential is no longer declared")?;
    Ok(credentials::CredentialSecret::new(
        session.credential.clone(),
        BTreeMap::from([(field.name.as_str().to_owned(), token)]),
    ))
}

/// What a local process implementation runs with: the step's runner and the
/// installed copy of the connector's tool.
struct Local<'a> {
    tools: &'a dyn ToolRunner,
    tool: Option<&'a Path>,
}

fn dispatch(
    context: &OperationAdmission,
    session: Session,
    env: &BTreeMap<String, String>,
    local: Local<'_>,
    dispatched: &mut Option<Dispatched>,
) -> Result<Readback, Failure> {
    let admitted = &context.admitted;
    let operation = admitted.operation();
    let provider_key = context.provider_key();
    let (hosted, secret, account) = match session {
        Session::Hosted(session) => (Some(session), None, None),
        Session::Local(secret) => (None, secret, None),
        Session::Account(session) => (None, None, Some(session)),
    };
    let implementation = match (&hosted, operation.hosted_http()) {
        (Some(session), None) => {
            *dispatched = Some(Dispatched::of(context));
            return private_driver(context, session, provider_key);
        }
        _ => operation
            .implementation
            .as_ref()
            .ok_or_else(|| Failure::Rejected {
                status: 0,
                code: "operation_unsupported".to_owned(),
                reason: "the local placement does not implement this operation".to_owned(),
                release: true,
            })?,
    };
    let arguments = JsonValue::Object(context.call.input.clone());
    let call = AdmittedCall {
        connector: admitted.connector.manifest.id.as_str(),
        operation: admitted.operation.as_str(),
        implementation: implementation.kind(),
        target: &admitted.target,
        arguments: &arguments,
        parameters: &admitted.instance.parameters,
        idempotency_key: provider_key.as_deref(),
        credential: secret.as_ref(),
        env,
        access: operation.access(),
        _sealed: Sealed,
    };
    let recovery = confirm::Recovery {
        acknowledged: context
            .admitted
            .pending
            .as_ref()
            .and_then(|pending| pending.acknowledgement.clone()),
        resend_safe: context.resend_safe(),
    };
    let mut record = |acknowledgement: &Acknowledgement| match &context.admitted.custody {
        Some(custody) => custody
            .acknowledged(acknowledgement.clone())
            .map_err(|error| error.to_string()),
        None => Ok(()),
    };
    let outcome = match implementation {
        Implementation::Http(spec) => {
            let (direct, gateway, implementation_digest, connector_binding);
            let egress = match &hosted {
                Some(session) => {
                    implementation_digest = context.implementation_digest().ok_or_else(|| {
                        rejected(
                            "operation_unsupported",
                            "the declared requests have no digest".to_owned(),
                        )
                    })?;
                    connector_binding = admitted.connector.binding();
                    gateway = hosted::Gateway {
                        connector_binding: &connector_binding,
                        session,
                        grant_id: hosted_grant(admitted)?,
                        operation: admitted.operation.as_str(),
                        operation_digest: admitted.intent().operation_digest.as_str(),
                        implementation_digest: &implementation_digest,
                        input: &context.call.input,
                        idempotency_key: provider_key.as_deref(),
                    };
                    Egress::Connect(&gateway)
                }
                None => {
                    direct = context.placements.declared_hosts(env)?;
                    Egress::Direct(direct.as_ref())
                }
            };
            let matches = |readback: &runx_contracts::connector::HttpReadback| {
                let mut matches = readback.matches.clone();
                matches.extend(crate::drivers::http::status_match(readback));
                matches
            };
            let checks = Checks {
                preconditions: spec.preconditions.iter().map(matches).collect(),
                readbacks: match call.mutation() {
                    true => spec.readbacks.iter().map(matches).collect(),
                    false => Vec::new(),
                },
            };
            let driver = crate::drivers::http::Http::new(egress, &call, spec);
            *dispatched = Some(Dispatched::of(context));
            confirm::run(&driver, &call, &checks, recovery, &mut record)
        }
        Implementation::Web(_) => {
            // The URL comes from the call, so the transport never
            // reaches a private network, whatever the operator allows
            // for declared hosts.
            let live;
            let transport: &dyn RuntimeHttpTransport = match context.placements.http.as_ref() {
                Some(transport) => transport.as_ref(),
                None => {
                    live = ReqwestHttpTransport::new().map_err(|error| Failure::Rejected {
                        status: 0,
                        code: "transport_unavailable".to_owned(),
                        reason: error.to_string(),
                        release: true,
                    })?;
                    &live
                }
            };
            let driver = crate::drivers::web::Web {
                transport,
                call: &call,
            };
            *dispatched = Some(Dispatched::of(context));
            confirm::run(&driver, &call, &Checks::default(), recovery, &mut record)
        }
        Implementation::Mcp(spec) => mcp(context, &call, spec, recovery, &mut record, dispatched),
        Implementation::Store(_) => {
            let checks = Checks {
                preconditions: Vec::new(),
                readbacks: match call.mutation() {
                    true => vec![crate::drivers::store::readback_matches()],
                    false => Vec::new(),
                },
            };
            let driver = crate::drivers::store::Store { call: &call };
            *dispatched = Some(Dispatched::of(context));
            confirm::run(&driver, &call, &checks, recovery, &mut record)
        }
        Implementation::Artifact(_) => {
            let session = account.as_ref().ok_or_else(|| {
                rejected(
                    "account_unavailable",
                    "an artifact is stored for the runx account the runtime is signed in as"
                        .to_owned(),
                )
            })?;
            let checks = Checks {
                preconditions: Vec::new(),
                readbacks: match call.mutation() {
                    true => vec![crate::drivers::artifact::readback_matches()],
                    false => Vec::new(),
                },
            };
            let driver = crate::drivers::artifact::Artifact {
                session,
                call: &call,
            };
            *dispatched = Some(Dispatched::of(context));
            confirm::run(&driver, &call, &checks, recovery, &mut record)
        }
        Implementation::Process(spec) => {
            let delivery = process_delivery(context, call.credential)?;
            let scopes = spec
                .runtime_scopes
                .iter()
                .map(|scope| scope.as_str().to_owned())
                .collect::<Vec<_>>();
            let checks = Checks {
                preconditions: Vec::new(),
                readbacks: match (call.mutation(), &spec.readback) {
                    (true, Some(readback)) => vec![readback.matches.as_slice().to_vec()],
                    _ => Vec::new(),
                },
            };
            let tool = local.tool.ok_or_else(|| {
                rejected(
                    "tool_unavailable",
                    "the connector's tool was not installed".to_owned(),
                )
            })?;
            let driver = crate::drivers::process::Process {
                tools: local.tools,
                tool: spec.tool.as_str(),
                manifest: tool,
                call: &call,
                scopes: &scopes,
                delivery: &delivery,
            };
            confirm::run(&driver, &call, &checks, recovery, &mut record)
        }
    };
    outcome.map_err(|mut failure| {
        if let Some(secret) = &secret {
            let reason = match &mut failure {
                Failure::Rejected { reason, .. }
                | Failure::Unknown(reason)
                | Failure::ReadbackPending(reason) => reason,
            };
            *reason = secret.scrub_text(std::mem::take(reason));
        }
        failure
    })
}

/// The grant a hosted instance holds.
fn hosted_grant(admitted: &Admitted) -> Result<&str, Failure> {
    match &admitted.instance.authority {
        InstanceAuthority::HostedGrant { grant_id } => Ok(grant_id.as_str()),
        _ => Err(rejected(
            "binding_unavailable",
            "a hosted instance must hold a grant".to_owned(),
        )),
    }
}

/// A hosted operation without declared requests, run by Connect's private
/// driver: the kernel sends the admitted call and records what it answers.
fn private_driver(
    context: &OperationAdmission,
    session: &hosted::HostedSession,
    provider_key: Option<String>,
) -> Result<Readback, Failure> {
    let admitted = &context.admitted;
    let operation = admitted.operation();
    let mut input = context.call.input.clone();
    if let Some(key) = provider_key {
        input.insert("idempotency_key".to_owned(), JsonValue::String(key));
    }
    HostedClient::call(
        session,
        &ProviderOperationRequest {
            connector_binding: admitted.connector.binding(),
            grant_id: hosted_grant(admitted)?.to_owned(),
            operation: admitted.operation.as_str().to_owned(),
            operation_digest: admitted.intent().operation_digest.as_str().to_owned(),
            target: admitted.target.clone(),
            scopes: operation
                .scopes
                .iter()
                .map(|scope| scope.as_str().to_owned())
                .collect(),
            input,
            access: operation.access(),
        },
    )
}

/// The credential an MCP server the runtime starts receives, as its receipt
/// records the delivery: the instance's declared credential, never the
/// secret. None for any other implementation, an instance that delivers no
/// credential, or a hosted operation.
#[cfg(feature = "mcp")]
fn stdio_server_delivery(
    context: &OperationAdmission,
) -> Option<runx_contracts::CredentialDeliveryObservation> {
    let admitted = &context.admitted;
    let instance = &admitted.instance;
    let manifest = &admitted.connector.manifest;
    let stdio = matches!(
        admitted.operation().implementation.as_ref()?,
        Implementation::Mcp(_)
    ) && matches!(
        manifest
            .placements
            .local
            .as_ref()
            .and_then(|local| local.mcp_server.as_ref()),
        Some(runx_contracts::connector::McpServer::Stdio { .. })
    );
    if !stdio || instance.placement != Placement::Local {
        return None;
    }
    let declaration = super::instances::delivered_credential(manifest, &instance.authority)
        .and_then(|credential| manifest.auth.credentials.get(credential))?;
    Some(crate::credentials::build_local_provision_observation(
        manifest.id.as_str(),
        declaration.kind.as_str(),
        instance.instance_id.as_str(),
    ))
}

/// The credential a process tool receives: the instance's, in the variable
/// its declaration names, redacted from everything the tool prints.
fn process_delivery(
    context: &OperationAdmission,
    secret: Option<&credentials::CredentialSecret>,
) -> Result<crate::credentials::CredentialDelivery, Failure> {
    let Some(secret) = secret else {
        return Ok(crate::credentials::CredentialDelivery::none());
    };
    let manifest = &context.admitted.connector.manifest;
    let instance = &context.admitted.instance;
    let declaration = super::instances::delivered_credential(manifest, &instance.authority)
        .and_then(|credential| manifest.auth.credentials.get(credential))
        .ok_or_else(|| {
            rejected(
                "binding_unavailable",
                "the instance delivers no declared credential".to_owned(),
            )
        })?;
    let value = match (declaration.fields.as_slice(), secret.fields.len()) {
        ([field], 1) => secret.field(field.name.as_str()).ok().map(str::to_owned),
        _ => serde_json::to_string(
            &secret
                .fields
                .iter()
                .map(|(name, value)| (name.as_str(), value.expose()))
                .collect::<BTreeMap<_, _>>(),
        )
        .ok(),
    }
    .ok_or_else(|| {
        rejected(
            "binding_unavailable",
            "the credential is incomplete".to_owned(),
        )
    })?;
    crate::credentials::CredentialDelivery::process_env(
        manifest.id.as_str(),
        declaration.kind.as_str(),
        declaration.delivery.env.as_str(),
        instance.instance_id.as_str(),
        value,
    )
    .map_err(|error| rejected("binding_unavailable", error.to_string()))
}

/// A tool call on the connector's MCP server, with every tool it touches
/// served exactly as the manifest pins it before anything is sent.
#[cfg(feature = "mcp")]
fn mcp(
    context: &OperationAdmission,
    call: &AdmittedCall<'_>,
    spec: &runx_contracts::connector::McpImplementation,
    recovery: confirm::Recovery,
    record: &mut dyn FnMut(&Acknowledgement) -> Result<(), String>,
    dispatched: &mut Option<Dispatched>,
) -> Result<Readback, Failure> {
    let unsupported = |reason: &str| Failure::Rejected {
        status: 0,
        code: "operation_unsupported".to_owned(),
        reason: reason.to_owned(),
        release: true,
    };
    let Some(server) = context
        .admitted
        .connector
        .manifest
        .placements
        .local
        .as_ref()
        .and_then(|local| local.mcp_server.as_ref())
    else {
        return Err(unsupported("the connector declares no MCP server"));
    };
    let acknowledged = context.admitted.operation().finality() == "acknowledged";
    if call.mutation() && spec.readback.is_none() && !acknowledged {
        return Err(unsupported("the write declares no readback"));
    }
    let session = match server {
        runx_contracts::connector::McpServer::StreamableHttp { url, auth, headers } => {
            if context.placements.http.is_some() && !context.placements.mcp.stands_in() {
                return Err(unsupported("an injected transport reaches no MCP server"));
            }
            let headers = mcp_headers(call, headers)?;
            let credential = if !auth.is_empty() {
                let secret = call.credential.ok_or_else(|| {
                    rejected(
                        "binding_unavailable",
                        "the MCP server authenticates but the instance delivered no credential"
                            .to_owned(),
                    )
                })?;
                let presentation = auth.get(&secret.name).ok_or_else(|| {
                    unsupported("the MCP server does not present the instance's credential")
                })?;
                if matches!(
                    presentation,
                    runx_contracts::connector::HttpAuth::Oauth1
                        | runx_contracts::connector::HttpAuth::Url { .. }
                ) {
                    return Err(unsupported(
                        "an MCP session presents its credential as a header",
                    ));
                }
                let url = url::Url::parse(url.as_str())
                    .map_err(|_| unsupported("the MCP server URL is invalid"))?;
                let header = crate::drivers::http::auth::credential_header(
                    presentation,
                    secret,
                    crate::http::HttpMethod::Post,
                    &url,
                )
                .map_err(|reason| rejected("binding_unavailable", reason))?;
                let Some(header) = header else {
                    return Err(unsupported(
                        "an MCP session presents its credential as a header",
                    ));
                };
                if headers
                    .keys()
                    .any(|name| name.eq_ignore_ascii_case(&header.name))
                {
                    return Err(unsupported(
                        "an MCP header overrides credential presentation",
                    ));
                }
                Some(header)
            } else {
                None
            };
            let transport = ReqwestHttpTransport::for_provider_operation(
                crate::hosted_private_network_allowed(false, call.env),
            )
            .map_err(|error| rejected("transport_unavailable", error.to_string()))?;
            *dispatched = Some(Dispatched::of(context));
            context
                .placements
                .mcp
                .open(&transport, url.as_str(), credential.as_ref(), &headers)
        }
        runx_contracts::connector::McpServer::Stdio { command, args } => {
            let server = stdio_server(context, call, command.as_str(), args)?;
            let opened = context
                .placements
                .mcp
                .open_stdio(&server, crate::http::PROVIDER_OPERATION_REQUEST_TIMEOUT);
            // A server the runtime started received the instance's credential,
            // whether or not its session then opened; a stand-in received none.
            let started = match &opened {
                Ok(session) => session.runs_process(),
                Err(failure) => failure.started,
            };
            *dispatched = Some(Dispatched {
                credential: started.then(|| stdio_server_delivery(context)).flatten(),
                ..Dispatched::of(context)
            });
            opened.map_err(|failure| failure.reason)
        }
    }
    .map_err(|reason| rejected("mcp_unavailable", reason))?;
    let served = session
        .served()
        .map_err(|reason| rejected("mcp_unavailable", reason))?;
    if let Some(served) = &served {
        pinned(served, spec.tool.as_str(), &spec.tool_digest)?;
        if let Some(readback) = &spec.readback {
            pinned(served, readback.tool.as_str(), &readback.tool_digest)?;
        }
    }
    let checks = Checks {
        preconditions: Vec::new(),
        readbacks: match (call.mutation(), &spec.readback) {
            (true, Some(readback)) => vec![readback.matches.clone()],
            _ => Vec::new(),
        },
    };
    let driver = crate::drivers::mcp::Mcp {
        session: &session,
        call,
        spec,
    };
    confirm::run(&driver, call, &checks, recovery, record)
}

/// The session headers the call renders: each from its template, omitted
/// when its argument is absent, and only as a single-line string.
#[cfg(feature = "mcp")]
fn mcp_headers(
    call: &AdmittedCall<'_>,
    headers: &BTreeMap<String, runx_contracts::connector::Template>,
) -> Result<BTreeMap<String, String>, Failure> {
    let inputs = runx_contracts::connector::TemplateInputs {
        arguments: Some(call.arguments),
        parameters: Some(call.parameters),
        ..runx_contracts::connector::TemplateInputs::default()
    };
    let mut rendered = BTreeMap::new();
    for (name, template) in headers {
        let value = match template.render(&inputs) {
            Ok(value) => value,
            Err(runx_contracts::connector::TemplateError::Unavailable {
                binding: "$argument",
                ..
            }) => continue,
            Err(error) => return Err(rejected("invalid_request", error.to_string())),
        };
        match value {
            JsonValue::String(value) if !value.chars().any(char::is_control) => {
                rendered.insert(name.clone(), value);
            }
            _ => {
                return Err(rejected(
                    "invalid_request",
                    format!("MCP header {name} must render a single-line string"),
                ));
            }
        }
    }
    Ok(rendered)
}

/// The stdio server as the runtime starts it: the base child environment
/// and the instance's credential in the variable its declaration names, in
/// the run's workspace.
#[cfg(feature = "mcp")]
fn stdio_server(
    context: &OperationAdmission,
    call: &AdmittedCall<'_>,
    command: &str,
    args: &[runx_contracts::schema::NonEmptyString],
) -> Result<crate::drivers::mcp::StdioServer, Failure> {
    let unavailable = |reason: String| rejected("mcp_unavailable", reason);
    let delivery = process_delivery(context, call.credential)?;
    let mut env = crate::process_invocation::process_base_environment(call.env)
        .map_err(|error| unavailable(error.to_string()))?;
    delivery
        .ensure_environment_disjoint(&env)
        .map_err(|error| unavailable(error.to_string()))?;
    env.extend(
        delivery
            .secret_env()
            .iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned())),
    );
    Ok(crate::drivers::mcp::StdioServer {
        command: command.to_owned(),
        args: args.iter().map(|arg| arg.as_str().to_owned()).collect(),
        cwd: crate::process_invocation::process_workspace(call.env)
            .map_err(|error| unavailable(error.to_string()))?,
        env,
    })
}

#[cfg(not(feature = "mcp"))]
fn mcp(
    _context: &OperationAdmission,
    _call: &AdmittedCall<'_>,
    _spec: &runx_contracts::connector::McpImplementation,
    _recovery: confirm::Recovery,
    _record: &mut dyn FnMut(&Acknowledgement) -> Result<(), String>,
    _dispatched: &mut Option<Dispatched>,
) -> Result<Readback, Failure> {
    Err(rejected(
        "operation_unsupported",
        "this build has no MCP client".to_owned(),
    ))
}

/// The tool is served exactly as the manifest pins it.
#[cfg(feature = "mcp")]
fn pinned(
    served: &BTreeMap<String, runx_contracts::connector::Digest>,
    tool: &str,
    pin: &runx_contracts::connector::Digest,
) -> Result<(), Failure> {
    match served.get(tool) {
        Some(digest) if digest == pin => Ok(()),
        Some(_) => Err(rejected(
            "tool_changed",
            format!(
                "the server serves tool {tool} with another definition than the connector pins; confirm it with `runx connector import-mcp`"
            ),
        )),
        None => Err(rejected(
            "tool_unavailable",
            format!("the server does not serve tool {tool}"),
        )),
    }
}

fn rejected(code: &str, reason: String) -> Failure {
    Failure::Rejected {
        status: 0,
        code: code.to_owned(),
        reason,
        release: true,
    }
}

/// What a write produced, as its operation declares.
fn effects(context: &OperationAdmission, result: &JsonValue) -> Result<Vec<Reference>, String> {
    let manifest = &context.admitted.connector.manifest;
    let Some(effect) = &context.admitted.operation().effect else {
        return Ok(Vec::new());
    };
    render_effects(
        manifest,
        effect,
        context.admitted.operation().repeated_write(),
        &runx_contracts::connector::TemplateInputs {
            arguments: Some(&JsonValue::Object(context.call.input.clone())),
            parameters: Some(&context.admitted.instance.parameters),
            result: Some(result),
            ..runx_contracts::connector::TemplateInputs::default()
        },
    )
}

/// One reference per rendered locator, of the effect's kind, each matching
/// the connector's own pattern for that kind when it declares references.
fn render_effects(
    manifest: &runx_contracts::connector::ConnectorManifest,
    effect: &runx_contracts::connector::OperationEffect,
    repeated_write: bool,
    inputs: &runx_contracts::connector::TemplateInputs<'_>,
) -> Result<Vec<Reference>, String> {
    let elements = match &effect.each {
        None => vec![None],
        Some(each) => match each
            .render(inputs)
            .map_err(|error| format!("the effect's result array cannot be read: {error}"))?
        {
            JsonValue::Array(items) if repeated_write => {
                applied(items)?.into_iter().map(Some).collect()
            }
            JsonValue::Array(items) => items.into_iter().map(Some).collect(),
            _ => return Err("the effect repeats over a result that is not an array".to_owned()),
        },
    };
    let patterns = manifest
        .references
        .iter()
        .filter(|reference| reference.kind == effect.kind)
        .map(|reference| runx_parser::whole_match(reference.locator.as_str()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("the connector's reference pattern is invalid: {error}"))?;
    elements
        .iter()
        .map(|item| {
            let locator = effect
                .locator
                .render_string(&runx_contracts::connector::TemplateInputs {
                    item: item.as_ref(),
                    ..*inputs
                })
                .map_err(|error| format!("the effect locator cannot be rendered: {error}"))?;
            // A locator names something; an empty one names nothing.
            if locator.trim().is_empty() {
                return Err("the effect locator rendered empty".to_owned());
            }
            if !patterns.is_empty() && !patterns.iter().any(|pattern| pattern.is_match(&locator)) {
                return Err(format!(
                    "the effect locator {locator:?} is not a {} {} declares",
                    ReferenceType::from(effect.kind).as_str(),
                    manifest.id
                ));
            }
            Ok(Reference {
                provider: Some(manifest.id.as_str().to_owned().into()),
                ..Reference::with_uri(effect.kind.into(), locator)
            })
        })
        .collect()
}

/// The provider's answer for each item a repeated write applied: an item
/// refused or never sent produced nothing.
fn applied(outcomes: Vec<JsonValue>) -> Result<Vec<JsonValue>, String> {
    outcomes
        .into_iter()
        .filter_map(|outcome| {
            let JsonValue::Object(mut outcome) = outcome else {
                return Some(Err("a repeated write's outcome is not a record".to_owned()));
            };
            match outcome.get("status").and_then(JsonValue::as_str) {
                Some("applied") => Some(
                    outcome
                        .remove("acknowledgement")
                        .ok_or_else(|| "an applied item carries no answer".to_owned()),
                ),
                Some(_) => None,
                None => Some(Err("a repeated write's outcome has no status".to_owned())),
            }
        })
        .collect()
}

fn finish(
    context: &OperationAdmission,
    readback: Readback,
    implementation_digest: Option<&str>,
    effects: Vec<Reference>,
) -> Result<OperationOutcome, RuntimeError> {
    let admitted = &context.admitted;
    let instance = &admitted.instance;
    let operation = admitted.operation();
    let (transport, _) = placement(context);
    let mut packet = readback
        .into_packet(
            &context.shape,
            context.call.expected_result.as_ref(),
            &Seal {
                access: operation.access().as_str(),
                finality: operation.finality(),
                transport,
                principal_ref: instance.principal_ref.as_str(),
                grant_id: admitted.capability.grant_id.as_deref(),
                account_ref: instance.account_ref.as_deref(),
                intent_digest: admitted.intent_digest().as_str(),
                operation_digest: admitted.intent().operation_digest.as_str(),
                implementation_digest,
            },
        )
        .map_err(|reason| failed(context, reason))?;
    let ephemeral = packet::split_ephemeral(&mut packet, &context.shape.ephemeral)
        .map_err(|reason| failed(context, reason))?;
    let packet = runx_contracts::JsonValue::from_serialize(&packet)
        .map_err(|error| engine(format!("the packet cannot be encoded: {error}")))?;
    Ok(OperationOutcome {
        value: JsonValue::Object(JsonObject::from([(
            "provider_operation".to_owned(),
            packet,
        )])),
        ephemeral: ephemeral.map(|result| {
            JsonValue::Object(JsonObject::from([(
                "provider_operation".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "result".to_owned(),
                    JsonValue::Object(result),
                )])),
            )]))
        }),
        effects,
    })
}

/// The transport the operation's packet names and the boundary it runs in.
fn placement(context: &OperationAdmission) -> (&'static str, ExecutionBoundaryKind) {
    let implementation = context.admitted.operation().implementation.as_ref();
    match (context.admitted.instance.placement, implementation) {
        (Placement::Hosted, _) => ("runx_connect", ExecutionBoundaryKind::RemoteProvider),
        (Placement::Local, Some(Implementation::Process(_))) => {
            ("local_process", ExecutionBoundaryKind::TrustedHostProcess)
        }
        // A stdio server is a process the runtime starts with the instance's
        // credential, so it is trusted, not contained.
        (Placement::Local, Some(Implementation::Mcp(_))) => {
            let server = context
                .admitted
                .connector
                .manifest
                .placements
                .local
                .as_ref()
                .and_then(|local| local.mcp_server.as_ref());
            match server {
                Some(runx_contracts::connector::McpServer::Stdio { .. }) => {
                    ("local_mcp", ExecutionBoundaryKind::TrustedHostProcess)
                }
                _ => ("local_mcp", ExecutionBoundaryKind::NativeCapability),
            }
        }
        (Placement::Local, Some(Implementation::Web(_))) => {
            ("local_web", ExecutionBoundaryKind::NativeCapability)
        }
        (Placement::Local, Some(Implementation::Store(_))) => {
            ("local_store", ExecutionBoundaryKind::NativeCapability)
        }
        // The runtime sends what admission proved; runx's storage is attested,
        // not enforced, by the runtime.
        (Placement::Local, Some(Implementation::Artifact(_))) => {
            ("local_artifact", ExecutionBoundaryKind::RemoteProvider)
        }
        (Placement::Local, _) => ("local_http", ExecutionBoundaryKind::NativeCapability),
    }
}

enum Lane {
    Approved(Box<Approved>),
    Pending {
        reason: String,
        request: Box<ResolutionRequest>,
    },
}

struct Approved {
    record: ApprovalRecord,
    evidence: AuthorityApproval,
    refs: Vec<Reference>,
}

impl Approved {
    fn new(record: ApprovalRecord, refs: Vec<Reference>) -> Self {
        let evidence = AuthorityApproval {
            approval_ref: Reference::with_uri(ReferenceType::Verification, record.uri()),
            approved_by_ref: None,
            approved_at: (record.actor == ApprovalActor::Human)
                .then(|| IsoDateTime::from(record.recorded_at.clone())),
        };
        Self {
            record,
            evidence,
            refs,
        }
    }
}

/// Gather approval evidence. A recovered attempt keeps the lane it was
/// approved under; otherwise a host-admitted paid job, then a person.
fn approve(
    context: &OperationAdmission,
    step: &GraphStep,
    host: &mut dyn crate::Host,
) -> Result<Lane, RuntimeEffectError> {
    let admitted = &context.admitted;
    let operation = admitted.operation();
    let paid = context.lanes.paid_job.as_ref();
    let floored = operation.approval == ApprovalFloor::Required || context.call.approval.is_some();
    if let Some(record) = context
        .admitted
        .pending
        .as_ref()
        .and_then(|pending| pending.approval.clone())
    {
        return match record.actor {
            ApprovalActor::Human => Ok(Lane::Approved(Box::new(Approved::new(record, Vec::new())))),
            ApprovalActor::PaidExternalJob => {
                let (key, refs) = paid_authority(context, paid)?;
                if key != record.key {
                    return Err(denied("the pending attempt changed its paid job authority"));
                }
                Ok(Lane::Approved(Box::new(Approved::new(record, refs))))
            }
        };
    }
    if paid.is_some() {
        if floored {
            return Err(denied(
                "an explicitly gated or approval-floored operation needs exact approval, not paid job authority",
            ));
        }
        let (key, refs) = paid_authority(context, paid)?;
        return Ok(Lane::Approved(Box::new(Approved::new(
            ApprovalRecord::new(ApprovalActor::PaidExternalJob, key),
            refs,
        ))));
    }
    human(context, step, host)
}

fn paid_authority(
    context: &OperationAdmission,
    authority: Option<&ExternalJobStageRequest>,
) -> Result<(String, Vec<Reference>), RuntimeEffectError> {
    let authority = authority.ok_or_else(|| denied("the paid job authority is missing"))?;
    let principal = authority
        .continuation
        .principal_ref
        .as_reference()
        .uri
        .as_str()
        .to_owned();
    if principal != context.admitted.instance.principal_ref.as_str() {
        return Err(denied(
            "the paid job authority belongs to another principal",
        ));
    }
    let evidence = |reference: &Reference| Reference {
        provider: Some("runx".to_owned().into()),
        proof_kind: Some(ProofKind::EffectEvidence),
        ..reference.clone()
    };
    let continuation = Reference {
        provider: Some("runx".to_owned().into()),
        proof_kind: Some(ProofKind::EffectEvidence),
        ..Reference::with_uri(
            ReferenceType::Target,
            format!(
                "runx:external-job:{}",
                authority.continuation.continuation_id.as_str()
            ),
        )
    };
    Ok((
        authority.operation_key.as_str().to_owned(),
        vec![
            evidence(&authority.continuation.invocation_ref),
            evidence(&authority.continuation.source_run_ref),
            continuation,
        ],
    ))
}

fn human(
    context: &OperationAdmission,
    step: &GraphStep,
    host: &mut dyn crate::Host,
) -> Result<Lane, RuntimeEffectError> {
    let admitted = &context.admitted;
    let intent = admitted.intent();
    let text = admitted
        .approval
        .as_ref()
        .ok_or_else(|| state("an approval is owed but has no text"))?;
    let gate_id = format!("operation:{}", admitted.intent_digest());
    let mut summary = JsonObject::from([
        (
            "provider".to_owned(),
            JsonValue::String(intent.connector.as_str().to_owned()),
        ),
        (
            "operation".to_owned(),
            JsonValue::String(intent.operation.as_str().to_owned()),
        ),
        (
            "description".to_owned(),
            JsonValue::String(admitted.operation().description.as_str().to_owned()),
        ),
        (
            "instance".to_owned(),
            JsonValue::String(intent.instance_id.as_str().to_owned()),
        ),
        (
            "principal_ref".to_owned(),
            JsonValue::String(admitted.instance.principal_ref.as_str().to_owned()),
        ),
        (
            "target".to_owned(),
            JsonValue::String(intent.target.as_str().to_owned()),
        ),
        (
            "payload_digest".to_owned(),
            JsonValue::String(intent.payload_digest.as_str().to_owned()),
        ),
        (
            "required_scopes".to_owned(),
            JsonValue::Array(
                admitted
                    .operation()
                    .scopes
                    .iter()
                    .map(|scope| JsonValue::String(scope.as_str().to_owned()))
                    .collect(),
            ),
        ),
        (
            "plan_digest".to_owned(),
            JsonValue::String(admitted.intent_digest().as_str().to_owned()),
        ),
    ]);
    if let Some(grant_id) = &admitted.capability.grant_id {
        summary.insert(
            "grant_ref".to_owned(),
            JsonValue::String(format!("runx:grant:{grant_id}")),
        );
    }
    if let Some(account) = &admitted.instance.account_ref {
        summary.insert(
            "account_ref".to_owned(),
            JsonValue::String(account.as_str().to_owned()),
        );
    }
    if let Some(amount) = &intent.amount {
        summary.insert(
            "amount".to_owned(),
            JsonValue::from_serialize(amount).map_err(|error| state(error.to_string()))?,
        );
    }
    // A person approves the act with the call: the intent binds its digest.
    if let Some(act) = &context.call.act {
        summary.insert(
            "act".to_owned(),
            JsonValue::from_serialize(act).map_err(|error| state(error.to_string()))?,
        );
    }
    let gate = ApprovalGate {
        id: gate_id.clone().into(),
        reason: text.reason.clone().into(),
        gate_type: text
            .gate_type
            .clone()
            .or_else(|| Some("operation".to_owned())),
        summary: Some(summary),
    };
    // The request is addressed by the call that asks; the gate keeps the
    // intent, and a resumed run answers only the request it waited on.
    let request_id = crate::execution::run_record::request_address(&context.lanes.run_id, &step.id);
    let request = ResolutionRequest::Approval {
        id: request_id.clone().into(),
        gate: gate.clone(),
    };
    match crate::request_approval(host, request_id, gate)
        .map_err(|error| denied(error.to_string()))?
    {
        // The resolver admits only host-attested human answers.
        ApprovalResolution::Approved {
            idempotency_key, ..
        } => Ok(Lane::Approved(Box::new(Approved::new(
            ApprovalRecord::new(ApprovalActor::Human, idempotency_key),
            Vec::new(),
        )))),
        ApprovalResolution::Denied { reason, .. } => {
            Err(denied(reason.unwrap_or_else(|| {
                format!("operation step '{}' was denied", step.id)
            })))
        }
        ApprovalResolution::Pending { .. } => Ok(Lane::Pending {
            reason: format!("approval {gate_id:?} for step '{}' is pending", step.id),
            request: Box::new(request),
        }),
    }
}

/// `runx:<intent digest>`, or a UUID derived from it where the provider
/// requires one.
fn idempotency_key(intent_digest: &str, format: IdempotencyKeyFormat) -> String {
    match (format, intent_digest.strip_prefix("sha256:")) {
        (IdempotencyKeyFormat::Uuid, Some(hex)) if hex.len() == 64 => format!(
            "{}-{}-4{}-8{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[13..16],
            &hex[17..20],
            &hex[20..32]
        ),
        _ => format!("runx:{intent_digest}"),
    }
}

fn read_failure(context: &OperationAdmission, step_id: &str, reason: String) -> RuntimeError {
    if context.call.readback_of.is_some() {
        return RuntimeError::ProviderReadbackPending {
            step_id: step_id.to_owned(),
            reason,
        };
    }
    failed(context, reason)
}

fn failed(context: &OperationAdmission, reason: String) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: format!(
            "{}/{}",
            context.admitted.connector.manifest.id, context.admitted.operation
        ),
        message: reason,
    }
}

pub(crate) fn receipt_root(env: &BTreeMap<String, String>, cwd: &Path) -> std::path::PathBuf {
    crate::paths::RunxPaths::resolve(env, cwd).receipts()
}

fn verification(uri: String, provider: &str, proof_kind: ProofKind) -> Reference {
    Reference {
        provider: Some(provider.to_owned().into()),
        proof_kind: Some(proof_kind),
        ..Reference::with_uri(ReferenceType::Verification, uri)
    }
}

fn invalid(message: impl Into<String>) -> RuntimeEffectError {
    RuntimeEffectError::InvalidMetadata {
        family: OPERATION_EFFECT_FAMILY.to_owned(),
        message: message.into(),
    }
}

fn denied(message: impl Into<String>) -> RuntimeEffectError {
    RuntimeEffectError::Denied {
        family: OPERATION_EFFECT_FAMILY.to_owned(),
        verb: AuthorityVerb::Write,
        message: message.into(),
    }
}

fn state(message: impl ToString) -> RuntimeEffectError {
    RuntimeEffectError::Failed {
        family: OPERATION_EFFECT_FAMILY.to_owned(),
        operation: "operation effect state",
        message: message.to_string(),
    }
}

fn engine(message: impl Into<String>) -> RuntimeError {
    RuntimeError::engine(
        "executing an operation step",
        RuntimeError::ReceiptInvalid {
            message: message.into(),
        },
    )
}

#[cfg(test)]
mod tests;
