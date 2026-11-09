//! What the runtime observed of one credential delivery: the non-secret
//! record of what it handed an invocation. It lives in the process only; a
//! receipt seals only its `credential_refs`, so it is no published contract.
use runx_contracts::Reference;
use runx_contracts::schema::{IsoDateTime, NonEmptyString};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialDeliveryMode {
    ProcessEnv,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialDeliveryPurpose {
    ProviderApi,
    Registry,
    ArtifactStore,
    WebhookVerification,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMaterialRole {
    PersonalToken,
    ApiKey,
    ClientSecret,
    SessionToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialDeliveryObservationStatus {
    Delivered,
    Denied,
    NotDelivered,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum CredentialDeliveryObservationSchema {
    #[serde(rename = "runx.credential_delivery.observation.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CredentialDeliveryObservation {
    pub schema: CredentialDeliveryObservationSchema,
    pub observation_id: NonEmptyString,
    pub request_id: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<NonEmptyString>,
    pub status: CredentialDeliveryObservationStatus,
    pub run_ref: Reference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_ref: Option<Reference>,
    pub profile_id: NonEmptyString,
    pub provider: NonEmptyString,
    pub purpose: CredentialDeliveryPurpose,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_mode: Option<CredentialDeliveryMode>,
    pub credential_refs: Vec<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material_id_digest: Option<NonEmptyString>,
    pub delivered_roles: Vec<CredentialMaterialRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redaction_refs: Option<Vec<Reference>>,
    pub observed_at: IsoDateTime,
}
