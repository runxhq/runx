//! External x402 v2 wire contracts and the Runx-owned v1 declaration.
//!
//! External values deliberately preserve unknown fields for tolerant-reader
//! interoperability. The `runx.invocation` declaration is strict and reuses
//! the immutable commitment types owned by [`crate::paid_invocation`]. This
//! module is data-only: HTTP, base64, assembly, provider calls, and settlement
//! behavior do not belong here.

use std::fmt;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Value, json};

use crate::schema::{Identity, NonEmptyString, Property, RunxSchema, nullable, object_schema};
use crate::{
    JsonObject, JsonValue, OfferRevisionRef, PaidInvocationCanonicalizerVersion,
    ParentInvocationBinding, PaymentIdempotencyBinding, Reference, Sha256Digest,
};

pub const X402_PROTOCOL_VERSION: u8 = 2;
pub const X402_UPSTREAM_COMMIT: &str = "71eb9a55e081e7b81ba3046d0bd17c3eb9c7bf81";
pub const X402_UPSTREAM_PACKAGE: &str = "@x402/core";
pub const X402_UPSTREAM_PACKAGE_VERSION: &str = "2.27.0";

pub const X402_PAYMENT_REQUIRED_HEADER: &str = "PAYMENT-REQUIRED";
pub const X402_PAYMENT_SIGNATURE_HEADER: &str = "PAYMENT-SIGNATURE";
pub const X402_PAYMENT_RESPONSE_HEADER: &str = "PAYMENT-RESPONSE";
pub const RUNX_INVOCATION_EXTENSION_KEY: &str = "runx.invocation";

pub const X402_RESOURCE_INFO_SCHEMA_ID: &str =
    "https://schemas.runx.ai/external/x402/v2/resource-info.schema.json";
pub const X402_PAYMENT_REQUIREMENTS_SCHEMA_ID: &str =
    "https://schemas.runx.ai/external/x402/v2/payment-requirements.schema.json";
pub const X402_PAYMENT_REQUIRED_SCHEMA_ID: &str =
    "https://schemas.runx.ai/external/x402/v2/payment-required.schema.json";
pub const X402_PAYMENT_PAYLOAD_SCHEMA_ID: &str =
    "https://schemas.runx.ai/external/x402/v2/payment-payload.schema.json";
pub const X402_SETTLE_RESPONSE_SCHEMA_ID: &str =
    "https://schemas.runx.ai/external/x402/v2/settle-response.schema.json";
pub const RUNX_X402_INVOCATION_EXTENSION_SCHEMA: &str = "runx.x402.invocation_extension.v1";

/// The only external protocol version accepted or emitted by this package.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct X402Version2;

impl Serialize for X402Version2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(X402_PROTOCOL_VERSION)
    }
}

impl<'de> Deserialize<'de> for X402Version2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version = u64::deserialize(deserializer)?;
        if version == u64::from(X402_PROTOCOL_VERSION) {
            Ok(Self)
        } else {
            Err(de::Error::custom("x402Version must be 2"))
        }
    }
}

impl RunxSchema for X402Version2 {
    fn json_schema() -> Value {
        json!({ "type": "integer", "const": X402_PROTOCOL_VERSION })
    }
}

/// x402 v2's upstream CAIP-2 reader: at least three bytes and one colon.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct X402Network(String);

impl X402Network {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (value.len() >= 3 && value.contains(':')).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for X402Network {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("network must be a CAIP-2 identifier"))
    }
}

impl RunxSchema for X402Network {
    fn json_schema() -> Value {
        json!({ "type": "string", "minLength": 3, "pattern": "^.*:.*$" })
    }
}

/// A positive finite JSON number, matching the upstream timeout validator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct X402PositiveNumber(f64);

impl X402PositiveNumber {
    pub fn new(value: f64) -> Option<Self> {
        (value.is_finite() && value > 0.0).then_some(Self(value))
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for X402PositiveNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("value must be positive and finite"))
    }
}

impl Serialize for X402PositiveNumber {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.0.fract() == 0.0 && self.0 <= u64::MAX as f64 {
            serializer.serialize_u64(self.0 as u64)
        } else {
            serializer.serialize_f64(self.0)
        }
    }
}

impl RunxSchema for X402PositiveNumber {
    fn json_schema() -> Value {
        json!({ "type": "number", "exclusiveMinimum": 0 })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct X402ServiceName(String);

impl X402ServiceName {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        printable_ascii(&value, 32).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for X402ServiceName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("invalid x402 serviceName"))
    }
}

impl RunxSchema for X402ServiceName {
    fn json_schema() -> Value {
        printable_ascii_schema(32)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct X402Tag(String);

impl X402Tag {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        printable_ascii(&value, 32).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for X402Tag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("invalid x402 tag"))
    }
}

impl RunxSchema for X402Tag {
    fn json_schema() -> Value {
        printable_ascii_schema(32)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct X402Tags(Vec<X402Tag>);

impl X402Tags {
    pub fn new(value: Vec<X402Tag>) -> Option<Self> {
        (value.len() <= 5).then_some(Self(value))
    }

    pub fn as_slice(&self) -> &[X402Tag] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for X402Tags {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Vec::<X402Tag>::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("x402 tags must contain at most 5 values"))
    }
}

impl RunxSchema for X402Tags {
    fn json_schema() -> Value {
        json!({ "type": "array", "items": X402Tag::json_schema(), "maxItems": 5 })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct X402IconUrl(String);

impl X402IconUrl {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (value.len() <= 2_048).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for X402IconUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("invalid x402 iconUrl"))
    }
}

impl RunxSchema for X402IconUrl {
    fn json_schema() -> Value {
        json!({ "type": "string", "maxLength": 2048 })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402ResourceInfo {
    pub url: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<X402ServiceName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<X402Tags>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<X402IconUrl>,
    #[serde(flatten)]
    pub additional: JsonObject,
}

impl RunxSchema for X402ResourceInfo {
    fn json_schema() -> Value {
        external_object_schema(
            X402_RESOURCE_INFO_SCHEMA_ID,
            vec![
                Property::new("url", NonEmptyString::json_schema(), true),
                optional_nullable("description", String::json_schema()),
                optional_nullable("mimeType", String::json_schema()),
                optional_nullable("serviceName", X402ServiceName::json_schema()),
                optional_nullable("tags", X402Tags::json_schema()),
                optional_nullable("iconUrl", X402IconUrl::json_schema()),
            ],
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402PaymentRequirements {
    pub scheme: NonEmptyString,
    pub network: X402Network,
    pub amount: NonEmptyString,
    pub asset: NonEmptyString,
    pub pay_to: NonEmptyString,
    pub max_timeout_seconds: X402PositiveNumber,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<JsonObject>,
    #[serde(flatten)]
    pub additional: JsonObject,
}

impl RunxSchema for X402PaymentRequirements {
    fn json_schema() -> Value {
        external_object_schema(
            X402_PAYMENT_REQUIREMENTS_SCHEMA_ID,
            vec![
                Property::new("scheme", NonEmptyString::json_schema(), true),
                Property::new("network", X402Network::json_schema(), true),
                Property::new("amount", NonEmptyString::json_schema(), true),
                Property::new("asset", NonEmptyString::json_schema(), true),
                Property::new("payTo", NonEmptyString::json_schema(), true),
                Property::new("maxTimeoutSeconds", X402PositiveNumber::json_schema(), true),
                optional_nullable("extra", JsonObject::json_schema()),
            ],
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct X402AcceptedRequirements(Vec<X402PaymentRequirements>);

impl X402AcceptedRequirements {
    pub fn new(value: Vec<X402PaymentRequirements>) -> Option<Self> {
        (!value.is_empty()).then_some(Self(value))
    }

    pub fn as_slice(&self) -> &[X402PaymentRequirements] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for X402AcceptedRequirements {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Vec::<X402PaymentRequirements>::deserialize(deserializer)?;
        Self::new(value)
            .ok_or_else(|| de::Error::custom("accepts must contain at least one requirement"))
    }
}

impl RunxSchema for X402AcceptedRequirements {
    fn json_schema() -> Value {
        json!({
            "type": "array",
            "items": X402PaymentRequirements::json_schema(),
            "minItems": 1,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402PaymentRequired {
    pub x402_version: X402Version2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub resource: X402ResourceInfo,
    pub accepts: X402AcceptedRequirements,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<JsonObject>,
    #[serde(flatten)]
    pub additional: JsonObject,
}

impl RunxSchema for X402PaymentRequired {
    fn json_schema() -> Value {
        external_object_schema(
            X402_PAYMENT_REQUIRED_SCHEMA_ID,
            vec![
                Property::new("x402Version", X402Version2::json_schema(), true),
                optional_nullable("error", String::json_schema()),
                Property::new("resource", X402ResourceInfo::json_schema(), true),
                Property::new("accepts", X402AcceptedRequirements::json_schema(), true),
                optional_nullable("extensions", JsonObject::json_schema()),
            ],
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402PaymentPayload {
    pub x402_version: X402Version2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<X402ResourceInfo>,
    pub accepted: X402PaymentRequirements,
    pub payload: JsonObject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<JsonObject>,
    #[serde(flatten)]
    pub additional: JsonObject,
}

impl RunxSchema for X402PaymentPayload {
    fn json_schema() -> Value {
        external_object_schema(
            X402_PAYMENT_PAYLOAD_SCHEMA_ID,
            vec![
                Property::new("x402Version", X402Version2::json_schema(), true),
                optional_nullable("resource", X402ResourceInfo::json_schema()),
                Property::new("accepted", X402PaymentRequirements::json_schema(), true),
                Property::new("payload", JsonObject::json_schema(), true),
                optional_nullable("extensions", JsonObject::json_schema()),
            ],
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402SettleResponse {
    pub success: bool,
    pub transaction: String,
    pub network: X402Network,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<JsonObject>,
    #[serde(flatten)]
    pub additional: JsonObject,
}

impl RunxSchema for X402SettleResponse {
    fn json_schema() -> Value {
        external_object_schema(
            X402_SETTLE_RESPONSE_SCHEMA_ID,
            vec![
                Property::new("success", bool::json_schema(), true),
                Property::new("transaction", String::json_schema(), true),
                Property::new("network", X402Network::json_schema(), true),
                optional_nullable("errorReason", String::json_schema()),
                optional_nullable("errorMessage", String::json_schema()),
                optional_nullable("payer", String::json_schema()),
                optional_nullable("amount", String::json_schema()),
                optional_nullable("extensions", JsonObject::json_schema()),
                optional_nullable("extra", JsonObject::json_schema()),
            ],
        )
    }
}

/// Strict Runx information advertised under the external `runx.invocation` key.
///
/// This is a declaration only. The authoritative challenge binding remains the
/// `PaidInvocationPaymentChallenge` `quote_ref` and `payload_digest` pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "purpose", rename_all = "snake_case", deny_unknown_fields)]
#[runx_schema(id = "runx.x402.invocation_extension.v1")]
pub enum RunxX402InvocationExtensionInfo {
    Discovery {
        offer_revision: OfferRevisionRef,
        package_digest: Sha256Digest,
    },
    Invocation {
        invocation_id: NonEmptyString,
        quote_ref: Box<Reference>,
        offer_revision: OfferRevisionRef,
        package_digest: Sha256Digest,
        input_digest: Sha256Digest,
        canonicalizer_version: PaidInvocationCanonicalizerVersion,
        idempotency: PaymentIdempotencyBinding,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<ParentInvocationBinding>,
    },
}

/// The standard external x402 `{ info, schema }` declaration, specialized to
/// the strict Runx v1 invocation projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RunxX402InvocationExtension {
    pub info: RunxX402InvocationExtensionInfo,
    pub schema: JsonObject,
}

fn external_object_schema(id: &'static str, properties: Vec<Property>) -> Value {
    object_schema(properties, false, Some(Identity::BareId { url: id }))
}

fn optional_nullable(name: &'static str, schema: Value) -> Property {
    Property::new(name, nullable(schema), false)
}

fn printable_ascii(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
}

fn printable_ascii_schema(max_len: usize) -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "maxLength": max_len,
        "pattern": "^[\\x20-\\x7e]+$",
    })
}

impl fmt::Display for X402Network {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Convert a typed extension declaration into the opaque external extension map value.
pub fn runx_invocation_extension_value(
    extension: &RunxX402InvocationExtension,
) -> Result<JsonValue, serde_json::Error> {
    serde_json::from_value(serde_json::to_value(extension)?)
}

/// Decode the opaque external extension map value into the strict Runx declaration.
pub fn parse_runx_invocation_extension(
    value: JsonValue,
) -> Result<RunxX402InvocationExtension, de::value::Error> {
    value.deserialize_into()
}
