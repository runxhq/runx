//! The kernel's purchase of a paid skill (S10). Buying is paying an x402
//! resource, runx's paid-run route, through the canonical `pay` skill, so a
//! purchase passes the same approval, money limit and receipts as any
//! payment. The buyer then follows the purchase it reads, never the seller's
//! run, and claims from the seller what runx never holds, with a key its home
//! derives for that purchase alone.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{Ed25519KeyPair, KeyPair};
use runx_contracts::{
    JsonObject, JsonValue, PaidInvocationOutcomeGate, Purchase, PurchaseClaim, PurchaserKey,
    PurchaserSignature, purchase_claim_message,
};
use thiserror::Error;

use crate::http::{HttpMethod, RuntimeHttpHeader, RuntimeHttpRequest, RuntimeHttpTransport};
use crate::paths::RunxPaths;

/// The derivation purpose of purchaser keys in a home's key.
pub const PURCHASER_KEY_PURPOSE: &str = "runx.purchaser_key.v1";
/// The canonical payment skill a purchase pays through.
pub const PAY_SKILL: &str = "runx/pay";

/// Why a purchase cannot proceed.
#[derive(Debug, Error)]
pub enum PurchaseError {
    #[error("the home key cannot derive a purchaser key: {0}")]
    Key(String),
    #[error("a purchase claim cannot be written: {0}")]
    Claim(String),
    #[error("the seller refused the claim with HTTP {status}: {body}")]
    ClaimRefused { status: u16, body: String },
    #[error("the claim could not reach the seller: {0}")]
    Transport(String),
}

/// A new purchase's run id, which is also its purchase key: the paid-run
/// request and its payment share it, and a resume names it again.
pub fn new_purchase_run_id() -> Result<String, PurchaseError> {
    crate::execution::run_record::mint_run_id("purchase")
        .map_err(|error| PurchaseError::Key(error.to_string()))
}

/// The key one purchase is claimed with.
pub struct PurchaserKeyPair(Ed25519KeyPair);

impl PurchaserKeyPair {
    /// The key this home claims one purchase with, derived from its private
    /// key and the purchase key: the same on every resume of that purchase,
    /// and never the same for two purchases.
    pub fn for_purchase(paths: &RunxPaths, purchase_key: &str) -> Result<Self, PurchaseError> {
        let seed = crate::receipts::local_key::derive_home_seed(
            paths,
            PURCHASER_KEY_PURPOSE,
            purchase_key,
        )
        .map_err(|error| PurchaseError::Key(error.to_string()))?;
        Ed25519KeyPair::from_seed_unchecked(seed.as_ref())
            .map(Self)
            .map_err(|error| PurchaseError::Key(error.to_string()))
    }

    pub fn public_key(&self) -> PurchaserKey {
        PurchaserKey::new(format!(
            "ed25519:{}",
            URL_SAFE_NO_PAD.encode(self.0.public_key().as_ref())
        ))
        .unwrap_or_else(|| unreachable!("an Ed25519 public key is 32 bytes"))
    }

    /// The claim this key signs for one endpoint, at one time, presenting
    /// runx's signed purchase receipt.
    pub fn sign_claim(
        &self,
        claim_url: &str,
        issued_at: &str,
        receipt: JsonObject,
    ) -> Result<PurchaseClaim, PurchaseError> {
        let digest = receipt
            .get("digest")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                PurchaseError::Claim("the purchase receipt names no digest".to_owned())
            })?;
        let message = purchase_claim_message(claim_url, issued_at, digest)
            .map_err(|error| PurchaseError::Claim(error.to_string()))?;
        let signature = PurchaserSignature::new(format!(
            "ed25519:{}",
            URL_SAFE_NO_PAD.encode(self.0.sign(&message).as_ref())
        ))
        .unwrap_or_else(|| unreachable!("an Ed25519 signature is 64 bytes"));
        serde_json::from_value(serde_json::json!({
            "receipt": receipt,
            "claim_url": claim_url,
            "issued_at": issued_at,
            "signature": signature,
        }))
        .map_err(|error| PurchaseError::Claim(error.to_string()))
    }
}

/// What one purchase pays and where: one runner of a paid skill, bought at
/// runx's paid-run route for the given inputs.
pub struct PurchaseTerms<'a> {
    /// The paid-run route, as runx's challenge names it.
    pub resource_url: &'a str,
    pub runner: &'a str,
    pub inputs: &'a JsonObject,
    pub purchaser_key: &'a PurchaserKey,
    /// The purchase's idempotency key: the paid-run request and the payment
    /// share it, so a retry or resume reaches the same purchase.
    pub purchase_key: &'a str,
    /// The exact amount, when the buyer can compute it from the listing.
    pub amount_minor: Option<u64>,
    /// The most the buyer authorizes: the exact amount, or the offer's ceiling.
    pub max_amount_minor: u64,
    pub currency: &'a str,
}

/// The inputs the canonical `pay` skill takes to make this purchase.
#[must_use]
pub fn pay_inputs(terms: &PurchaseTerms<'_>) -> JsonObject {
    let mut payment_signal = serde_json::json!({
        "resource": {
            "url": terms.resource_url,
            "method": "POST",
            "body": {
                "runner": terms.runner,
                "inputs": terms.inputs,
                "purchaser_key": terms.purchaser_key,
            },
            "headers": {"idempotency-key": terms.purchase_key},
        },
        "amount_minor": terms.amount_minor.unwrap_or(terms.max_amount_minor),
    });
    if terms.amount_minor.is_none()
        && let Some(signal) = payment_signal.as_object_mut()
    {
        // Priced by a size only the seller's host knows: the authority caps it.
        signal.remove("amount_minor");
    }
    serde_json::from_value(serde_json::json!({
        "payment_signal": payment_signal,
        "parent_payment_authority": {
            "term_id": format!("purchase:{}", terms.purchase_key),
            "currency": terms.currency,
            "max_amount_minor": terms.max_amount_minor,
            "counterparty": terms.resource_url,
        },
        "idempotency_seed": terms.purchase_key,
    }))
    .unwrap_or_default()
}

/// Where a purchase stands for its buyer.
pub enum PurchaseState {
    /// Paid; the seller's run has not reached an outcome.
    Open,
    /// Fulfilment won: the result is the buyer's, and a claim may follow.
    Fulfilled,
    /// A refund won; the purchase names why.
    Refunded,
}

#[must_use]
pub fn purchase_state(purchase: &Purchase) -> PurchaseState {
    match purchase.invocation.outcome_gate {
        PaidInvocationOutcomeGate::FulfilmentWon if purchase.receipt.is_some() => {
            PurchaseState::Fulfilled
        }
        PaidInvocationOutcomeGate::RefundWon => PurchaseState::Refunded,
        _ => PurchaseState::Open,
    }
}

/// Claim a fulfilled purchase from its seller: send the claim this home's
/// purchaser key signs to the endpoint the purchase names, and return the
/// seller's answer. A purchase whose offer names no claim endpoint has none.
pub fn claim_purchase(
    transport: &(impl RuntimeHttpTransport + ?Sized),
    key: &PurchaserKeyPair,
    purchase: &Purchase,
    issued_at: &str,
) -> Result<Option<JsonValue>, PurchaseError> {
    let (Some(claim_url), Some(receipt)) = (&purchase.claim_url, &purchase.receipt) else {
        return Ok(None);
    };
    let claim = key.sign_claim(claim_url.as_str(), issued_at, receipt.clone())?;
    let body =
        serde_json::to_string(&claim).map_err(|error| PurchaseError::Claim(error.to_string()))?;
    let response = transport
        .send(RuntimeHttpRequest {
            method: HttpMethod::Post,
            url: claim_url.as_str().to_owned(),
            headers: vec![
                RuntimeHttpHeader::new("content-type", "application/json"),
                RuntimeHttpHeader::new("accept", "application/json"),
            ],
            body: Some(body),
        })
        .map_err(|error| PurchaseError::Transport(error.to_string()))?;
    if !(200..=299).contains(&response.status) {
        return Err(PurchaseError::ClaimRefused {
            status: response.status,
            body: response.body.chars().take(500).collect(),
        });
    }
    serde_json::from_str(&response.body)
        .map(Some)
        .map_err(|error| PurchaseError::Claim(format!("the seller's answer is not JSON: {error}")))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::paths::RUNX_HOME_ENV;

    fn home() -> Result<(tempfile::TempDir, RunxPaths), std::io::Error> {
        let root = tempfile::tempdir()?;
        let paths = RunxPaths::resolve(
            &BTreeMap::from([(RUNX_HOME_ENV.to_owned(), root.path().display().to_string())]),
            root.path(),
        );
        Ok((root, paths))
    }

    #[test]
    fn a_home_derives_one_stable_purchaser_key_per_purchase()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, paths) = home()?;
        let first = PurchaserKeyPair::for_purchase(&paths, "purchase-1")?.public_key();
        let again = PurchaserKeyPair::for_purchase(&paths, "purchase-1")?.public_key();
        let other = PurchaserKeyPair::for_purchase(&paths, "purchase-2")?.public_key();
        assert_eq!(first, again);
        assert_ne!(first, other);

        let (_elsewhere, other_home) = home()?;
        assert_ne!(
            PurchaserKeyPair::for_purchase(&other_home, "purchase-1")?.public_key(),
            first
        );
        Ok(())
    }

    #[test]
    fn a_claim_is_signed_over_its_endpoint_time_and_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, paths) = home()?;
        let key = PurchaserKeyPair::for_purchase(&paths, "purchase-1")?;
        let receipt: JsonObject = serde_json::from_value(serde_json::json!({
            "id": "sha256:receipt",
            "schema": "runx.hosted_paid_invocation_terminal_receipt.v1",
            "digest": "sha256:abc",
        }))?;
        let claim = key.sign_claim(
            "https://inbox.example.com/claim",
            "2026-10-15T00:00:00Z",
            receipt,
        )?;
        let message = purchase_claim_message(
            "https://inbox.example.com/claim",
            "2026-10-15T00:00:00Z",
            "sha256:abc",
        )?;
        let public =
            URL_SAFE_NO_PAD.decode(key.public_key().as_str().trim_start_matches("ed25519:"))?;
        let signature =
            URL_SAFE_NO_PAD.decode(claim.signature.as_str().trim_start_matches("ed25519:"))?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public)
            .verify(&message, &signature)
            .map_err(|_| "the claim signature does not verify")?;
        Ok(())
    }

    #[test]
    fn pay_inputs_buy_one_runner_at_the_paid_run_route() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, paths) = home()?;
        let key = PurchaserKeyPair::for_purchase(&paths, "purchase-1")?.public_key();
        let inputs: JsonObject =
            serde_json::from_value(serde_json::json!({"duration_seconds": 3600}))?;
        let terms = PurchaseTerms {
            resource_url: "https://api.runx.test/v1/skills/ausca/agent-inbox/run",
            runner: "create",
            inputs: &inputs,
            purchaser_key: &key,
            purchase_key: "purchase-1",
            amount_minor: Some(5),
            max_amount_minor: 5,
            currency: "USD",
        };
        let value = serde_json::to_value(pay_inputs(&terms))?;
        assert_eq!(
            value["payment_signal"]["resource"]["body"]["runner"],
            "create"
        );
        assert_eq!(
            value["payment_signal"]["resource"]["body"]["purchaser_key"],
            key.as_str()
        );
        assert_eq!(
            value["payment_signal"]["resource"]["headers"]["idempotency-key"],
            "purchase-1"
        );
        assert_eq!(value["payment_signal"]["amount_minor"], 5);
        assert_eq!(
            value["parent_payment_authority"]["counterparty"],
            terms.resource_url
        );
        assert_eq!(value["idempotency_seed"], "purchase-1");

        let capped = serde_json::to_value(pay_inputs(&PurchaseTerms {
            amount_minor: None,
            max_amount_minor: 525,
            ..terms
        }))?;
        assert!(capped["payment_signal"].get("amount_minor").is_none());
        assert_eq!(capped["parent_payment_authority"]["max_amount_minor"], 525);
        Ok(())
    }
}
