//! What a host brings a paused run (`runx resume <run-id> -`): an agent's
//! answers and a person's decisions, each bound by digest to the request it
//! answers, or a close. A decision therefore never reaches a request that
//! changed after the person saw it.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::JsonValue;
use crate::decision::ClosureDisposition;
use crate::schema::{NonEmptyString, RunxSchema};

/// Each map is keyed by the address of the request it answers. `answers` is
/// the agent's lane; `approvals` is the person's, and every entry in it
/// resolves as theirs. A close comes alone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.resume.v1")]
pub struct ResumeDocument {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub answers: BTreeMap<String, BoundAnswer>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub approvals: BTreeMap<String, BoundApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<RunClose>,
}

/// An agent's answer to an input request or an agent act.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundAnswer {
    /// The digest of the request answered, as the run reported it.
    pub request_digest: NonEmptyString,
    pub value: JsonValue,
}

/// A person's decision, validated against the request it answers: an
/// approval takes `approved` and `reason`; a claimed call's retry takes
/// `value: {"retry": true}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundApproval {
    pub request_digest: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<JsonValue>,
}

/// Seal the run without continuing it: a person stopped it, or a new attempt
/// replaced it. An effect the run left unconfirmed stays unconfirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RunClose {
    pub disposition: CloseDisposition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloseDisposition {
    Declined,
    Superseded,
}

impl From<CloseDisposition> for ClosureDisposition {
    fn from(disposition: CloseDisposition) -> Self {
        match disposition {
            CloseDisposition::Declined => Self::Declined,
            CloseDisposition::Superseded => Self::Superseded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ResumeDocument;

    #[test]
    fn a_document_binds_each_entry_and_refuses_an_unbound_one() -> Result<(), serde_json::Error> {
        let value = serde_json::json!({
            "answers": {"draft": {"request_digest": "sha256:a", "value": {"text": "hello"}}},
            "approvals": {
                "send.approval": {"request_digest": "sha256:b", "approved": true, "reason": "checked"},
                "send/retry": {"request_digest": "sha256:c", "value": {"retry": true}}
            }
        });
        let document: ResumeDocument = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(document)?, value);
        for unbound in [
            serde_json::json!({"answers": {"draft": {"text": "hello"}}}),
            serde_json::json!({"approvals": {"send.approval": true}}),
            serde_json::json!({"draft": {"text": "hello"}}),
            serde_json::json!({"close": {"disposition": "closed"}}),
        ] {
            assert!(serde_json::from_value::<ResumeDocument>(unbound).is_err());
        }
        Ok(())
    }
}
