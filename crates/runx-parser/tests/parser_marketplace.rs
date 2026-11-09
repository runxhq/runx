use runx_contracts::{PaidSkillPrice, PortableAmountMinor};
use runx_parser::{SkillMethodManifest, parse_method_manifest_yaml, validate_method_manifest};

const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn validate(yaml: &str) -> Result<SkillMethodManifest, String> {
    let raw = parse_method_manifest_yaml(yaml).map_err(|error| error.to_string())?;
    validate_method_manifest("transcribe", raw).map_err(|error| error.to_string())
}

/// A skill with a free `preview` method and a `transcribe` method whose
/// inputs a price may read: an enumerated duration, a free-text note, an
/// optional tier and a committed artifact. `offer` is the `transcribe`
/// method's offer, and `steps` follow its own.
fn manifest_with(offer: &str, steps: &str) -> String {
    format!(
        r#"
minimum_runx: 0.12.0
default_method: transcribe
methods:
  preview:
    steps:
      - id: preview
        tool: {{command: preview}}
  transcribe:
    inputs:
      duration_seconds: {{type: integer, enum: [600, 1800, 3600]}}
      language: {{type: string, enum: [en, fr]}}
      note: {{type: string}}
      tier: {{type: string, enum: [basic, plus], optional: true}}
      artifact: {{type: object}}
{offer}
    steps:
      - id: transcribe
        tool: {{command: transcribe}}
{steps}"#
    )
}

fn manifest(offer: &str) -> String {
    manifest_with(offer, "")
}

fn offer(price: &str) -> String {
    format!(
        r#"    offer:
      price: {price}
      currency: USD
      accepts: [x402, stripe-spt]"#
    )
}

fn price_of(parsed: SkillMethodManifest) -> Result<PaidSkillPrice, String> {
    Ok(parsed.methods["transcribe"]
        .offer
        .as_ref()
        .ok_or_else(|| "missing paid method".to_owned())?
        .price
        .clone())
}

fn refused(price: &str) -> Result<String, String> {
    match validate(&manifest(&offer(price))) {
        Ok(_) => Err(format!("price {price} unexpectedly passed")),
        Err(error) => Ok(error),
    }
}

#[test]
fn marketplace_paid_and_free_methods_share_one_manifest() -> Result<(), String> {
    let parsed = validate(&manifest(&offer("{basis: fixed, amount_minor: 125}")))?;
    let offer = parsed.methods["transcribe"]
        .offer
        .as_ref()
        .ok_or_else(|| "missing paid method".to_owned())?;
    assert_eq!(
        offer.price,
        PaidSkillPrice::Fixed {
            amount_minor: PortableAmountMinor::new(125).ok_or("amount")?
        }
    );
    assert_eq!(offer.currency.as_str(), "USD");
    assert_eq!(
        serde_json::to_value(&offer.accepts).map_err(|error| error.to_string())?,
        serde_json::json!(["x402", "stripe-spt"])
    );
    assert!(parsed.methods["preview"].offer.is_none());
    Ok(())
}

#[test]
fn missing_marketplace_keeps_all_methods_free() -> Result<(), String> {
    let parsed = validate(&manifest(""))?;
    assert!(parsed.methods.values().all(|method| method.offer.is_none()));
    Ok(())
}

#[test]
fn an_input_choice_price_covers_every_combination_of_its_enum_inputs() -> Result<(), String> {
    let price = price_of(validate(&manifest(&offer(
        r#"{basis: by_input, cases: [
        {inputs: {duration_seconds: 600, language: en}, amount_minor: 5}, {inputs: {duration_seconds: 600, language: fr}, amount_minor: 6},
        {inputs: {duration_seconds: 1800, language: en}, amount_minor: 10}, {inputs: {duration_seconds: 1800, language: fr}, amount_minor: 11},
        {inputs: {duration_seconds: 3600, language: en}, amount_minor: 20}, {inputs: {duration_seconds: 3600, language: fr}, amount_minor: 21}]}"#,
    )))?)?;
    assert!(matches!(price, PaidSkillPrice::ByInput { .. }));

    let missing = refused(
        "{basis: by_input, cases: [
        {inputs: {duration_seconds: 600}, amount_minor: 5}, {inputs: {duration_seconds: 1800}, amount_minor: 10}]}",
    )?;
    assert!(
        missing.contains("prices 2 of the 3 combinations"),
        "{missing}"
    );

    let undeclared = refused(
        "{basis: by_input, cases: [
        {inputs: {duration_seconds: 600}, amount_minor: 5}, {inputs: {duration_seconds: 1800}, amount_minor: 10}, {inputs: {duration_seconds: 7200}, amount_minor: 30}]}",
    )?;
    assert!(
        undeclared.contains("not one of its enum values"),
        "{undeclared}"
    );
    Ok(())
}

#[test]
fn a_price_reads_only_required_inputs_of_the_right_kind() -> Result<(), String> {
    for (price, expected) in [
        (
            "{basis: by_input, cases: [{inputs: {missing: 1}, amount_minor: 1}]}",
            "reads input missing, which method transcribe does not require",
        ),
        (
            "{basis: by_input, cases: [
            {inputs: {tier: basic}, amount_minor: 1}, {inputs: {tier: plus}, amount_minor: 2}]}",
            "reads input tier, which method transcribe does not require",
        ),
        (
            "{basis: by_input, cases: [{inputs: {note: hello}, amount_minor: 1}]}",
            "so method transcribe declares its enum",
        ),
        (
            "{basis: by_size, artifact: {$argument: /note/artifact_uri}, base_amount_minor: 1, unit_bytes: 1024, unit_amount_minor: 1, max_bytes: 4096}",
            "so method transcribe declares it an object or leaves its type open",
        ),
        (
            "{basis: by_size, artifact: {$argument: /missing/artifact_uri}, base_amount_minor: 1, unit_bytes: 1024, unit_amount_minor: 1, max_bytes: 4096}",
            "reads input missing, which method transcribe does not require",
        ),
    ] {
        let error = refused(price)?;
        assert!(error.contains(expected), "{error}");
    }

    let price = price_of(validate(&manifest(&offer(
        "{basis: by_size, artifact: {$argument: /artifact/artifact_uri}, base_amount_minor: 25, unit_bytes: 1048576, unit_amount_minor: 5, max_bytes: 104857600}",
    )))?)?;
    assert_eq!(price.priced_inputs(), ["artifact"]);
    Ok(())
}

#[test]
fn a_prepared_offer_is_no_longer_accepted() -> Result<(), String> {
    let quoted = r#"    offer:
      currency: USD
      accepts: [x402]
      quote: preview
      amount_range: {minimum_minor: 5, maximum_minor: 20}"#;
    let Err(error) = validate(&manifest(quoted)) else {
        return Err("a quote method and amount range unexpectedly passed".to_owned());
    };
    assert!(
        error.contains("methods.transcribe.offer is invalid"),
        "{error}"
    );
    Ok(())
}

#[test]
fn an_offer_names_the_leased_resource_it_acts_on() -> Result<(), String> {
    let acting = offer("{basis: fixed, amount_minor: 5}").replace(
        "      currency: USD\n",
        "      currency: USD\n      requires_lease: {connector: ausca/inbox, credential: lease, target: {$concat: [\"ausca:agent-inbox/\", {$argument: /language}]}}\n",
    );
    // The lease is the buyer's in one of the package's own connectors: one a
    // step calls.
    let extend =
        "      - id: extend\n        operation: ausca/inbox/inbox.extend\n        inputs: {}\n";
    let declared = manifest_with(&acting, extend);
    let parsed = validate(&declared)?;
    let lease = parsed.methods["transcribe"]
        .offer
        .as_ref()
        .and_then(|offer| offer.requires_lease.clone())
        .ok_or_else(|| "missing requires_lease".to_owned())?;
    assert_eq!(lease.credential.as_str(), "lease");

    let Err(error) = validate(&manifest(&acting)) else {
        return Err("a lease in a connector no step calls unexpectedly passed".to_owned());
    };
    assert!(
        error.contains("which no step of the package calls"),
        "{error}"
    );

    let unrequired = declared.replace("/language", "/tier");
    let Err(error) = validate(&unrequired) else {
        return Err("requires_lease on an unrequired input unexpectedly passed".to_owned());
    };
    assert!(error.contains("does not require"), "{error}");
    Ok(())
}

#[test]
fn marketplace_rejects_typed_digests_and_unknown_rail_fields() -> Result<(), String> {
    // The registry computes the method's schema digests when it's published,
    // so an offer never types them.
    let typed = format!(
        "{}\n      input_schema_digest: {DIGEST_A}",
        offer("{basis: fixed, amount_minor: 125}")
    );
    let Err(error) = validate(&manifest(&typed)) else {
        return Err("a typed schema digest unexpectedly passed".to_owned());
    };
    assert!(
        error.contains("unknown field `input_schema_digest`"),
        "{error}"
    );

    let provider_shape = format!(
        "{}\n      stripe_price_id: price_private",
        offer("{basis: fixed, amount_minor: 125}")
    );
    let Err(error) = validate(&manifest(&provider_shape)) else {
        return Err("provider field unexpectedly passed".to_owned());
    };
    assert!(error.contains("unknown field `stripe_price_id`"), "{error}");
    Ok(())
}

#[test]
fn marketplace_rejects_ambiguous_terms() -> Result<(), String> {
    let base = offer("{basis: fixed, amount_minor: 125}");
    for (from, to, expected) in [
        ("amount_minor: 125", "amount_minor: 0", "amount_minor"),
        ("currency: USD", "currency: usd", "currency"),
        (
            "accepts: [x402, stripe-spt]",
            "accepts: [x402, x402]",
            "settlement families",
        ),
    ] {
        let Err(error) = validate(&manifest(&base.replace(from, to))) else {
            return Err(format!("{to} unexpectedly passed"));
        };
        assert!(error.contains(expected), "{error}");
    }
    Ok(())
}
