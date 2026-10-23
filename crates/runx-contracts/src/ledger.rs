//! Ledger-entry contract used for artifact chain records.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::schema::{
    NamedSchemas, NonEmptyString, Property, RunxSchema, object_schema, record_named, titled,
};
use crate::{DigestAlgorithm, JsonObject};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum LedgerEntrySchemaVersion {
    #[serde(rename = "runx.ledger.entry.v1")]
    V1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum LedgerChainVersion {
    #[serde(rename = "runx.ledger.chain.v1")]
    V1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum LedgerCanonicalization {
    #[serde(rename = "runx.stable-json.v1")]
    StableJsonV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum LedgerPayloadVersion {
    #[serde(rename = "1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct LedgerDigestHex(String);

impl<'de> Deserialize<'de> for LedgerDigestHex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.len() == 64
            && value
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            Ok(Self(value))
        } else {
            Err(serde::de::Error::custom(
                "ledger digest must be 64 lowercase hex characters",
            ))
        }
    }
}

impl RunxSchema for LedgerDigestHex {
    fn json_schema() -> Value {
        json!({ "pattern": "^[a-f0-9]{64}$", "type": "string" })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct LedgerChain {
    pub version: LedgerChainVersion,
    pub algorithm: DigestAlgorithm,
    pub canonicalization: LedgerCanonicalization,
    pub index: u64,
    pub previous_digest: Option<LedgerDigestHex>,
    pub entry_digest: LedgerDigestHex,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct LedgerProducer {
    pub skill: NonEmptyString,
    pub runner: NonEmptyString,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct LedgerEntryMeta {
    pub artifact_id: NonEmptyString,
    pub run_id: NonEmptyString,
    pub step_id: Option<NonEmptyString>,
    pub producer: LedgerProducer,
    pub created_at: NonEmptyString,
    pub digest: NonEmptyString,
    pub size_bytes: u64,
    pub parent_artifact_id: Option<NonEmptyString>,
    pub receipt_id: Option<NonEmptyString>,
    pub redacted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct LedgerPayload {
    #[serde(rename = "type")]
    pub entry_type: Option<NonEmptyString>,
    pub version: LedgerPayloadVersion,
    pub data: JsonObject,
    pub meta: LedgerEntryMeta,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerEntry {
    pub schema_version: LedgerEntrySchemaVersion,
    pub chain: LedgerChain,
    pub entry: LedgerPayload,
}

impl RunxSchema for LedgerEntry {
    fn json_schema() -> Value {
        let mut schema = object_schema(
            vec![
                Property::new(
                    "schema_version",
                    LedgerEntrySchemaVersion::json_schema(),
                    true,
                ),
                Property::new("chain", LedgerChain::json_schema(), true),
                Property::new("entry", LedgerPayload::json_schema(), true),
            ],
            true,
            None,
        );
        if let Some(object) = schema.as_object_mut() {
            object.insert(
                "$schema".to_owned(),
                json!("https://json-schema.org/draft/2020-12/schema"),
            );
            object.insert(
                "$id".to_owned(),
                json!("https://schemas.runx.ai/runx/ledger-entry/v1.json"),
            );
            object.insert("x-runx-schema".to_owned(), json!("runx.ledger.entry.v1"));
        }
        titled(schema, "LedgerEntry")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "LedgerEntry", Self::json_schema) {
            LedgerEntrySchemaVersion::named_schemas(named);
            LedgerChain::named_schemas(named);
            LedgerPayload::named_schemas(named);
        }
    }
}
