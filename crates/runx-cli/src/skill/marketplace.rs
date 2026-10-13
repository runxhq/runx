// Module rationale: paid registry skills keep the ordinary `runx skill`
// command while a named runtime service owns the hosted HTTP boundary. The CLI
// validates and renders the x402 presentation; settlement remains an ordinary
// skill and never enters this router.
use std::collections::BTreeMap;

use runx_contracts::{JsonObject, JsonValue, X402_PAYMENT_REQUIRED_HEADER};
use runx_runtime::{HostedSkillChallenge, request_hosted_skill_challenge};
use runx_x402::payment_required_from_answer;

use super::resolver::ResolvedSkillRef;

pub(super) fn discover_paid_skill(
    resolved: &ResolvedSkillRef,
    requested_runner: Option<&str>,
    inputs: &JsonObject,
    env: &BTreeMap<String, String>,
) -> Result<JsonValue, String> {
    let listing = resolved
        .paid_listing
        .as_ref()
        .ok_or_else(|| "paid registry listing is unavailable".to_owned())?;
    let runner = selected_runner(listing, requested_runner)?;
    let base_url = resolved
        .hosted_registry_url
        .as_deref()
        .ok_or_else(|| "paid registry skill has no hosted execution surface".to_owned())?;
    let challenge = request_hosted_skill_challenge(
        base_url,
        listing.skill_id.as_str(),
        runner.as_str(),
        runx_runtime::hosted_private_network_allowed(false, env),
    )
    .map_err(|error| format!("marketplace discovery failed: {error}"))?;
    render_paid_skill_challenge(listing, runner, inputs, challenge)
}

fn render_paid_skill_challenge(
    listing: &runx_contracts::PaidSkillListing,
    runner: String,
    inputs: &JsonObject,
    challenge: HostedSkillChallenge,
) -> Result<JsonValue, String> {
    let HostedSkillChallenge { response, .. } = challenge;
    if response.status != 402 {
        return Err(format!(
            "marketplace discovery returned HTTP {}; expected 402",
            response.status
        ));
    }
    let header = response
        .headers
        .iter()
        .find(|header| {
            header
                .name
                .eq_ignore_ascii_case(X402_PAYMENT_REQUIRED_HEADER)
        })
        .map(|header| header.value.as_str());
    let body = payment_required_from_answer(&response.body, header)
        .map_err(|error| format!("marketplace challenge: {error}"))?;
    // The registry transport may be an internal origin behind a proxy. The
    // canonical paid resource is the one declared by the matching x402 header
    // and body, not the address used to fetch that challenge.
    let resource_url = body.resource.url.as_str().to_owned();
    let offer = listing
        .offers
        .as_map()
        .get(runner.as_str())
        .ok_or_else(|| format!("paid listing has no offer for runner '{runner}'"))?;
    let hosted_inputs = offer.hosted_inputs(inputs);

    serde_json::from_value(serde_json::json!({
        "status": "payment_required",
        "outcome": "deferred",
        "skill_name": listing.skill_id,
        "runner": runner,
        "result": {
            "payment_required": body,
            "resource": {
                "url": resource_url,
                "method": "POST",
                "body": {
                    "runner": runner,
                    "inputs": hosted_inputs,
                },
            },
            "requires_idempotency_key": true,
            "summary": "Hosted marketplace settlement is required before this skill can run."
        }
    }))
    .map_err(|error| format!("failed to render marketplace challenge: {error}"))
}

fn selected_runner(
    listing: &runx_contracts::PaidSkillListing,
    requested: Option<&str>,
) -> Result<String, String> {
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
    use runx_contracts::PaidSkillListing;
    use runx_runtime::{RuntimeHttpHeader, RuntimeHttpResponse};

    use super::*;

    #[test]
    fn paid_registry_skill_renders_the_exact_hosted_x402_challenge() -> Result<(), String> {
        let body = challenge_body("300000");
        let challenge = hosted_challenge(&body, &body)?;
        let listing = listing()?;
        let output = render_paid_skill_challenge(
            &listing,
            "invoke".to_owned(),
            &JsonObject::from([("document".to_owned(), JsonValue::String("ref".to_owned()))]),
            challenge,
        )?;

        let output: serde_json::Value =
            serde_json::to_value(output).map_err(|error| error.to_string())?;
        assert_eq!(output["status"], "payment_required");
        assert_eq!(output["runner"], "invoke");
        assert_eq!(output["result"]["payment_required"], body);
        assert_eq!(output["result"]["resource"]["url"], body["resource"]["url"]);
        assert_eq!(
            output["result"]["resource"]["body"]["inputs"],
            serde_json::json!({"document": "ref"})
        );
        assert!(output["result"]["resource"].get("headers").is_none());
        assert_eq!(output["result"]["requires_idempotency_key"], true);
        assert!(
            output["result"]["resource"]
                .get("requires_idempotency_key")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn mediated_paid_registry_skill_removes_admission_owned_inputs() -> Result<(), String> {
        let body = challenge_body("300000");
        let output = render_paid_skill_challenge(
            &mediated_listing()?,
            "invoke".to_owned(),
            &JsonObject::from([
                ("document".to_owned(), JsonValue::String("ref".to_owned())),
                (
                    "marketplace_offer".to_owned(),
                    JsonValue::Object(JsonObject::from([(
                        "listing_ref".to_owned(),
                        JsonValue::String("managed".to_owned()),
                    )])),
                ),
                (
                    "idempotency_seed".to_owned(),
                    JsonValue::String("managed".to_owned()),
                ),
            ]),
            hosted_challenge(&body, &body)?,
        )?;

        let output: serde_json::Value =
            serde_json::to_value(output).map_err(|error| error.to_string())?;
        assert_eq!(
            output["result"]["resource"]["body"]["inputs"],
            serde_json::json!({"document": "ref"})
        );
        Ok(())
    }

    #[test]
    fn paid_registry_skill_rejects_different_header_and_body_challenges() -> Result<(), String> {
        let body = challenge_body("300000");
        let header = challenge_body("400000");

        let result = render_paid_skill_challenge(
            &listing()?,
            "invoke".to_owned(),
            &JsonObject::new(),
            hosted_challenge(&body, &header)?,
        );

        assert!(matches!(
            &result,
            Err(error) if error.contains("header and body are different challenges")
        ));
        Ok(())
    }

    fn hosted_challenge(
        body: &serde_json::Value,
        header: &serde_json::Value,
    ) -> Result<HostedSkillChallenge, String> {
        let header: runx_contracts::X402PaymentRequired =
            serde_json::from_value(header.clone()).map_err(|error| error.to_string())?;
        let encoded = runx_x402::encode_payment_required_header(&header)
            .map_err(|error| error.to_string())?;
        let mut response = RuntimeHttpResponse::new(402, body.to_string());
        response.headers = vec![RuntimeHttpHeader::new(
            X402_PAYMENT_REQUIRED_HEADER,
            encoded,
        )];
        Ok(HostedSkillChallenge {
            resource_url: "https://registry.internal.test/v1/skills/ausca/document-ocr/run"
                .to_owned(),
            response,
        })
    }

    fn challenge_body(amount: &str) -> serde_json::Value {
        serde_json::json!({
            "x402Version": 2,
            "resource": {
                "url": "https://api.runx.test/v1/skills/ausca/document-ocr/run",
                "description": "OCR",
                "mimeType": "application/json"
            },
            "accepts": [{
                "scheme": "exact",
                "network": "eip155:8453",
                "amount": amount,
                "asset": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
                "payTo": "0x26572ff23c6c52bfb1a69cb0c9114a8be443b422",
                "maxTimeoutSeconds": 300
            }]
        })
    }

    fn listing() -> Result<PaidSkillListing, String> {
        serde_json::from_value(serde_json::json!({
            "skill_id": "ausca/document-ocr",
            "version": "0.1.0",
            "skill_digest": format!("sha256:{}", "a".repeat(64)),
            "profile_digest": format!("sha256:{}", "b".repeat(64)),
            "package_digest": format!("sha256:{}", "c".repeat(64)),
            "vendor_ref": { "type": "principal", "uri": "runx:principal:ausca" },
            "offers": {
                "invoke": {
                    "amount_minor": 30,
                    "currency": "USD",
                    "accepted_settlement_families": ["x402"],
                    "offer_revision": {
                        "offer_id": "ausca/document-ocr#invoke",
                        "revision": "0.1.0",
                        "revision_digest": format!("sha256:{}", "d".repeat(64)),
                        "input_schema_digest": format!("sha256:{}", "e".repeat(64)),
                        "output_schema_digest": format!("sha256:{}", "f".repeat(64))
                    }
                }
            }
        }))
        .map_err(|error| error.to_string())
    }

    fn mediated_listing() -> Result<PaidSkillListing, String> {
        serde_json::from_value(serde_json::json!({
            "skill_id": "ausca/document-ocr",
            "version": "0.1.0",
            "skill_digest": format!("sha256:{}", "a".repeat(64)),
            "profile_digest": format!("sha256:{}", "b".repeat(64)),
            "package_digest": format!("sha256:{}", "c".repeat(64)),
            "vendor_ref": { "type": "principal", "uri": "runx:principal:ausca" },
            "offers": {
                "invoke": {
                    "amount_minor": 30,
                    "currency": "USD",
                    "accepted_settlement_families": ["x402"],
                    "offer_revision": {
                        "offer_id": "ausca/document-ocr#invoke",
                        "revision": "0.1.0",
                        "revision_digest": format!("sha256:{}", "d".repeat(64)),
                        "input_schema_digest": format!("sha256:{}", "e".repeat(64)),
                        "output_schema_digest": format!("sha256:{}", "f".repeat(64))
                    },
                    "mediation": {
                        "listing_ref": "runx:listing:ausca/document-ocr@0.1.0#invoke",
                        "endpoint_url": "https://ausca.test/v1/extract-text",
                        "vendor_offer_revision": {
                            "offer_id": "document.ocr",
                            "revision": "ocr-r1",
                            "revision_digest": format!("sha256:{}", "1".repeat(64)),
                            "input_schema_digest": format!("sha256:{}", "2".repeat(64)),
                            "output_schema_digest": format!("sha256:{}", "3".repeat(64))
                        },
                        "vendor_package_digest": format!("sha256:{}", "4".repeat(64)),
                        "vendor_amount_minor": 25,
                        "platform_fee_minor": 5,
                        "currency": "USD",
                        "settlement_family": "x402",
                        "expected_receipt_class": "executed"
                    },
                    "executor": {
                        "skill": "marketplace-invoke",
                        "runner": "invoke",
                        "package_digest": format!("sha256:{}", "5".repeat(64)),
                        "execution_closure_digest": format!("sha256:{}", "6".repeat(64))
                    }
                }
            }
        }))
        .map_err(|error| error.to_string())
    }
}
