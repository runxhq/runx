//! Public, rail-neutral marketplace offer and listing contracts.
//!
//! Author terms describe commercial requirements for an ordinary skill runner.
//! Registry listings bind those terms to immutable package identity and an
//! authenticated seller. Protocol adapters, provider payloads, credentials,
//! headers, settlement, and SDKs remain outside this inert V1 boundary.

use std::collections::BTreeMap;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::paid_invocation::{
    CurrencyCode, EmbeddedOfferRevisionRef, MediatedReceiptClass, MediationEndpointUrl,
    MediationListingRef, OfferRevisionRef, PaidInvocationMediation, PaidInvocationPresentation,
    PortableAmountMinor, PrincipalReference, SettlementFamilies, SettlementFamily, Sha256Digest,
};
use crate::schema::{NonEmptyString, RunxSchema};

pub const PAID_SKILL_LISTING_SCHEMA: &str = "runx.marketplace.paid_skill_listing.v1";

/// Provider-neutral commercial terms authored for one ordinary skill runner.
///
/// Fixed and prepared prices are distinct whole-contract variants. A fixed
/// offer always carries one exact buyer total. A prepared offer delegates only
/// the vendor amount to an immutable direct paid invocation; its platform fee
/// and admitted amount range remain fixed by the public listing.
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mediation: Option<PaidSkillMediationTerms>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor: Option<PaidSkillExecutorBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PaidInvocationPresentation>,
}

/// Seller-authored endpoint terms. Registry identity supplies `listing_ref`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillMediationTerms {
    pub endpoint_url: MediationEndpointUrl,
    pub vendor_offer_revision: EmbeddedOfferRevisionRef,
    pub vendor_package_digest: Sha256Digest,
    pub vendor_amount_minor: PortableAmountMinor,
    pub platform_fee_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub settlement_family: SettlementFamily,
    pub expected_receipt_class: MediatedReceiptClass,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillVendorAmountRange {
    pub minimum_minor: PortableAmountMinor,
    pub maximum_minor: PortableAmountMinor,
}

impl PaidSkillVendorAmountRange {
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

impl<'de> Deserialize<'de> for PaidSkillVendorAmountRange {
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
            .ok_or_else(|| de::Error::custom("vendor amount range minimum must not exceed maximum"))
    }
}

/// Seller-authored mediated terms whose vendor amount is resolved from a
/// previously prepared direct invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillPreparedMediationTerms {
    pub endpoint_url: MediationEndpointUrl,
    pub vendor_amount_range: PaidSkillVendorAmountRange,
    pub vendor_offer_revision: EmbeddedOfferRevisionRef,
    pub vendor_package_digest: Sha256Digest,
    pub platform_fee_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub settlement_family: SettlementFamily,
    pub expected_receipt_class: MediatedReceiptClass,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillPreparedOfferTerms {
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    pub input_schema_digest: Sha256Digest,
    pub output_schema_digest: Sha256Digest,
    pub mediation: PaidSkillPreparedMediationTerms,
    pub executor: PaidSkillExecutorBinding,
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

    pub fn mediation_and_executor_are_consistent(&self) -> bool {
        match self {
            Self::Fixed(terms) => terms.mediation.is_some() == terms.executor.is_some(),
            Self::Prepared(_) => true,
        }
    }
}

/// Exact public execution package selected by an endpoint listing.
///
/// The listing package remains inert commercial data. Hosted admission resolves
/// this binding and refuses package or closure drift before the paid invocation
/// is quoted.
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mediation: Option<PaidInvocationMediation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor: Option<PaidSkillExecutorBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PaidInvocationPresentation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillPreparedMediation {
    pub listing_ref: MediationListingRef,
    pub endpoint_url: MediationEndpointUrl,
    pub vendor_amount_range: PaidSkillVendorAmountRange,
    pub vendor_offer_revision: EmbeddedOfferRevisionRef,
    pub vendor_package_digest: Sha256Digest,
    pub platform_fee_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub settlement_family: SettlementFamily,
    pub expected_receipt_class: MediatedReceiptClass,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillPreparedRunnerOffer {
    pub offer_revision: EmbeddedOfferRevisionRef,
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    pub mediation: PaidSkillPreparedMediation,
    pub executor: PaidSkillExecutorBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PaidInvocationPresentation>,
}

impl PaidSkillRunnerOffer {
    pub fn from_terms(
        offer_revision: OfferRevisionRef,
        listing_ref: MediationListingRef,
        terms: &PaidSkillOfferTerms,
    ) -> Option<Self> {
        match terms {
            PaidSkillOfferTerms::Fixed(terms) => {
                if terms.mediation.is_some() != terms.executor.is_some() {
                    return None;
                }
                let mediation = terms
                    .mediation
                    .as_ref()
                    .map(|mediation| PaidInvocationMediation {
                        listing_ref,
                        endpoint_url: mediation.endpoint_url.clone(),
                        vendor_offer_revision: mediation.vendor_offer_revision.clone(),
                        vendor_package_digest: mediation.vendor_package_digest.clone(),
                        vendor_amount_minor: mediation.vendor_amount_minor,
                        platform_fee_minor: mediation.platform_fee_minor,
                        currency: mediation.currency.clone(),
                        settlement_family: mediation.settlement_family.clone(),
                        expected_receipt_class: mediation.expected_receipt_class,
                        prepared_price: None,
                    });
                if let Some(mediation) = &mediation {
                    let total = mediation
                        .vendor_amount_minor
                        .get()
                        .checked_add(mediation.platform_fee_minor.get())?;
                    if total != terms.amount_minor.get()
                        || mediation.currency != terms.currency
                        || !terms
                            .accepted_settlement_families
                            .as_slice()
                            .contains(&mediation.settlement_family)
                    {
                        return None;
                    }
                }
                Some(Self::Fixed(PaidSkillFixedRunnerOffer {
                    offer_revision: offer_revision.into(),
                    amount_minor: terms.amount_minor,
                    currency: terms.currency.clone(),
                    accepted_settlement_families: terms.accepted_settlement_families.clone(),
                    mediation,
                    executor: terms.executor.clone(),
                    presentation: terms.presentation.clone(),
                }))
            }
            PaidSkillOfferTerms::Prepared(terms) => {
                let mediation = &terms.mediation;
                if mediation.currency != terms.currency
                    || !terms
                        .accepted_settlement_families
                        .as_slice()
                        .contains(&mediation.settlement_family)
                {
                    return None;
                }
                Some(Self::Prepared(PaidSkillPreparedRunnerOffer {
                    offer_revision: offer_revision.into(),
                    currency: terms.currency.clone(),
                    accepted_settlement_families: terms.accepted_settlement_families.clone(),
                    mediation: PaidSkillPreparedMediation {
                        listing_ref,
                        endpoint_url: mediation.endpoint_url.clone(),
                        vendor_amount_range: mediation.vendor_amount_range.clone(),
                        vendor_offer_revision: mediation.vendor_offer_revision.clone(),
                        vendor_package_digest: mediation.vendor_package_digest.clone(),
                        platform_fee_minor: mediation.platform_fee_minor,
                        currency: mediation.currency.clone(),
                        settlement_family: mediation.settlement_family.clone(),
                        expected_receipt_class: mediation.expected_receipt_class,
                    },
                    executor: terms.executor.clone(),
                    presentation: terms.presentation.clone(),
                }))
            }
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

    #[test]
    fn mediated_offer_binds_listing_and_rejects_commercial_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        let terms: PaidSkillOfferTerms = serde_json::from_value(json!({
            "amount_minor": 125,
            "currency": "USD",
            "accepted_settlement_families": ["x402"],
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64)),
            "executor": {
                "skill": "marketplace-invoke",
                "runner": "invoke",
                "package_digest": format!("sha256:{}", "6".repeat(64)),
                "execution_closure_digest": format!("sha256:{}", "7".repeat(64))
            },
            "mediation": {
                "endpoint_url": "https://vendor.example/v1/invocations",
                "vendor_offer_revision": {
                    "offer_id": "vendor/transcribe",
                    "revision": "vendor-r1",
                    "revision_digest": format!("sha256:{}", "1".repeat(64)),
                    "input_schema_digest": format!("sha256:{}", "2".repeat(64)),
                    "output_schema_digest": format!("sha256:{}", "3".repeat(64))
                },
                "vendor_package_digest": format!("sha256:{}", "4".repeat(64)),
                "vendor_amount_minor": 100,
                "platform_fee_minor": 25,
                "currency": "USD",
                "settlement_family": "x402",
                "expected_receipt_class": "executed"
            }
        }))?;
        let revision: OfferRevisionRef = serde_json::from_value(json!({
            "offer_id": "acme/transcribe#transcribe",
            "revision": "1.0.0",
            "revision_digest": format!("sha256:{}", "b".repeat(64)),
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64))
        }))?;
        let listing_ref = MediationListingRef::new("runx:listing:acme/transcribe@1.0.0#transcribe")
            .ok_or("invalid test listing ref")?;
        let offer = PaidSkillRunnerOffer::from_terms(revision.clone(), listing_ref, &terms)
            .ok_or("valid mediation was refused")?;
        let PaidSkillRunnerOffer::Fixed(offer) = offer else {
            return Err("fixed terms produced a prepared offer".into());
        };
        assert_eq!(
            offer
                .mediation
                .as_ref()
                .map(|value| value.listing_ref.as_str()),
            Some("runx:listing:acme/transcribe@1.0.0#transcribe"),
        );
        assert_eq!(
            offer.executor.as_ref().map(|value| value.skill.as_str()),
            Some("marketplace-invoke")
        );

        let mut wrong_total = terms.clone();
        let PaidSkillOfferTerms::Fixed(wrong_total_terms) = &mut wrong_total else {
            return Err("fixed terms decoded as prepared pricing".into());
        };
        wrong_total_terms.amount_minor =
            PortableAmountMinor::new(126).ok_or("invalid test amount")?;
        assert!(
            PaidSkillRunnerOffer::from_terms(
                revision.clone(),
                MediationListingRef::new("runx:listing:acme/transcribe@1.0.0#transcribe")
                    .ok_or("invalid test listing ref")?,
                &wrong_total,
            )
            .is_none()
        );

        let mut wrong_rail = terms.clone();
        let PaidSkillOfferTerms::Fixed(wrong_rail_terms) = &mut wrong_rail else {
            return Err("fixed terms decoded as prepared pricing".into());
        };
        wrong_rail_terms.accepted_settlement_families =
            serde_json::from_value(json!(["other-rail"]))?;
        assert!(
            PaidSkillRunnerOffer::from_terms(
                revision.clone(),
                MediationListingRef::new("runx:listing:acme/transcribe@1.0.0#transcribe")
                    .ok_or("invalid test listing ref")?,
                &wrong_rail,
            )
            .is_none()
        );

        let mut missing_executor = terms;
        let PaidSkillOfferTerms::Fixed(missing_executor_terms) = &mut missing_executor else {
            return Err("fixed terms decoded as prepared pricing".into());
        };
        missing_executor_terms.executor = None;
        assert!(
            PaidSkillRunnerOffer::from_terms(
                revision,
                MediationListingRef::new("runx:listing:acme/transcribe@1.0.0#transcribe")
                    .ok_or("invalid test listing ref")?,
                &missing_executor,
            )
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn prepared_offer_binds_vendor_source_without_a_rail_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let terms: PaidSkillOfferTerms = serde_json::from_value(json!({
            "currency": "USD",
            "accepted_settlement_families": ["x402"],
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64)),
            "executor": {
                "skill": "marketplace-invoke",
                "runner": "invoke",
                "package_digest": format!("sha256:{}", "6".repeat(64)),
                "execution_closure_digest": format!("sha256:{}", "7".repeat(64))
            },
            "mediation": {
                "endpoint_url": "https://vendor.example/v1/invocations",
                "vendor_amount_range": {"minimum_minor": 30, "maximum_minor": 75},
                "vendor_offer_revision": {
                    "offer_id": "vendor/documents#analysis",
                    "revision": "analysis-r1",
                    "revision_digest": format!("sha256:{}", "1".repeat(64)),
                    "input_schema_digest": format!("sha256:{}", "2".repeat(64)),
                    "output_schema_digest": format!("sha256:{}", "3".repeat(64))
                },
                "vendor_package_digest": format!("sha256:{}", "4".repeat(64)),
                "platform_fee_minor": 5,
                "currency": "USD",
                "settlement_family": "x402",
                "expected_receipt_class": "executed"
            }
        }))?;
        let revision: OfferRevisionRef = serde_json::from_value(json!({
            "offer_id": "acme/document-analysis#invoke",
            "revision": "1.0.0",
            "revision_digest": format!("sha256:{}", "b".repeat(64)),
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64))
        }))?;
        let offer = PaidSkillRunnerOffer::from_terms(
            revision,
            MediationListingRef::new("runx:listing:acme/document-analysis@1.0.0#invoke")
                .ok_or("invalid test listing ref")?,
            &terms,
        )
        .ok_or("valid prepared mediation was refused")?;
        let PaidSkillRunnerOffer::Prepared(offer) = offer else {
            return Err("prepared terms produced a fixed offer".into());
        };
        assert_eq!(offer.mediation.vendor_amount_range.minimum_minor.get(), 30);
        assert_eq!(
            offer.mediation.listing_ref.as_str(),
            "runx:listing:acme/document-analysis@1.0.0#invoke"
        );

        let reversed = serde_json::from_value::<PaidSkillOfferTerms>(json!({
            "currency": "USD",
            "accepted_settlement_families": ["x402"],
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64)),
            "executor": {
                "skill": "marketplace-invoke",
                "runner": "invoke",
                "package_digest": format!("sha256:{}", "6".repeat(64)),
                "execution_closure_digest": format!("sha256:{}", "7".repeat(64))
            },
            "mediation": {
                "endpoint_url": "https://vendor.example/v1/invocations",
                "vendor_amount_range": {"minimum_minor": 76, "maximum_minor": 75},
                "vendor_offer_revision": {
                    "offer_id": "vendor/documents#analysis",
                    "revision": "analysis-r1",
                    "revision_digest": format!("sha256:{}", "1".repeat(64)),
                    "input_schema_digest": format!("sha256:{}", "2".repeat(64)),
                    "output_schema_digest": format!("sha256:{}", "3".repeat(64))
                },
                "vendor_package_digest": format!("sha256:{}", "4".repeat(64)),
                "platform_fee_minor": 5,
                "currency": "USD",
                "settlement_family": "x402",
                "expected_receipt_class": "executed"
            }
        }));
        assert!(reversed.is_err());
        Ok(())
    }
}
