//! Credential delivery contracts: the non-secret observation a receipt records.
use serde::{Deserialize, Serialize};

use crate::Reference;
use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialDeliveryMode {
    ProcessEnv,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialDeliveryPurpose {
    ProviderApi,
    Registry,
    ArtifactStore,
    WebhookVerification,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMaterialRole {
    PersonalToken,
    ApiKey,
    ClientSecret,
    SessionToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialDeliveryObservationStatus {
    Delivered,
    Denied,
    NotDelivered,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum CredentialDeliveryObservationSchema {
    #[serde(rename = "runx.credential_delivery.observation.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.credential_delivery.observation.v1")]
pub struct CredentialDeliveryObservation {
    pub schema: CredentialDeliveryObservationSchema,
    pub observation_id: NonEmptyString,
    pub request_id: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<NonEmptyString>,
    pub status: CredentialDeliveryObservationStatus,
    pub harness_ref: Reference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_ref: Option<Reference>,
    pub profile_id: NonEmptyString,
    pub provider: NonEmptyString,
    pub purpose: CredentialDeliveryPurpose,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_mode: Option<CredentialDeliveryMode>,
    pub credential_refs: Vec<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material_ref_hash: Option<NonEmptyString>,
    pub delivered_roles: Vec<CredentialMaterialRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redaction_refs: Option<Vec<Reference>>,
    pub observed_at: IsoDateTime,
}
