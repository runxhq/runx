#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use runx_contracts::{
    ExecutionEvent, InstanceAuthority, JsonObject, PURCHASE_BALANCE_CHOICE, PaidSkillOfferTerms,
    Placement, PurchaseMethod, ResolutionRequest, ResolutionResponse, ResolutionResponseActor,
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
        .join("../../fixtures/contracts/purchase")
        .join(name);
    serde_json::from_str::<Value>(&std::fs::read_to_string(path).unwrap()).unwrap()["payload"]
        .clone()
}

/// A canonical Runx quote whose opaque payload happens to use x402.
fn quote(network: &str, amount: u64) -> Value {
    let payload = json!({
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
            "runx.purchase": {"info": {"purpose": "purchase", "purchase_id": "paid_1"}}
        }
    });
    json!({
        "status": "admitted",
        "value": {
            "purchase": quoted(amount, None)["purchase"],
            "payment_options": [{
                "settlement_family": "x402",
                "protocol_version": "2",
                "media_type": "application/json",
                "payload_digest": runx_contracts::sha256_prefixed(
                    &runx_contracts::to_canonical_bytes(&serde_json::from_value::<runx_contracts::JsonValue>(payload.clone()).unwrap()).unwrap()
                ),
                "payload": payload,
                "quote_ref": {"type": "receipt", "uri": "runx:receipt:quote:paid_1"},
                "quote_expires_at": "2099-01-01T00:00:00Z",
                "payment_method": {
                    "method": "exact", "network": network,
                    "asset": "USDC", "pay_to": "0x00000000000000000000000000000000000000aa"
                }
            }]
        }
    })
}

/// The quoted purchase as its buyer reads it, before payment: an account
/// with a balance reads it beside a purchase that takes the balance.
fn quoted(amount: u64, balance_micro: Option<u64>) -> Value {
    let mut reading = fixture("purchase-read-fulfilled.json");
    let object = reading.as_object_mut().unwrap();
    object.remove("receipt");
    object.remove("result");
    object.remove("holds");
    let purchase = &mut reading["purchase"];
    purchase["purchase_id"] = json!("paid_1");
    purchase["amount_minor"] = json!(amount);
    purchase["currency"] = json!("USD");
    purchase["expires_at"] = json!("2099-01-01T00:00:00Z");
    purchase["accepted_settlement_families"] = json!(["x402"]);
    purchase["payment_state"] = json!("unpaid");
    purchase["execution_state"] = json!("unstarted");
    purchase["outcome_gate"] = json!("open");
    let purchase = purchase.as_object_mut().unwrap();
    for field in ["act_ref", "payment_ref", "settlement_family"] {
        purchase.remove(field);
    }
    if let Some(balance_micro) = balance_micro {
        purchase.insert(
            "accepted_settlement_families".to_owned(),
            json!(["x402", "runx_balance"]),
        );
        reading["balance_micro"] = json!(balance_micro);
    }
    reading
}

fn purchase(outcome: &str) -> Value {
    let mut reading = fixture("purchase-read-fulfilled.json");
    reading["purchase"]["purchase_id"] = json!("paid_1");
    reading["purchase"]["amount_minor"] = json!(5);
    reading["purchase"]["currency"] = json!("USD");
    match outcome {
        "fulfilled" => {}
        "refunded" => {
            reading["purchase"]["outcome_gate"] = json!("refund_won");
            let object = reading.as_object_mut().unwrap();
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
    reading
}

/// runx's API as a purchase sees it: who the token is, the paid-run route
/// (a quote without a payment, a purchase with one) and the purchase read.
#[derive(Clone, Default)]
struct Runx {
    principal: Arc<Mutex<String>>,
    quote: Arc<Mutex<Value>>,
    /// What the buyer's runx balance holds, when the purchase takes it.
    balance_micro: Arc<Mutex<Option<u64>>>,
    /// Whether the quoted purchase has been paid, which its read then shows.
    paid: Arc<Mutex<bool>>,
    /// What each purchase answers, in order; the last repeats.
    bought: Arc<Mutex<Vec<(u16, Value)>>>,
    sent: Arc<Mutex<Vec<RuntimeHttpRequest>>>,
    read_purchase: Arc<Mutex<Option<Value>>>,
    after_submission_purchase: Arc<Mutex<Option<Value>>>,
}

impl Runx {
    fn new(quote: Value, bought: Vec<(u16, Value)>) -> Self {
        Self {
            principal: Arc::new(Mutex::new("buyer-1".to_owned())),
            quote: Arc::new(Mutex::new(quote)),
            balance_micro: Arc::default(),
            paid: Arc::default(),
            bought: Arc::new(Mutex::new(bought)),
            sent: Arc::default(),
            read_purchase: Arc::default(),
            after_submission_purchase: Arc::default(),
        }
    }

    fn with_balance(self, balance_micro: u64) -> Self {
        *self.balance_micro.lock().unwrap() = Some(balance_micro);
        self.quote.lock().unwrap()["value"]["purchase"]["accepted_settlement_families"] =
            json!(["x402", "runx_balance"]);
        self
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
                &json!({"status": "success", "principal": {"principal_id": self.principal.lock().unwrap().clone()}}),
            ),
            (HttpMethod::Post, ROUTE) => {
                let sent: Value =
                    serde_json::from_str(request.body.as_deref().unwrap_or("{}")).unwrap();
                if sent.get("account_payment").is_none() {
                    assert_eq!(
                        request
                            .headers
                            .iter()
                            .find(|h| h.name == "accept")
                            .unwrap()
                            .value,
                        format!(
                            "application/json; profile=\"{}\"",
                            runx_contracts::QUOTE_PURCHASE_RESULT_SCHEMA
                        )
                    );
                    body(402, &self.quote.lock().unwrap())
                } else {
                    if let Some(purchase) = self.after_submission_purchase.lock().unwrap().clone() {
                        *self.read_purchase.lock().unwrap() = Some(purchase);
                    }
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
                    let quoted = self.quote.lock().unwrap();
                    let purchase = self
                        .read_purchase
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or_else(|| quoted["value"]["purchase"].clone());
                    let mut view = json!({"purchase": purchase});
                    if let Some(balance) = *self.balance_micro.lock().unwrap() {
                        view["balance_micro"] = json!(balance);
                    }
                    return Ok(body(200, &view));
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

/// A project whose account holds one `fixture/wallet` grant.
struct Project {
    root: tempfile::TempDir,
    env: BTreeMap<String, String>,
    store: Arc<KernelStore>,
    wallet: String,
}

impl Project {
    fn new() -> Self {
        Self::with_pays(None)
    }

    fn with_pays(pays: Option<&str>) -> Self {
        let root = tempfile::tempdir().unwrap();
        crate::kernel::connectors::plugin_with_fixture_connectors(root.path(), &["fixture/wallet"]);
        if let Some(pays) = pays {
            let path = root.path().join("connectors/fixture/wallet/connector.yaml");
            let manifest = std::fs::read_to_string(&path).unwrap();
            let start = manifest.find("pays:\n").unwrap();
            let end = manifest[start..].find("operations:\n").unwrap() + start;
            std::fs::write(
                path,
                format!("{}{}{}", &manifest[..start], pays, &manifest[end..]),
            )
            .unwrap();
        }
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
            .connector("fixture/wallet")
            .unwrap();
        let instance = pinned(
            &connector,
            instance_id(&["hosted", "fixture/wallet", "grant_wallet"]).unwrap(),
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

    fn choice(&self, quote: &Value, index: usize) -> String {
        let option = &quote["value"]["payment_options"][index];
        super::payment_choice(
            &self.wallet,
            &serde_json::from_value(json!({
                "settlement_family": option["settlement_family"],
                "payload_digest": option["payload_digest"],
            }))
            .unwrap(),
        )
        .unwrap()
    }

    fn approve(&self) -> Buyer {
        Buyer::answering(
            ResolutionResponseActor::Human,
            json!({"approved": true, "choice": self.choice(&quote(NETWORK, 5), 0)}),
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
    assert_eq!(purchase.quote.purchase_id.as_str(), "paid_1");
    assert_eq!(
        purchase.quote.payment_options[0]
            .payment_method
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap()
            .as_str(),
        NETWORK
    );
    assert_eq!(purchase.methods.len(), 1);
    let PurchaseMethod::Grant {
        instance_id,
        connector,
        total_minor,
        ..
    } = &purchase.methods[0]
    else {
        panic!("the method is the hosted payment grant");
    };
    assert_eq!(instance_id.as_str(), project.wallet);
    assert_eq!(connector.as_str(), "fixture/wallet");
    assert_eq!(total_minor.get(), 5);
    assert_eq!(
        purchase.methods[0].choice(),
        project.choice(&quote(NETWORK, 5), 0)
    );
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
    assert_eq!(purchase.purchase.purchase_id.as_str(), "paid_1");
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
        json!({"purchase_id": "paid_1", "method": "grant", "grant_id": "grant_wallet", "option": {"settlement_family": "x402", "payload_digest": quote(NETWORK, 5)["value"]["payment_options"][0]["payload_digest"]}})
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
        json!({"approved": true, "choice": project.choice(&quote(NETWORK, 5), 0)}),
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

#[test]
fn a_balance_that_covers_the_price_is_offered_first() {
    let project = Project::new();
    let runx =
        Runx::new(quote(NETWORK, 5), vec![(201, purchase("fulfilled"))]).with_balance(80_000);
    let mut buyer = Buyer::pending();
    let Err(RuntimeError::ResolutionPending { request, .. }) =
        project.buy(&runx, &mut buyer, Some(1_000_000))
    else {
        panic!("a purchase waits on its buyer's decision");
    };
    let ResolutionRequest::Purchase { purchase, .. } = *request else {
        panic!("the request is a purchase");
    };
    assert_eq!(purchase.methods.len(), 2);
    assert_eq!(
        purchase.methods[0],
        PurchaseMethod::Balance {
            available_micro: 80_000,
            total_minor: runx_contracts::PortableAmountMinor::new(5).unwrap(),
        }
    );
    assert_eq!(purchase.methods[0].choice(), PURCHASE_BALANCE_CHOICE);
    assert_eq!(
        purchase.methods[1].choice(),
        project.choice(&quote(NETWORK, 5), 0)
    );
}

#[test]
fn a_balance_short_of_the_price_is_not_offered() {
    let project = Project::new();
    let runx =
        Runx::new(quote(NETWORK, 5), vec![(201, purchase("fulfilled"))]).with_balance(49_999);
    let mut buyer = Buyer::pending();
    let Err(RuntimeError::ResolutionPending { request, .. }) = project.buy(&runx, &mut buyer, None)
    else {
        panic!("a purchase waits on its buyer's decision");
    };
    let ResolutionRequest::Purchase { purchase, .. } = *request else {
        panic!("the request is a purchase");
    };
    assert_eq!(purchase.methods.len(), 1);
    assert_eq!(
        purchase.methods[0].choice(),
        project.choice(&quote(NETWORK, 5), 0)
    );
}

#[test]
fn a_purchase_paid_from_the_balance_names_the_balance_and_counts_against_the_limit() {
    let project = Project::new();
    let runx =
        Runx::new(quote(NETWORK, 5), vec![(201, purchase("fulfilled"))]).with_balance(80_000);
    let mut buyer = Buyer::answering(
        ResolutionResponseActor::Human,
        json!({"approved": true, "choice": PURCHASE_BALANCE_CHOICE}),
    );
    let Ok(Bought::Fulfilled { .. }) = project.buy(&runx, &mut buyer, Some(1_000_000)) else {
        panic!("a purchase paid from the balance is bought");
    };
    let purchases = runx.purchases();
    assert_eq!(purchases.len(), 1);
    assert_eq!(
        purchases[0].0["account_payment"],
        json!({"purchase_id": "paid_1", "method": "balance"})
    );
    assert_eq!(
        project.committed(),
        50_000,
        "the run's limit counts the price whichever method pays"
    );
}

#[test]
fn a_balance_alone_pays_where_no_grant_does() {
    let project = Project::new();
    let runx = Runx::new(
        quote("solana:mainnet", 5),
        vec![(201, purchase("fulfilled"))],
    )
    .with_balance(50_000);
    let mut buyer = Buyer::pending();
    let Err(RuntimeError::ResolutionPending { request, .. }) = project.buy(&runx, &mut buyer, None)
    else {
        panic!("the balance pays where no grant does");
    };
    let ResolutionRequest::Purchase { purchase, .. } = *request else {
        panic!("the request is a purchase");
    };
    assert_eq!(purchase.methods.len(), 1);
    assert_eq!(purchase.methods[0].choice(), PURCHASE_BALANCE_CHOICE);
}

#[test]
fn a_non_x402_quote_offers_a_compatible_grant_without_evm_terms() {
    let project = Project::with_pays(Some(
        "pays:\n  mpp:\n    methods: [stripe]\n    currencies: [USD]\n",
    ));
    let mut offered = quote(NETWORK, 5);
    offered["value"]["purchase"]["accepted_settlement_families"] = json!(["mpp"]);
    let challenge = &mut offered["value"]["payment_options"][0];
    challenge["settlement_family"] = json!("mpp");
    challenge
        .as_object_mut()
        .unwrap()
        .remove("protocol_version");
    challenge["payment_method"] = json!({"method": "stripe", "pay_to": "acct_merchant"});
    challenge["payload"] = json!({"opaque_protocol_request": "signed-challenge"});
    challenge["payload_digest"] = json!(runx_contracts::sha256_prefixed(
        &runx_contracts::to_canonical_bytes(
            &serde_json::from_value::<runx_contracts::JsonValue>(challenge["payload"].clone())
                .unwrap()
        )
        .unwrap()
    ));
    let expected_digest = challenge["payload_digest"].clone();
    let runx = Runx::new(offered, vec![]);
    let mut buyer = Buyer::pending();
    let Err(RuntimeError::ResolutionPending { request, .. }) = project.buy(&runx, &mut buyer, None)
    else {
        panic!("a compatible non-x402 quote waits on its buyer");
    };
    let ResolutionRequest::Purchase { purchase, .. } = *request else {
        panic!("a purchase decision")
    };
    assert_eq!(
        purchase.quote.payment_options[0]
            .option
            .settlement_family
            .as_str(),
        "mpp"
    );
    assert_eq!(
        purchase.quote.payment_options[0]
            .option
            .payload_digest
            .as_str(),
        expected_digest.as_str().unwrap()
    );
    assert!(
        purchase.quote.payment_options[0]
            .payment_method
            .as_ref()
            .unwrap()
            .network
            .is_none()
    );
    assert_eq!(purchase.methods.len(), 1);
    let PurchaseMethod::Grant { option, .. } = &purchase.methods[0] else {
        panic!("grant")
    };
    assert_eq!(
        option.payload_digest.as_str(),
        expected_digest.as_str().unwrap()
    );
    assert!(runx.purchases().is_empty());
    assert_eq!(project.committed(), 0);
}

#[test]
fn unknown_network_terms_do_not_satisfy_a_restricted_grant() {
    let project = Project::new();
    let mut offered = quote(NETWORK, 5);
    offered["value"]["payment_options"][0]
        .as_object_mut()
        .unwrap()
        .remove("payment_method");
    let runx = Runx::new(offered, vec![]);
    let mut buyer = Buyer::pending();
    let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut buyer, None) else {
        panic!("unknown terms refuse")
    };
    assert_eq!(code, "payment_method_unavailable");
    assert!(buyer.asked.is_empty());
    assert_eq!(project.committed(), 0);
}

#[test]
fn an_altered_challenge_or_unaccepted_family_refuses_before_any_decision() {
    for field in ["payload", "settlement_family"] {
        let project = Project::new();
        let mut offered = quote(NETWORK, 5);
        offered["value"]["payment_options"][0][field] = json!("altered");
        let runx = Runx::new(offered, vec![]);
        let mut buyer = Buyer::pending();
        let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut buyer, None) else {
            panic!("an altered challenge refuses")
        };
        assert_eq!(code, "quote_invalid");
        assert!(buyer.asked.is_empty());
        assert_eq!(project.committed(), 0);
    }
}

#[test]
fn a_changed_purchase_read_refuses_before_asking_or_reserving_money() {
    let project = Project::new();
    let offered = quote(NETWORK, 5);
    let mut changed = offered["value"]["purchase"].clone();
    changed["amount_minor"] = json!(10);
    let runx = Runx::new(offered, vec![]);
    *runx.read_purchase.lock().unwrap() = Some(changed);
    let mut buyer = Buyer::pending();
    let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut buyer, None) else {
        panic!("a changed quote refuses")
    };
    assert_eq!(code, "quote_changed");
    assert!(buyer.asked.is_empty());
    assert!(runx.purchases().is_empty());
    assert_eq!(project.committed(), 0);
}

#[test]
fn multiple_options_for_one_grant_have_distinct_choices_and_recovery_keeps_the_selection() {
    let project = Project::new();
    let mut offered = quote(NETWORK, 5);
    let mut second = offered["value"]["payment_options"][0].clone();
    second["payload"]["accepts"][0]["payTo"] = json!("0x00000000000000000000000000000000000000bb");
    second["payment_method"]["pay_to"] = json!("0x00000000000000000000000000000000000000bb");
    second["payload_digest"] = json!(runx_contracts::sha256_prefixed(
        &runx_contracts::to_canonical_bytes(
            &serde_json::from_value::<runx_contracts::JsonValue>(second["payload"].clone())
                .unwrap()
        )
        .unwrap()
    ));
    offered["value"]["payment_options"]
        .as_array_mut()
        .unwrap()
        .push(second.clone());
    let selected = project.choice(&offered, 1);
    let runx = Runx::new(
        offered.clone(),
        vec![
            (503, json!({"code": "unavailable"})),
            (201, purchase("fulfilled")),
        ],
    );
    let Err(RuntimeError::ResolutionPending { request, .. }) =
        project.buy(&runx, &mut Buyer::pending(), Some(1_000_000))
    else {
        panic!("decision required")
    };
    let ResolutionRequest::Purchase { purchase, .. } = *request else {
        panic!("purchase")
    };
    assert_eq!(purchase.quote.payment_options.len(), 2);
    assert_eq!(purchase.methods.len(), 2);
    assert_ne!(purchase.methods[0].choice(), purchase.methods[1].choice());
    assert_eq!(purchase.methods[1].choice(), selected);
    let mut buyer = Buyer::answering(
        ResolutionResponseActor::Human,
        json!({"approved": true, "choice": selected}),
    );
    assert!(matches!(
        project.buy(&runx, &mut buyer, Some(1_000_000)),
        Err(RuntimeError::ProviderEffectUnknown { .. })
    ));
    // Reopen the real SQLite store and discard the host's decision. Recovery
    // reads the durable method, even if the current quote order changes.
    let reopened = KernelStore::for_project(&crate::paths::RunxPaths::resolve(
        &project.env,
        project.root.path(),
    ))
    .unwrap();
    assert_eq!(
        spend::status(&reopened, "run_1")
            .unwrap()
            .committed_usd_micro,
        50_000
    );
    runx.quote.lock().unwrap()["value"]["payment_options"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(matches!(
        project.buy(&runx, &mut Buyer::pending(), Some(1_000_000)),
        Ok(Bought::Fulfilled { .. })
    ));
    let sent = runx.purchases();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0], sent[1]);
    assert_eq!(
        sent[0].0["account_payment"]["option"]["payload_digest"],
        second["payload_digest"]
    );
    assert_eq!(project.committed(), 50_000);
}

#[test]
fn duplicate_payment_options_are_refused_before_asking_or_reserving() {
    let project = Project::new();
    let mut offered = quote(NETWORK, 5);
    let option = offered["value"]["payment_options"][0].clone();
    offered["value"]["payment_options"]
        .as_array_mut()
        .unwrap()
        .push(option);
    let runx = Runx::new(offered, vec![]);
    let mut buyer = Buyer::pending();
    let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut buyer, Some(1_000_000)) else {
        panic!("duplicate refused")
    };
    assert_eq!(code, "quote_invalid");
    assert!(buyer.asked.is_empty());
    assert_eq!(project.committed(), 0);
}

#[test]
fn expired_options_are_not_offered_even_when_another_option_keeps_the_purchase_open() {
    let project = Project::new();
    let mut offered = quote(NETWORK, 5);
    let mut current = offered["value"]["payment_options"][0].clone();
    current["payload"]["accepts"][0]["payTo"] = json!("0x00000000000000000000000000000000000000bb");
    current["payment_method"]["pay_to"] = json!("0x00000000000000000000000000000000000000bb");
    current["payload_digest"] = json!(runx_contracts::sha256_prefixed(
        &runx_contracts::to_canonical_bytes(
            &serde_json::from_value::<runx_contracts::JsonValue>(current["payload"].clone())
                .unwrap(),
        )
        .unwrap(),
    ));
    offered["value"]["payment_options"][0]["quote_expires_at"] = json!("2020-01-01T00:00:00Z");
    offered["value"]["payment_options"]
        .as_array_mut()
        .unwrap()
        .push(current.clone());
    let runx = Runx::new(offered, vec![]);
    let Err(RuntimeError::ResolutionPending { request, .. }) =
        project.buy(&runx, &mut Buyer::pending(), None)
    else {
        panic!("the current option waits on its buyer");
    };
    let ResolutionRequest::Purchase { purchase, .. } = *request else {
        panic!("purchase");
    };
    assert_eq!(purchase.methods.len(), 1);
    let PurchaseMethod::Grant { option, .. } = &purchase.methods[0] else {
        panic!("grant");
    };
    assert_eq!(
        option.payload_digest.as_str(),
        current["payload_digest"].as_str().unwrap()
    );
    assert!(runx.purchases().is_empty());
}

#[test]
fn an_expired_purchase_refuses_before_a_decision_or_reservation() {
    let project = Project::new();
    let mut offered = quote(NETWORK, 5);
    offered["value"]["payment_options"][0]["quote_expires_at"] = json!("2020-01-01T00:00:00Z");
    offered["value"]["purchase"]["expires_at"] = json!("2020-01-01T00:00:00Z");
    let runx = Runx::new(offered, vec![]);
    let mut buyer = Buyer::pending();
    let Ok(Bought::Refused { code, .. }) = project.buy(&runx, &mut buyer, None) else {
        panic!("expired quote");
    };
    assert_eq!(code, "quote_expired");
    assert!(buyer.asked.is_empty());
    assert!(runx.purchases().is_empty());
    assert_eq!(project.committed(), 0);
}

#[test]
fn a_later_refusal_cannot_release_what_a_purchase_may_have_paid() {
    let project = Project::new();
    let runx = Runx::new(
        quote(NETWORK, 5),
        vec![
            (503, json!({"code":"unavailable"})),
            (409, json!({"code":"quote_unavailable"})),
        ],
    );
    assert!(matches!(
        project.buy(&runx, &mut project.approve(), Some(1_000_000)),
        Err(RuntimeError::ProviderEffectUnknown { .. })
    ));
    assert!(matches!(
        project.buy(&runx, &mut Buyer::pending(), Some(1_000_000)),
        Err(RuntimeError::ProviderEffectUnknown { .. })
    ));
    assert_eq!(project.committed(), 50_000);
    assert_eq!(runx.purchases().len(), 2);
    assert_eq!(runx.purchases()[0], runx.purchases()[1]);
}

#[test]
fn an_account_switch_cannot_resend_an_unresolved_purchase() {
    let project = Project::new();
    let runx = Runx::new(
        quote(NETWORK, 5),
        vec![(503, json!({"code":"unavailable"}))],
    );
    assert!(matches!(
        project.buy(&runx, &mut project.approve(), Some(1_000_000)),
        Err(RuntimeError::ProviderEffectUnknown { .. })
    ));
    *runx.principal.lock().unwrap() = "buyer-2".to_owned();
    assert!(matches!(
        project.buy(&runx, &mut Buyer::pending(), Some(1_000_000)),
        Err(RuntimeError::ProviderEffectUnknown { .. })
    ));
    assert_eq!(
        runx.purchases().len(),
        1,
        "no purchase reaches the replacement account"
    );
    assert_eq!(project.committed(), 50_000);
}

#[test]
fn even_a_first_http_refusal_requires_independent_unpaid_readback() {
    let project = Project::new();
    let offered = quote(NETWORK, 5);
    let mut settling = offered["value"]["purchase"].clone();
    settling["payment_state"] = json!("settling");
    settling["settlement_family"] = json!("x402");
    settling["payment_ref"] = json!({"type":"event","uri":"runx:payment:original"});
    let runx = Runx::new(
        offered,
        vec![(409, json!({"code":"purchase_binding_mismatch"}))],
    );
    *runx.after_submission_purchase.lock().unwrap() = Some(settling);
    assert!(matches!(
        project.buy(&runx, &mut project.approve(), Some(1_000_000)),
        Err(RuntimeError::ProviderEffectUnknown { .. })
    ));
    assert_eq!(project.committed(), 50_000);
}
