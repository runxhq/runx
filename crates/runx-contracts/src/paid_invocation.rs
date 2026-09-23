//! Public, rail-neutral paid-invocation contracts.
//!
//! This module owns inert V1 wire values only. Payment admission, settlement,
//! persistence, execution, recovery, and provider SDKs belong to hosted
//! implementations behind this boundary.

use std::collections::BTreeSet;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::schema::{
    BoundedString, Identity, IsoDateTime, NonEmptyString, Property, RunxSchema,
    any_of_with_identity, const_string, object_schema,
};
use crate::{
    JsonValue, MAX_PORTABLE_INTEGER, RECEIPT_CANONICALIZATION, Reference, ReferenceType,
    RunxPrincipalId,
};

pub const PAID_INVOCATION_SCHEMA: &str = "runx.payment.paid_invocation.v1";
pub const OFFER_REVISION_REF_SCHEMA: &str = "runx.payment.offer_revision_ref.v1";
pub const PARENT_INVOCATION_BINDING_SCHEMA: &str = "runx.payment.parent_invocation_binding.v1";
pub const QUOTE_PAID_INVOCATION_REQUEST_SCHEMA: &str =
    "runx.payment.quote_paid_invocation.request.v1";
pub const QUOTE_PAID_INVOCATION_RESULT_SCHEMA: &str =
    "runx.payment.quote_paid_invocation.result.v1";
pub const EXECUTE_PAID_INVOCATION_REQUEST_SCHEMA: &str =
    "runx.payment.execute_paid_invocation.request.v1";
pub const EXECUTE_PAID_INVOCATION_RESULT_SCHEMA: &str =
    "runx.payment.execute_paid_invocation.result.v1";
pub const GET_PAID_INVOCATION_REQUEST_SCHEMA: &str = "runx.payment.get_paid_invocation.request.v1";
pub const GET_PAID_INVOCATION_RESULT_SCHEMA: &str = "runx.payment.get_paid_invocation.result.v1";
pub const CANCEL_PAID_INVOCATION_REQUEST_SCHEMA: &str =
    "runx.payment.cancel_paid_invocation.request.v1";
pub const CANCEL_PAID_INVOCATION_RESULT_SCHEMA: &str =
    "runx.payment.cancel_paid_invocation.result.v1";

pub const QUOTE_PAID_INVOCATION: &str = "QuotePaidInvocation";
pub const EXECUTE_PAID_INVOCATION: &str = "ExecutePaidInvocation";
pub const GET_PAID_INVOCATION: &str = "GetPaidInvocation";
pub const CANCEL_PAID_INVOCATION: &str = "CancelPaidInvocation";

/// A lowercase, prefixed SHA-256 digest.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let hex = value.strip_prefix("sha256:")?;
        (hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        .then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            de::Error::custom("digest must be sha256: followed by 64 lowercase hex characters")
        })
    }
}

impl RunxSchema for Sha256Digest {
    fn json_schema() -> Value {
        json!({
            "type": "string",
            "pattern": "^sha256:[0-9a-f]{64}$",
        })
    }
}

/// A positive minor-unit amount that crosses Rust and JavaScript JSON
/// boundaries without integer precision loss.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PortableAmountMinor(u64);

impl PortableAmountMinor {
    pub fn new(value: u64) -> Option<Self> {
        (1..=MAX_PORTABLE_INTEGER)
            .contains(&value)
            .then_some(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for PortableAmountMinor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            de::Error::custom(format!(
                "amount_minor must be between 1 and {MAX_PORTABLE_INTEGER}"
            ))
        })
    }
}

impl RunxSchema for PortableAmountMinor {
    fn json_schema() -> Value {
        json!({
            "type": "integer",
            "minimum": 1,
            "maximum": MAX_PORTABLE_INTEGER,
        })
    }
}

/// An ISO 4217-style uppercase three-letter currency code.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CurrencyCode(String);

impl CurrencyCode {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase()))
            .then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CurrencyCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value)
            .ok_or_else(|| de::Error::custom("currency must be three uppercase ASCII letters"))
    }
}

impl RunxSchema for CurrencyCode {
    fn json_schema() -> Value {
        json!({ "type": "string", "pattern": "^[A-Z]{3}$" })
    }
}

/// A bounded, provider-neutral settlement-family identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct SettlementFamily(String);

impl SettlementFamily {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
            });
        valid.then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SettlementFamily {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("invalid settlement family"))
    }
}

impl RunxSchema for SettlementFamily {
    fn json_schema() -> Value {
        json!({
            "type": "string",
            "minLength": 1,
            "maxLength": 64,
            "pattern": "^[a-z0-9][a-z0-9._-]{0,63}$",
        })
    }
}

/// One to sixteen unique settlement-family identifiers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SettlementFamilies(Vec<SettlementFamily>);

impl SettlementFamilies {
    pub fn new(value: Vec<SettlementFamily>) -> Option<Self> {
        let unique = value.iter().collect::<BTreeSet<_>>().len() == value.len();
        (unique && (1..=16).contains(&value.len())).then_some(Self(value))
    }

    pub fn as_slice(&self) -> &[SettlementFamily] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SettlementFamilies {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Vec::<SettlementFamily>::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            de::Error::custom("settlement families must contain 1..=16 unique values")
        })
    }
}

impl RunxSchema for SettlementFamilies {
    fn json_schema() -> Value {
        json!({
            "type": "array",
            "items": SettlementFamily::json_schema(),
            "minItems": 1,
            "maxItems": 16,
            "uniqueItems": true,
        })
    }
}

/// Immutable identity of one endpoint-mediated marketplace leg.
///
/// This is rail-neutral commercial data. Protocol challenges, credentials,
/// settlement payloads and provider evidence remain outside the contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidInvocationMediation {
    pub listing_ref: MediationListingRef,
    pub endpoint_url: MediationEndpointUrl,
    pub vendor_offer_revision: EmbeddedOfferRevisionRef,
    pub vendor_package_digest: Sha256Digest,
    pub vendor_amount_minor: PortableAmountMinor,
    pub platform_fee_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub settlement_family: SettlementFamily,
    pub expected_receipt_class: MediatedReceiptClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepared_price: Option<PreparedInvocationPriceBinding>,
}

/// Immutable evidence that resolved a measured marketplace price.
///
/// The source invocation is read through the paid-invocation service before
/// the outer quote is admitted. Rail adapters then compare the fresh vendor
/// challenge with these exact commercial and input identities before signing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PreparedInvocationPriceBinding {
    pub source_invocation_id: BoundedString<128>,
    pub input_digest: Sha256Digest,
}

/// Content identity for an immutable marketplace listing revision.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct MediationListingRef(String);

impl MediationListingRef {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (value.starts_with("runx:listing:")
            && value.len() <= 512
            && value.len() > "runx:listing:".len()
            && value
                .bytes()
                .all(|byte| !byte.is_ascii_control() && !byte.is_ascii_whitespace()))
        .then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for MediationListingRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("listing_ref is invalid"))
    }
}

impl RunxSchema for MediationListingRef {
    fn json_schema() -> Value {
        json!({
            "type": "string",
            "pattern": "^runx:listing:[^\\s]+$",
            "maxLength": 512,
        })
    }
}

/// Exact public HTTPS endpoint selected by an immutable mediated listing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct MediationEndpointUrl(String);

impl MediationEndpointUrl {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let remainder = value.strip_prefix("https://")?;
        let authority = remainder.split(['/', '?']).next().unwrap_or_default();
        (!authority.is_empty()
            && !authority.contains('@')
            && !value.contains('#')
            && value.len() <= 2_048
            && value
                .bytes()
                .all(|byte| !byte.is_ascii_control() && !byte.is_ascii_whitespace()))
        .then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for MediationEndpointUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            de::Error::custom("endpoint_url must be an exact credential-free HTTPS URL")
        })
    }
}

impl RunxSchema for MediationEndpointUrl {
    fn json_schema() -> Value {
        json!({
            "type": "string",
            "pattern": "^https://[^\\s@/#?]+[^\\s#]*$",
            "maxLength": 2048,
        })
    }
}

/// Composite proof currently requires a genuinely executed inner receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediatedReceiptClass {
    Executed,
}

/// A reference proven to identify a principal at deserialization time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PrincipalReference(Reference);

impl PrincipalReference {
    pub fn new(value: Reference) -> Option<Self> {
        (value.reference_type == ReferenceType::Principal).then_some(Self(value))
    }

    pub fn as_reference(&self) -> &Reference {
        &self.0
    }

    /// Construct the canonical owner reference for an already-validated hosted
    /// Runx principal identifier.
    pub fn from_runx_principal_id(value: RunxPrincipalId) -> Self {
        Self(Reference::runx(ReferenceType::Principal, value.as_str()))
    }
}

impl<'de> Deserialize<'de> for PrincipalReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Reference::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("reference type must be principal"))
    }
}

impl RunxSchema for PrincipalReference {
    fn json_schema() -> Value {
        let mut schema = Reference::json_schema();
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.insert("type".to_owned(), const_string("principal"));
        }
        schema
    }
}

/// An opaque reference to payment evidence or a hosted payment record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaymentReference(Reference);

impl PaymentReference {
    pub fn new(value: Reference) -> Self {
        Self(value)
    }

    pub fn as_reference(&self) -> &Reference {
        &self.0
    }
}

impl RunxSchema for PaymentReference {
    fn json_schema() -> Value {
        Reference::json_schema()
    }
}

/// The exact hosted run attached to a paid invocation.
///
/// The paid-invocation feature owns only this reference. Hosted run state and
/// output remain behind the run-control service and its own store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PaidInvocationRunReference(Reference);

impl PaidInvocationRunReference {
    pub fn new(value: Reference) -> Option<Self> {
        let identifier = value.uri.as_str().strip_prefix("runx:run:")?;
        let valid_identifier = !identifier.is_empty()
            && identifier.len() <= 256
            && identifier.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_alphanumeric()
                    || (index > 0 && matches!(byte, b'.' | b'_' | b':' | b'-'))
            });
        (value.reference_type == ReferenceType::Act && valid_identifier).then_some(Self(value))
    }

    pub fn as_reference(&self) -> &Reference {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PaidInvocationRunReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Reference::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            de::Error::custom("run_ref must be an act reference with a runx:run: URI")
        })
    }
}

impl RunxSchema for PaidInvocationRunReference {
    fn json_schema() -> Value {
        let mut schema = Reference::json_schema();
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.insert("type".to_owned(), const_string("act"));
            properties.insert(
                "uri".to_owned(),
                json!({
                    "type": "string",
                    "pattern": "^runx:run:[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$",
                }),
            );
        }
        schema
    }
}

/// The sole V1 canonicalization identifier for digest-bound inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum PaidInvocationCanonicalizerVersion {
    #[serde(rename = "runx.receipt.c14n.v1")]
    ReceiptC14nV1,
}

impl PaidInvocationCanonicalizerVersion {
    pub const fn as_str(self) -> &'static str {
        RECEIPT_CANONICALIZATION
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaymentIdempotencyBinding {
    pub key: NonEmptyString,
    pub binding_digest: Sha256Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.offer_revision_ref.v1")]
pub struct OfferRevisionRef {
    pub offer_id: NonEmptyString,
    pub revision: NonEmptyString,
    pub revision_digest: Sha256Digest,
    pub input_schema_digest: Sha256Digest,
    pub output_schema_digest: Sha256Digest,
}

/// The exact `OfferRevisionRef` wire value embedded inside another contract.
///
/// Standalone contract identity belongs only on the top-level schema. This
/// transparent wrapper prevents duplicate `$id` declarations when a document
/// contains more than one offer revision reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EmbeddedOfferRevisionRef(OfferRevisionRef);

impl EmbeddedOfferRevisionRef {
    pub fn as_offer_revision(&self) -> &OfferRevisionRef {
        &self.0
    }
}

impl From<OfferRevisionRef> for EmbeddedOfferRevisionRef {
    fn from(value: OfferRevisionRef) -> Self {
        Self(value)
    }
}

impl RunxSchema for EmbeddedOfferRevisionRef {
    fn json_schema() -> Value {
        let mut schema = OfferRevisionRef::json_schema();
        if let Some(object) = schema.as_object_mut() {
            object.remove("$id");
            object.remove("$schema");
            object.remove("x-runx-schema");
            if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
                properties.remove("schema");
            }
        }
        schema
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.parent_invocation_binding.v1")]
pub struct ParentInvocationBinding {
    pub invocation_id: NonEmptyString,
    pub execution_digest: Sha256Digest,
}

/// Caller-reported acquisition context. It is inert analytics metadata: it
/// cannot select an offer, change price, move money, or affect execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct QuotePaidInvocationAttribution {
    pub source: PaidInvocationAttributionSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<PaidInvocationAttributionCampaign>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaidInvocationAttributionConfidence {
    SelfReported,
    Trusted,
}

/// Attribution captured for one invocation. Confidence is assigned by Runx;
/// callers can report source and campaign but cannot assert trust.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidInvocationAttribution {
    pub source: PaidInvocationAttributionSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<PaidInvocationAttributionCampaign>,
    pub confidence: PaidInvocationAttributionConfidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PaidInvocationAttributionSource(String);

impl PaidInvocationAttributionSource {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        valid_attribution_label(&value, 64).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PaidInvocationAttributionSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("attribution source is invalid"))
    }
}

impl RunxSchema for PaidInvocationAttributionSource {
    fn json_schema() -> Value {
        json!({
            "type": "string",
            "pattern": "^[a-z0-9][a-z0-9._-]{0,63}$",
            "maxLength": 64,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PaidInvocationAttributionCampaign(String);

impl PaidInvocationAttributionCampaign {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        valid_attribution_label(&value, 128).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PaidInvocationAttributionCampaign {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("attribution campaign is invalid"))
    }
}

impl RunxSchema for PaidInvocationAttributionCampaign {
    fn json_schema() -> Value {
        json!({
            "type": "string",
            "pattern": "^[a-z0-9][a-z0-9._-]{0,127}$",
            "maxLength": 128,
        })
    }
}

fn valid_attribution_label(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaidInvocationPaymentState {
    Unpaid,
    Settling,
    Settled,
    Refunded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaidInvocationExecutionState {
    Unstarted,
    Queued,
    Running,
    WaitingExternal,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaidInvocationOutcomeGate {
    Open,
    FulfilmentWon,
    RefundWon,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.paid_invocation.v1")]
pub struct PaidInvocation {
    pub invocation_id: NonEmptyString,
    pub principal: PrincipalReference,
    /// Authenticated principal that owns the sold execution capability.
    /// This is distinct from `counterparty`, which identifies the endpoint or
    /// payment-side resource used for this transaction leg.
    pub vendor_ref: PrincipalReference,
    pub counterparty: PaymentReference,
    pub offer_revision: OfferRevisionRef,
    pub package_digest: Sha256Digest,
    pub input_digest: Sha256Digest,
    pub canonicalizer_version: PaidInvocationCanonicalizerVersion,
    pub amount_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mediation: Option<PaidInvocationMediation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement_family: Option<SettlementFamily>,
    pub idempotency: PaymentIdempotencyBinding,
    pub expires_at: IsoDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentInvocationBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<PaidInvocationAttribution>,
    pub payment_state: PaidInvocationPaymentState,
    pub execution_state: PaidInvocationExecutionState,
    pub outcome_gate: PaidInvocationOutcomeGate,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_job_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_ref: Option<PaymentReference>,
    pub created_at: IsoDateTime,
    pub updated_at: IsoDateTime,
}

/// Vendor-authored presentation of one payable resource: what discovery and
/// payment challenges show a buyer. It cannot select settlement targets or
/// move money, and it never enters quote identity. Examples and schemas are
/// carried as canonical JSON text so a listing that embeds them stays exactly
/// comparable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidInvocationPresentation {
    pub service_name: BoundedString<120>,
    pub description: BoundedString<500>,
    pub media_type: BoundedString<200>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<BoundedString<64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<BoundedString<2048>>,
    pub input_example: BoundedString<65_536>,
    pub output_example: BoundedString<65_536>,
    pub input_schema: BoundedString<65_536>,
    pub output_schema: BoundedString<65_536>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidInvocationPaymentChallenge {
    pub settlement_family: SettlementFamily,
    pub protocol_version: NonEmptyString,
    pub media_type: NonEmptyString,
    pub payload: JsonValue,
    pub payload_digest: Sha256Digest,
    pub quote_ref: Reference,
    pub quote_expires_at: IsoDateTime,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaidInvocationRefusalCode {
    OfferUnavailable,
    QuoteExpired,
    TermsChanged,
    ReplayConflict,
    PaymentNotAuthorized,
    CapacityUnavailable,
    NotFound,
    CancellationNotAvailable,
}

pub type PaidInvocationRefusalReason = BoundedString<512>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.quote_paid_invocation.request.v1")]
pub struct QuotePaidInvocationRequest {
    pub principal: PrincipalReference,
    /// Authenticated principal that owns the sold execution capability.
    pub vendor_ref: PrincipalReference,
    pub counterparty: PaymentReference,
    pub offer_revision: OfferRevisionRef,
    pub package_digest: Sha256Digest,
    pub input_digest: Sha256Digest,
    pub canonicalizer_version: PaidInvocationCanonicalizerVersion,
    pub amount_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mediation: Option<PaidInvocationMediation>,
    pub idempotency: PaymentIdempotencyBinding,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentInvocationBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<QuotePaidInvocationAttribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PaidInvocationPresentation>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotePaidInvocationAdmission {
    pub invocation: PaidInvocation,
    pub challenge: PaidInvocationPaymentChallenge,
}

impl RunxSchema for QuotePaidInvocationAdmission {
    fn json_schema() -> Value {
        object_schema(
            vec![
                Property::new("invocation", PaidInvocation::json_schema(), true),
                Property::new(
                    "challenge",
                    json!({ "$ref": "#/$defs/PaidInvocationPaymentChallenge" }),
                    true,
                ),
            ],
            true,
            None,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotePaidInvocationResult {
    Admitted {
        value: Box<QuotePaidInvocationAdmission>,
    },
    Refused {
        code: PaidInvocationRefusalCode,
        reason: PaidInvocationRefusalReason,
    },
}

impl RunxSchema for QuotePaidInvocationResult {
    fn json_schema() -> Value {
        let admitted = object_schema(
            vec![
                Property::new("status", const_string("admitted"), true),
                Property::new("value", QuotePaidInvocationAdmission::json_schema(), true),
            ],
            true,
            None,
        );
        let refused = refusal_variant_schema();
        let mut schema = any_of_with_identity(
            vec![admitted, refused],
            Some(Identity::Runx {
                logical: QUOTE_PAID_INVOCATION_RESULT_SCHEMA,
                url: None,
            }),
        );
        if let Some(object) = schema.as_object_mut() {
            object.insert(
                "$defs".to_owned(),
                json!({
                    "PaidInvocationPaymentChallenge": PaidInvocationPaymentChallenge::json_schema()
                }),
            );
        }
        schema
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.execute_paid_invocation.request.v1")]
pub struct ExecutePaidInvocationRequest {
    pub invocation_id: NonEmptyString,
    pub settlement_family: SettlementFamily,
    pub payment_ref: PaymentReference,
    pub idempotency: PaymentIdempotencyBinding,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidInvocationAdmission {
    pub invocation: PaidInvocation,
}

macro_rules! paid_invocation_result {
    ($name:ident, $schema_id:literal) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
        #[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
        #[runx_schema(id = $schema_id)]
        pub enum $name {
            Admitted {
                value: Box<PaidInvocationAdmission>,
            },
            Refused {
                code: PaidInvocationRefusalCode,
                reason: PaidInvocationRefusalReason,
            },
        }
    };
}

paid_invocation_result!(
    ExecutePaidInvocationResult,
    "runx.payment.execute_paid_invocation.result.v1"
);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.get_paid_invocation.request.v1")]
pub struct GetPaidInvocationRequest {
    pub invocation_id: NonEmptyString,
}

/// Bounded, customer-safe reason a refund won the outcome gate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidInvocationFailure {
    pub code: BoundedString<96>,
    pub message: BoundedString<500>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct GetPaidInvocationAdmission {
    pub invocation: PaidInvocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_ref: Option<PaidInvocationRunReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_ref: Option<Reference>,
    /// What fulfilled the invocation: the source run (act) or the durable
    /// continuation's result (artifact). Present only after fulfilment won.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<Reference>,
    /// Why a refund won. Present only after refund won.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<PaidInvocationFailure>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[runx_schema(id = "runx.payment.get_paid_invocation.result.v1")]
pub enum GetPaidInvocationResult {
    Admitted {
        value: Box<GetPaidInvocationAdmission>,
    },
    Refused {
        code: PaidInvocationRefusalCode,
        reason: PaidInvocationRefusalReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.cancel_paid_invocation.request.v1")]
pub struct CancelPaidInvocationRequest {
    pub invocation_id: NonEmptyString,
    pub idempotency: PaymentIdempotencyBinding,
}

paid_invocation_result!(
    CancelPaidInvocationResult,
    "runx.payment.cancel_paid_invocation.result.v1"
);

fn refusal_variant_schema() -> Value {
    object_schema(
        vec![
            Property::new("status", const_string("refused"), true),
            Property::new("code", PaidInvocationRefusalCode::json_schema(), true),
            Property::new("reason", PaidInvocationRefusalReason::json_schema(), true),
        ],
        true,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizer_value_is_bound_to_receipt_constant() {
        assert_eq!(
            PaidInvocationCanonicalizerVersion::ReceiptC14nV1.as_str(),
            RECEIPT_CANONICALIZATION
        );
    }

    #[test]
    fn bounded_values_reject_ambiguous_forms() {
        assert!(Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).is_some());
        assert!(Sha256Digest::new(format!("sha256:{}", "A".repeat(64))).is_none());
        assert!(CurrencyCode::new("USD").is_some());
        assert!(CurrencyCode::new("usd").is_none());
        assert!(SettlementFamily::new("hosted.mock-v1").is_some());
        assert!(SettlementFamily::new("Hosted").is_none());
        assert!(PaidInvocationAttributionSource::new("frantic").is_some());
        assert!(PaidInvocationAttributionSource::new("Frantic").is_none());
        assert!(PaidInvocationAttributionSource::new("frantic/bounty").is_none());
        assert!(PaidInvocationAttributionCampaign::new("bounty-131").is_some());
        assert!(PaidInvocationAttributionCampaign::new("-bounty-131").is_none());
    }

    #[test]
    fn paid_run_reference_requires_the_exact_run_namespace() {
        let run = Reference::with_uri(ReferenceType::Act, "runx:run:hosted-1");
        assert!(PaidInvocationRunReference::new(run).is_some());
        let authority = Reference::with_uri(ReferenceType::Act, "runx:act:hosted-1");
        assert!(PaidInvocationRunReference::new(authority).is_none());
        let wrong_type = Reference::with_uri(ReferenceType::Artifact, "runx:run:hosted-1");
        assert!(PaidInvocationRunReference::new(wrong_type).is_none());
        let malformed = Reference::with_uri(ReferenceType::Act, "runx:run:-hosted");
        assert!(PaidInvocationRunReference::new(malformed).is_none());
        let oversized =
            Reference::with_uri(ReferenceType::Act, format!("runx:run:{}", "r".repeat(257)));
        assert!(PaidInvocationRunReference::new(oversized).is_none());
    }
}
