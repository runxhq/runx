//! Source packet contract: normalized intake with an explicit redaction status.
//!
//! `SourcePacket` is the canonical wire shape every source adapter produces
//! before handing off to triage / action skills. It carries provider-neutral
//! identity (`source_ref`) and the typed workflow slots that downstream skills
//! consume, plus optional raw `adapter_payload` for replay and audit.
//!
//! Provider names and locators live inside the central refs, not top-level
//! fields, so the same packet can be rendered through Slack, GitHub, Teams,
//! Linear, Jira, email, or any other channel without contract changes.

use serde::{Deserialize, Serialize};

use crate::operational_proposal::OperationalProposalRedactionStatus;
use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::{ContentDigest, JsonObject, Reference, SignalAuthenticity};

pub const SOURCE_PACKET_SCHEMA: &str = "runx.source_packet.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum SourcePacketSchema {
    #[serde(rename = "runx.source_packet.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.source_packet.v1")]
pub struct SourcePacket {
    pub schema: SourcePacketSchema,
    /// Stable identifier for this packet. Adapters typically derive this from
    /// `source_ref` plus a content hash so re-deliveries are idempotent.
    pub packet_id: NonEmptyString,
    /// Central runx reference identifying the source. Provider names and
    /// locators live inside the reference; the packet itself stays
    /// provider-neutral.
    pub source_ref: Reference,
    /// Open signal type identifier (e.g. `alert`, `support_ticket`). Adapters
    /// can publish their own identifier without a contract edit.
    pub signal_type: NonEmptyString,
    pub title: NonEmptyString,
    pub observed_at: IsoDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_preview: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authenticity: Option<SignalAuthenticity>,
    /// Redaction status applied to `body_preview`, `workflow_inputs`, and any
    /// human-visible fields in `adapter_payload`.
    pub redaction_status: OperationalProposalRedactionStatus,
    /// Typed workflow slots that the downstream triage / action skill reads.
    /// The contract carries them as opaque JSON; the consuming skill is
    /// responsible for shape-validating its own slice.
    #[serde(default, skip_serializing_if = "JsonObject::is_empty")]
    pub workflow_inputs: JsonObject,
    /// Raw adapter payload. Optional; held for replay and audit. Must already
    /// have any redactions applied that `redaction_status` declares.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_payload: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<ContentDigest>,
    /// Related refs the adapter wants to surface alongside the source: thread,
    /// parent issue, dedupe candidates, originating webhook delivery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_refs: Vec<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<JsonObject>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReferenceType;

    #[test]
    fn source_packet_round_trips_minimal_shape() -> Result<(), serde_json::Error> {
        let packet = SourcePacket {
            schema: SourcePacketSchema::V1,
            packet_id: "src_pkt_001".into(),
            source_ref: Reference {
                reference_type: ReferenceType::Thread,
                uri: "slack://team/T1/channel/C1/thread/1700000000.0001".into(),
                provider: Some("slack".into()),
                locator: Some("team/C1/1700000000.0001".into()),
                label: None,
                observed_at: None,
                proof_kind: None,
            },
            signal_type: "chat_message".into(),
            title: "Incoming customer message".into(),
            observed_at: "2026-05-28T12:00:00Z".into(),
            body_preview: Some("Customer reports delivery failure".into()),
            authenticity: None,
            redaction_status: OperationalProposalRedactionStatus::Redacted,
            workflow_inputs: JsonObject::new(),
            adapter_payload: None,
            digest: None,
            related_refs: Vec::new(),
            extensions: None,
        };

        let json = serde_json::to_value(&packet)?;
        let round_tripped: SourcePacket = serde_json::from_value(json)?;
        assert_eq!(packet, round_tripped);
        Ok(())
    }

    #[test]
    fn source_packet_rejects_unknown_top_level_field() {
        let json = serde_json::json!({
            "schema": "runx.source_packet.v1",
            "packet_id": "src_pkt_001",
            "source_ref": {
                "type": "thread",
                "uri": "slack://team/T1/channel/C1/thread/1700000000.0001",
            },
            "signal_type": "chat_message",
            "title": "Incoming customer message",
            "observed_at": "2026-05-28T12:00:00Z",
            "redaction_status": "redacted",
            "stray_field": "nope",
        });
        let parsed: Result<SourcePacket, _> = serde_json::from_value(json);
        assert!(
            parsed.is_err(),
            "deny_unknown_fields should reject unknown top-level field"
        );
    }
}
