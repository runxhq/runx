//! Public, rail-neutral marketplace offer and listing contracts.
//!
//! A paid skill carries its own terms on the method it sells (D10): running it
//! is the purchase, and the kernel pays. Author terms describe the price of an
//! ordinary skill method. Registry listings bind those terms to immutable
//! package identity and an authenticated seller. Protocol adapters, provider
//! payloads, credentials, headers, settlement and SDKs stay outside this inert
//! boundary.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::canonical_json::canonical_stable_json;
use crate::connector::{CredentialName, Template};
use crate::purchase::{
    CurrencyCode, EmbeddedOfferVersionBinding, PortableAmountMinor, PrincipalReference,
    SettlementFamilies,
};
use crate::schema::{NamedSchemas, NonEmptyString, RunxSchema, record_named};
use crate::{Digest, JsonObject, JsonValue, MAX_PORTABLE_INTEGER};

pub const PAID_SKILL_LISTING_SCHEMA: &str = "runx.marketplace.paid_skill_listing.v1";

const MAX_PRICED_INPUTS: usize = 4;
const MAX_PRICE_CASES: usize = 256;

/// How an offer prices one call: from the listing and the inputs the method
/// receives, never from the method. Cloud and the buyer's kernel compute the
/// same amount, so nothing the buyer or the seller's method says can change it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, RunxSchema)]
#[serde(tag = "basis", rename_all = "snake_case", deny_unknown_fields)]
pub enum PaidSkillPrice {
    /// One amount for every call.
    Fixed { amount_minor: PortableAmountMinor },
    /// An amount for each combination of values of the priced inputs, each
    /// an `enum` input the method requires. Each case names every priced
    /// input by name, and the cases cover every combination exactly once.
    ByInput { cases: Vec<PaidSkillPriceCase> },
    /// A base amount plus a unit amount for each started `unit_bytes` of the
    /// committed artifact the `input` holds, up to `max_bytes`.
    BySize {
        input: NonEmptyString,
        base_amount_minor: PortableAmountMinor,
        unit_bytes: std::num::NonZeroU64,
        unit_amount_minor: PortableAmountMinor,
        max_bytes: std::num::NonZeroU64,
    },
}

/// One `by_input` case: the value each priced input takes, by name, and what
/// a call with them costs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillPriceCase {
    pub inputs: BTreeMap<String, PriceChoice>,
    pub amount_minor: PortableAmountMinor,
}

/// A selector value: a string, a portable integer or a boolean. A case
/// matches an input that is the same JSON value, as JavaScript compares it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct PriceChoice(JsonValue);

impl Eq for PriceChoice {}

impl PriceChoice {
    pub fn new(value: JsonValue) -> Option<Self> {
        let portable = match &value {
            JsonValue::String(_) | JsonValue::Bool(_) => true,
            JsonValue::Number(number) => number.as_f64().is_some_and(|number| {
                number.fract() == 0.0 && number.abs() <= MAX_PORTABLE_INTEGER as f64
            }),
            _ => false,
        };
        portable.then_some(Self(value))
    }

    pub fn as_value(&self) -> &JsonValue {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PriceChoice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(JsonValue::deserialize(deserializer)?).ok_or_else(|| {
            de::Error::custom("a price choice is a string, a portable integer or a boolean")
        })
    }
}

impl RunxSchema for PriceChoice {
    fn json_schema() -> Value {
        json!({
            "oneOf": [
                { "type": "string" },
                {
                    "type": "integer",
                    "minimum": -(MAX_PORTABLE_INTEGER as i64),
                    "maximum": MAX_PORTABLE_INTEGER,
                },
                { "type": "boolean" },
            ],
            "title": "PriceChoice",
        })
    }

    fn named_schemas(named: &mut NamedSchemas) {
        record_named(named, "PriceChoice", Self::json_schema);
    }
}

impl<'de> Deserialize<'de> for PaidSkillPrice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "basis", rename_all = "snake_case", deny_unknown_fields)]
        enum WirePrice {
            Fixed {
                amount_minor: PortableAmountMinor,
            },
            ByInput {
                cases: Vec<PaidSkillPriceCase>,
            },
            BySize {
                input: NonEmptyString,
                base_amount_minor: PortableAmountMinor,
                unit_bytes: std::num::NonZeroU64,
                unit_amount_minor: PortableAmountMinor,
                max_bytes: std::num::NonZeroU64,
            },
        }

        let price = match WirePrice::deserialize(deserializer)? {
            WirePrice::Fixed { amount_minor } => Self::Fixed { amount_minor },
            WirePrice::ByInput { cases } => Self::ByInput { cases },
            WirePrice::BySize {
                input,
                base_amount_minor,
                unit_bytes,
                unit_amount_minor,
                max_bytes,
            } => Self::BySize {
                input,
                base_amount_minor,
                unit_bytes,
                unit_amount_minor,
                max_bytes,
            },
        };
        price.validate().map_err(de::Error::custom)?;
        Ok(price)
    }
}

/// Why a price cannot be stated or computed.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PriceError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("priced input {0} is missing")]
    MissingInput(String),
    #[error("no price case matches these inputs")]
    NoCase,
    #[error("the priced artifact's committed size is unknown")]
    ArtifactSizeUnknown,
    #[error("the priced artifact is empty")]
    ArtifactEmpty,
    #[error("the priced artifact is {size_bytes} bytes, over the offer's {max_bytes}")]
    ArtifactTooLarge { size_bytes: u64, max_bytes: u64 },
}

impl PriceError {
    /// The stable name of this refusal, the same in every implementation.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid",
            Self::MissingInput(_) => "missing_input",
            Self::NoCase => "no_case",
            Self::ArtifactSizeUnknown => "artifact_size_unknown",
            Self::ArtifactEmpty => "artifact_empty",
            Self::ArtifactTooLarge { .. } => "artifact_too_large",
        }
    }
}

impl PaidSkillPrice {
    /// The price's own invariants: every case names the same one to four
    /// priced inputs, each combination of their values has one case, and a
    /// size price's largest amount is still a portable integer.
    pub fn validate(&self) -> Result<(), PriceError> {
        match self {
            Self::Fixed { .. } => Ok(()),
            Self::ByInput { cases } => {
                if cases.is_empty() || cases.len() > MAX_PRICE_CASES {
                    return Err(PriceError::Invalid("a by_input price has one to 256 cases"));
                }
                let priced = cases[0].inputs.keys().collect::<Vec<_>>();
                if priced.is_empty() || priced.len() > MAX_PRICED_INPUTS {
                    return Err(PriceError::Invalid(
                        "a by_input price reads one to four inputs",
                    ));
                }
                let mut seen = BTreeSet::new();
                for case in cases {
                    if !case.inputs.keys().eq(priced.iter().copied()) {
                        return Err(PriceError::Invalid(
                            "each price case names the same priced inputs",
                        ));
                    }
                    let key = canonical_stable_json(&case.inputs).map_err(|_| {
                        PriceError::Invalid("a price case cannot be written as JSON")
                    })?;
                    if !seen.insert(key) {
                        return Err(PriceError::Invalid(
                            "each combination of priced input values has one case",
                        ));
                    }
                }
                Ok(())
            }
            Self::BySize { max_bytes, .. } => self
                .artifact_amount(max_bytes.get())
                .map(|_| ())
                .ok_or(PriceError::Invalid(
                    "a by_size price must stay a portable amount up to max_bytes",
                )),
        }
    }

    /// The input names this price reads.
    pub fn priced_inputs(&self) -> Vec<&str> {
        match self {
            Self::Fixed { .. } => Vec::new(),
            Self::ByInput { cases } => cases
                .first()
                .map(|case| case.inputs.keys().map(String::as_str).collect())
                .unwrap_or_default(),
            Self::BySize { input, .. } => vec![input.as_str()],
        }
    }

    /// The stored artifact a `by_size` price reads: the `artifact_uri` its
    /// input holds. Its committed size, looked up by the caller, is what the
    /// call costs.
    pub fn priced_artifact_ref<'a>(&self, inputs: &'a JsonObject) -> Option<&'a str> {
        let Self::BySize { input, .. } = self else {
            return None;
        };
        inputs
            .get(input.as_str())?
            .as_object()?
            .get("artifact_uri")?
            .as_str()
            .filter(|reference| stored_artifact_ref(reference))
    }

    /// What one call with `inputs` costs. A `by_size` price reads the
    /// committed size of the artifact its input names, which the caller looks
    /// up before payment.
    pub fn amount_minor(
        &self,
        inputs: &JsonObject,
        artifact_size_bytes: Option<u64>,
    ) -> Result<PortableAmountMinor, PriceError> {
        match self {
            Self::Fixed { amount_minor } => Ok(*amount_minor),
            Self::ByInput { cases } => {
                for name in self.priced_inputs() {
                    if !inputs.contains_key(name) {
                        return Err(PriceError::MissingInput(name.to_owned()));
                    }
                }
                cases
                    .iter()
                    .find(|case| {
                        case.inputs
                            .iter()
                            .all(|(name, choice)| inputs.get(name) == Some(choice.as_value()))
                    })
                    .map(|case| case.amount_minor)
                    .ok_or(PriceError::NoCase)
            }
            Self::BySize { max_bytes, .. } => {
                let size_bytes = artifact_size_bytes.ok_or(PriceError::ArtifactSizeUnknown)?;
                if size_bytes == 0 {
                    return Err(PriceError::ArtifactEmpty);
                }
                if size_bytes > max_bytes.get() {
                    return Err(PriceError::ArtifactTooLarge {
                        size_bytes,
                        max_bytes: max_bytes.get(),
                    });
                }
                self.artifact_amount(size_bytes)
                    .ok_or(PriceError::Invalid("the artifact price overflows"))
            }
        }
    }

    fn artifact_amount(&self, size_bytes: u64) -> Option<PortableAmountMinor> {
        let Self::BySize {
            base_amount_minor,
            unit_bytes,
            unit_amount_minor,
            ..
        } = self
        else {
            return None;
        };
        let units = size_bytes.div_ceil(unit_bytes.get());
        units
            .checked_mul(unit_amount_minor.get())
            .and_then(|amount| amount.checked_add(base_amount_minor.get()))
            .and_then(PortableAmountMinor::new)
    }
}

/// `runx:artifact:sha256:<64 lowercase hex>`, the reference a stored artifact
/// is read by.
fn stored_artifact_ref(reference: &str) -> bool {
    reference
        .strip_prefix("runx:artifact:sha256:")
        .is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// The lease a buyer must hold to buy a paid method, such as the inbox an
/// extension extends: only a buyer holding that lease, on the resource the
/// method's inputs name, may buy, so the buyer hands the seller's run no
/// bearer. It is checked before the quote and again before payment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RequiredLease {
    /// The seller connector that holds the lease.
    pub connector: NonEmptyString,
    /// That connector's `kind: lease` credential.
    pub credential: CredentialName,
    /// The leased resource, rendered from the method's inputs alone.
    pub target: Template,
}

impl<'de> Deserialize<'de> for RequiredLease {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireLease {
            connector: NonEmptyString,
            credential: CredentialName,
            target: Template,
        }

        let wire = WireLease::deserialize(deserializer)?;
        let target = &wire.target;
        if !target.binds_argument()
            || target.binds_parameter()
            || target.binds_exchange()
            || target.binds_result()
            || target.binds_runx()
        {
            return Err(de::Error::custom(
                "requires_lease target is rendered from the method's inputs alone",
            ));
        }
        Ok(Self {
            connector: wire.connector,
            credential: wire.credential,
            target: wire.target,
        })
    }
}

/// What a method charges, as its `run.yaml` offer states it. The registry
/// computes the digests of the method's input and output schemas when it's
/// published, and the listing, the purchase and the receipt bind those.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillOfferTerms {
    pub price: PaidSkillPrice,
    pub currency: CurrencyCode,
    /// The settlement families the seller takes.
    pub accepts: SettlementFamilies,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_lease: Option<RequiredLease>,
}

/// One immutable, registry-resolved method offer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillMethodOffer {
    pub offer_version: EmbeddedOfferVersionBinding,
    pub price: PaidSkillPrice,
    pub currency: CurrencyCode,
    pub accepts: SettlementFamilies,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_lease: Option<RequiredLease>,
}

/// One to sixty-four method offers keyed by unique method name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PaidSkillOffers(BTreeMap<String, PaidSkillMethodOffer>);

impl PaidSkillOffers {
    pub fn new(value: BTreeMap<String, PaidSkillMethodOffer>) -> Option<Self> {
        let valid_names = value.keys().all(|method| !method.is_empty());
        (valid_names && (1..=64).contains(&value.len())).then_some(Self(value))
    }
}

impl<'de> Deserialize<'de> for PaidSkillOffers {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = BTreeMap::<String, PaidSkillMethodOffer>::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            de::Error::custom("paid skill offers must contain 1..=64 non-empty method names")
        })
    }
}

impl RunxSchema for PaidSkillOffers {
    fn json_schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": PaidSkillMethodOffer::json_schema(),
            "propertyNames": { "minLength": 1 },
            "minProperties": 1,
            "maxProperties": 64,
        })
    }

    fn named_schemas(named: &mut NamedSchemas) {
        PaidSkillMethodOffer::named_schemas(named);
    }
}

/// Immutable marketplace identity and commercial terms for paid methods in one
/// published skill version. Seller identity is resolved by the authenticated
/// registry boundary and is never accepted from `run.yaml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.marketplace.paid_skill_listing.v1")]
pub struct PaidSkillListing {
    pub skill_id: NonEmptyString,
    pub version: NonEmptyString,
    pub skill_digest: Digest,
    pub profile_digest: Digest,
    /// The registry's digest of the published bundle's files.
    pub bundle_digest: Digest,
    pub seller_ref: PrincipalReference,
    pub offers: PaidSkillOffers,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer_version() -> Value {
        json!({
            "offer_id": "acme/browser#create",
            "version": "1.0.0",
            "profile_digest": format!("sha256:{}", "b".repeat(64)),
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64))
        })
    }

    fn inputs(value: Value) -> JsonObject {
        serde_json::from_value(value).unwrap_or_default()
    }

    #[allow(clippy::expect_used)]
    fn amount(value: u64) -> PortableAmountMinor {
        PortableAmountMinor::new(value).expect("a test amount is portable")
    }

    #[test]
    fn paid_skill_listing_accepts_multiple_rails_without_rail_shapes()
    -> Result<(), serde_json::Error> {
        let listing: PaidSkillListing = serde_json::from_value(json!({
            "skill_id": "acme/transcribe",
            "version": "1.0.0",
            "skill_digest": format!("sha256:{}", "a".repeat(64)),
            "profile_digest": format!("sha256:{}", "b".repeat(64)),
            "bundle_digest": format!("sha256:{}", "c".repeat(64)),
            "seller_ref": {"type": "principal", "uri": "runx:principal:acme"},
            "offers": {"transcribe": {
                "offer_version": offer_version(),
                "price": {"basis": "fixed", "amount_minor": 125},
                "currency": "USD",
                "accepts": ["rail-a", "rail-b"]
            }}
        }))?;
        let offers = serde_json::to_value(&listing.offers)?;
        assert_eq!(offers["transcribe"]["accepts"], json!(["rail-a", "rail-b"]));
        let price: PaidSkillPrice = serde_json::from_value(offers["transcribe"]["price"].clone())?;
        assert_eq!(
            price.amount_minor(&JsonObject::new(), None),
            Ok(amount(125))
        );

        let schema = PaidSkillListing::json_schema().to_string();
        assert!(!schema.contains("PAYMENT-REQUIRED"));
        assert!(!schema.contains("facilitator"));
        Ok(())
    }

    #[test]
    fn paid_skill_listing_rejects_empty_method_identity() {
        let offer = json!({
            "offer_version": offer_version(),
            "price": {"basis": "fixed", "amount_minor": 125},
            "currency": "USD",
            "accepts": ["rail-a"]
        });
        let result = serde_json::from_value::<PaidSkillListing>(json!({
            "skill_id": "acme/transcribe",
            "version": "1.0.0",
            "skill_digest": format!("sha256:{}", "a".repeat(64)),
            "profile_digest": format!("sha256:{}", "b".repeat(64)),
            "bundle_digest": format!("sha256:{}", "c".repeat(64)),
            "seller_ref": {"type": "principal", "uri": "runx:principal:acme"},
            "offers": {"": offer}
        }));
        assert!(result.is_err());
    }

    #[test]
    fn a_by_input_price_reads_its_inputs() -> Result<(), serde_json::Error> {
        let price: PaidSkillPrice = serde_json::from_value(json!({
            "basis": "by_input",
            "cases": [
                {"inputs": {"duration_seconds": 600}, "amount_minor": 5},
                {"inputs": {"duration_seconds": 1800}, "amount_minor": 10},
                {"inputs": {"duration_seconds": 3600}, "amount_minor": 20}
            ]
        }))?;
        assert_eq!(
            price.amount_minor(&inputs(json!({"duration_seconds": 1800})), None),
            Ok(amount(10))
        );
        assert_eq!(
            price.amount_minor(&inputs(json!({"duration_seconds": 900})), None),
            Err(PriceError::NoCase)
        );
        // A whole float is that integer, as JavaScript and the canonical
        // encoding both read it.
        assert_eq!(
            price.amount_minor(&inputs(json!({"duration_seconds": 1800.0})), None),
            Ok(amount(10))
        );
        assert_eq!(
            price.amount_minor(&inputs(json!({"duration_seconds": "1800"})), None),
            Err(PriceError::NoCase)
        );
        assert_eq!(
            price.amount_minor(&JsonObject::new(), None),
            Err(PriceError::MissingInput("duration_seconds".to_owned()))
        );

        let pair: PaidSkillPrice = serde_json::from_value(json!({
            "basis": "by_input",
            "cases": [
                {"inputs": {"from": "en", "to": "fr"}, "amount_minor": 3},
                {"inputs": {"from": "fr", "to": "en"}, "amount_minor": 4}
            ]
        }))?;
        assert_eq!(
            pair.amount_minor(&inputs(json!({"from": "fr", "to": "en"})), None),
            Ok(amount(4))
        );
        Ok(())
    }

    #[test]
    fn a_by_input_price_states_each_combination_once() {
        for invalid in [
            json!({"basis": "by_input", "cases": [{"inputs": {}, "amount_minor": 1}]}),
            json!({"basis": "by_input", "cases": []}),
            json!({"basis": "by_input", "cases": [
                {"inputs": {"a": 1}, "amount_minor": 1},
                {"inputs": {"b": 1}, "amount_minor": 2}
            ]}),
            json!({"basis": "by_input", "cases": [
                {"inputs": {"a": 1}, "amount_minor": 1},
                {"inputs": {"a": 1}, "amount_minor": 2}
            ]}),
            json!({"basis": "by_input", "cases": [{"inputs": {"a": 1.5}, "amount_minor": 1}]}),
            json!({"basis": "by_input", "cases": [{"inputs": {"a": null}, "amount_minor": 1}]}),
            json!({"basis": "by_input", "cases": [{"inputs": {"a": 1, "b": 2, "c": 3, "d": 4, "e": 5}, "amount_minor": 1}]}),
            json!({"basis": "fixed", "amount_minor": 0}),
            json!({"basis": "fixed", "amount_minor": 5, "quote": "price"}),
            json!({"model": "fixed", "amount_minor": 5}),
        ] {
            assert!(
                serde_json::from_value::<PaidSkillPrice>(invalid.clone()).is_err(),
                "{invalid} must be refused"
            );
        }
    }

    #[test]
    fn a_by_size_price_charges_each_started_unit() -> Result<(), serde_json::Error> {
        let price: PaidSkillPrice = serde_json::from_value(json!({
            "basis": "by_size",
            "input": "artifact",
            "base_amount_minor": 25,
            "unit_bytes": 1_048_576,
            "unit_amount_minor": 5,
            "max_bytes": 10_485_760
        }))?;
        let none = JsonObject::new();
        assert_eq!(price.amount_minor(&none, Some(1)), Ok(amount(30)));
        assert_eq!(price.amount_minor(&none, Some(1_048_576)), Ok(amount(30)));
        assert_eq!(price.amount_minor(&none, Some(1_048_577)), Ok(amount(35)));
        assert_eq!(price.amount_minor(&none, Some(10_485_760)), Ok(amount(75)));
        assert_eq!(
            price.amount_minor(&none, Some(10_485_761)),
            Err(PriceError::ArtifactTooLarge {
                size_bytes: 10_485_761,
                max_bytes: 10_485_760
            })
        );
        assert_eq!(
            price.amount_minor(&none, Some(0)),
            Err(PriceError::ArtifactEmpty)
        );
        assert_eq!(
            price.amount_minor(&none, None),
            Err(PriceError::ArtifactSizeUnknown)
        );

        let overflowing = json!({
            "basis": "by_size",
            "input": "artifact",
            "base_amount_minor": 1,
            "unit_bytes": 1,
            "unit_amount_minor": MAX_PORTABLE_INTEGER,
            "max_bytes": 2
        });
        assert!(serde_json::from_value::<PaidSkillPrice>(overflowing).is_err());
        Ok(())
    }

    /// The registry resolves a method's offer from these terms; what it
    /// reads here is the lease a buyer must hold.
    #[test]
    fn offer_terms_name_the_lease_a_buyer_holds() -> Result<(), serde_json::Error> {
        let terms: PaidSkillOfferTerms = serde_json::from_value(json!({
            "price": {"basis": "by_input", "cases": [
                {"inputs": {"duration_seconds": 3600}, "amount_minor": 5}
            ]},
            "currency": "USD",
            "accepts": ["x402"],
            "requires_lease": {
                "connector": "ausca/inbox",
                "credential": "lease",
                "target": {"$concat": ["ausca:agent-inbox/", {"$argument": "/inbox_id"}]}
            }
        }))?;
        assert!(terms.requires_lease.is_some());
        for target in [
            json!("ausca:agent-inbox"),
            json!({"$result": "/inbox_id"}),
            json!({"$concat": ["ausca:agent-inbox/", {"$parameter": "/inbox_id"}]}),
        ] {
            assert!(
                serde_json::from_value::<RequiredLease>(json!({
                    "connector": "ausca/inbox",
                    "credential": "lease",
                    "target": target
                }))
                .is_err(),
                "{target} must be refused"
            );
        }
        assert!(
            serde_json::from_value::<PaidSkillOfferTerms>(json!({
                "currency": "USD",
                "accepts": ["x402"],
                "quote": "price",
                "amount_range": {"minimum_minor": 5, "maximum_minor": 20}
            }))
            .is_err(),
            "a prepared offer is no longer a shape"
        );
        assert!(
            serde_json::from_value::<PaidSkillOfferTerms>(json!({
                "price": {"basis": "fixed", "amount_minor": 5},
                "currency": "USD",
                "accepts": ["x402"],
                "input_schema_digest": format!("sha256:{}", "d".repeat(64))
            }))
            .is_err(),
            "the registry computes a method's schema digests at publish"
        );
        Ok(())
    }
}
