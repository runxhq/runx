//! Type-driven JSON Schema for runx contracts (Phase 1 of
//! `rust-contract-pipeline-inversion`).
//!
//! A contract type that derives [`RunxSchema`] emits its own wire-conformant
//! JSON Schema document, so the Rust type is the single source of truth and the
//! parallel TypeScript schema sources stay deleted. The emitted document
//! reproduces the committed shape: fully inlined, closed string enums as
//! `type: string` with `enum`, `additionalProperties: false`, and the `$id` /
//! `x-runx-schema` identity.
// Module rationale: the schema emitter keeps shared JSON Schema
// construction helpers and primitive type impls together so generated contract
// shapes are reviewable as one boundary.

use std::collections::BTreeMap;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub use runx_contracts_derive::RunxSchema;

/// Deserialize a boolean field that MUST be `true`, failing closed otherwise.
///
/// The single forcing primitive shared by every proposal-only / human-gated
/// authority block (e.g. `OperationalProposalAuthority::proposal_only`): a
/// proposal can never claim an authority it was not granted, so the wire value
/// is rejected unless it is exactly `true`. Wire as
/// `#[serde(deserialize_with = "crate::schema::deserialize_true_bool")]`.
pub fn deserialize_true_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = bool::deserialize(deserializer)?;
    if value {
        Ok(true)
    } else {
        Err(de::Error::custom("value must be true"))
    }
}

/// Deserialize a boolean field that MUST be `false`, failing closed otherwise.
///
/// The forcing primitive's mirror: the granted-authority flags on a
/// proposal-only block must be exactly `false` so a reviewable handoff can never
/// deserialize into a self-granted consequence. Wire as
/// `#[serde(deserialize_with = "crate::schema::deserialize_false_bool")]`.
pub fn deserialize_false_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = bool::deserialize(deserializer)?;
    if value {
        Err(de::Error::custom("value must be false"))
    } else {
        Ok(false)
    }
}

/// A type that can emit its own JSON Schema document.
pub trait RunxSchema {
    /// The inlined JSON Schema for this type.
    fn json_schema() -> Value;
}

/// One object property: its wire name, schema, and whether it is required.
pub struct Property {
    pub name: &'static str,
    pub schema: Value,
    pub required: bool,
}

impl Property {
    pub fn new(name: &'static str, schema: Value, required: bool) -> Self {
        Self {
            name,
            schema,
            required,
        }
    }
}

/// The top-level identity envelope a contract document carries.
///
/// Most contracts are `Runx { logical }`: a `schemas.runx.ai` `$id` derived
/// from the logical name, the `x-runx-schema` marker, and an optional `schema`
/// const discriminant. A handful of legacy contracts carry only a bare `$id`
/// (the `runx.ai/spec` and `runx.ai/schemas` documents) with no
/// `x-runx-schema` and no injected `schema` discriminant; those use `BareId`.
pub enum Identity<'a> {
    /// Logical-name identity: `schemas.runx.ai` `$id`, `x-runx-schema` marker,
    /// and an injected optional `schema` const. The `$id` is `url` when given
    /// (for the few logical names whose canonical `$id` does not match the
    /// mechanical [`schema_id_url`] transform), otherwise derived.
    Runx {
        logical: &'a str,
        url: Option<&'a str>,
    },
    /// A bare `$id` with no `x-runx-schema` marker and no injected `schema`
    /// discriminant (the `runx.ai/spec` / `runx.ai/schemas` documents).
    BareId { url: &'a str },
}

/// Start a document with its top-level identity envelope, if any: `$schema`,
/// `$id`, and for [`Identity::Runx`] also `x-runx-schema`. Returns the logical
/// name of a [`Identity::Runx`] identity so object documents can inject their
/// optional `schema` discriminant; nested schemas pass `None` and start empty.
fn identity_envelope(identity: Option<Identity<'_>>) -> (Map<String, Value>, Option<&str>) {
    let mut schema = Map::new();
    let Some(identity) = identity else {
        return (schema, None);
    };
    schema.insert(
        "$schema".to_owned(),
        json!("https://json-schema.org/draft/2020-12/schema"),
    );
    match identity {
        Identity::Runx { logical, url } => {
            let id = url
                .map(str::to_owned)
                .unwrap_or_else(|| schema_id_url(logical));
            schema.insert("$id".to_owned(), json!(id));
            schema.insert("x-runx-schema".to_owned(), json!(logical));
            (schema, Some(logical))
        }
        Identity::BareId { url } => {
            schema.insert("$id".to_owned(), json!(url));
            (schema, None)
        }
    }
}

/// Assemble an object schema in the committed shape. When `identity` is set the
/// document carries the top-level envelope; nested objects pass `None`.
pub fn object_schema(
    properties: Vec<Property>,
    deny_unknown: bool,
    identity: Option<Identity<'_>>,
) -> Value {
    let mut required: Vec<Value> = Vec::new();
    let mut props = Map::new();
    for property in properties {
        if property.required {
            required.push(Value::String(property.name.to_owned()));
        }
        props.insert(property.name.to_owned(), property.schema);
    }

    let (mut schema, logical) = identity_envelope(identity);
    if let Some(logical) = logical {
        // Every top-level contract carries an optional `schema` discriminant
        // whose const equals its logical name. Emit it from the identity so no
        // type needs a redundant marker field.
        props
            .entry("schema".to_owned())
            .or_insert_with(|| const_string(logical));
    }
    schema.insert("additionalProperties".to_owned(), json!(!deny_unknown));
    schema.insert("type".to_owned(), json!("object"));
    if !required.is_empty() {
        schema.insert("required".to_owned(), Value::Array(required));
    }
    schema.insert("properties".to_owned(), Value::Object(props));
    Value::Object(schema)
}

/// Assemble an object schema, merging any `#[serde(flatten)]` fields. Each
/// entry in `flattened` is the emitted object schema of a flattened field's
/// type; its `properties` and `required` entries are lifted into the parent, as
/// serde does on the wire. A flattened object's own `additionalProperties` and
/// identity keys are dropped (only the parent's `deny_unknown` and `identity`
/// apply). `flattened` entries that are not plain objects (e.g. a flattened
/// map) relax the parent to accept additional properties, matching serde's
/// open-ended flatten capture.
// Function rationale: flatten merging mirrors serde's object
// projection rules and is easier to audit as one schema-construction path.
pub fn object_schema_with_flatten(
    properties: Vec<Property>,
    flattened: Vec<Value>,
    deny_unknown: bool,
    identity: Option<Identity<'_>>,
) -> Value {
    let mut required: Vec<Value> = Vec::new();
    let mut props = Map::new();
    for property in properties {
        if property.required {
            required.push(Value::String(property.name.to_owned()));
        }
        props.insert(property.name.to_owned(), property.schema);
    }

    // A flattened map (or any non-object schema) captures arbitrary extra keys,
    // so the parent can no longer be closed.
    let mut deny_unknown = deny_unknown;
    for flat in flattened {
        let is_object = flat.get("type").and_then(Value::as_str) == Some("object");
        match flat.get("properties").and_then(Value::as_object) {
            Some(inner_props) if is_object => {
                let inner_required: Vec<&str> = flat
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                for (name, schema) in inner_props {
                    if inner_required.contains(&name.as_str()) {
                        required.push(Value::String(name.clone()));
                    }
                    props.insert(name.clone(), schema.clone());
                }
            }
            _ => {
                // Non-object flatten (e.g. a `BTreeMap` capture) opens the
                // object to additional properties.
                deny_unknown = false;
            }
        }
    }

    let (mut schema, logical) = identity_envelope(identity);
    if let Some(logical) = logical {
        props
            .entry("schema".to_owned())
            .or_insert_with(|| const_string(logical));
    }
    schema.insert("additionalProperties".to_owned(), json!(!deny_unknown));
    schema.insert("type".to_owned(), json!("object"));
    if !required.is_empty() {
        schema.insert("required".to_owned(), Value::Array(required));
    }
    schema.insert("properties".to_owned(), Value::Object(props));
    Value::Object(schema)
}

/// Assemble an open-map ("dictionary") document in the committed shape: an
/// object whose values all match `value_schema`, rendered with the committed
/// `patternProperties: { "^(.*)$": <value schema> }` form. When `identity` is
/// set the document carries the top-level envelope (the `output.schema.json`
/// document is a bare-`$id` map of this kind). No `additionalProperties` and no
/// injected `schema` discriminant are emitted; the pattern alone constrains the
/// values.
pub fn object_map_schema(value_schema: Value, identity: Option<Identity<'_>>) -> Value {
    let (mut schema, _) = identity_envelope(identity);
    schema.insert("type".to_owned(), json!("object"));
    schema.insert(
        "patternProperties".to_owned(),
        json!({ "^(.*)$": value_schema }),
    );
    Value::Object(schema)
}

/// A closed string enum: `{ "enum": [...], "type": "string" }`, optionally
/// carrying a top-level identity envelope (the `$schema`, `$id`, and for
/// [`Identity::Runx`] also `x-runx-schema` keys). One `enum` keyword names the
/// whole value set, so code generators read a typed enum instead of a union of
/// single-value subschemas.
pub fn string_enum(variants: &[&str], identity: Option<Identity<'_>>) -> Value {
    let (mut schema, _) = identity_envelope(identity);
    schema.insert("type".to_owned(), json!("string"));
    schema.insert("enum".to_owned(), json!(variants));
    Value::Object(schema)
}

/// A union of subschemas rendered as `{ "anyOf": [...] }`, the committed shape
/// for data-carrying enums (externally-tagged, internally-tagged, and untagged
/// representations all collapse to an `anyOf` of variant subschemas).
pub fn any_of(variants: Vec<Value>) -> Value {
    json!({ "anyOf": variants })
}

/// An `anyOf` union of variant subschemas carrying a top-level identity
/// envelope. Used by data-carrying enums that are themselves a contract
/// document (e.g. the `runx.ai/spec` documents emitted as a bare-`$id`
/// `anyOf`). The identity keys (`$schema`, `$id`, and for [`Identity::Runx`]
/// also `x-runx-schema`) sit alongside the `anyOf`. Unlike [`object_schema`],
/// no injected `schema` discriminant property is added: the union variants own
/// their own shape.
pub fn any_of_with_identity(variants: Vec<Value>, identity: Option<Identity<'_>>) -> Value {
    let (mut schema, _) = identity_envelope(identity);
    schema.insert("anyOf".to_owned(), Value::Array(variants));
    Value::Object(schema)
}

/// A required-but-nullable property schema: the inner type's schema unioned with
/// `null`. Matches the committed shape for an `Option<T>` field that has no
/// `skip_serializing_if` (it must be present on the wire but may be `null`):
/// `{ "anyOf": [<T schema>, { "type": "null" }] }`.
pub fn nullable(inner: Value) -> Value {
    json!({ "anyOf": [inner, { "type": "null" }] })
}

/// An externally-tagged data variant: a single-key object `{ "<tag>": <inner> }`
/// where the key is the variant's wire name and the value is its payload schema.
/// Matches serde's default (externally-tagged) struct/tuple-variant encoding.
pub fn externally_tagged_variant(tag: &'static str, inner: Value) -> Value {
    object_schema(vec![Property::new(tag, inner, true)], true, None)
}

/// A single string literal leaf: `{ "const": <s>, "type": "string" }`.
pub fn const_string(value: &str) -> Value {
    json!({ "const": value, "type": "string" })
}

/// Map a logical schema name (`runx.reference.v1`) to its canonical `$id` URL
/// (`https://schemas.runx.ai/runx/reference/v1.json`). Each dot-delimited
/// segment is path-joined with `/`, and underscores within a segment become
/// hyphens (`runx.external_adapter.response.v1` ->
/// `.../runx/external-adapter/response/v1.json`).
pub fn schema_id_url(logical: &str) -> String {
    let path = logical
        .split('.')
        .map(|segment| segment.replace('_', "-"))
        .collect::<Vec<_>>()
        .join("/");
    format!("https://schemas.runx.ai/{path}.json")
}

impl RunxSchema for String {
    fn json_schema() -> Value {
        json!({ "type": "string" })
    }
}

impl RunxSchema for bool {
    fn json_schema() -> Value {
        json!({ "type": "boolean" })
    }
}

impl RunxSchema for f64 {
    fn json_schema() -> Value {
        json!({ "type": "number" })
    }
}

macro_rules! integer_schema {
    ($($ty:ty),+) => {
        $(impl RunxSchema for $ty {
            fn json_schema() -> Value {
                json!({ "type": "integer" })
            }
        })+
    };
}
integer_schema!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl RunxSchema for std::num::NonZeroU64 {
    fn json_schema() -> Value {
        json!({ "type": "integer", "minimum": 1 })
    }
}

impl<T: RunxSchema> RunxSchema for Vec<T> {
    fn json_schema() -> Value {
        json!({ "type": "array", "items": T::json_schema() })
    }
}

/// A non-empty array (`minItems: 1`) for contract fields where an empty list
/// would erase the proof-bearing edge the record exists to carry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct NonEmptyVec<T>(Vec<T>);

impl<T> NonEmptyVec<T> {
    pub fn new(value: Vec<T>) -> Option<Self> {
        if value.is_empty() {
            None
        } else {
            Some(Self(value))
        }
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
}

impl<T> From<Vec<T>> for NonEmptyVec<T> {
    fn from(value: Vec<T>) -> Self {
        debug_assert!(
            !value.is_empty(),
            "NonEmptyVec::from received an empty outbound value"
        );
        Self(value)
    }
}

impl<T> std::ops::Deref for NonEmptyVec<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de, T> Deserialize<'de> for NonEmptyVec<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Vec::<T>::deserialize(deserializer)?;
        Self::new(value)
            .ok_or_else(|| serde::de::Error::custom("array must contain at least one item"))
    }
}

impl<T: RunxSchema> RunxSchema for NonEmptyVec<T> {
    fn json_schema() -> Value {
        let mut schema = Vec::<T>::json_schema();
        if let Some(object) = schema.as_object_mut() {
            object.insert("minItems".to_owned(), json!(1));
        }
        schema
    }
}

impl<T: RunxSchema> RunxSchema for Option<T> {
    fn json_schema() -> Value {
        T::json_schema()
    }
}

impl<T: RunxSchema> RunxSchema for Box<T> {
    fn json_schema() -> Value {
        T::json_schema()
    }
}

impl<T: RunxSchema> RunxSchema for BTreeMap<String, T> {
    fn json_schema() -> Value {
        json!({ "type": "object", "additionalProperties": T::json_schema() })
    }
}

/// A non-empty string (`minLength: 1`), the ubiquitous contract constraint. It
/// validates on deserialization so an empty value cannot cross the wire
/// boundary, and emits `{ minLength: 1, type: string }`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    /// Construct from any string-like, returning `None` for an empty value.
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        if value.is_empty() {
            None
        } else {
            Some(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

// Infallible wraps for ergonomics: the wire-in guarantee (non-empty) is
// enforced on deserialization, where untrusted input crosses the boundary.
impl From<String> for NonEmptyString {
    fn from(value: String) -> Self {
        debug_assert!(
            !value.is_empty(),
            "NonEmptyString::from received an empty outbound value"
        );
        Self(value)
    }
}

impl From<&str> for NonEmptyString {
    fn from(value: &str) -> Self {
        debug_assert!(
            !value.is_empty(),
            "NonEmptyString::from received an empty outbound value"
        );
        Self(value.to_owned())
    }
}

impl PartialEq<String> for NonEmptyString {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl PartialEq<&str> for NonEmptyString {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<NonEmptyString> for String {
    fn eq(&self, other: &NonEmptyString) -> bool {
        self == &other.0
    }
}

impl PartialEq<NonEmptyString> for str {
    fn eq(&self, other: &NonEmptyString) -> bool {
        self == other.0.as_str()
    }
}

impl std::ops::Deref for NonEmptyString {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for NonEmptyString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NonEmptyString {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl PartialEq<str> for NonEmptyString {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl<'de> Deserialize<'de> for NonEmptyString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| serde::de::Error::custom("string must be non-empty"))
    }
}

impl RunxSchema for NonEmptyString {
    fn json_schema() -> Value {
        json!({ "minLength": 1, "type": "string" })
    }
}

/// A non-empty string with a contract-visible maximum length.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct BoundedString<const MAX_LENGTH: usize>(String);

impl<const MAX_LENGTH: usize> BoundedString<MAX_LENGTH> {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty() && value.chars().count() <= MAX_LENGTH).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de, const MAX_LENGTH: usize> Deserialize<'de> for BoundedString<MAX_LENGTH> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            de::Error::custom(format!(
                "string length must be between 1 and {MAX_LENGTH} characters"
            ))
        })
    }
}

impl<const MAX_LENGTH: usize> RunxSchema for BoundedString<MAX_LENGTH> {
    fn json_schema() -> Value {
        json!({ "minLength": 1, "maxLength": MAX_LENGTH, "type": "string" })
    }
}

/// An array whose inclusive item bounds are enforced on the wire and emitted
/// into the same JSON Schema inspected by operators and agents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BoundedVec<T, const MIN_ITEMS: usize, const MAX_ITEMS: usize>(Vec<T>);

impl<T, const MIN_ITEMS: usize, const MAX_ITEMS: usize> BoundedVec<T, MIN_ITEMS, MAX_ITEMS> {
    pub fn new(value: Vec<T>) -> Option<Self> {
        (MIN_ITEMS..=MAX_ITEMS)
            .contains(&value.len())
            .then_some(Self(value))
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
}

impl<'de, T, const MIN_ITEMS: usize, const MAX_ITEMS: usize> Deserialize<'de>
    for BoundedVec<T, MIN_ITEMS, MAX_ITEMS>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Vec::<T>::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            de::Error::custom(format!(
                "array length must be between {MIN_ITEMS} and {MAX_ITEMS} items"
            ))
        })
    }
}

impl<T: RunxSchema, const MIN_ITEMS: usize, const MAX_ITEMS: usize> RunxSchema
    for BoundedVec<T, MIN_ITEMS, MAX_ITEMS>
{
    fn json_schema() -> Value {
        json!({
            "type": "array",
            "minItems": MIN_ITEMS,
            "maxItems": MAX_ITEMS,
            "items": T::json_schema(),
        })
    }
}

/// The ISO-8601 datetime pattern the contracts commit to (`...Z`, optional
/// fractional seconds).
pub const ISO_DATETIME_PATTERN: &str = r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$";

/// A non-empty ISO-8601 datetime string. Emits `{ minLength: 1, pattern, type }`.
/// Validation of the pattern itself stays at the schema layer (the wire
/// contract); this newtype only guarantees non-emptiness in Rust.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct IsoDateTime(String);

impl IsoDateTime {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        if value.is_empty() {
            None
        } else {
            Some(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for IsoDateTime {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for IsoDateTime {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::ops::Deref for IsoDateTime {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for IsoDateTime {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<String> for IsoDateTime {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl PartialEq<&str> for IsoDateTime {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<str> for IsoDateTime {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<IsoDateTime> for String {
    fn eq(&self, other: &IsoDateTime) -> bool {
        self == &other.0
    }
}

impl PartialEq<IsoDateTime> for str {
    fn eq(&self, other: &IsoDateTime) -> bool {
        self == other.0.as_str()
    }
}

impl std::fmt::Display for IsoDateTime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IsoDateTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| serde::de::Error::custom("datetime must be non-empty"))
    }
}

impl RunxSchema for IsoDateTime {
    fn json_schema() -> Value {
        json!({ "minLength": 1, "pattern": ISO_DATETIME_PATTERN, "type": "string" })
    }
}
