//! Admission contracts: the operation intent a call is admitted as, and the
//! `runx.readiness.v1` projection.
//!
//! An exact intent (operation, instance, target, payload digest) is admitted
//! or refused by the kernel. Anything less exact gets a [`Readiness`]
//! projection: could this requirement be admitted with the instances,
//! capabilities and delegations present. A projection is never an admission;
//! only a call is admitted.

use serde::{Deserialize, Serialize};

use crate::Reference;
use crate::connector::{
    ConnectorDigestError, ConnectorName, Digest, InstanceId, OperationName, Placement, stable_digest,
};
use crate::schema::{NonEmptyString, RunxSchema};

/// An operation call before admission: everything a grant, a delegation or
/// a person is asked to cover. Its digest is the one identity of the call:
/// the execution term, the approval, the idempotency key and the receipt all
/// bind it, so a call that changes in any of these fields is a new call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationIntent {
    pub connector: ConnectorName,
    pub manifest_digest: Digest,
    pub connector_digest: Digest,
    pub operation: OperationName,
    /// What the operation means: its verb, scopes, target and floors.
    pub operation_digest: Digest,
    /// The instance fixes the placement, principal and account.
    pub instance_id: InstanceId,
    pub target: NonEmptyString,
    pub payload_digest: Digest,
    /// Digest of the caller's request key; present for mutations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_key_digest: Option<Digest>,
    /// Present when the operation requires an amount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<OperationAmount>,
    /// Digest of the approval text a person is shown, when the call is gated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_digest: Option<Digest>,
    /// Digest of the act the call performs, when its caller states one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act_digest: Option<Digest>,
}

impl OperationIntent {
    /// The digest every derived term, approval and receipt binds.
    pub fn digest(&self) -> Result<Digest, ConnectorDigestError> {
        stable_digest(self)
    }
}

/// An exact amount an operation moves, in the provider's smallest unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationAmount {
    pub units: u64,
    pub unit: NonEmptyString,
}

/// The checks admission runs, in order; each refuses before the next.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, RunxSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionCheck {
    /// The connector is the required one, or one the requirement allows, and
    /// the operation is listed.
    Requirement,
    /// The requirement resolves to one available instance.
    Instance,
    /// The instance's capability covers the operation's scopes and target.
    Capability,
    /// Every delegation link from principal to actor covers the intent.
    Delegation,
    /// Approval evidence satisfies the operation's approval floor.
    Approval,
    /// The instance's placement implements the operation.
    Implementation,
    /// The use's declared tolerance is within the operation's guarantees.
    Tolerance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Pass,
    Fail,
    /// Evidence is still owed: approval for an admission, or anything that
    /// depends on the exact call for a readiness projection.
    Required,
    /// Not evaluated: an earlier check failed, or the check needs the exact call.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AdmissionReason {
    pub check: AdmissionCheck,
    pub outcome: CheckOutcome,
    pub detail: NonEmptyString,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<Reference>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.readiness.v1")]
pub struct Readiness {
    /// The skill requirement key.
    pub requirement: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector: Option<ConnectorName>,
    pub operation: OperationName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<InstanceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    /// True when no check failed. Approval may still be owed at call time.
    pub ready: bool,
    pub reasons: Vec<AdmissionReason>,
}
