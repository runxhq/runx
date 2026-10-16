use runx_contracts::{PaidSkillPrice, PortableAmountMinor};
use runx_parser::{SkillRunnerManifest, parse_runner_manifest_yaml, validate_runner_manifest};

const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn validate(yaml: &str) -> Result<SkillRunnerManifest, String> {
    let raw = parse_runner_manifest_yaml(yaml).map_err(|error| error.to_string())?;
    validate_runner_manifest(raw).map_err(|error| error.to_string())
}

/// A skill with a free `preview` runner and a `transcribe` runner whose inputs
/// a price may read: an enumerated duration, a free-text note, an optional
/// tier and a committed artifact.
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
    inputs:
      duration_seconds: {{type: integer, required: true, schema: {{enum: [600, 1800, 3600]}}}}
      language: {{type: string, required: true, schema: {{enum: [en, fr]}}}}
      note: {{type: string, required: true}}
      tier: {{type: string, schema: {{enum: [basic, plus]}}}}
      artifact: {{type: object, required: true}}
      envelope: {{type: object, required: true, artifact: true}}
    steps:
      - id: transcribe
        tool: {{command: transcribe}}
"#
    )
}

fn offer(price: &str) -> String {
    format!(
        r#"marketplace:
  offers:
    transcribe:
      price: {price}
      currency: USD
      accepted_settlement_families: [x402, stripe-spt]
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}"#
    )
}

fn price_of(parsed: SkillRunnerManifest) -> Result<PaidSkillPrice, String> {
    Ok(parsed
        .marketplace
        .ok_or_else(|| "missing marketplace".to_owned())?
        .offers
        .get("transcribe")
        .ok_or_else(|| "missing paid runner".to_owned())?
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
fn marketplace_paid_and_free_runners_share_one_manifest() -> Result<(), String> {
    let parsed = validate(&manifest(&offer("{model: fixed, amount_minor: 125}")))?;
    let offers = &parsed
        .marketplace
        .as_ref()
        .ok_or_else(|| "missing marketplace".to_owned())?
        .offers;
    assert_eq!(offers.len(), 1);
    let offer = &offers["transcribe"];
    assert_eq!(
        offer.price,
        PaidSkillPrice::Fixed {
            amount_minor: PortableAmountMinor::new(125).ok_or("amount")?
        }
    );
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
fn an_input_choice_price_covers_every_combination_of_its_enum_inputs() -> Result<(), String> {
    let price = price_of(validate(&manifest(&offer(
        r#"{model: input_choice, selectors: [duration_seconds, language], cases: [
        {when: [600, en], amount_minor: 5}, {when: [600, fr], amount_minor: 6},
        {when: [1800, en], amount_minor: 10}, {when: [1800, fr], amount_minor: 11},
        {when: [3600, en], amount_minor: 20}, {when: [3600, fr], amount_minor: 21}]}"#,
    )))?)?;
    assert!(matches!(price, PaidSkillPrice::InputChoice { .. }));

    let missing = refused(
        "{model: input_choice, selectors: [duration_seconds], cases: [
        {when: [600], amount_minor: 5}, {when: [1800], amount_minor: 10}]}",
    )?;
    assert!(
        missing.contains("prices 2 of the 3 combinations"),
        "{missing}"
    );

    let undeclared = refused(
        "{model: input_choice, selectors: [duration_seconds], cases: [
        {when: [600], amount_minor: 5}, {when: [1800], amount_minor: 10}, {when: [7200], amount_minor: 30}]}",
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
            "{model: input_choice, selectors: [missing], cases: [{when: [1], amount_minor: 1}]}",
            "reads input missing, which runner transcribe does not require",
        ),
        (
            "{model: input_choice, selectors: [tier], cases: [
            {when: [basic], amount_minor: 1}, {when: [plus], amount_minor: 2}]}",
            "reads input tier, which runner transcribe does not require",
        ),
        (
            "{model: input_choice, selectors: [note], cases: [{when: [hello], amount_minor: 1}]}",
            "so runner transcribe declares its enum",
        ),
        (
            "{model: artifact_bytes, artifact_input: note, base_amount_minor: 1, unit_bytes: 1024, unit_amount_minor: 1, max_bytes: 4096}",
            "so runner transcribe declares it a json or object input",
        ),
        (
            "{model: artifact_bytes, artifact_input: envelope, base_amount_minor: 1, unit_bytes: 1024, unit_amount_minor: 1, max_bytes: 4096}",
            "so runner transcribe declares it a json or object input",
        ),
    ] {
        let error = refused(price)?;
        assert!(error.contains(expected), "{error}");
    }

    let price = price_of(validate(&manifest(&offer(
        "{model: artifact_bytes, artifact_input: artifact, base_amount_minor: 25, unit_bytes: 1048576, unit_amount_minor: 5, max_bytes: 104857600}",
    )))?)?;
    assert_eq!(price.priced_inputs(), ["artifact"]);
    Ok(())
}

#[test]
fn a_prepared_offer_is_no_longer_accepted() -> Result<(), String> {
    let quoted = format!(
        r#"marketplace:
  offers:
    transcribe:
      currency: USD
      accepted_settlement_families: [x402]
      quote: preview
      amount_range: {{minimum_minor: 5, maximum_minor: 20}}
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}"#
    );
    let Err(error) = validate(&manifest(&quoted)) else {
        return Err("a quote runner and amount range unexpectedly passed".to_owned());
    };
    assert!(
        error.contains("marketplace.offers.transcribe is invalid"),
        "{error}"
    );
    Ok(())
}

#[test]
fn an_offer_names_the_held_resource_it_acts_on() -> Result<(), String> {
    let acting = offer("{model: fixed, amount_minor: 5}").replace(
        "      currency: USD\n",
        "      currency: USD\n      acts_on: {connector: ausca/inbox, credential: lease, target: {$concat: [\"ausca:agent-inbox/\", {$argument: /language}]}}\n",
    );
    let declared = format!(
        "{}\nconnectors:\n  ausca/inbox:\n    operations: [inbox.extend]",
        manifest(&acting)
    );
    let parsed = validate(&declared)?;
    let acts_on = parsed
        .marketplace
        .ok_or_else(|| "missing marketplace".to_owned())?
        .offers["transcribe"]
        .acts_on
        .clone()
        .ok_or_else(|| "missing acts_on".to_owned())?;
    assert_eq!(acts_on.credential.as_str(), "lease");

    let Err(error) = validate(&manifest(&acting)) else {
        return Err("acts_on an undeclared connector unexpectedly passed".to_owned());
    };
    assert!(
        error.contains("which the skill does not declare"),
        "{error}"
    );

    let unrequired = declared.replace("/language", "/tier");
    let Err(error) = validate(&unrequired) else {
        return Err("acts_on an unrequired input unexpectedly passed".to_owned());
    };
    assert!(error.contains("does not require"), "{error}");
    Ok(())
}

#[test]
fn marketplace_rejects_undeclared_runner_and_unknown_rail_fields() -> Result<(), String> {
    let undeclared =
        offer("{model: fixed, amount_minor: 125}").replace("    transcribe:\n", "    missing:\n");
    let Err(error) = validate(&manifest(&undeclared)) else {
        return Err("undeclared offer unexpectedly passed".to_owned());
    };
    assert!(error.contains("references an undeclared runner"));

    let provider_shape = format!(
        "{}\n      stripe_price_id: price_private",
        offer("{model: fixed, amount_minor: 125}")
    );
    let Err(error) = validate(&manifest(&provider_shape)) else {
        return Err("provider field unexpectedly passed".to_owned());
    };
    assert!(error.contains("unknown field `stripe_price_id`"), "{error}");
    Ok(())
}

#[test]
fn marketplace_rejects_ambiguous_terms() -> Result<(), String> {
    let base = offer("{model: fixed, amount_minor: 125}");
    for (from, to, expected) in [
        ("amount_minor: 125", "amount_minor: 0", "amount_minor"),
        ("currency: USD", "currency: usd", "currency"),
        (
            "accepted_settlement_families: [x402, stripe-spt]",
            "accepted_settlement_families: [x402, x402]",
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

#[test]
fn continuation_dependencies_name_other_declared_runners_once()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "skill: async-work\nrunners:\n  schedule:\n    continuations: [continue]\n    steps:\n      - id: scheduled\n        tool: data.digest\n        inputs: {value: waiting}\n  continue:\n    steps:\n      - id: observed\n        tool: data.digest\n        inputs: {value: ready}\n";
    let parsed = validate_runner_manifest(parse_runner_manifest_yaml(source)?)?;
    assert_eq!(parsed.runners["schedule"].continuations, ["continue"]);
    for invalid in ["[missing]", "[schedule]", "[continue, continue]"] {
        assert!(
            validate_runner_manifest(parse_runner_manifest_yaml(
                &source.replace("[continue]", invalid)
            )?)
            .is_err(),
            "{invalid}"
        );
    }
    Ok(())
}

#[test]
fn a_schedule_producer_must_declare_its_fulfilment_continuations()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "skill: async-work\nrunners:\n  schedule:\n    steps:\n      - id: schedule\n        tool: {module: schedule.mjs}\n        outputs: {schedule_intent: object}\n        artifacts:\n          named_emits: {schedule_intent: schedule_intent}\n          packets: {schedule_intent: runx.external_job_schedule_intent.v1}\n";
    let Err(error) = validate_runner_manifest(parse_runner_manifest_yaml(source)?) else {
        return Err("a schedule without a continuation was accepted".into());
    };
    assert!(
        error
            .to_string()
            .contains("must declare its continuation runners")
    );
    Ok(())
}
