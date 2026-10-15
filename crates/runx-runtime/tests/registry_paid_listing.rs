use runx_contracts::{JsonObject, PaidSkillPrice, PortableAmountMinor};
use runx_runtime::registry::{
    FileRegistryStore, IngestSkillOptions, RegistryPackageFile, RegistryPublisher,
    RegistryResolveOptions, ingest_skill_markdown, read_registry_skill, resolve_registry_skill,
};
use tempfile::tempdir;

const MARKDOWN: &str = r#"---
name: transcribe
description: Transcribe one supplied media input.
---
Transcribe one bounded input and return the declared output.
"#;
const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn profile(amount_minor: u64) -> String {
    format!(
        r#"skill: transcribe
marketplace:
  offers:
    transcribe:
      price: {{model: fixed, amount_minor: {amount_minor}}}
      currency: USD
      accepted_settlement_families: [x402, stripe-spt]
      input_schema_digest: {DIGEST_A}
      output_schema_digest: {DIGEST_B}
runners:
  preview:
    steps:
      - id: preview
        tool: {{ command: node, args: [scripts/transcribe.mjs, preview] }}
  transcribe:
    default: true
    steps:
      - id: transcribe
        tool: {{ command: node, args: [scripts/transcribe.mjs, transcribe] }}
"#
    )
}

fn options(profile_document: String) -> IngestSkillOptions {
    IngestSkillOptions {
        owner: Some("acme".to_owned()),
        profile_document: Some(profile_document),
        package_files: vec![RegistryPackageFile {
            path: "scripts/transcribe.mjs".to_owned(),
            content: "process.stdout.write('{}');\n".to_owned(),
        }],
        ..IngestSkillOptions::default()
    }
}

/// A transcription priced by its input: one amount per supported length.
fn priced_by_input_profile() -> String {
    profile(125)
        .replace(
            "      price: {model: fixed, amount_minor: 125}\n",
            "      price: {model: input_choice, selectors: [minutes], cases: [{when: [5], amount_minor: 50}, {when: [60], amount_minor: 500}]}\n",
        )
        .replace(
            "  transcribe:\n    default: true\n",
            "  transcribe:\n    default: true\n    inputs:\n      minutes: {type: integer, required: true, schema: {enum: [5, 60]}}\n",
        )
}

fn amount(value: u64) -> Result<PortableAmountMinor, Box<dyn std::error::Error>> {
    PortableAmountMinor::new(value).ok_or_else(|| "amount out of range".into())
}

#[test]
fn registry_paid_listing_is_resolved_once_and_returned_without_profile_reparsing()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let store = FileRegistryStore::new(temp.path());
    let version = ingest_skill_markdown(&store, MARKDOWN, options(profile(125)))?;
    let listing = version
        .paid_listing
        .as_ref()
        .ok_or("missing paid listing")?;

    assert_eq!(listing.skill_id.as_str(), "acme/transcribe");
    assert_eq!(listing.version.as_str(), version.version);
    assert_eq!(
        listing.vendor_ref.as_reference().uri.as_str(),
        "runx:principal:acme"
    );
    assert_eq!(listing.offers.as_map().len(), 1);
    let offer = &listing.offers.as_map()["transcribe"];
    assert_eq!(
        offer.price,
        PaidSkillPrice::Fixed {
            amount_minor: amount(125)?
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
    assert!(!listing.offers.as_map().contains_key("preview"));

    let read = store
        .get_version("acme/transcribe", Some(&version.version))?
        .ok_or("missing stored version")?;
    assert_eq!(read.paid_listing, version.paid_listing);
    let resolved = resolve_registry_skill(
        &store,
        "acme/transcribe",
        RegistryResolveOptions {
            version: Some(version.version.clone()),
            registry_url: None,
        },
    )?
    .ok_or("missing resolution")?;
    assert_eq!(resolved.paid_listing, version.paid_listing);
    let detail = read_registry_skill(&store, "acme/transcribe", Some(&version.version), None)?
        .ok_or("missing detail")?;
    assert_eq!(detail.paid_listing, version.paid_listing);
    Ok(())
}

#[test]
fn registry_paid_listing_price_change_revisions_profile_not_package()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let store = FileRegistryStore::new(temp.path());
    let first = ingest_skill_markdown(&store, MARKDOWN, options(profile(125)))?;
    let second = ingest_skill_markdown(&store, MARKDOWN, options(profile(250)))?;

    assert_ne!(first.version, second.version);
    assert_ne!(first.profile_digest, second.profile_digest);
    assert_eq!(first.package_digest, second.package_digest);
    assert_ne!(first.paid_listing, second.paid_listing);
    let second_offer = &second
        .paid_listing
        .as_ref()
        .ok_or("missing second listing")?
        .offers
        .as_map()["transcribe"];
    assert_eq!(
        second_offer.price.amount_minor(&JsonObject::new(), None),
        Ok(amount(250)?)
    );
    Ok(())
}

#[test]
fn registry_lists_an_offer_priced_by_its_input() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let store = FileRegistryStore::new(temp.path());
    let version = ingest_skill_markdown(&store, MARKDOWN, options(priced_by_input_profile()))?;
    let offer = &version
        .paid_listing
        .as_ref()
        .ok_or("missing paid listing")?
        .offers
        .as_map()["transcribe"];
    let inputs: JsonObject = serde_json::from_value(serde_json::json!({"minutes": 60}))?;
    assert_eq!(offer.price.amount_minor(&inputs, None), Ok(amount(500)?));
    Ok(())
}

#[test]
fn registry_paid_listing_cannot_change_seller_inside_one_version()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let store = FileRegistryStore::new(temp.path());
    let first = ingest_skill_markdown(&store, MARKDOWN, options(profile(125)))?;
    let mut changed_seller = options(profile(125));
    changed_seller.version = Some(first.version);
    changed_seller.publisher = Some(RegistryPublisher {
        kind: "publisher".to_owned(),
        id: "other-seller".to_owned(),
        handle: Some("other-seller".to_owned()),
        display_name: None,
    });

    let error = match ingest_skill_markdown(&store, MARKDOWN, changed_seller) {
        Ok(_) => {
            return Err("seller mutation unexpectedly replaced an immutable listing".into());
        }
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("already exists with a different digest")
    );
    Ok(())
}
