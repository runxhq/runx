//! Public, rail-neutral purchase contracts: one paid skill run a buyer buys
//! from a seller, from quote to receipt.
//!
//! This module owns inert V1 wire values only. Payment admission, settlement,
//! persistence, execution, recovery, and provider SDKs belong to hosted
//! implementations behind this boundary.

use std::collections::BTreeSet;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::purchase_claim::BuyerKey;
use crate::schema::{
    BoundedString, Identity, IsoDateTime, NamedSchemas, NonEmptyString, Property, RunxSchema,
    any_of_with_identity, const_string, object_schema, record_named, titled,
};
use crate::{
    Digest, JsonObject, JsonValue, MAX_PORTABLE_INTEGER, RECEIPT_CANONICALIZATION, Reference,
    ReferenceType, RunxPrincipalId,
};

pub const PURCHASE_SCHEMA: &str = "runx.payment.purchase.v1";
pub const OFFER_VERSION_BINDING_SCHEMA: &str = "runx.payment.offer_version_binding.v1";
pub const QUOTE_PURCHASE_REQUEST_SCHEMA: &str = "runx.payment.quote_purchase.request.v1";
pub const QUOTE_PURCHASE_RESULT_SCHEMA: &str = "runx.payment.quote_purchase.result.v1";
pub const EXECUTE_PURCHASE_REQUEST_SCHEMA: &str = "runx.payment.execute_purchase.request.v1";
pub const EXECUTE_PURCHASE_RESULT_SCHEMA: &str = "runx.payment.execute_purchase.result.v1";
pub const GET_PURCHASE_REQUEST_SCHEMA: &str = "runx.payment.get_purchase.request.v1";
pub const GET_PURCHASE_RESULT_SCHEMA: &str = "runx.payment.get_purchase.result.v1";
pub const PURCHASE_READ_SCHEMA: &str = "runx.payment.purchase_read.v1";
pub const CANCEL_PURCHASE_REQUEST_SCHEMA: &str = "runx.payment.cancel_purchase.request.v1";
pub const CANCEL_PURCHASE_RESULT_SCHEMA: &str = "runx.payment.cancel_purchase.result.v1";

pub const QUOTE_PURCHASE: &str = "QuotePurchase";
pub const EXECUTE_PURCHASE: &str = "ExecutePurchase";
pub const GET_PURCHASE: &str = "GetPurchase";
pub const CANCEL_PURCHASE: &str = "CancelPurchase";

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

/// The settlement family runx's own ledger settles: an account buyer pays
/// from its runx balance, and the price moves inside runx.
pub const RUNX_BALANCE_SETTLEMENT_FAMILY: &str = "runx_balance";

/// A bounded, provider-neutral settlement-family identifier.
pub const SETTLEMENT_FAMILY_PATTERN: &str = "^[a-z0-9][a-z0-9._-]{0,63}$";

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

impl std::borrow::Borrow<str> for SettlementFamily {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl crate::schema::SchemaMapKey for SettlementFamily {
    const PATTERN: Option<&'static str> = Some(SETTLEMENT_FAMILY_PATTERN);
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
            "pattern": SETTLEMENT_FAMILY_PATTERN,
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

    pub fn accepts(&self, family: &str) -> bool {
        self.0.iter().any(|accepted| accepted.as_str() == family)
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

    /// Construct the canonical principal reference for an already-validated
    /// hosted Runx principal identifier.
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
        titled(schema, "PrincipalReference")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "PrincipalReference", Self::json_schema) {
            Reference::named_schemas(named);
        }
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
        titled(Reference::json_schema(), "PaymentReference")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "PaymentReference", Self::json_schema) {
            Reference::named_schemas(named);
        }
    }
}

/// The exact hosted run attached to a purchase.
///
/// The purchase feature owns only this reference. Hosted run state and output
/// remain behind the run-control service and its own store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PurchaseRunReference(Reference);

impl PurchaseRunReference {
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

impl<'de> Deserialize<'de> for PurchaseRunReference {
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

impl RunxSchema for PurchaseRunReference {
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
        titled(schema, "PurchaseRunReference")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "PurchaseRunReference", Self::json_schema) {
            Reference::named_schemas(named);
        }
    }
}

/// The sole V1 canonicalization identifier for digest-bound inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum PurchaseCanonicalizerVersion {
    #[serde(rename = "runx.receipt.c14n.v1")]
    ReceiptC14nV1,
}

impl PurchaseCanonicalizerVersion {
    pub const fn as_str(self) -> &'static str {
        RECEIPT_CANONICALIZATION
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaymentIdempotencyBinding {
    pub key: NonEmptyString,
    pub binding_digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.offer_version_binding.v1")]
pub struct OfferVersionBinding {
    pub offer_id: NonEmptyString,
    /// The package version the offer is sold at.
    pub version: NonEmptyString,
    /// The digest of the profile that declares the offer at that version.
    pub profile_digest: Digest,
    pub input_schema_digest: Digest,
    pub output_schema_digest: Digest,
}

/// The exact `OfferVersionBinding` wire value embedded inside another contract.
///
/// Standalone contract identity belongs only on the top-level schema. This
/// transparent wrapper prevents duplicate `$id` declarations when a document
/// contains more than one offer version reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EmbeddedOfferVersionBinding(OfferVersionBinding);

impl EmbeddedOfferVersionBinding {
    pub fn as_offer_version(&self) -> &OfferVersionBinding {
        &self.0
    }
}

impl From<OfferVersionBinding> for EmbeddedOfferVersionBinding {
    fn from(value: OfferVersionBinding) -> Self {
        Self(value)
    }
}

impl RunxSchema for EmbeddedOfferVersionBinding {
    fn json_schema() -> Value {
        let mut schema = OfferVersionBinding::json_schema();
        if let Some(object) = schema.as_object_mut() {
            object.remove("$id");
            object.remove("$schema");
            object.remove("x-runx-schema");
            if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
                properties.remove("schema");
            }
        }
        titled(schema, "EmbeddedOfferVersionBinding")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        record_named(named, "EmbeddedOfferVersionBinding", Self::json_schema);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum PurchasePaymentState {
    Unpaid,
    Settling,
    Settled,
    Refunded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum PurchaseExecutionState {
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
pub enum PurchaseOutcomeGate {
    Open,
    FulfilmentWon,
    RefundWon,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.purchase.v1")]
pub struct Purchase {
    pub purchase_id: NonEmptyString,
    /// The buyer: the principal that pays.
    pub buyer_ref: PrincipalReference,
    /// The seller: the publisher's principal enrolled to sell the skill.
    pub seller_ref: PrincipalReference,
    /// What is bought: the skill runner binding the seller's run performs.
    pub skill_ref: Reference,
    pub offer_version: OfferVersionBinding,
    /// The execution package digest the purchase runs (`runx.skill-package.v1`
    /// over the files a run loads), not the registry's digest of the published
    /// package files that a listing binds.
    pub package_digest: Digest,
    pub input_digest: Digest,
    pub canonicalizer_version: PurchaseCanonicalizerVersion,
    pub amount_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement_family: Option<SettlementFamily>,
    pub idempotency: PaymentIdempotencyBinding,
    /// The key the buyer claims with; runx's purchase receipt binds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buyer_key: Option<BuyerKey>,
    pub expires_at: IsoDateTime,
    pub payment_state: PurchasePaymentState,
    pub execution_state: PurchaseExecutionState,
    pub outcome_gate: PurchaseOutcomeGate,
    /// The purchase's act, fixed before any run: what the seller's run
    /// performs and capacity reserves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_job_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_ref: Option<PaymentReference>,
    pub created_at: IsoDateTime,
    pub updated_at: IsoDateTime,
}

/// Protocol-owned discovery and display terms. These do not authorize a
/// payment: the settlement executor verifies the complete challenge itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PurchasePaymentMethod {
    pub method: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pay_to: Option<NonEmptyString>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PurchasePaymentChallenge {
    pub settlement_family: SettlementFamily,
    /// Present only when the payment protocol carries a version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<NonEmptyString>,
    pub media_type: NonEmptyString,
    pub payload: JsonValue,
    pub payload_digest: Digest,
    pub quote_ref: Reference,
    pub quote_expires_at: IsoDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_method: Option<PurchasePaymentMethod>,
}

/// One exact quoted option. Its provider attempt retains this binding through
/// settlement and recovery; this is not another payment or purchase identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PurchasePaymentOptionBinding {
    pub settlement_family: SettlementFamily,
    pub payload_digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum PurchaseRefusalCode {
    OfferUnavailable,
    QuoteExpired,
    TermsChanged,
    ReplayConflict,
    PaymentNotAuthorized,
    CapacityUnavailable,
    NotFound,
    CancellationNotAvailable,
}

pub type PurchaseRefusalReason = BoundedString<512>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.quote_purchase.request.v1")]
pub struct QuotePurchaseRequest {
    pub buyer_ref: PrincipalReference,
    /// The seller: the publisher's principal enrolled to sell the skill.
    pub seller_ref: PrincipalReference,
    /// What is bought: the skill runner binding the seller's run performs.
    pub skill_ref: Reference,
    pub offer_version: OfferVersionBinding,
    /// The execution package digest the purchase runs (`runx.skill-package.v1`
    /// over the files a run loads), not the registry's digest of the published
    /// package files that a listing binds.
    pub package_digest: Digest,
    pub input_digest: Digest,
    pub canonicalizer_version: PurchaseCanonicalizerVersion,
    pub amount_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub accepted_settlement_families: SettlementFamilies,
    pub idempotency: PaymentIdempotencyBinding,
    /// The key the buyer claims with; runx's purchase receipt binds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buyer_key: Option<BuyerKey>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotePurchaseAdmission {
    pub purchase: Purchase,
    pub payment_options: crate::schema::NonEmptyVec<PurchasePaymentChallenge>,
}

impl RunxSchema for QuotePurchaseAdmission {
    fn json_schema() -> Value {
        let schema = object_schema(
            vec![
                Property::new("purchase", Purchase::json_schema(), true),
                Property::new(
                    "payment_options",
                    json!({ "type": "array", "minItems": 1, "items": { "$ref": "#/$defs/PurchasePaymentChallenge" } }),
                    true,
                ),
            ],
            true,
            None,
        );
        titled(schema, "QuotePurchaseAdmission")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "QuotePurchaseAdmission", Self::json_schema) {
            Purchase::named_schemas(named);
            PurchasePaymentChallenge::named_schemas(named);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotePurchaseResult {
    Admitted {
        value: Box<QuotePurchaseAdmission>,
    },
    Refused {
        code: PurchaseRefusalCode,
        reason: PurchaseRefusalReason,
    },
}

impl RunxSchema for QuotePurchaseResult {
    fn json_schema() -> Value {
        let admitted = object_schema(
            vec![
                Property::new("status", const_string("admitted"), true),
                Property::new("value", QuotePurchaseAdmission::json_schema(), true),
            ],
            true,
            None,
        );
        let refused = refusal_variant_schema();
        let mut schema = any_of_with_identity(
            vec![admitted, refused],
            Some(Identity::Runx {
                logical: QUOTE_PURCHASE_RESULT_SCHEMA,
                url: None,
            }),
        );
        if let Some(object) = schema.as_object_mut() {
            object.insert(
                "$defs".to_owned(),
                json!({
                    "PurchasePaymentChallenge": PurchasePaymentChallenge::json_schema(),
                    "PurchasePaymentOptionBinding": PurchasePaymentOptionBinding::json_schema()
                }),
            );
        }
        titled(schema, "QuotePurchaseResult")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "QuotePurchaseResult", Self::json_schema) {
            QuotePurchaseAdmission::named_schemas(named);
            PurchasePaymentOptionBinding::named_schemas(named);
            PurchaseRefusalCode::named_schemas(named);
            PurchaseRefusalReason::named_schemas(named);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.execute_purchase.request.v1")]
pub struct ExecutePurchaseRequest {
    pub purchase_id: NonEmptyString,
    pub settlement_family: SettlementFamily,
    pub payment_ref: PaymentReference,
    pub idempotency: PaymentIdempotencyBinding,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PurchaseAdmission {
    pub purchase: Purchase,
}

macro_rules! purchase_result {
    ($name:ident, $schema_id:literal) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
        #[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
        #[runx_schema(id = $schema_id)]
        pub enum $name {
            Admitted {
                value: Box<PurchaseAdmission>,
            },
            Refused {
                code: PurchaseRefusalCode,
                reason: PurchaseRefusalReason,
            },
        }
    };
}

purchase_result!(
    ExecutePurchaseResult,
    "runx.payment.execute_purchase.result.v1"
);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.get_purchase.request.v1")]
pub struct GetPurchaseRequest {
    pub purchase_id: NonEmptyString,
}

/// Bounded, customer-safe reason a refund won the outcome gate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PurchaseFailure {
    pub code: BoundedString<96>,
    pub message: BoundedString<500>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct GetPurchaseAdmission {
    pub purchase: Purchase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_ref: Option<PurchaseRunReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_ref: Option<Reference>,
    /// What fulfilled the purchase: the source run (act) or the durable
    /// continuation's result (artifact). Present only after fulfilment won.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<Reference>,
    /// Why a refund won. Present only after refund won.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<PurchaseFailure>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[runx_schema(id = "runx.payment.get_purchase.result.v1")]
pub enum GetPurchaseResult {
    Admitted {
        value: Box<GetPurchaseAdmission>,
    },
    Refused {
        code: PurchaseRefusalCode,
        reason: PurchaseRefusalReason,
    },
}

/// What a buyer reads of its purchase: the purchase, its receipt once
/// sealed, the runner's public result once fulfilment won, and what runx
/// holds for it. Never the seller's run, step outputs or grants, and never a
/// held value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.purchase_read.v1")]
pub struct PurchaseRead {
    pub purchase: Purchase,
    /// runx's signed purchase receipt, once sealed: what a claim presents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    /// Each resource runx holds for the purchase's buyer, once fulfilment won.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<PurchaseHold>,
    /// Why a refund won. Present only after refund won.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<PurchaseFailure>,
    /// What the reading account's runx balance holds, in micro-USD, while the
    /// purchase can still be paid from it. Never shown to a buyer key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance_micro: Option<u64>,
}

/// One resource a fulfilled purchase bought, held by runx for the purchase's
/// buyer (S10, N11): where it is held and until when, and the digest of the
/// held value, which tells holds apart without showing one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PurchaseHold {
    pub connector: NonEmptyString,
    pub credential: NonEmptyString,
    pub target: NonEmptyString,
    pub expires_at: IsoDateTime,
    pub value_digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.payment.cancel_purchase.request.v1")]
pub struct CancelPurchaseRequest {
    pub purchase_id: NonEmptyString,
    pub idempotency: PaymentIdempotencyBinding,
}

purchase_result!(
    CancelPurchaseResult,
    "runx.payment.cancel_purchase.result.v1"
);

fn refusal_variant_schema() -> Value {
    object_schema(
        vec![
            Property::new("status", const_string("refused"), true),
            Property::new("code", PurchaseRefusalCode::json_schema(), true),
            Property::new("reason", PurchaseRefusalReason::json_schema(), true),
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
            PurchaseCanonicalizerVersion::ReceiptC14nV1.as_str(),
            RECEIPT_CANONICALIZATION
        );
    }

    #[test]
    fn bounded_values_reject_ambiguous_forms() {
        assert!(Digest::new(format!("sha256:{}", "a".repeat(64))).is_some());
        assert!(Digest::new(format!("sha256:{}", "A".repeat(64))).is_none());
        assert!(CurrencyCode::new("USD").is_some());
        assert!(CurrencyCode::new("usd").is_none());
        assert!(SettlementFamily::new("hosted.mock-v1").is_some());
        assert!(SettlementFamily::new("Hosted").is_none());
    }

    #[test]
    fn paid_run_reference_requires_the_exact_run_namespace() {
        let run = Reference::with_uri(ReferenceType::Act, "runx:run:hosted-1");
        assert!(PurchaseRunReference::new(run).is_some());
        let authority = Reference::with_uri(ReferenceType::Act, "runx:act:hosted-1");
        assert!(PurchaseRunReference::new(authority).is_none());
        let wrong_type = Reference::with_uri(ReferenceType::Artifact, "runx:run:hosted-1");
        assert!(PurchaseRunReference::new(wrong_type).is_none());
        let malformed = Reference::with_uri(ReferenceType::Act, "runx:run:-hosted");
        assert!(PurchaseRunReference::new(malformed).is_none());
        let oversized =
            Reference::with_uri(ReferenceType::Act, format!("runx:run:{}", "r".repeat(257)));
        assert!(PurchaseRunReference::new(oversized).is_none());
    }
}
