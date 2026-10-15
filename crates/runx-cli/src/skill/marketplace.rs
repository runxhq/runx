// Module rationale: a paid registry skill keeps the ordinary `runx skill`
// command. The kernel buys it (S10): it pays runx's paid-run route through the
// canonical `pay` skill, so the purchase passes the same approval, money limit
// and receipts as any payment; follows the purchase its buyer reads; and
// claims from the seller what runx never holds.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use runx_contracts::{JsonObject, JsonValue, PaidSkillListing, PriceError, Purchase};
use runx_runtime::purchase::{
    PAY_SKILL, PurchaseState, PurchaseTerms, PurchaserKeyPair, claim_purchase, new_purchase_run_id,
    pay_inputs, purchase_state,
};
use runx_runtime::{
    HostedApiEnvironment, ReqwestHttpTransport, RunResult, SkillRunRequest, hosted_api_transport,
    hosted_private_network_allowed, read_hosted_purchase,
};

use super::SkillPlan;
use super::resolver::ResolvedSkillRef;

/// How long the buyer waits for the seller's run before reporting the
/// purchase as still open; running the command again with the same run id
/// continues it.
const PURCHASE_WAIT_ENV: &str = "RUNX_PURCHASE_WAIT_SECONDS";
const DEFAULT_PURCHASE_WAIT: Duration = Duration::from_secs(600);
const PURCHASE_POLL: Duration = Duration::from_secs(2);

/// Where a purchase stopped.
pub(super) enum PurchaseOutcome {
    /// The payment waits on an approval or answer; resuming the payment run,
    /// then running the same command with `--run-id`, continues the purchase.
    Payment(RunResult),
    /// The purchase as its buyer reads it, with the seller's claim answer.
    Purchase(JsonValue),
}

pub(super) fn purchase_paid_skill(
    plan: &SkillPlan,
    resolved: &ResolvedSkillRef,
    inputs: &JsonObject,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PurchaseOutcome, String> {
    let listing = resolved
        .paid_listing
        .as_ref()
        .ok_or_else(|| "paid registry listing is unavailable".to_owned())?;
    let runner = selected_runner(listing, plan.runner.as_deref())?;
    let offer = &listing.offers.as_map()[runner.as_str()];
    let (amount_minor, max_amount_minor) = match offer.price.amount_minor(inputs, None) {
        Ok(amount) => (Some(amount.get()), amount.get()),
        // Priced by a stored artifact's size, which runx reads before payment:
        // the buyer authorizes the offer's ceiling and runx charges the exact amount.
        Err(PriceError::ArtifactSizeUnknown) => {
            let ceiling = offer
                .price
                .ceiling_minor()
                .ok_or_else(|| "the offer states no ceiling for its price".to_owned())?;
            (None, ceiling.get())
        }
        Err(error) => {
            return Err(format!(
                "the offer prices no purchase with these inputs: {error}"
            ));
        }
    };

    let hosted = HostedApiEnvironment::resolve(resolved.hosted_registry_url.as_deref(), env, cwd)
        .map_err(|error| format!("buying a paid skill needs a runx account: {error}"))?;
    let token = hosted
        .require_token()
        .map_err(|error| format!("buying a paid skill needs a runx account: {error}"))?;
    let resource_url = paid_run_url(hosted.base_url(), listing.skill_id.as_str())?;
    let run_id = match &plan.run_id {
        Some(run_id) => run_id.clone(),
        None => new_purchase_run_id().map_err(|error| error.to_string())?,
    };
    let paths = runx_runtime::RunxPaths::resolve(env, cwd);
    let key = PurchaserKeyPair::for_purchase(&paths, &run_id).map_err(|error| error.to_string())?;
    let purchaser_key = key.public_key();

    let payment = pay(
        plan,
        pay_inputs(&PurchaseTerms {
            resource_url: &resource_url,
            runner: &runner,
            inputs,
            purchaser_key: &purchaser_key,
            purchase_key: &run_id,
            amount_minor,
            max_amount_minor,
            currency: offer.currency.as_str(),
        }),
        &run_id,
        env,
        cwd,
    )?;
    if !payment.succeeded() {
        return Ok(PurchaseOutcome::Payment(payment));
    }
    let purchase_id = paid_invocation_id(&payment.output)?;

    let transport = hosted_api_transport(hosted_private_network_allowed(false, env))
        .map_err(|error| error.to_string())?;
    let purchase = follow(
        &transport,
        hosted.base_url(),
        token,
        &purchase_id,
        wait(env),
    )?;
    let claimed = match purchase_state(&purchase) {
        PurchaseState::Fulfilled => {
            let seller = ReqwestHttpTransport::new().map_err(|error| error.to_string())?;
            claim_purchase(&seller, &key, &purchase, &runx_runtime::now_iso8601())
                .map_err(|error| error.to_string())?
        }
        PurchaseState::Open | PurchaseState::Refunded => None,
    };
    render_purchase(listing, &runner, &run_id, &purchase, claimed).map(PurchaseOutcome::Purchase)
}

/// Pay runx's paid-run route through the canonical `pay` skill.
fn pay(
    plan: &SkillPlan,
    inputs: JsonObject,
    run_id: &str,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RunResult, String> {
    let pay_skill = super::resolve_skill_package(Path::new(PAY_SKILL), env, cwd)
        .map_err(|error| format!("the payment skill {PAY_SKILL} is unavailable: {error}"))?;
    let request = SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: plan.run_authority.as_deref().cloned(),
        act: plan.act.clone(),
        skill_path: pay_skill,
        receipt_dir: plan.receipt_dir.clone(),
        run_id: Some(run_id.to_owned()),
        answers: None,
        inputs,
        env: env.clone(),
        cwd: PathBuf::from(cwd),
        managed_agent: plan.managed_agent.clone(),
    };
    crate::runtime::local_orchestrator(env)
        .map_err(|error| format!("failed to initialize runtime effects: {error}"))?
        .run_skill(&request)
        .map_err(|error| error.to_string())
}

/// The paid invocation the payment bought: the purchase id.
fn paid_invocation_id(output: &JsonValue) -> Result<String, String> {
    let output = serde_json::to_value(output).map_err(|error| error.to_string())?;
    output["result"]["provider_operation"]["data"]["result"]["invocation_id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "the payment names no purchase it bought".to_owned())
}

/// Read the purchase until the seller's run reaches an outcome, or the wait ends.
fn follow(
    transport: &impl runx_runtime::RuntimeHttpTransport,
    base_url: &str,
    token: &str,
    purchase_id: &str,
    wait: Duration,
) -> Result<Purchase, String> {
    let started = Instant::now();
    loop {
        let purchase = read_hosted_purchase(transport, base_url, token, purchase_id)
            .map_err(|error| error.to_string())?;
        if !matches!(purchase_state(&purchase), PurchaseState::Open) || started.elapsed() >= wait {
            return Ok(purchase);
        }
        std::thread::sleep(PURCHASE_POLL);
    }
}

fn wait(env: &BTreeMap<String, String>) -> Duration {
    env.get(PURCHASE_WAIT_ENV)
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(DEFAULT_PURCHASE_WAIT, Duration::from_secs)
}

fn paid_run_url(base_url: &str, skill_id: &str) -> Result<String, String> {
    let (owner, name) = skill_id
        .split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .ok_or_else(|| format!("paid skill id {skill_id} is not owner/name"))?;
    Ok(format!(
        "{}/v1/skills/{owner}/{name}/run",
        base_url.trim_end_matches('/')
    ))
}

/// The purchase as the command reports it. The claim answer reaches only the
/// caller: no receipt records it.
fn render_purchase(
    listing: &PaidSkillListing,
    runner: &str,
    run_id: &str,
    purchase: &Purchase,
    claimed: Option<JsonValue>,
) -> Result<JsonValue, String> {
    let (status, outcome) = match purchase_state(purchase) {
        PurchaseState::Fulfilled => ("success", "fulfilled"),
        PurchaseState::Refunded => ("failure", "refunded"),
        PurchaseState::Open => ("pending", "deferred"),
    };
    let mut result = serde_json::json!({ "purchase": purchase });
    if let (Some(claimed), Some(object)) = (claimed, result.as_object_mut()) {
        object.insert(
            "claimed".to_owned(),
            serde_json::to_value(claimed).map_err(|error| error.to_string())?,
        );
    }
    serde_json::from_value(serde_json::json!({
        "status": status,
        "outcome": outcome,
        "skill_name": listing.skill_id,
        "runner": runner,
        "run_id": run_id,
        "result": result,
    }))
    .map_err(|error| format!("failed to render the purchase: {error}"))
}

fn selected_runner(listing: &PaidSkillListing, requested: Option<&str>) -> Result<String, String> {
    if let Some(runner) = requested {
        return listing
            .offers
            .as_map()
            .contains_key(runner)
            .then(|| runner.to_owned())
            .ok_or_else(|| format!("paid listing has no offer for runner '{runner}'"));
    }
    if listing.offers.as_map().len() == 1 {
        return listing
            .offers
            .as_map()
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| "paid listing has no runnable offers".to_owned());
    }
    Err("paid listing has multiple offers; select a runner explicitly".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> Result<PaidSkillListing, String> {
        serde_json::from_value(serde_json::json!({
            "skill_id": "ausca/agent-inbox",
            "version": "1.0.0",
            "skill_digest": format!("sha256:{}", "a".repeat(64)),
            "profile_digest": format!("sha256:{}", "b".repeat(64)),
            "package_digest": format!("sha256:{}", "c".repeat(64)),
            "vendor_ref": { "type": "principal", "uri": "runx:principal:ausca" },
            "offers": {
                "create": {
                    "price": {"model": "fixed", "amount_minor": 5},
                    "currency": "USD",
                    "accepted_settlement_families": ["x402"],
                    "offer_revision": {
                        "offer_id": "ausca/agent-inbox#create",
                        "revision": "1.0.0",
                        "revision_digest": format!("sha256:{}", "d".repeat(64)),
                        "input_schema_digest": format!("sha256:{}", "e".repeat(64)),
                        "output_schema_digest": format!("sha256:{}", "f".repeat(64))
                    }
                },
                "extend": {
                    "price": {"model": "fixed", "amount_minor": 5},
                    "currency": "USD",
                    "accepted_settlement_families": ["x402"],
                    "offer_revision": {
                        "offer_id": "ausca/agent-inbox#extend",
                        "revision": "1.0.0",
                        "revision_digest": format!("sha256:{}", "d".repeat(64)),
                        "input_schema_digest": format!("sha256:{}", "e".repeat(64)),
                        "output_schema_digest": format!("sha256:{}", "f".repeat(64))
                    }
                }
            }
        }))
        .map_err(|error| error.to_string())
    }

    #[test]
    fn a_purchase_names_one_offered_runner() -> Result<(), String> {
        let listing = listing()?;
        assert_eq!(selected_runner(&listing, Some("create"))?, "create");
        assert!(selected_runner(&listing, Some("delete")).is_err());
        assert!(selected_runner(&listing, None).is_err());
        Ok(())
    }

    #[test]
    fn the_paid_run_route_is_the_skill_s_run_endpoint() -> Result<(), String> {
        assert_eq!(
            paid_run_url("https://api.runx.ai/", "ausca/agent-inbox")?,
            "https://api.runx.ai/v1/skills/ausca/agent-inbox/run"
        );
        assert!(paid_run_url("https://api.runx.ai", "agent-inbox").is_err());
        Ok(())
    }

    #[test]
    fn the_purchase_id_is_the_paid_invocation_the_payment_bought() -> Result<(), String> {
        let output: JsonValue = serde_json::from_value(serde_json::json!({
            "result": {"provider_operation": {"data": {"result": {"invocation_id": "paid_7Hq2x9"}}}}
        }))
        .map_err(|error| error.to_string())?;
        assert_eq!(paid_invocation_id(&output)?, "paid_7Hq2x9");
        Ok(())
    }
}
