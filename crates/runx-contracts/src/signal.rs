//! Signal contracts: trust-tagged events that enter the act lifecycle.
use serde::{Deserialize, Serialize};

use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::{ContentDigest, JsonObject, Links, Reference};

pub const SIGNAL_SCHEMA: &str = "runx.signal.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum SignalSchema {
    #[serde(rename = "runx.signal.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignalTrustLevel {
    Unverified,
    Observed,
    VerifiedDelivery,
    VerifiedSignature,
    OperatorAttested,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SignalAuthenticity {
    pub host_ref: Reference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_by_ref: Option<Reference>,
    pub trust_level: SignalTrustLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<IsoDateTime>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signature_refs: Vec<Reference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<Reference>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.signal.v1")]
pub struct Signal {
    pub schema: SignalSchema,
    pub signal_id: NonEmptyString,
    pub source_ref: Reference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authenticity: Option<SignalAuthenticity>,
    /// Open signal type identifier (e.g. `issue_opened`). Adapters can publish
    /// their own identifier without a contract edit.
    pub signal_type: NonEmptyString,
    pub title: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_preview: Option<NonEmptyString>,
    pub observed_at: IsoDateTime,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<ContentDigest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Links>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<JsonObject>,
}
