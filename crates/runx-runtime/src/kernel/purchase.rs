//! The kernel's purchase of a paid skill (S10, N10, N11).
//!
//! A call that reaches a runner the registry sells is bought, never run here,
//! at any depth: a top-level `runx skill`, a `skill:` step, an agent's skill
//! call. The kernel takes Cloud's quote for the call's inputs, and the call
//! waits on the person's exact decision: the quote's offer, seller and price,
//! and each payment method that can pay it: the account's runx balance, when
//! the purchase takes it and it covers the price, and each hosted payment
//! grant that pays on the quote's network. On their answer the kernel reserves
//! the amount against the run's limit, whichever method pays, in the commit
//! that claims the purchase, then makes one keyed purchase that runx pays from
//! that method. A purchase is an effect: claimed before it is sent,
//! resent under its key when its outcome is unknown, settled when it is read
//! back fulfilled or refunded. What it bought stays in runx custody: the call
//! returns the purchase as its buyer reads it, with what runx holds for it,
//! never a held value.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use runx_contracts::{
    InstanceAuthority, JsonObject, PaidSkillOfferTerms, Placement, PortableAmountMinor, Purchase,
    PurchaseExecutionState, PurchaseMethod, PurchaseOutcomeGate, PurchasePaymentChallenge,
    PurchasePaymentOption, PurchasePaymentOptionBinding, PurchasePaymentState, PurchaseQuote,
    PurchaseRead, PurchaseRequest, QUOTE_PURCHASE_RESULT_SCHEMA, QuotePurchaseResult,
    RUNX_BALANCE_SETTLEMENT_FAMILY, ResolutionRequest, ResolutionResponseActor, sha256_prefixed,
};
use serde::{Deserialize, Serialize};

use super::custody::{ApprovalActor, ApprovalRecord, Attempt, EffectCustody, Pending, Settlement};
use super::operation::hosted::{HostedClient, HostedSession};
use super::operation::packet::Readback;
use super::spend::{self, SpendError};
use super::store::{Append, Expected, KernelStore, StreamEvent};
use crate::RuntimeError;
use crate::host::Host;
use crate::http::HttpMethod;

/// How long a call waits for the seller's run before it holds as pending; a
/// resume reads the purchase again.
const WAIT_ENV: &str = "RUNX_PURCHASE_WAIT_SECONDS";
const DEFAULT_WAIT: Duration = Duration::from_secs(600);
const POLL: Duration = Duration::from_secs(2);
/// The term every purchase attempt is recorded under: a purchase is
/// authorized by the person's decision, not by a connector's capability.
const PURCHASE_TERM: &str = "runx:purchase";

/// A runner the registry sells, reached by a call.
#[derive(Clone, Debug)]
pub(crate) struct PaidRunner {
    pub skill_id: String,
    pub version: Option<String>,
    pub runner: String,
    pub terms: PaidSkillOfferTerms,
}

impl PaidRunner {
    /// The runner a call reaches through `reference`, when the registry
    /// sells it: a `registry:` reference whose runner carries an offer. A
    /// package run from its own source is its author's, and is run, not
    /// bought.
    pub(crate) fn reached(
        reference: &str,
        version: Option<&str>,
        runner: &str,
        offer: Option<&PaidSkillOfferTerms>,
    ) -> Option<Self> {
        let registry_ref = runx_parser::registry_ref(reference)?;
        let terms = offer?.clone();
        let parsed = crate::registry::parse_registry_ref(registry_ref);
        Some(Self {
            skill_id: parsed.skill_id,
            version: parsed.version.or_else(|| version.map(str::to_owned)),
            runner: runner.to_owned(),
            terms,
        })
    }

    /// What a purchase of this runner is, as its custody and readback name it.
    fn target(&self) -> String {
        format!("runx:purchase:{}#{}", self.skill_id, self.runner)
    }

    fn route(&self) -> Option<String> {
        let (publisher, name) = self.skill_id.split_once('/').filter(|(publisher, name)| {
            !publisher.is_empty() && !name.is_empty() && !name.contains('/')
        })?;
        Some(format!("/v1/skills/{publisher}/{name}/run"))
    }
}

/// One call that buys a paid runner.
pub(crate) struct PurchaseCall<'a> {
    pub paid: &'a PaidRunner,
    pub inputs: &'a JsonObject,
    /// The run the call is made in; its root's ledger holds the reservation.
    pub run_id: &'a str,
    pub step_id: &'a str,
    /// Where the decision is asked: the address a person's answer names.
    pub address: &'a str,
    /// The run's money limit, in micro-USD.
    pub limit: Option<u64>,
    pub env: &'a BTreeMap<String, String>,
    pub cwd: &'a Path,
    /// The runx API session the purchase is made through.
    pub hosted: &'a HostedClient,
}

/// How a purchase ended for its call.
pub(crate) enum Bought {
    /// Fulfilled: what the buyer reads, and what settles the act once the
    /// step that bought it is sealed.
    Fulfilled {
        purchase: Box<PurchaseRead>,
        settlement: PurchaseSettlement,
    },
    /// Nothing was bought: refused before any money moved, declined, or
    /// refunded.
    Refused { code: String, reason: String },
}

/// What settles a fulfilled purchase's act when its step is sealed.
pub(crate) struct PurchaseSettlement {
    custody: EffectCustody,
    spend: Option<(String, String)>,
}

impl PurchaseSettlement {
    pub(crate) fn sealed(self, receipt_id: String) -> Settlement {
        self.custody.settlement(receipt_id, &[], self.spend)
    }
}

/// What a resend of an attempted purchase needs, committed with the attempt.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum PurchaseEvent {
    Decided {
        idempotency_key: String,
        purchase_id: String,
        method: Paid,
        /// The run whose ledger reserved the amount, and its key there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spend: Option<(String, String)>,
    },
    Answered {
        idempotency_key: String,
        purchase_id: String,
    },
}

impl StreamEvent for PurchaseEvent {
    const KIND: &'static str = "purchase";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Decided { .. } => "decided",
            Self::Answered { .. } => "answered",
        }
    }
}

/// Cloud's quote for one call's inputs: the unpaid purchase it admitted, the
/// challenge that pays it, and what the account's balance holds when
/// the purchase takes it.
struct Quote {
    purchase: Purchase,
    payment_options: Vec<PurchasePaymentChallenge>,
    balance_micro: Option<u64>,
    expires_at: runx_contracts::schema::IsoDateTime,
}

/// What pays a decided purchase, as the attempt records and sends it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum Paid {
    /// The account's runx balance.
    Balance,
    /// A hosted payment grant.
    Grant {
        grant_id: String,
        option: PurchasePaymentOptionBinding,
    },
}

/// A payment method the payer holds that can pay the quote.
struct Method {
    offered: PurchaseMethod,
    paid: Paid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    approved: bool,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    choice: Option<String>,
}

/// Buy the call's runner, or continue the purchase an earlier attempt of the
/// same step left unsettled.
pub(crate) fn buy(call: &PurchaseCall<'_>, host: &mut dyn Host) -> Result<Bought, RuntimeError> {
    if call
        .env
        .contains_key(crate::execution_environment::RUNX_HOSTED_WORKSPACE_POLICY_JSON_ENV)
    {
        return Ok(refused(
            "purchase_unattended",
            "a run no person answers cannot buy; a paid skill is bought where its buyer decides",
        ));
    }
    let Some(route) = call.paid.route() else {
        return Ok(refused(
            "purchase_invalid",
            format!("paid skill {} is not publisher/name", call.paid.skill_id),
        ));
    };
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(call.env, call.cwd))
        .map_err(state)?;
    let key = sha256_prefixed(format!("purchase\0{}\0{}", call.run_id, call.step_id).as_bytes());
    let custody = EffectCustody::for_key(Arc::clone(&store), key.clone());
    let recorded = custody.custody().map_err(state)?;
    let client = call.hosted;

    if let Some(pending) = recorded.pending {
        // An attempt may have paid: only Cloud's answer under its key settles it.
        let session = client
            .session(call.env, call.cwd)
            .map_err(|reason| unknown(&pending, reason))?;
        return recover(call, &route, &session, &store, &custody, &key, &pending);
    }

    let session = match client.session(call.env, call.cwd) {
        Ok(session) => session,
        Err(reason) => {
            return Ok(refused(
                "account_required",
                format!("buying a paid skill needs a runx account: {reason}"),
            ));
        }
    };
    let purchase_key = sha256_prefixed(format!("{key}\0{}", recorded.sequence).as_bytes());
    let quote = match quote(call, &route, &session, &format!("{purchase_key}:quote")) {
        Ok(quote) => quote,
        Err(refusal) => return Ok(refusal),
    };
    if let Some(refusal) = disagreement(call, &quote) {
        return Ok(refusal);
    }
    let amount_micro = match usd_micro(&quote.purchase) {
        Ok(amount) => amount,
        Err(refusal) => return Ok(refusal),
    };
    if let Some(limit) = call.limit {
        let status = spend::status(&store, spend::root_run(call.run_id)).map_err(state)?;
        let remaining = limit.saturating_sub(status.committed_usd_micro);
        if amount_micro > remaining {
            return Ok(refused(
                "cost_limit_exceeded",
                format!(
                    "the purchase costs {amount_micro} micro-USD and the run has {remaining} of its {limit} left"
                ),
            ));
        }
    }
    let methods = methods(call, &store, &quote, amount_micro)?;
    if methods.is_empty() {
        return Ok(refused(
            "payment_method_unavailable",
            format!(
                "nothing this account holds pays {amount_micro} micro-USD through {}: add funds to its runx balance, or connect a compatible payment grant",
                quote
                    .payment_options
                    .iter()
                    .map(|option| option.settlement_family.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    let request = purchase_request(call, &quote, &methods)?;
    let resolution = ResolutionRequest::Purchase {
        id: call.address.into(),
        purchase: Box::new(request),
    };
    let request_digest =
        crate::execution::run_record::request_digest(&resolution).map_err(RuntimeError::from)?;
    let Some(response) = host.resolve(resolution.clone())? else {
        return Err(RuntimeError::ResolutionPending {
            step_id: call.step_id.to_owned(),
            reason: format!(
                "buying {}#{} waits on its buyer's decision",
                call.paid.skill_id, call.paid.runner
            ),
            request: Box::new(resolution),
            child: None,
        });
    };
    if response.actor != ResolutionResponseActor::Human {
        return Ok(refused(
            "purchase_not_decided",
            "a purchase is decided by a person, never by an agent",
        ));
    }
    let decision: Decision =
        match serde_json::to_value(&response.payload).and_then(serde_json::from_value) {
            Ok(decision) => decision,
            Err(error) => {
                return Ok(refused(
                    "purchase_answer_invalid",
                    format!("a purchase is answered with approved and choice: {error}"),
                ));
            }
        };
    if !decision.approved {
        return Ok(refused(
            "purchase_declined",
            decision
                .reason
                .unwrap_or_else(|| "the buyer declined the purchase".to_owned()),
        ));
    }
    let Some(method) = decision.choice.as_deref().and_then(|choice| {
        methods
            .iter()
            .find(|method| method.offered.choice() == choice)
    }) else {
        return Ok(refused(
            "purchase_choice_invalid",
            "an approved purchase names one of the request's methods as its choice",
        ));
    };

    let spend_cover = match call.limit {
        Some(limit) => match spend::cover(
            &store,
            call.run_id,
            limit,
            &purchase_key,
            &request_digest,
            amount_micro,
        ) {
            Ok(cover) => Some(cover),
            Err(SpendError::Exceeded { .. }) => {
                return Ok(refused(
                    "cost_limit_exceeded",
                    "the run spent its limit while the purchase waited",
                ));
            }
            Err(error) => return Err(state(error)),
        },
        None => None,
    };
    let spend = spend_cover
        .as_ref()
        .map(|_| (call.run_id.to_owned(), purchase_key.clone()));
    let decided = PurchaseEvent::Decided {
        idempotency_key: purchase_key.clone(),
        purchase_id: quote.purchase.purchase_id.as_str().to_owned(),
        method: method.paid.clone(),
        spend: spend.clone(),
    };
    let approval = ApprovalRecord::new(ApprovalActor::Human, request_digest.clone());
    custody
        .attempt_with(
            &Attempt {
                intent_digest: &request_digest,
                authority_term_id: PURCHASE_TERM,
                mandate_ref: None,
                idempotency_key: &purchase_key,
                instance_binding: &session.identity_binding(),
                attempt: 1,
                approval: Some(&approval),
                implementation_digest: None,
                admitted_at: recorded.head,
            },
            &[],
            spend_cover.as_ref(),
            vec![append(&store, &key, &decided)?],
        )
        .map_err(state)?;
    let sent = Sent {
        recovering: false,
        idempotency_key: purchase_key,
        purchase_id: quote.purchase.purchase_id.as_str().to_owned(),
        method: method.paid.clone(),
        spend,
    };
    send(call, &route, &session, &store, &custody, &key, &sent)
}

/// What an attempt sends, and what it reserved.
struct Sent {
    recovering: bool,
    idempotency_key: String,
    purchase_id: String,
    method: Paid,
    spend: Option<(String, String)>,
}

/// Continue an attempt an earlier pass left unsettled: read back the
/// purchase it bought, or send it again under its key.
fn recover(
    call: &PurchaseCall<'_>,
    route: &str,
    session: &HostedSession,
    store: &Arc<KernelStore>,
    custody: &EffectCustody,
    key: &str,
    pending: &Pending,
) -> Result<Bought, RuntimeError> {
    if pending.instance_binding != session.identity_binding() {
        return Err(unknown(pending,
            "the current account and endpoint do not match the recorded purchase binding; the original attempt remains unresolved".to_owned()));
    }
    let mut sent = None;
    let mut answered_id = None;
    for recorded in store.read::<PurchaseEvent>(key).map_err(state)? {
        match recorded.event {
            PurchaseEvent::Decided {
                idempotency_key,
                purchase_id,
                method,
                spend,
            } if idempotency_key == pending.idempotency_key => {
                sent = Some(Sent {
                    recovering: true,
                    idempotency_key,
                    purchase_id,
                    method,
                    spend,
                });
            }
            PurchaseEvent::Answered {
                idempotency_key,
                purchase_id: answered,
            } if idempotency_key == pending.idempotency_key => answered_id = Some(answered),
            _ => {}
        }
    }
    let Some(sent) = sent else {
        return Err(unknown(
            pending,
            "the attempt records no decision to send it again with".to_owned(),
        ));
    };
    match answered_id {
        Some(purchase_id) => follow(call, session, custody, &purchase_id, &sent),
        None => send(call, route, session, store, custody, key, &sent),
    }
}

/// Send the purchase under its key: runx pays it from the chosen method
/// against the quote, and runs the seller.
fn send(
    call: &PurchaseCall<'_>,
    route: &str,
    session: &HostedSession,
    store: &Arc<KernelStore>,
    custody: &EffectCustody,
    key: &str,
    sent: &Sent,
) -> Result<Bought, RuntimeError> {
    let mut account_payment = serde_json::to_value(&sent.method)
        .map_err(|error| RuntimeError::json("writing the purchase", error))?;
    if let Some(object) = account_payment.as_object_mut() {
        object.insert(
            "purchase_id".to_owned(),
            serde_json::json!(sent.purchase_id),
        );
    }
    let mut body = serde_json::json!({
        "runner": call.paid.runner,
        "inputs": call.inputs,
        "account_payment": account_payment,
    });
    if let (Some(version), Some(object)) = (&call.paid.version, body.as_object_mut()) {
        object.insert("version".to_owned(), serde_json::json!(version));
    }
    let held = |reason: String| -> RuntimeError {
        if let Err(error) = custody.unknown(reason.clone()) {
            return state(error);
        }
        RuntimeError::ProviderEffectUnknown {
            plan_digest: call.paid.target(),
            idempotency_key: sent.idempotency_key.clone(),
            reason,
        }
    };
    let response = match session.exchange(
        HttpMethod::Post,
        route,
        Some(body.to_string()),
        Some(&sent.idempotency_key),
        "application/json",
    ) {
        Ok(response) => response,
        Err(error) => return Err(held(format!("the purchase may have been sent: {error}"))),
    };
    match response.status {
        200..=299 => {
            let reading: PurchaseRead = serde_json::from_str(&response.body)
                .map_err(|error| held(format!("the purchase answer is not a purchase: {error}")))?;
            let purchase_id = reading.purchase.purchase_id.as_str().to_owned();
            store
                .append(vec![append(
                    store,
                    key,
                    &PurchaseEvent::Answered {
                        idempotency_key: sent.idempotency_key.clone(),
                        purchase_id: purchase_id.clone(),
                    },
                )?])
                .map_err(state)?;
            settle_or_follow(call, session, custody, reading, sent)
        }
        408 | 425 | 429 | 500..=599 => Err(held(format!(
            "runx answered the purchase with HTTP {}",
            response.status
        ))),
        status => {
            if sent.recovering {
                return Err(held(format!(
                    "the purchase may already have paid; HTTP {status} on recovery cannot prove otherwise"
                )));
            }
            // An HTTP failure can follow settlement. Release only after an
            // independent read proves this account's exact purchase unpaid.
            let observed = read(call, session, &sent.purchase_id).map_err(|_| {
                held("the purchase refusal has no independent unpaid readback".to_owned())
            })?;
            if observed.purchase.purchase_id.as_str() != sent.purchase_id
                || observed.purchase.buyer_ref.as_reference().uri.as_str() != session.principal_uri
                || observed.purchase.payment_state != PurchasePaymentState::Unpaid
                || observed.purchase.execution_state != PurchaseExecutionState::Unstarted
            {
                return Err(held(
                    "the purchase refusal does not prove that this purchase moved no money"
                        .to_owned(),
                ));
            }
            let (code, reason) = api_refusal(status, &response.body);
            let release = sent
                .spend
                .as_ref()
                .map(|(run_id, spend_key)| spend::release(custody.store(), run_id, spend_key))
                .transpose()
                .map_err(state)?
                .flatten();
            custody
                .rejected(code.clone(), &[], true, release)
                .map_err(state)?;
            Ok(Bought::Refused { code, reason })
        }
    }
}

/// Read the purchase until the seller's run reaches an outcome, or the wait
/// ends and the call holds.
fn follow(
    call: &PurchaseCall<'_>,
    session: &HostedSession,
    custody: &EffectCustody,
    purchase_id: &str,
    sent: &Sent,
) -> Result<Bought, RuntimeError> {
    let reading = read(call, session, purchase_id)?;
    settle_or_follow(call, session, custody, reading, sent)
}

fn settle_or_follow(
    call: &PurchaseCall<'_>,
    session: &HostedSession,
    custody: &EffectCustody,
    mut reading: PurchaseRead,
    sent: &Sent,
) -> Result<Bought, RuntimeError> {
    let started = Instant::now();
    let wait = wait(call.env);
    loop {
        match reading.purchase.outcome_gate {
            PurchaseOutcomeGate::FulfilmentWon if reading.receipt.is_some() => {
                let purchase_id = reading.purchase.purchase_id.as_str().to_owned();
                custody
                    .read_back(Readback {
                        provider: "runx".to_owned(),
                        operation: "purchase".to_owned(),
                        target: call.paid.target(),
                        result: serde_json::to_value(&reading)
                            .and_then(serde_json::from_value)
                            .map_err(|error| RuntimeError::json("recording the purchase", error))?,
                        readback_uri: format!("runx:purchase:{purchase_id}"),
                        operation_id: Some(purchase_id),
                        idempotency_key: Some(sent.idempotency_key.clone()),
                    })
                    .map_err(state)?;
                return Ok(Bought::Fulfilled {
                    purchase: Box::new(reading),
                    settlement: PurchaseSettlement {
                        custody: custody.clone(),
                        spend: sent.spend.clone(),
                    },
                });
            }
            PurchaseOutcomeGate::RefundWon => {
                let release = sent
                    .spend
                    .as_ref()
                    .map(|(run_id, spend_key)| spend::release(custody.store(), run_id, spend_key))
                    .transpose()
                    .map_err(state)?
                    .flatten();
                custody
                    .rejected("refunded".to_owned(), &[], true, release)
                    .map_err(state)?;
                let reason = reading.failure.as_ref().map_or_else(
                    || "the seller's run did not fulfil the purchase; it was refunded".to_owned(),
                    |failure| format!("{}: {}", failure.code.as_str(), failure.message.as_str()),
                );
                return Ok(Bought::Refused {
                    code: "purchase_refunded".to_owned(),
                    reason,
                });
            }
            _ if started.elapsed() >= wait => {
                return Err(RuntimeError::ProviderReadbackPending {
                    step_id: call.step_id.to_owned(),
                    reason: format!(
                        "purchase {} is still running for its seller; resume the run to read it again",
                        reading.purchase.purchase_id
                    ),
                });
            }
            _ => {
                std::thread::sleep(POLL);
                reading = read(call, session, reading.purchase.purchase_id.as_str())?;
            }
        }
    }
}

fn read(
    call: &PurchaseCall<'_>,
    session: &HostedSession,
    purchase_id: &str,
) -> Result<PurchaseRead, RuntimeError> {
    let pending = |reason: String| RuntimeError::ProviderReadbackPending {
        step_id: call.step_id.to_owned(),
        reason,
    };
    let response = session
        .exchange(
            HttpMethod::Get,
            &format!("/v1/purchases/{}", url_segment(purchase_id)),
            None,
            None,
            "application/json",
        )
        .map_err(|error| pending(format!("the purchase could not be read: {error}")))?;
    if !(200..=299).contains(&response.status) {
        return Err(pending(format!(
            "runx answered the purchase read with HTTP {}",
            response.status
        )));
    }
    serde_json::from_str(&response.body)
        .map_err(|error| pending(format!("the purchase read is not a purchase: {error}")))
}

/// Cloud's quote for the call: the unpaid purchase it admitted, its
/// requirement and its expiry.
fn quote(
    call: &PurchaseCall<'_>,
    route: &str,
    session: &HostedSession,
    key: &str,
) -> Result<Quote, Bought> {
    let mut body = serde_json::json!({"runner": call.paid.runner, "inputs": call.inputs});
    if let (Some(version), Some(object)) = (&call.paid.version, body.as_object_mut()) {
        object.insert("version".to_owned(), serde_json::json!(version));
    }
    let response = session
        .exchange(
            HttpMethod::Post,
            route,
            Some(body.to_string()),
            Some(key),
            &format!("application/json; profile=\"{QUOTE_PURCHASE_RESULT_SCHEMA}\""),
        )
        .map_err(|error| {
            refused(
                "quote_unavailable",
                format!("runx could not quote: {error}"),
            )
        })?;
    if response.status != 402 {
        let (code, reason) = match response.status {
            200..=299 => (
                "quote_invalid".to_owned(),
                "runx admitted an unpaid purchase".to_owned(),
            ),
            status => api_refusal(status, &response.body),
        };
        return Err(Bought::Refused { code, reason });
    }
    let result: QuotePurchaseResult = serde_json::from_str(&response.body)
        .map_err(|error| refused("quote_invalid", format!("the quote is not runx's: {error}")))?;
    let QuotePurchaseResult::Admitted { value } = result else {
        return Err(refused(
            "quote_unavailable",
            "runx refused to quote this purchase",
        ));
    };
    let purchase = value.purchase;
    let payment_options = value.payment_options.into_vec();
    let moment = |value: &runx_contracts::schema::IsoDateTime| {
        runx_core::policy::parse_rfc3339_moment(value.as_str())
            .ok_or_else(|| refused("quote_invalid", "the quote has an invalid expiry"))
    };
    let mut identities = BTreeSet::new();
    let mut latest_expiry = None;
    for option in &payment_options {
        if !purchase
            .accepted_settlement_families
            .accepts(option.settlement_family.as_str())
            || option.payload_digest.as_str()
                != sha256_prefixed(
                    &runx_contracts::to_canonical_bytes(&option.payload).map_err(|_| {
                        refused(
                            "quote_invalid",
                            "the payment option has no canonical payload",
                        )
                    })?,
                )
            || !identities.insert((
                option.settlement_family.as_str(),
                option.payload_digest.as_str(),
            ))
        {
            return Err(refused(
                "quote_invalid",
                "the payment options are not uniquely bound to the quoted terms",
            ));
        }
        let expiry = moment(&option.quote_expires_at)?;
        latest_expiry = Some(latest_expiry.map_or(expiry, |latest| std::cmp::max(latest, expiry)));
    }
    if latest_expiry != Some(moment(&purchase.expires_at)?) {
        return Err(refused(
            "quote_invalid",
            "the purchase expiry does not match its payment options",
        ));
    }
    let now = moment(&crate::time::now_iso8601().into())?;
    if latest_expiry.is_none_or(|expiry| expiry <= now) {
        return Err(refused(
            "quote_expired",
            "the quoted payment options have expired",
        ));
    }
    let expires_at = purchase.expires_at.clone();
    let purchase_id = purchase.purchase_id.as_str();
    let response = session
        .exchange(
            HttpMethod::Get,
            &format!("/v1/purchases/{}", url_segment(purchase_id)),
            None,
            None,
            "application/json",
        )
        .map_err(|error| {
            refused(
                "quote_unavailable",
                format!("runx could not read the quote: {error}"),
            )
        })?;
    if !(200..=299).contains(&response.status) {
        let (code, reason) = api_refusal(response.status, &response.body);
        return Err(Bought::Refused { code, reason });
    }
    let reading: PurchaseRead = serde_json::from_str(&response.body).map_err(|error| {
        refused(
            "quote_invalid",
            format!("the quoted purchase is not runx's: {error}"),
        )
    })?;
    if reading.purchase != purchase {
        return Err(refused(
            "quote_changed",
            "the purchase changed after it was quoted",
        ));
    }
    let balance_micro = reading.balance_micro.filter(|_| {
        purchase
            .accepted_settlement_families
            .accepts(RUNX_BALANCE_SETTLEMENT_FAMILY)
    });
    Ok(Quote {
        purchase,
        payment_options,
        balance_micro,
        expires_at,
    })
}

/// The quote must price what the listing prices: a fixed or chosen price is
/// computed here too, so runx cannot charge another amount. A size price
/// reads the size runx's store committed, which only runx knows.
fn disagreement(call: &PurchaseCall<'_>, quote: &Quote) -> Option<Bought> {
    let expected = match call.paid.terms.price.amount_minor(call.inputs, None) {
        Ok(amount) => amount.get(),
        Err(runx_contracts::PriceError::ArtifactSizeUnknown) => return None,
        Err(error) => {
            return Some(refused(
                "price_invalid",
                format!("the offer prices no purchase with these inputs: {error}"),
            ));
        }
    };
    (quote.purchase.amount_minor.get() != expected
        || quote.purchase.currency.as_str() != call.paid.terms.currency.as_str())
    .then(|| {
        refused(
            "quote_disagrees",
            format!(
                "runx quoted {} {} where the listing prices {expected} {}",
                quote.purchase.amount_minor.get(),
                quote.purchase.currency.as_str(),
                call.paid.terms.currency.as_str()
            ),
        )
    })
}

/// The account's runx balance when the purchase takes it and it covers the
/// price, then every hosted payment grant the payer holds whose connector pays
/// on the quote's network.
fn methods(
    call: &PurchaseCall<'_>,
    store: &Arc<KernelStore>,
    quote: &Quote,
    amount_micro: u64,
) -> Result<Vec<Method>, RuntimeError> {
    let total = PortableAmountMinor::new(quote.purchase.amount_minor.get())
        .ok_or_else(|| engine("the quoted amount is not a portable amount"))?;
    let records = super::instances::Instances::new(Arc::clone(store))
        .list()
        .map_err(state)?;
    let scope = super::connectors::ConnectorScope::at(call.cwd).ok();
    let now = runx_core::policy::parse_rfc3339_moment(&crate::time::now_iso8601())
        .ok_or_else(|| engine("the current time cannot be read"))?;
    let mut methods = Vec::new();
    if let Some(available_micro) = quote
        .balance_micro
        .filter(|available| *available >= amount_micro)
    {
        methods.push(Method {
            offered: PurchaseMethod::Balance {
                available_micro,
                total_minor: total,
            },
            paid: Paid::Balance,
        });
    }
    for record in records.into_iter().filter(|record| !record.revoked) {
        let instance = record.instance;
        let (Placement::Hosted, InstanceAuthority::HostedGrant { grant_id }) =
            (&instance.placement, &instance.authority)
        else {
            continue;
        };
        let Some(pays) = scope
            .as_ref()
            .and_then(|scope| scope.connector(instance.connector.as_str()).ok())
            .and_then(|connector| connector.manifest.pays.clone())
        else {
            continue;
        };
        for option in &quote.payment_options {
            if !runx_core::policy::parse_rfc3339_moment(option.quote_expires_at.as_str())
                .is_some_and(|expiry| expiry > now)
            {
                continue;
            }
            let terms = option.payment_method.as_ref();
            let compatible = pays
                .get(option.settlement_family.as_str())
                .is_some_and(|method| {
                    method.matches(
                        terms.map(|terms| terms.method.as_str()),
                        terms
                            .and_then(|terms| terms.network.as_ref())
                            .map(|network| network.as_str()),
                        Some(quote.purchase.currency.as_str()),
                    )
                });
            if !compatible {
                continue;
            }
            let option = PurchasePaymentOptionBinding {
                settlement_family: option.settlement_family.clone(),
                payload_digest: option.payload_digest.clone(),
            };
            let choice = payment_choice(instance.instance_id.as_str(), &option)?;
            methods.push(Method {
                offered: PurchaseMethod::Grant {
                    choice: choice.into(),
                    instance_id: instance.instance_id.as_str().into(),
                    connector: instance.connector.as_str().into(),
                    option: option.clone(),
                    total_minor: total,
                },
                paid: Paid::Grant {
                    grant_id: grant_id.as_str().to_owned(),
                    option,
                },
            });
        }
    }
    Ok(methods)
}

fn payment_choice(
    instance_id: &str,
    option: &PurchasePaymentOptionBinding,
) -> Result<String, RuntimeError> {
    let value = serde_json::to_value((instance_id, option))
        .and_then(serde_json::from_value::<runx_contracts::JsonValue>)
        .map_err(|error| RuntimeError::json("binding the payment choice", error))?;
    let bytes =
        runx_contracts::to_canonical_bytes(&value).map_err(|error| engine(&error.to_string()))?;
    Ok(sha256_prefixed(&bytes))
}

fn purchase_request(
    call: &PurchaseCall<'_>,
    quote: &Quote,
    methods: &[Method],
) -> Result<PurchaseRequest, RuntimeError> {
    let purchase = &quote.purchase;
    serde_json::from_value(serde_json::json!({
        "skill": call.paid.skill_id,
        "runner": call.paid.runner,
        "offer_version": purchase.offer_version,
        "package_digest": purchase.package_digest,
        "seller_ref": purchase.seller_ref,
        "amount_minor": purchase.amount_minor,
        "currency": purchase.currency,
        "input_digest": purchase.input_digest,
        "quote": PurchaseQuote {
            purchase_id: purchase.purchase_id.clone(),
            expires_at: quote.expires_at.clone(),
            payment_options: quote.payment_options.iter().map(|option| PurchasePaymentOption {
                option: PurchasePaymentOptionBinding {
                    settlement_family: option.settlement_family.clone(),
                    payload_digest: option.payload_digest.clone(),
                },
                expires_at: option.quote_expires_at.clone(),
                quote_ref: option.quote_ref.clone(),
                payment_method: option.payment_method.clone(),
            }).collect(),
        },
        "methods": methods
            .iter()
            .map(|method| &method.offered)
            .collect::<Vec<_>>(),
    }))
    .map_err(|error| RuntimeError::json("writing the purchase request", error))
}

/// What the purchase moves, counted against a money limit: a USD amount in
/// cents is micro-USD times ten thousand.
fn usd_micro(purchase: &Purchase) -> Result<u64, Bought> {
    if purchase.currency.as_str() != "USD" {
        return Err(refused(
            "currency_uncounted",
            format!(
                "a purchase in {} cannot be counted against a money limit; only USD can",
                purchase.currency.as_str()
            ),
        ));
    }
    purchase
        .amount_minor
        .get()
        .checked_mul(10_000)
        .ok_or_else(|| refused("price_invalid", "the purchase is too large to count"))
}

fn api_refusal(status: u16, body: &str) -> (String, String) {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    let code = parsed
        .as_ref()
        .and_then(|value| value.get("code").and_then(serde_json::Value::as_str))
        .map_or_else(|| format!("http_{status}"), str::to_owned);
    let reason = parsed
        .as_ref()
        .and_then(|value| {
            ["reason", "error", "detail"]
                .iter()
                .find_map(|field| value.get(*field).and_then(serde_json::Value::as_str))
        })
        .map_or_else(
            || format!("runx refused the purchase with HTTP {status}"),
            str::to_owned,
        );
    (code, reason)
}

fn append(store: &KernelStore, key: &str, event: &PurchaseEvent) -> Result<Append, RuntimeError> {
    let version = store.version::<PurchaseEvent>(key).map_err(state)?;
    Append::kernel(
        key.to_owned(),
        if version == 0 {
            Expected::Absent
        } else {
            Expected::Version(version)
        },
        event,
    )
    .map_err(state)
}

fn wait(env: &BTreeMap<String, String>) -> Duration {
    env.get(WAIT_ENV)
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(DEFAULT_WAIT, Duration::from_secs)
}

/// A purchase id as one path segment: only unreserved characters pass.
fn url_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn refused(code: impl Into<String>, reason: impl Into<String>) -> Bought {
    Bought::Refused {
        code: code.into(),
        reason: reason.into(),
    }
}

fn unknown(pending: &Pending, reason: String) -> RuntimeError {
    RuntimeError::ProviderEffectUnknown {
        plan_digest: pending.intent_digest.clone(),
        idempotency_key: pending.idempotency_key.clone(),
        reason,
    }
}

fn state(error: impl ToString) -> RuntimeError {
    RuntimeError::effect_state("recording the purchase", error.to_string())
}

fn engine(message: &str) -> RuntimeError {
    RuntimeError::effect_state("writing the purchase", message.to_owned())
}

#[cfg(test)]
mod tests;
