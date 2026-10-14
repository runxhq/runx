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

fn prepared(quote: &str) -> String {
    format!(
        r#"marketplace:
  offers:
    transcribe:
      currency: USD
      accepted_settlement_families: [x402]
      quote: {quote}
      amount_range: {{minimum_minor: 5, maximum_minor: 20}}
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}"#
    )
}

#[test]
fn a_prepared_offer_is_priced_by_a_free_runner_of_the_skill() -> Result<(), String> {
    let parsed = validate(&manifest(&prepared("preview")))?;
    let offer = &parsed
        .marketplace
        .ok_or_else(|| "missing marketplace".to_owned())?
        .offers["transcribe"];
    let PaidSkillOfferTerms::Prepared(offer) = offer else {
        return Err("a quote and range decoded as a fixed offer".to_owned());
    };
    assert_eq!(offer.quote.as_str(), "preview");
    assert_eq!(offer.amount_range.minimum_minor.get(), 5);
    assert_eq!(offer.amount_range.maximum_minor.get(), 20);
    Ok(())
}

#[test]
fn a_quote_names_another_runner_that_is_not_sold() -> Result<(), String> {
    for quote in ["missing", "transcribe"] {
        let Err(error) = validate(&manifest(&prepared(quote))) else {
            return Err(format!("quote {quote} unexpectedly passed"));
        };
        assert!(error.contains("must name another runner"), "{error}");
    }
    let sold_quote = prepared("preview").replace(
        "  offers:\n",
        &format!(
            "  offers:\n    preview:\n      amount_minor: 1\n      currency: USD\n      accepted_settlement_families: [x402]\n      input_schema_digest: {DIGEST_A}\n      output_schema_digest: {DIGEST_B}\n"
        ),
    );
    let Err(error) = validate(&manifest(&sold_quote)) else {
        return Err("a sold quote runner unexpectedly passed".to_owned());
    };
    assert!(error.contains("a quote is free"), "{error}");
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
fn an_offer_names_no_wrapper_that_runs_it() -> Result<(), String> {
    // A paid skill is bought directly: the terms a wrapper carried are not
    // part of an offer.
    for wrapper in [
        "      mediation: {endpoint_url: https://vendor.example/v1/invocations}",
        "      executor: {skill: marketplace-invoke, runner: invoke}",
    ] {
        let marketplace = format!(
            r#"marketplace:
  offers:
    transcribe:
      amount_minor: 125
      currency: USD
      accepted_settlement_families: [x402]
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}
{wrapper}"#
        );
        let Err(error) = validate(&manifest(&marketplace)) else {
            return Err(format!("{wrapper} unexpectedly passed"));
        };
        assert!(error.contains("is invalid"), "{error}");
    }
    Ok(())
}
