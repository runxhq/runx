//! Public, rail-neutral marketplace offer and listing contracts.
//!
//! A paid skill carries its own terms on the runner it sells (D10): running it
//! is the purchase, and the kernel pays. Author terms describe the price of an
//! ordinary skill runner. Registry listings bind those terms to immutable
//! package identity and an authenticated seller. Protocol adapters, provider
//! payloads, credentials, headers, settlement and SDKs stay outside this inert
//! boundary.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::canonical_json::canonical_stable_json;
use crate::paid_invocation::{
    CurrencyCode, EmbeddedOfferRevisionRef, OfferRevisionRef, PaidInvocationPresentation,
    PortableAmountMinor, PrincipalReference, SettlementFamilies, Sha256Digest,
};
use crate::schema::{NonEmptyString, RunxSchema};
use crate::{JsonObject, JsonValue, MAX_PORTABLE_INTEGER};

pub const PAID_SKILL_LISTING_SCHEMA: &str = "runx.marketplace.paid_skill_listing.v1";

const MAX_PRICE_SELECTORS: usize = 4;
const MAX_PRICE_CASES: usize = 256;

/// How an offer prices one call: from the listing and the inputs the runner
/// receives, never from the runner. Cloud and the buyer's kernel compute the
/// same amount, so nothing the buyer or the seller's runner says can change it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, RunxSchema)]
#[serde(tag = "model", rename_all = "snake_case", deny_unknown_fields)]
pub enum PaidSkillPrice {
    /// One amount for every call.
    Fixed { amount_minor: PortableAmountMinor },
    /// An amount for each combination of values of the selector inputs, each
    /// an `enum` input the runner requires. The cases cover every combination
    /// exactly once.
    InputChoice {
        selectors: Vec<NonEmptyString>,
        cases: Vec<PaidSkillPriceCase>,
    },
    /// A base amount plus a unit amount for each started `unit_bytes` of the
    /// committed artifact the `artifact_input` names, up to `max_bytes`.
    ArtifactBytes {
        artifact_input: NonEmptyString,
        base_amount_minor: PortableAmountMinor,
        unit_bytes: std::num::NonZeroU64,
        unit_amount_minor: PortableAmountMinor,
        max_bytes: std::num::NonZeroU64,
    },
}

/// One `input_choice` case: the selector values, in selector order, and what
/// a call with them costs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillPriceCase {
    pub when: Vec<PriceChoice>,
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
            ]
        })
    }
}

impl<'de> Deserialize<'de> for PaidSkillPrice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "model", rename_all = "snake_case", deny_unknown_fields)]
        enum WirePrice {
            Fixed {
                amount_minor: PortableAmountMinor,
            },
            InputChoice {
                selectors: Vec<NonEmptyString>,
                cases: Vec<PaidSkillPriceCase>,
            },
            ArtifactBytes {
                artifact_input: NonEmptyString,
                base_amount_minor: PortableAmountMinor,
                unit_bytes: std::num::NonZeroU64,
                unit_amount_minor: PortableAmountMinor,
                max_bytes: std::num::NonZeroU64,
            },
        }

        let price = match WirePrice::deserialize(deserializer)? {
            WirePrice::Fixed { amount_minor } => Self::Fixed { amount_minor },
            WirePrice::InputChoice { selectors, cases } => Self::InputChoice { selectors, cases },
            WirePrice::ArtifactBytes {
                artifact_input,
                base_amount_minor,
                unit_bytes,
                unit_amount_minor,
                max_bytes,
            } => Self::ArtifactBytes {
                artifact_input,
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
    #[error("price selector input {0} is missing")]
    MissingSelector(String),
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
            Self::MissingSelector(_) => "missing_selector",
            Self::NoCase => "no_case",
            Self::ArtifactSizeUnknown => "artifact_size_unknown",
            Self::ArtifactEmpty => "artifact_empty",
            Self::ArtifactTooLarge { .. } => "artifact_too_large",
        }
    }
}

impl PaidSkillPrice {
    /// The price's own invariants: unique selectors, one case per combination
    /// with one value per selector, and a byte price whose largest amount is
    /// still a portable integer.
    pub fn validate(&self) -> Result<(), PriceError> {
        match self {
            Self::Fixed { .. } => Ok(()),
            Self::InputChoice { selectors, cases } => {
                if selectors.is_empty() || selectors.len() > MAX_PRICE_SELECTORS {
                    return Err(PriceError::Invalid(
                        "an input_choice price names one to four selectors",
                    ));
                }
                let names = selectors.iter().map(NonEmptyString::as_str);
                if names.collect::<BTreeSet<_>>().len() != selectors.len() {
                    return Err(PriceError::Invalid(
                        "an input_choice price names each selector once",
                    ));
                }
                if cases.is_empty() || cases.len() > MAX_PRICE_CASES {
                    return Err(PriceError::Invalid(
                        "an input_choice price has one to 256 cases",
                    ));
                }
                let mut seen = BTreeSet::new();
                for case in cases {
                    if case.when.len() != selectors.len() {
                        return Err(PriceError::Invalid(
                            "each price case gives one value per selector",
                        ));
                    }
                    let key = canonical_stable_json(&case.when).map_err(|_| {
                        PriceError::Invalid("a price case cannot be written as JSON")
                    })?;
                    if !seen.insert(key) {
                        return Err(PriceError::Invalid(
                            "each combination of selector values has one price case",
                        ));
                    }
                }
                Ok(())
            }
            Self::ArtifactBytes { max_bytes, .. } => self
                .artifact_amount(max_bytes.get())
                .map(|_| ())
                .ok_or(PriceError::Invalid(
                    "an artifact_bytes price must stay a portable amount up to max_bytes",
                )),
        }
    }

    /// The input names this price reads.
    pub fn priced_inputs(&self) -> Vec<&str> {
        match self {
            Self::Fixed { .. } => Vec::new(),
            Self::InputChoice { selectors, .. } => {
                selectors.iter().map(NonEmptyString::as_str).collect()
            }
            Self::ArtifactBytes { artifact_input, .. } => vec![artifact_input.as_str()],
        }
    }

    /// The stored artifact an `artifact_bytes` price reads: the
    /// `artifact_ref` its input holds. Its committed size, looked up by the
    /// caller, is what the call costs.
    pub fn priced_artifact_ref<'a>(&self, inputs: &'a JsonObject) -> Option<&'a str> {
        let Self::ArtifactBytes { artifact_input, .. } = self else {
            return None;
        };
        inputs
            .get(artifact_input.as_str())?
            .as_object()?
            .get("artifact_ref")?
            .as_str()
            .filter(|reference| stored_artifact_ref(reference))
    }

    /// What one call with `inputs` costs. An `artifact_bytes` price reads the
    /// committed size of the artifact its input names, which the caller looks
    /// up before payment.
    pub fn amount_minor(
        &self,
        inputs: &JsonObject,
        artifact_size_bytes: Option<u64>,
    ) -> Result<PortableAmountMinor, PriceError> {
        match self {
            Self::Fixed { amount_minor } => Ok(*amount_minor),
            Self::InputChoice { selectors, cases } => {
                let values = selectors
                    .iter()
                    .map(|selector| {
                        inputs
                            .get(selector.as_str())
                            .ok_or_else(|| PriceError::MissingSelector(selector.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                cases
                    .iter()
                    .find(|case| {
                        case.when
                            .iter()
                            .map(PriceChoice::as_value)
                            .eq(values.iter().copied())
                    })
                    .map(|case| case.amount_minor)
                    .ok_or(PriceError::NoCase)
            }
            Self::ArtifactBytes { max_bytes, .. } => {
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
        let Self::ArtifactBytes {
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

const MAX_CLAIM_URL_LENGTH: usize = 512;
const MAX_CLAIM_POINTER_SEGMENTS: usize = 8;
const MAX_CLAIM_VALUE_LENGTH: usize = 128;

/// Where a buyer claims what a fulfilled purchase bought but runx never holds,
/// such as a lease capability. The endpoint is https on the seller's own host,
/// fixed by the offer; only path parameters are filled, each from the runner's
/// public result by a JSON pointer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillClaim {
    pub url: NonEmptyString,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bind: BTreeMap<String, NonEmptyString>,
}

/// Why a claim endpoint cannot be stated or resolved.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ClaimError {
    #[error("{0}")]
    Invalid(String),
    #[error("claim parameter {0} is not a plain path segment in the result")]
    Unresolved(String),
}

impl ClaimError {
    /// The stable name of this refusal, the same in every implementation.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid",
            Self::Unresolved(_) => "unresolved",
        }
    }
}

impl PaidSkillClaim {
    pub fn new(
        url: NonEmptyString,
        bind: BTreeMap<String, NonEmptyString>,
    ) -> Result<Self, ClaimError> {
        let claim = Self { url, bind };
        claim.validate()?;
        Ok(claim)
    }

    fn validate(&self) -> Result<(), ClaimError> {
        let url = self.url.as_str();
        let invalid = |reason: &str| Err(ClaimError::Invalid(format!("claim url {reason}")));
        if url.len() > MAX_CLAIM_URL_LENGTH {
            return invalid("is too long");
        }
        let Some(rest) = url.strip_prefix("https://") else {
            return invalid("must be https");
        };
        let Some((authority, path)) = rest.split_once('/') else {
            return invalid("must have a path");
        };
        if !valid_authority(authority) {
            return invalid("must name a fixed host");
        }
        let parameters = path_parameters(path)
            .ok_or_else(|| ClaimError::Invalid("claim url path is invalid".to_owned()))?;
        let bound = self
            .bind
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if parameters != bound {
            return Err(ClaimError::Invalid(
                "every claim url parameter is bound once, and only those".to_owned(),
            ));
        }
        if let Some(pointer) = self
            .bind
            .values()
            .find(|pointer| !valid_pointer(pointer.as_str()))
        {
            return Err(ClaimError::Invalid(format!(
                "claim binding {pointer} is not a JSON pointer of one to eight segments"
            )));
        }
        Ok(())
    }

    /// The claim endpoint for a fulfilled purchase whose public result is
    /// `result`. A bound value must be a plain path segment, so the result
    /// cannot redirect the claim.
    pub fn resolve(&self, result: &JsonValue) -> Result<String, ClaimError> {
        let mut url = self.url.as_str().to_owned();
        for (parameter, pointer) in &self.bind {
            let value = json_pointer(result, pointer.as_str())
                .and_then(JsonValue::as_str)
                .filter(|value| plain_segment(value))
                .ok_or_else(|| ClaimError::Unresolved(parameter.clone()))?;
            url = url.replace(&format!("{{{parameter}}}"), value);
        }
        Ok(url)
    }
}

impl<'de> Deserialize<'de> for PaidSkillClaim {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireClaim {
            url: NonEmptyString,
            #[serde(default)]
            bind: BTreeMap<String, NonEmptyString>,
        }

        let wire = WireClaim::deserialize(deserializer)?;
        Self::new(wire.url, wire.bind).map_err(de::Error::custom)
    }
}

fn valid_authority(authority: &str) -> bool {
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let labels = host.split('.').collect::<Vec<_>>();
    let host_valid = labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        });
    let port_valid = port.is_none_or(|port| {
        (1..=5).contains(&port.len())
            && port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|port| port > 0)
    });
    host_valid && port_valid
}

/// The `{name}` parameters of a claim path, each named once, or None when the
/// path holds anything but unreserved characters, `/` and parameters.
fn path_parameters(path: &str) -> Option<BTreeSet<&str>> {
    let mut parameters = BTreeSet::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        if !rest[..open].bytes().all(path_byte) {
            return None;
        }
        let close = rest[open..].find('}')? + open;
        let name = &rest[open + 1..close];
        if !parameter_name(name) || !parameters.insert(name) {
            return None;
        }
        rest = &rest[close + 1..];
    }
    rest.bytes().all(path_byte).then_some(parameters)
}

fn path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'-' | b'/')
}

fn parameter_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && name.len() <= 32
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_pointer(pointer: &str) -> bool {
    let Some(rest) = pointer.strip_prefix('/') else {
        return false;
    };
    let segments = rest.split('/').collect::<Vec<_>>();
    segments.len() <= MAX_CLAIM_POINTER_SEGMENTS
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && segment.match_indices('~').all(|(index, _)| {
                    matches!(segment.as_bytes().get(index + 1), Some(b'0' | b'1'))
                })
        })
}

/// The value at an RFC 6901 pointer, if there is one.
fn json_pointer<'a>(value: &'a JsonValue, pointer: &str) -> Option<&'a JsonValue> {
    pointer
        .strip_prefix('/')?
        .split('/')
        .try_fold(value, |value, segment| {
            let key = segment.replace("~1", "/").replace("~0", "~");
            match value {
                JsonValue::Object(object) => object.get(&key),
                JsonValue::Array(items) => key
                    .parse::<usize>()
                    .ok()
                    .filter(|_| key == "0" || !key.starts_with('0'))
                    .and_then(|index| items.get(index)),
                _ => None,
            }
        })
}

fn plain_segment(value: &str) -> bool {
    (1..=MAX_CLAIM_VALUE_LENGTH).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'-'))
        && value != "."
        && value != ".."
}

/// Provider-neutral commercial terms authored for one ordinary skill runner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidSkillOfferTerms {
    pub price: PaidSkillPrice,
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    pub input_schema_digest: Sha256Digest,
    pub output_schema_digest: Sha256Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<PaidSkillClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PaidInvocationPresentation>,
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
#[serde(deny_unknown_fields)]
pub struct PaidSkillRunnerOffer {
    pub offer_revision: EmbeddedOfferRevisionRef,
    pub price: PaidSkillPrice,
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<PaidSkillClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PaidInvocationPresentation>,
}

impl PaidSkillRunnerOffer {
    /// The registry-resolved offer for author terms under `offer_revision`.
    #[must_use]
    pub fn from_terms(offer_revision: OfferRevisionRef, terms: &PaidSkillOfferTerms) -> Self {
        Self {
            offer_revision: offer_revision.into(),
            price: terms.price.clone(),
            currency: terms.currency.clone(),
            accepted_settlement_families: terms.accepted_settlement_families.clone(),
            claim: terms.claim.clone(),
            presentation: terms.presentation.clone(),
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

    fn revision() -> Value {
        json!({
            "offer_id": "acme/browser#create",
            "revision": "1.0.0",
            "revision_digest": format!("sha256:{}", "b".repeat(64)),
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64))
        })
    }

    fn inputs(value: Value) -> JsonObject {
        serde_json::from_value(value).unwrap_or_default()
    }

    fn result(value: Value) -> JsonValue {
        serde_json::from_value(value).unwrap_or(JsonValue::Null)
    }

    fn amount(value: u64) -> PortableAmountMinor {
        PortableAmountMinor::new(value).unwrap()
    }

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
                "offer_revision": revision(),
                "price": {"model": "fixed", "amount_minor": 125},
                "currency": "USD",
                "accepted_settlement_families": ["rail-a", "rail-b"]
            }}
        }))?;
        let offer = &listing.offers.as_map()["transcribe"];
        assert_eq!(
            offer.price.amount_minor(&JsonObject::new(), None),
            Ok(amount(125))
        );

        let schema = PaidSkillListing::json_schema().to_string();
        assert!(!schema.contains("PAYMENT-REQUIRED"));
        assert!(!schema.contains("facilitator"));
        Ok(())
    }

    #[test]
    fn paid_skill_listing_rejects_empty_runner_identity() {
        let offer = json!({
            "offer_revision": revision(),
            "price": {"model": "fixed", "amount_minor": 125},
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
    fn an_input_choice_price_reads_its_selectors() -> Result<(), serde_json::Error> {
        let price: PaidSkillPrice = serde_json::from_value(json!({
            "model": "input_choice",
            "selectors": ["duration_seconds"],
            "cases": [
                {"when": [600], "amount_minor": 5},
                {"when": [1800], "amount_minor": 10},
                {"when": [3600], "amount_minor": 20}
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
            Err(PriceError::MissingSelector("duration_seconds".to_owned()))
        );

        let pair: PaidSkillPrice = serde_json::from_value(json!({
            "model": "input_choice",
            "selectors": ["from", "to"],
            "cases": [
                {"when": ["en", "fr"], "amount_minor": 3},
                {"when": ["fr", "en"], "amount_minor": 4}
            ]
        }))?;
        assert_eq!(
            pair.amount_minor(&inputs(json!({"from": "fr", "to": "en"})), None),
            Ok(amount(4))
        );
        Ok(())
    }

    #[test]
    fn an_input_choice_price_states_each_combination_once() {
        for invalid in [
            json!({"model": "input_choice", "selectors": [], "cases": [{"when": [], "amount_minor": 1}]}),
            json!({"model": "input_choice", "selectors": ["a", "a"], "cases": [{"when": [1, 1], "amount_minor": 1}]}),
            json!({"model": "input_choice", "selectors": ["a"], "cases": []}),
            json!({"model": "input_choice", "selectors": ["a"], "cases": [{"when": [1, 2], "amount_minor": 1}]}),
            json!({"model": "input_choice", "selectors": ["a"], "cases": [
                {"when": [1], "amount_minor": 1},
                {"when": [1], "amount_minor": 2}
            ]}),
            json!({"model": "input_choice", "selectors": ["a"], "cases": [{"when": [1.5], "amount_minor": 1}]}),
            json!({"model": "input_choice", "selectors": ["a"], "cases": [{"when": [null], "amount_minor": 1}]}),
            json!({"model": "fixed", "amount_minor": 0}),
            json!({"model": "fixed", "amount_minor": 5, "quote": "price"}),
        ] {
            assert!(
                serde_json::from_value::<PaidSkillPrice>(invalid.clone()).is_err(),
                "{invalid} must be refused"
            );
        }
    }

    #[test]
    fn an_artifact_bytes_price_charges_each_started_unit() -> Result<(), serde_json::Error> {
        let price: PaidSkillPrice = serde_json::from_value(json!({
            "model": "artifact_bytes",
            "artifact_input": "artifact",
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
            "model": "artifact_bytes",
            "artifact_input": "artifact",
            "base_amount_minor": 1,
            "unit_bytes": 1,
            "unit_amount_minor": MAX_PORTABLE_INTEGER,
            "max_bytes": 2
        });
        assert!(serde_json::from_value::<PaidSkillPrice>(overflowing).is_err());
        Ok(())
    }

    #[test]
    fn a_claim_fills_only_path_segments_on_a_fixed_host() -> Result<(), serde_json::Error> {
        let claim: PaidSkillClaim = serde_json::from_value(json!({
            "url": "https://inbox.ausca.com/v1/agent-inboxes/{inbox_id}/claim",
            "bind": {"inbox_id": "/resource_lease/inbox_id"}
        }))?;
        assert_eq!(
            claim.resolve(&result(
                json!({"resource_lease": {"inbox_id": "inb_abc-123"}})
            )),
            Ok("https://inbox.ausca.com/v1/agent-inboxes/inb_abc-123/claim".to_owned())
        );
        for result in [
            json!({"resource_lease": {"inbox_id": "../admin"}}),
            json!({"resource_lease": {"inbox_id": "a/b"}}),
            json!({"resource_lease": {"inbox_id": ".."}}),
            json!({"resource_lease": {"inbox_id": 7}}),
            json!({"resource_lease": {}}),
        ] {
            assert_eq!(
                claim.resolve(&super::tests::result(result)),
                Err(ClaimError::Unresolved("inbox_id".to_owned()))
            );
        }

        for invalid in [
            json!({"url": "http://inbox.ausca.com/claim"}),
            json!({"url": "https://{host}.ausca.com/claim", "bind": {"host": "/host"}}),
            json!({"url": "https://inbox.ausca.com"}),
            json!({"url": "https://localhost/claim"}),
            json!({"url": "https://inbox.ausca.com/claim?inbox={id}", "bind": {"id": "/id"}}),
            json!({"url": "https://inbox.ausca.com/{id}/claim"}),
            json!({"url": "https://inbox.ausca.com/claim", "bind": {"id": "/id"}}),
            json!({"url": "https://inbox.ausca.com/{id}/{id}", "bind": {"id": "/id"}}),
            json!({"url": "https://inbox.ausca.com/{id}", "bind": {"id": "id"}}),
            json!({"url": "https://user@inbox.ausca.com/claim"}),
        ] {
            assert!(
                serde_json::from_value::<PaidSkillClaim>(invalid.clone()).is_err(),
                "{invalid} must be refused"
            );
        }
        Ok(())
    }

    #[test]
    fn a_runner_offer_carries_its_terms_price_and_claim() -> Result<(), serde_json::Error> {
        let terms: PaidSkillOfferTerms = serde_json::from_value(json!({
            "price": {"model": "input_choice", "selectors": ["duration_seconds"], "cases": [
                {"when": [3600], "amount_minor": 5}
            ]},
            "currency": "USD",
            "accepted_settlement_families": ["x402"],
            "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
            "output_schema_digest": format!("sha256:{}", "e".repeat(64)),
            "claim": {
                "url": "https://inbox.ausca.com/v1/agent-inboxes/{inbox_id}/claim",
                "bind": {"inbox_id": "/resource_lease/inbox_id"}
            }
        }))?;
        let offer = PaidSkillRunnerOffer::from_terms(
            serde_json::from_value::<OfferRevisionRef>(revision())?,
            &terms,
        );
        assert_eq!(offer.price, terms.price);
        assert_eq!(offer.claim, terms.claim);
        assert!(
            serde_json::from_value::<PaidSkillOfferTerms>(json!({
                "currency": "USD",
                "accepted_settlement_families": ["x402"],
                "quote": "price",
                "amount_range": {"minimum_minor": 5, "maximum_minor": 20},
                "input_schema_digest": format!("sha256:{}", "d".repeat(64)),
                "output_schema_digest": format!("sha256:{}", "e".repeat(64))
            }))
            .is_err(),
            "a prepared offer is no longer a shape"
        );
        Ok(())
    }
}
