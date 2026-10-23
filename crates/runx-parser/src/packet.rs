//! Pure parsing for Runx packet-schema documents.
//!
//! Filesystem discovery and catalog policy belong to the runtime. This module
//! owns the document contract so listing, execution, and publication cannot
//! disagree about packet identity or digests.

use runx_contracts::{JsonValue, PACKET_ID_EXPECTED, PacketId, sha256_prefixed};
use thiserror::Error;

pub const PACKET_ID_FIELD: &str = "x-runx-packet-id";

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedPacketSchema {
    pub packet_id: PacketId,
    pub value: JsonValue,
    pub digest: String,
}

#[derive(Debug, Error)]
pub enum PacketSchemaError {
    #[error("{path}: packet schema is not valid JSON: {source}")]
    InvalidJson {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path}: {PACKET_ID_FIELD} is not a packet id; {PACKET_ID_EXPECTED}")]
    InvalidPacketId { path: String },
    #[error(
        "{path}: {packet_id} is a core packet id; core packets are compiled into runx, and a packet file declares its package's id"
    )]
    CorePacketFile { path: String, packet_id: PacketId },
    #[error("{path}: $id of {packet_id} must be {expected}")]
    SchemaUrl {
        path: String,
        packet_id: PacketId,
        expected: String,
    },
}

/// Parse one JSON document as a package's packet schema.
///
/// JSON without `x-runx-packet-id` is not a packet schema and returns `None`.
/// Once the field is present it is strict: the id is a package id, never a
/// core one, and `$id` is that id's canonical schema URL.
pub fn parse_packet_schema_document(
    path: impl Into<String>,
    source: &str,
) -> Result<Option<ValidatedPacketSchema>, PacketSchemaError> {
    let path = path.into();
    let value = serde_json::from_str::<JsonValue>(source).map_err(|source| {
        PacketSchemaError::InvalidJson {
            path: path.clone(),
            source,
        }
    })?;
    let Some(raw_packet_id) = value
        .as_object()
        .and_then(|object| object.get(PACKET_ID_FIELD))
    else {
        return Ok(None);
    };
    let Some(packet_id) = raw_packet_id.as_str().and_then(PacketId::new) else {
        return Err(PacketSchemaError::InvalidPacketId { path });
    };
    if packet_id.is_core() {
        return Err(PacketSchemaError::CorePacketFile { path, packet_id });
    }
    let expected = packet_id.schema_url();
    if value
        .as_object()
        .and_then(|object| object.get("$id"))
        .and_then(JsonValue::as_str)
        != Some(expected.as_str())
    {
        return Err(PacketSchemaError::SchemaUrl {
            path,
            packet_id,
            expected,
        });
    }
    Ok(Some(ValidatedPacketSchema {
        packet_id,
        value,
        digest: sha256_prefixed(source.as_bytes()),
    }))
}
