#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use runx_contracts::{
    ExecutionEvent, InstanceAuthority, JsonObject, PaidSkillOfferTerms, Placement,
    ResolutionRequest, ResolutionResponse, ResolutionResponseActor,
};
use serde_json::{Value, json};

use super::{Bought, PaidRunner, PurchaseCall, buy};
use crate::RuntimeError;
use crate::host::Host;
use crate::http::{
    HttpMethod, RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
};
use crate::kernel::instances::{Instances, instance_id, pinned};
use crate::kernel::operation::hosted::HostedClient;
use crate::kernel::spend;
use crate::kernel::store::KernelStore;

const BASE_URL: &str = "https://api.runx.test";
const ROUTE: &str = "https://api.runx.test/v1/skills/acme/inbox/run";
const NETWORK: &str = "eip155:8453";

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/contracts/paid-invocation")
        .join(name);
    serde_json::from_str::<Value>(&std::fs::read_to_string(path).unwrap()).unwrap()["payload"]
        .clone()
}

/// runx's x402 challenge for the invocation it quoted.
fn quote(network: &str, amount: u64) -> Value {
    json!({
        "x402Version": 2,
        "resource": {"url": ROUTE},
        "accepts": [{
            "scheme": "exact",
            "network": network,
            "amount": (amount * 10_000).to_string(),
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "payTo": "0x00000000000000000000000000000000000000aa",
            "maxTimeoutSeconds": 300
        }],
        "extensions": {
            "runx.invocation": {"info": {"purpose": "invocation", "invocation_id": "paid_1"}}
        }
    })
}

/// The quoted invocation as its buyer reads it, before payment.
fn quoted(amount: u64) -> Value {
    let mut purchase = fixture("purchase-fulfilled.json");
    let object = purchase.as_object_mut().unwrap();
    object.remove("receipt");
    object.remove("result");
    object.remove("holds");
    let invocation = &mut purchase["invocation"];
    invocation["invocation_id"] = json!("paid_1");
    invocation["amount_minor"] = json!(amount);
    invocation["currency"] = json!("USD");
    invocation["payment_state"] = json!("unpaid");
    invocation["execution_state"] = json!("unstarted");
    invocation["outcome_gate"] = json!("open");
    let invocation = invocation.as_object_mut().unwrap();
    for field in ["execution_ref", "payment_ref", "settlement_family"] {
        invocation.remove(field);
    }
    purchase
}

fn purchase(outcome: &str) -> Value {
    let mut purchase = fixture("purchase-fulfilled.json");
    purchase["invocation"]["invocation_id"] = json!("paid_1");
    purchase["invocation"]["amount_minor"] = json!(5);
    purchase["invocation"]["currency"] = json!("USD");
    match outcome {
        "fulfilled" => {}
        "refunded" => {
            purchase["invocation"]["outcome_gate"] = json!("refund_won");
            let object = purchase.as_object_mut().unwrap();
            object.remove("receipt");
            object.remove("result");
            object.remove("holds");
            object.insert(
                "failure".to_owned(),
                json!({"code": "execution_failed", "message": "the inbox could not be provisioned"}),
            );
        }
        _ => unreachable!(),
    }
    purchase
}

/// runx's API as a purchase sees it: who the token is, the paid-run route
/// (a quote without a payment, a purchase with one) and the purchase read.
#[derive(Clone, Default)]
struct Runx {
    quote: Arc<Mutex<Value>>,
    /// Whether the quoted purchase has been paid, which its read then shows.
    paid: Arc<Mutex<bool>>,
    /// What each paid purchase answers, in order; the last repeats.
    bought: Arc<Mutex<Vec<(u16, Value)>>>,
    sent: Arc<Mutex<Vec<RuntimeHttpRequest>>>,
}

impl Runx {
    fn new(quote: Value, bought: Vec<(u16, Value)>) -> Self {
        Self {
            quote: Arc::new(Mutex::new(quote)),
            paid: Arc::default(),
            bought: Arc::new(Mutex::new(bought)),
            sent: Arc::default(),
        }
    }

    fn purchases(&self) -> Vec<(Value, Option<String>)> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.method == HttpMethod::Post && request.url == ROUTE)
            .map(|request| {
                let body: Value =
                    serde_json::from_str(request.body.as_deref().unwrap_or("{}")).unwrap();
                let key = request
                    .headers
                    .iter()
                    .find(|header| header.name.eq_ignore_ascii_case("idempotency-key"))
                    .map(|header| header.value.clone());
                (body, key)
            })
            .filter(|(body, _)| body.get("account_payment").is_some())
            .collect()
    }
}

impl RuntimeHttpTransport for Runx {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        self.sent.lock().unwrap().push(request.clone());
        let body = |status: u16, value: &Value| RuntimeHttpResponse::new(status, value.to_string());
        Ok(match (request.method, request.url.as_str()) {
            (HttpMethod::Get, url) if url == format!("{BASE_URL}/v1/me") => body(
                200,
                &json!({"status": "success", "principal": {"principal_id": "buyer-1"}}),
            ),
            (HttpMethod::Post, ROUTE) => {
                let sent: Value =
                    serde_json::from_str(request.body.as_deref().unwrap_or("{}")).unwrap();
                if sent.get("account_payment").is_none() {
                    body(402, &self.quote.lock().unwrap())
                } else {
                    let mut bought = self.bought.lock().unwrap();
                    let (status, value) = if bought.len() > 1 {
                        bought.remove(0)
                    } else {
                        bought[0].clone()
                    };
                    if (200..300).contains(&status) {
                        *self.paid.lock().unwrap() = true;
                    }
                    body(status, &value)
                }
            }
            (HttpMethod::Get, url) if url == format!("{BASE_URL}/v1/purchases/paid_1") => {
                if *self.paid.lock().unwrap() {
                    body(200, &purchase("fulfilled"))
                } else {
                    let amount = self.quote.lock().unwrap()["accepts"][0]["amount"]
                        .as_str()
                        .and_then(|amount| amount.parse::<u64>().ok())
                        .unwrap_or_default()
                        / 10_000;
                    body(200, &quoted(amount))
                }
            }
            _ => body(404, &json!({"code": "not_found"})),
        })
    }
}

/// A buyer who answers every purchase as scripted, or not yet.
struct Buyer {
    answer: Option<ResolutionResponse>,
    asked: Vec<ResolutionRequest>,
}

impl Buyer {
    fn pending() -> Self {
        Self {
            answer: None,
            asked: Vec::new(),
        }
    }

    fn answering(actor: ResolutionResponseActor, payload: Value) -> Self {
        Self {
            answer: Some(ResolutionResponse {
                actor,
                payload: serde_json::from_value(payload).unwrap(),
            }),
            asked: Vec::new(),
        }
    }
}

impl Host for Buyer {
    fn report(&mut self, _event: ExecutionEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError> {
        self.asked.push(request);
        Ok(self.answer.clone())
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }
}

/// A project whose account holds one `wallet/evm` grant.
struct Project {
    root: tempfile::TempDir,
    env: BTreeMap<String, String>,
    store: Arc<KernelStore>,
    wallet: String,
}

impl Project {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        crate::kernel::connectors::plugin_with_source_connectors(root.path(), &["wallet/evm"]);
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
            ("RUNX_PURCHASE_WAIT_SECONDS".to_owned(), "0".to_owned()),
        ]);
        let store =
            KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, root.path())).unwrap();
        let connector = crate::kernel::connectors::ConnectorScope::at(root.path())
            .unwrap()
            .connector("wallet/evm")
            .unwrap();
        let instance = pinned(
            &connector,
            instance_id(&["hosted", "wallet/evm", "grant_wallet"]).unwrap(),
            Placement::Hosted,
            "runx:principal:buyer-1".to_owned(),
            None,
            InstanceAuthority::HostedGrant {
                grant_id: "grant_wallet".into(),
            },
        );
        let wallet = instance.instance_id.as_str().to_owned();
        Instances::new(Arc::clone(&store))
            .confirm(instance, true)
            .unwrap();
        Self {
            root,
            env,
            store,
            wallet,
        }
    }

    fn paid() -> PaidRunner {
        let terms: PaidSkillOfferTerms = serde_json::from_value(json!({
            "price": {"model": "fixed", "amount_minor": 5},
            "currency": "USD",
            "accepted_settlement_families": ["x402"],
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64))
        }))
        .unwrap();
        PaidRunner::reached("registry:acme/inbox@1.0.0", None, "create", Some(&terms)).unwrap()
    }

    fn buy(
        &self,
        runx: &Runx,
        buyer: &mut Buyer,
        limit: Option<u64>,
    ) -> Result<Bought, RuntimeError> {
        let paid = Self::paid();
        let inputs = JsonObject::new();
        let hosted = HostedClient::with_transport(Arc::new(runx.clone()));
        buy(
            &PurchaseCall {
                paid: &paid,
                inputs: &inputs,
                run_id: "run_1",
                step_id: "purchase",
                address: "purchase",
                limit,
                env: &self.env,
                cwd: self.root.path(),
                hosted: &hosted,
            },
            buyer,
        )
    }

    fn committed(&self) -> u64 {
        spend::status(&self.store, "run_1")
            .unwrap()
            .committed_usd_micro
    }

    fn approve(&self) -> Buyer {
        Buyer::answering(
            ResolutionResponseActor::Human,
            json!({"approved": true, "choice": self.wallet}),
        )
    }
}

#[test]
fn only_a_registry_runner_that_carries_an_offer_is_bought() {
    let terms = Project::paid().terms;
    assert!(
        PaidRunner::reached("registry:acme/inbox", Some("1.0.0"), "create", Some(&terms)).is_some()
    );
    assert!(PaidRunner::reached("./inbox", None, "create", Some(&terms)).is_none());
    assert!(PaidRunner::reached("registry:acme/inbox", None, "create", None).is_none());
    let pinned = PaidRunner::reached(
        "registry:acme/inbox@1.0.0",
        Some("0.9.0"),
        "create",
        Some(&terms),
    )
    .unwrap();
    assert_eq!(pinned.version.as_deref(), Some("1.0.0"));
}

#[test]
fn a_purchase_waits_on_its_buyer_with_the_quote_and_each_method_that_pays_it() {
    let project = Project::new();
    let runx = Runx::new(quote(NETWORK, 5), vec![(201, purchase("fulfilled"))]);
    let mut buyer = Buyer::pending();
    let Err(RuntimeError::ResolutionPending { request, .. }) =
        project.buy(&runx, &mut buyer, Some(1_000_000))
    else {
        panic!("a purchase waits on its buyer's decision");
    };
    let ResolutionRequest::Purchase { id, purchase } = *request else {
        panic!("the request is a purchase");
    };
    assert_eq!(id.as_str(), "purchase");
    assert_eq!(purchase.amount_minor.get(), 5);
    assert_eq!(purchase.quote.invocation_id.as_str(), "paid_1");
    assert_eq!(purchase.quote.network.as_str(), NETWORK);
    assert_eq!(purchase.methods.len(), 1);
    assert_eq!(purchase.methods[0].instance_id.as_str(), project.wallet);
    assert_eq!(purchase.methods[0].connector.as_str(), "wallet/evm");
    assert!(
        runx.purchases().is_empty(),
        "nothing is bought before the decision"
    );
    assert_eq!(
        project.committed(),
        0,
        "nothing is reserved before the decision"
    );
}

#[test]
fn an_approved_purchase_reserves_its_amount_and_buys_from_the_chosen_method() {
    let project = Project::new();
    let runx = Runx::new(quote(NETWORK, 5), vec![(201, purchase("fulfilled"))]);
    let Ok(Bought::Fulfilled {
        purchase,
        settlement,
    }) = project.buy(&runx, &mut project.approve(), Some(1_000_000))
    else {
        panic!("an approved purchase is bought");
    };
    assert_eq!(purchase.invocation.invocation_id.as_str(), "paid_1");
    assert_eq!(
        purchase.holds.len(),
        1,
        "what it bought is held, never returned"
    );
    let purchases = runx.purchases();
    assert_eq!(purchases.len(), 1);
    let (body, key) = &purchases[0];
    assert_eq!(body["runner"], "create");
    assert_eq!(body["version"], "1.0.0");
    assert_eq!(
        body["account_payment"],
        json!({"invocation_id": "paid_1", "grant_id": "grant_wallet"})
    );
    assert!(key.is_some(), "a purchase is sent under its key");
    assert_eq!(project.committed(), 50_000, "five cents is reserved");
    let appends = settlement
        .sealed("receipt_1".to_owned())
        .appends(&project.store)
        .unwrap();
    project.store.append(appends).unwrap();
    assert_eq!(
        project.committed(),
        50_000,
        "the reservation settles as spent"
    );
}

#[test]
fn a_declined_or_unchosen_purchase_buys_and_reserves_nothing() {
    for answer in [
        json!({"approved": false, "reason": "too dear"}),
        json!({"approved": true}),
        json!({"approved": true, "choice": "inst_elsewhere"}),
    ] {
        let project = Project::new();
        let runx = Runx::new(quote(NETWORK, 5), vec![(201, purchase("fulfilled"))]);
        let mut buyer = Buyer::answering(ResolutionResponseActor::Human, answer.clone());
        let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut buyer, Some(1_000_000))
        else {
            panic!("{answer} buys nothing");
        };
        assert!(code.starts_with("purchase_"), "{code}");
        assert!(runx.purchases().is_empty());
        assert_eq!(project.committed(), 0);
    }
}

#[test]
fn an_agent_never_decides_a_purchase() {
    let project = Project::new();
    let runx = Runx::new(quote(NETWORK, 5), vec![(201, purchase("fulfilled"))]);
    let mut agent = Buyer::answering(
        ResolutionResponseActor::Agent,
        json!({"approved": true, "choice": project.wallet}),
    );
    let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut agent, None) else {
        panic!("an agent's answer buys nothing");
    };
    assert_eq!(code, "purchase_not_decided");
    assert!(runx.purchases().is_empty());
}

#[test]
fn a_refusal_before_payment_releases_the_reservation() {
    let project = Project::new();
    let runx = Runx::new(
        quote(NETWORK, 5),
        vec![(
            409,
            json!({"code": "quote_expired", "error": "the quote expired"}),
        )],
    );
    let Ok(Bought::Refused { code, .. }) =
        project.buy(&runx, &mut project.approve(), Some(1_000_000))
    else {
        panic!("a refused purchase is refused");
    };
    assert_eq!(code, "quote_expired");
    assert_eq!(
        project.committed(),
        0,
        "a refusal releases what was reserved"
    );
}

#[test]
fn an_unknown_outcome_holds_and_the_next_pass_sends_the_same_purchase_again() {
    let project = Project::new();
    let runx = Runx::new(
        quote(NETWORK, 5),
        vec![
            (503, json!({"code": "unavailable"})),
            (201, purchase("fulfilled")),
        ],
    );
    let Err(RuntimeError::ProviderEffectUnknown { .. }) =
        project.buy(&runx, &mut project.approve(), Some(1_000_000))
    else {
        panic!("an unanswered purchase holds");
    };
    assert_eq!(
        project.committed(),
        50_000,
        "what may have been spent stays reserved"
    );
    // The next pass needs no decision: it resends what was decided.
    let Ok(Bought::Fulfilled { .. }) = project.buy(&runx, &mut Buyer::pending(), Some(1_000_000))
    else {
        panic!("the resent purchase is bought");
    };
    let purchases = runx.purchases();
    assert_eq!(purchases.len(), 2);
    assert_eq!(
        purchases[0], purchases[1],
        "the same purchase under the same key"
    );
    assert_eq!(project.committed(), 50_000, "reserved once");
}

#[test]
fn a_refund_releases_the_reservation() {
    let project = Project::new();
    let runx = Runx::new(quote(NETWORK, 5), vec![(201, purchase("refunded"))]);
    let Ok(Bought::Refused { code, reason }) =
        project.buy(&runx, &mut project.approve(), Some(1_000_000))
    else {
        panic!("a refunded purchase bought nothing");
    };
    assert_eq!(code, "purchase_refunded");
    assert!(reason.contains("could not be provisioned"), "{reason}");
    assert_eq!(project.committed(), 0);
}

#[test]
fn a_quote_the_listing_does_not_price_is_refused_before_asking() {
    let project = Project::new();
    let runx = Runx::new(quote(NETWORK, 7), vec![(201, purchase("fulfilled"))]);
    let mut buyer = Buyer::pending();
    let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut buyer, None) else {
        panic!("a disagreeing quote is refused");
    };
    assert_eq!(code, "quote_disagrees");
    assert!(buyer.asked.is_empty(), "the buyer is never asked");
}

#[test]
fn a_call_no_method_pays_or_the_limit_cannot_cover_is_refused_before_asking() {
    let project = Project::new();
    let runx = Runx::new(
        quote("solana:mainnet", 5),
        vec![(201, purchase("fulfilled"))],
    );
    let mut buyer = Buyer::pending();
    let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut buyer, None) else {
        panic!("no method refuses");
    };
    assert_eq!(code, "payment_method_unavailable");

    let runx = Runx::new(quote(NETWORK, 5), vec![(201, purchase("fulfilled"))]);
    let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut buyer, Some(10_000)) else {
        panic!("a limit below the price refuses");
    };
    assert_eq!(code, "cost_limit_exceeded");
    assert!(buyer.asked.is_empty());
}

#[test]
fn a_run_no_person_answers_cannot_buy() {
    let mut project = Project::new();
    project.env.insert(
        crate::execution_environment::RUNX_HOSTED_WORKSPACE_POLICY_JSON_ENV.to_owned(),
        "{}".to_owned(),
    );
    let runx = Runx::new(quote(NETWORK, 5), vec![(201, purchase("fulfilled"))]);
    let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut project.approve(), None) else {
        panic!("an unattended run cannot buy");
    };
    assert_eq!(code, "purchase_unattended");
    assert!(runx.sent.lock().unwrap().is_empty(), "runx is never asked");
}
