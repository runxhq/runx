use runx_contracts::PaidSkillOfferTerms;
use runx_parser::{SkillRunnerManifest, parse_runner_manifest_yaml, validate_runner_manifest};

const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn validate(yaml: &str) -> Result<SkillRunnerManifest, String> {
    let raw = parse_runner_manifest_yaml(yaml).map_err(|error| error.to_string())?;
    validate_runner_manifest(raw).map_err(|error| error.to_string())
}

fn manifest(marketplace: &str) -> String {
    format!(
        r#"
skill: transcribe
{marketplace}
runners:
  preview:
    steps:
      - id: preview
        tool: {{command: preview}}
  transcribe:
    default: true
    steps:
      - id: transcribe
        tool: {{command: transcribe}}
"#
    )
}

#[test]
fn marketplace_paid_and_free_runners_share_one_manifest() -> Result<(), String> {
    let parsed = validate(&manifest(&format!(
        r#"marketplace:
  offers:
    transcribe:
      amount_minor: 125
      currency: USD
      accepted_settlement_families: [x402, stripe-spt]
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}"#
    )))?;
    let offers = &parsed
        .marketplace
        .ok_or_else(|| "missing marketplace".to_owned())?
        .offers;
    assert_eq!(offers.len(), 1);
    let offer = offers
        .get("transcribe")
        .ok_or_else(|| "missing paid runner".to_owned())?;
    let PaidSkillOfferTerms::Fixed(offer) = offer else {
        return Err("fixed offer decoded as prepared pricing".to_owned());
    };
    assert_eq!(offer.amount_minor.get(), 125);
    assert_eq!(offer.currency.as_str(), "USD");
    assert_eq!(
        offer
            .accepted_settlement_families
            .as_slice()
            .iter()
            .map(|family| family.as_str())
            .collect::<Vec<_>>(),
        ["x402", "stripe-spt"]
    );
    assert!(!offers.contains_key("preview"));
    Ok(())
}

#[test]
fn missing_marketplace_keeps_all_runners_free() -> Result<(), String> {
    let parsed = validate(&manifest(""))?;
    assert!(parsed.marketplace.is_none());
    Ok(())
}

#[test]
fn marketplace_accepts_rail_neutral_mediated_endpoint_terms() -> Result<(), String> {
    let parsed = validate(&manifest(&format!(
        r#"marketplace:
  offers:
    transcribe:
      amount_minor: 125
      currency: USD
      accepted_settlement_families: [x402]
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}
      executor:
        skill: marketplace-invoke
        runner: invoke
        package_digest: {DIGEST_A}
        execution_closure_digest: {DIGEST_B}
      mediation:
        endpoint_url: https://vendor.example/v1/invocations
        vendor_offer_revision:
          offer_id: vendor/transcribe
          revision: vendor-r1
          revision_digest: {DIGEST_A}
          input_schema_digest: {DIGEST_A}
          output_schema_digest: {DIGEST_B}
        vendor_package_digest: {DIGEST_A}
        vendor_amount_minor: 100
        platform_fee_minor: 25
        currency: USD
        settlement_family: x402
        expected_receipt_class: executed"#
    )))?;
    let offer = &parsed
        .marketplace
        .ok_or_else(|| "missing marketplace".to_owned())?
        .offers["transcribe"];
    let PaidSkillOfferTerms::Fixed(offer) = offer else {
        return Err("fixed offer decoded as prepared pricing".to_owned());
    };
    let mediation = offer
        .mediation
        .as_ref()
        .ok_or_else(|| "missing mediation".to_owned())?;
    assert_eq!(
        mediation.endpoint_url.as_str(),
        "https://vendor.example/v1/invocations"
    );
    assert_eq!(mediation.vendor_amount_minor.get(), 100);
    assert_eq!(mediation.platform_fee_minor.get(), 25);
    assert_eq!(mediation.settlement_family.as_str(), "x402");
    Ok(())
}

#[test]
fn marketplace_rejects_undeclared_runner_and_unknown_rail_fields() -> Result<(), String> {
    let Err(undeclared) = validate(&manifest(&format!(
        r#"marketplace:
  offers:
    missing:
      amount_minor: 125
      currency: USD
      accepted_settlement_families: [x402]
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}"#
    ))) else {
        return Err("undeclared offer unexpectedly passed".to_owned());
    };
    assert!(undeclared.contains("references an undeclared runner"));

    let Err(provider_shape) = validate(&manifest(&format!(
        r#"marketplace:
  offers:
    transcribe:
      amount_minor: 125
      currency: USD
      accepted_settlement_families: [stripe-spt]
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}
      stripe_price_id: price_private"#
    ))) else {
        return Err("provider field unexpectedly passed".to_owned());
    };
    assert!(provider_shape.contains("unknown field `stripe_price_id`"));
    Ok(())
}

#[test]
fn marketplace_rejects_ambiguous_terms() -> Result<(), String> {
    for (field, value, expected) in [
        ("amount_minor", "0", "amount_minor"),
        ("currency", "usd", "currency"),
        (
            "accepted_settlement_families",
            "[x402, x402]",
            "settlement families",
        ),
    ] {
        let marketplace = format!(
            r#"marketplace:
  offers:
    transcribe:
      amount_minor: 125
      currency: USD
      accepted_settlement_families: [x402]
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}"#
        )
        .replace(
            &format!(
                "{field}: {}",
                match field {
                    "amount_minor" => "125",
                    "currency" => "USD",
                    _ => "[x402]",
                }
            ),
            &format!("{field}: {value}"),
        );
        let Err(error) = validate(&manifest(&marketplace)) else {
            return Err("invalid offer unexpectedly passed".to_owned());
        };
        assert!(error.contains(expected), "{error}");
    }
    Ok(())
}

#[test]
fn marketplace_rejects_partial_endpoint_execution_binding() -> Result<(), String> {
    let missing_executor = manifest(&format!(
        r#"marketplace:
  offers:
    transcribe:
      amount_minor: 125
      currency: USD
      accepted_settlement_families: [x402]
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}
      mediation:
        endpoint_url: https://vendor.example/v1/invocations
        vendor_offer_revision:
          offer_id: vendor/transcribe
          revision: vendor-r1
          revision_digest: {DIGEST_A}
          input_schema_digest: {DIGEST_A}
          output_schema_digest: {DIGEST_B}
        vendor_package_digest: {DIGEST_A}
        vendor_amount_minor: 100
        platform_fee_minor: 25
        currency: USD
        settlement_family: x402
        expected_receipt_class: executed"#,
    ));
    let result = validate(&missing_executor);
    assert!(matches!(
        &result,
        Err(error) if error.contains("must declare executor and mediation together")
    ));

    let missing_mediation = manifest(&format!(
        r#"marketplace:
  offers:
    transcribe:
      amount_minor: 125
      currency: USD
      accepted_settlement_families: [x402]
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}
      executor:
        skill: marketplace-invoke
        runner: invoke
        package_digest: {DIGEST_A}
        execution_closure_digest: {DIGEST_B}"#,
    ));
    let result = validate(&missing_mediation);
    assert!(matches!(
        &result,
        Err(error) if error.contains("must declare executor and mediation together")
    ));
    Ok(())
}
