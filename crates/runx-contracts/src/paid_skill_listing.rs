//! Public, rail-neutral marketplace offer and listing contracts.
//!
//! A paid skill carries its own terms on the runner it sells (D10): running it
//! is the purchase, and the kernel pays. Author terms describe the price of an
//! ordinary skill runner. Registry listings bind those terms to immutable
//! package identity and an authenticated seller. Protocol adapters, provider
//! payloads, credentials, headers, settlement and SDKs stay outside this inert
//! boundary.

use std::collections::BTreeMap;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::paid_invocation::{
    CurrencyCode, EmbeddedOfferRevisionRef, OfferRevisionRef, PaidInvocationPresentation,
    PortableAmountMinor, PrincipalReference, SettlementFamilies, Sha256Digest,
};
use crate::schema::{NonEmptyString, RunxSchema};

pub const PAID_SKILL_LISTING_SCHEMA: &str = "runx.marketplace.paid_skill_listing.v1";

/// Provider-neutral commercial terms authored for one ordinary skill runner.
///
/// A fixed offer carries one exact price. A prepared offer is priced by its
/// inputs: a free, read-only `quote` runner in the same package names the
/// amount for the exact inputs the paid runner will receive, within the
/// offer's `amount_range`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(untagged)]
pub enum PaidSkillOfferTerms {
    Fixed(PaidSkillFixedOfferTerms),
    Prepared(PaidSkillPreparedOfferTerms),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillFixedOfferTerms {
    pub amount_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    pub input_schema_digest: Sha256Digest,
    pub output_schema_digest: Sha256Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PaidInvocationPresentation>,
}

/// The least and most a prepared offer's quote may name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillAmountRange {
    pub minimum_minor: PortableAmountMinor,
    pub maximum_minor: PortableAmountMinor,
}

impl PaidSkillAmountRange {
    pub fn new(
        minimum_minor: PortableAmountMinor,
        maximum_minor: PortableAmountMinor,
    ) -> Option<Self> {
        (minimum_minor <= maximum_minor).then_some(Self {
            minimum_minor,
            maximum_minor,
        })
    }

    pub fn contains(&self, amount: PortableAmountMinor) -> bool {
        (self.minimum_minor..=self.maximum_minor).contains(&amount)
    }
}

impl<'de> Deserialize<'de> for PaidSkillAmountRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireRange {
            minimum_minor: PortableAmountMinor,
            maximum_minor: PortableAmountMinor,
        }

        let value = WireRange::deserialize(deserializer)?;
        Self::new(value.minimum_minor, value.maximum_minor)
            .ok_or_else(|| de::Error::custom("amount range minimum must not exceed maximum"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillPreparedOfferTerms {
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    /// The free, read-only runner in this package that quotes the price.
    pub quote: NonEmptyString,
    pub amount_range: PaidSkillAmountRange,
    pub input_schema_digest: Sha256Digest,
    pub output_schema_digest: Sha256Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PaidInvocationPresentation>,
}

impl PaidSkillOfferTerms {
    pub fn input_schema_digest(&self) -> &Sha256Digest {
        match self {
            Self::Fixed(terms) => &terms.input_schema_digest,
            Self::Prepared(terms) => &terms.input_schema_digest,
        }
    }

    pub fn output_schema_digest(&self) -> &Sha256Digest {
        match self {
            Self::Fixed(terms) => &terms.output_schema_digest,
            Self::Prepared(terms) => &terms.output_schema_digest,
        }
    }
}

/// An exact execution package: the skill and runner, and the digests of the
/// package and closure that run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillExecutorBinding {
    pub skill: NonEmptyString,
    pub runner: NonEmptyString,
    pub package_digest: Sha256Digest,
    pub execution_closure_digest: Sha256Digest,
}

/// One immutable, registry-resolved runner offer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(untagged)]
pub enum PaidSkillRunnerOffer {
    Fixed(PaidSkillFixedRunnerOffer),
    Prepared(PaidSkillPreparedRunnerOffer),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillFixedRunnerOffer {
    pub offer_revision: EmbeddedOfferRevisionRef,
    pub amount_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PaidInvocationPresentation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillPreparedRunnerOffer {
    pub offer_revision: EmbeddedOfferRevisionRef,
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    pub quote: NonEmptyString,
    pub amount_range: PaidSkillAmountRange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PaidInvocationPresentation>,
}

impl PaidSkillRunnerOffer {
    /// The registry-resolved offer for author terms under `offer_revision`.
    #[must_use]
    pub fn from_terms(offer_revision: OfferRevisionRef, terms: &PaidSkillOfferTerms) -> Self {
        match terms {
            PaidSkillOfferTerms::Fixed(terms) => Self::Fixed(PaidSkillFixedRunnerOffer {
                offer_revision: offer_revision.into(),
                amount_minor: terms.amount_minor,
                currency: terms.currency.clone(),
                accepted_settlement_families: terms.accepted_settlement_families.clone(),
                presentation: terms.presentation.clone(),
            }),
            PaidSkillOfferTerms::Prepared(terms) => Self::Prepared(PaidSkillPreparedRunnerOffer {
                offer_revision: offer_revision.into(),
                currency: terms.currency.clone(),
                accepted_settlement_families: terms.accepted_settlement_families.clone(),
                quote: terms.quote.clone(),
                amount_range: terms.amount_range.clone(),
                presentation: terms.presentation.clone(),
            }),
        }
    }

    pub fn offer_revision(&self) -> &EmbeddedOfferRevisionRef {
        match self {
            Self::Fixed(offer) => &offer.offer_revision,
            Self::Prepared(offer) => &offer.offer_revision,
        }
    }

    pub fn currency(&self) -> &CurrencyCode {
        match self {
            Self::Fixed(offer) => &offer.currency,
            Self::Prepared(offer) => &offer.currency,
        }
    }

    pub fn accepted_settlement_families(&self) -> &SettlementFamilies {
        match self {
            Self::Fixed(offer) => &offer.accepted_settlement_families,
            Self::Prepared(offer) => &offer.accepted_settlement_families,
        }
    }
}

/// One to sixty-four runner offers keyed by unique runner identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PaidSkillOffers(BTreeMap<String, PaidSkillRunnerOffer>);

impl PaidSkillOffers {
    pub fn new(value: BTreeMap<String, PaidSkillRunnerOffer>) -> Option<Self> {
        let valid_names = value.keys().all(|runner| !runner.is_empty());
        (valid_names && (1..=64).contains(&value.len())).then_some(Self(value))
    }

    pub fn as_map(&self) -> &BTreeMap<String, PaidSkillRunnerOffer> {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PaidSkillOffers {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = BTreeMap::<String, PaidSkillRunnerOffer>::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            de::Error::custom("paid skill offers must contain 1..=64 non-empty runner names")
        })
    }
}

impl RunxSchema for PaidSkillOffers {
    fn json_schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": PaidSkillRunnerOffer::json_schema(),
            "propertyNames": { "minLength": 1 },
            "minProperties": 1,
            "maxProperties": 64,
        })
    }
}

/// Immutable marketplace identity and commercial terms for paid runners in one
/// published skill version. Seller identity is resolved by the authenticated
/// registry boundary and is never accepted from `run.yaml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.marketplace.paid_skill_listing.v1")]
pub struct PaidSkillListing {
    pub skill_id: NonEmptyString,
    pub version: NonEmptyString,
    pub skill_digest: Sha256Digest,
    pub profile_digest: Sha256Digest,
    pub package_digest: Sha256Digest,
    pub vendor_ref: PrincipalReference,
    pub offers: PaidSkillOffers,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_skill_listing_accepts_multiple_rails_without_rail_shapes()
    -> Result<(), serde_json::Error> {
        let listing: PaidSkillListing = serde_json::from_value(json!({
            "skill_id": "acme/transcribe",
            "version": "1.0.0",
            "skill_digest": format!("sha256:{}", "a".repeat(64)),
            "profile_digest": format!("sha256:{}", "b".repeat(64)),
            "package_digest": format!("sha256:{}", "c".repeat(64)),
            "vendor_ref": {"type": "principal", "uri": "runx:principal:acme"},
            "offers": {"transcribe": {
                "offer_revision": {
                    "offer_id": "acme/transcribe#transcribe",
                    "revision": "1.0.0",
                    "revision_digest": format!("sha256:{}", "b".repeat(64)),
                    "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
                    "output_schema_digest": format!("sha256:{}", "e".repeat(64))
                },
                "amount_minor": 125,
                "currency": "USD",
                "accepted_settlement_families": ["rail-a", "rail-b"]
            }}
        }))?;
        let amount_minor = match &listing.offers.as_map()["transcribe"] {
            PaidSkillRunnerOffer::Fixed(offer) => offer.amount_minor.get(),
            PaidSkillRunnerOffer::Prepared(_) => 0,
        };
        assert_eq!(amount_minor, 125);

        let schema = PaidSkillListing::json_schema().to_string();
        assert!(!schema.contains("PAYMENT-REQUIRED"));
        assert!(!schema.contains("facilitator"));
        Ok(())
    }

    #[test]
    fn paid_skill_listing_rejects_empty_runner_identity() {
        let offer = json!({
            "offer_revision": {
                "offer_id": "acme/transcribe#transcribe",
                "revision": "1.0.0",
                "revision_digest": format!("sha256:{}", "b".repeat(64)),
                "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
                "output_schema_digest": format!("sha256:{}", "e".repeat(64))
            },
            "amount_minor": 125,
            "currency": "USD",
            "accepted_settlement_families": ["rail-a"]
        });
        let result = serde_json::from_value::<PaidSkillListing>(json!({
            "skill_id": "acme/transcribe",
            "version": "1.0.0",
            "skill_digest": format!("sha256:{}", "a".repeat(64)),
            "profile_digest": format!("sha256:{}", "b".repeat(64)),
            "package_digest": format!("sha256:{}", "c".repeat(64)),
            "vendor_ref": {"type": "principal", "uri": "runx:principal:acme"},
            "offers": {"": offer}
        }));
        assert!(result.is_err());
    }

    fn revision() -> Value {
        json!({
            "offer_id": "acme/browser#create",
            "revision": "1.0.0",
            "revision_digest": format!("sha256:{}", "b".repeat(64)),
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64))
        })
    }

    #[test]
    fn a_prepared_offer_names_its_quote_runner_and_range() -> Result<(), serde_json::Error> {
        let terms: PaidSkillOfferTerms = serde_json::from_value(json!({
            "currency": "USD",
            "accepted_settlement_families": ["x402"],
            "quote": "price",
            "amount_range": {"minimum_minor": 5, "maximum_minor": 20},
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64))
        }))?;
        let PaidSkillOfferTerms::Prepared(prepared) = &terms else {
            panic!("a quote and range make a prepared offer");
        };
        assert_eq!(prepared.quote.as_str(), "price");
        assert!(
            prepared
                .amount_range
                .contains(PortableAmountMinor::new(20).unwrap())
        );
        assert!(
            !prepared
                .amount_range
                .contains(PortableAmountMinor::new(21).unwrap())
        );

        let offer = PaidSkillRunnerOffer::from_terms(
            serde_json::from_value::<OfferRevisionRef>(revision())?,
            &terms,
        );
        let PaidSkillRunnerOffer::Prepared(offer) = offer else {
            panic!("a prepared offer resolves prepared");
        };
        assert_eq!(offer.quote.as_str(), "price");
        assert!(
            serde_json::from_value::<PaidSkillAmountRange>(
                json!({"minimum_minor": 9, "maximum_minor": 2})
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn an_offer_names_no_wrapper_that_runs_it() {
        // Mediation and an executor were a wrapper's; a paid skill is bought
        // directly, so terms that name either are refused.
        for wrapper in [
            json!({"mediation": {"endpoint_url": "https://seller.example/quote"}}),
            json!({"executor": {"skill": "runx/marketplace-invoke", "runner": "invoke"}}),
        ] {
            let mut terms = json!({
                "amount_minor": 125,
                "currency": "USD",
                "accepted_settlement_families": ["x402"],
                "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
                "output_schema_digest": format!("sha256:{}", "e".repeat(64))
            });
            for (key, value) in wrapper.as_object().unwrap() {
                terms[key] = value.clone();
            }
            assert!(
                serde_json::from_value::<PaidSkillOfferTerms>(terms).is_err(),
                "{wrapper}"
            );
        }
    }
}
