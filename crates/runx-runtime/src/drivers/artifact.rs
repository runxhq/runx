//! The artifact connector's native implementation: one bounded value stored
//! in the artifact space of the runx account the runtime is signed in as.
//!
//! The bytes are the call's `value` as JSON, or its canonical `data_base64`,
//! stored under the media type the call names and the idempotency key the
//! kernel derives from the call's intent, so the same call lands once in any
//! run. runx answers, for the principal the session authenticated, with the
//! artifact as its hosted store holds it: `ref`, `digest`, `size_bytes`,
//! `media_type` and `created_at`. The call's result is `artifact`, that
//! artifact as a `runx.artifact.v1` labelled `configured_model`, since its
//! bytes are in runx's hosted store, off this machine, so a model that runs
//! elsewhere is no further for them to go; and beside it `principal_uri`, the
//! account it's held for, which may differ from the actor the run declared.
//! The kernel confirms the write by reading the reference back.
//!
//! A hosted worker runs its runtime against a gateway bound to one durable
//! run, which every artifact call names; anywhere else the call names the
//! runtime's own run.

use base64::Engine as _;
use runx_contracts::connector::{ReadbackExpected, ReadbackMatch};
use runx_contracts::schema::{IsoDateTime, NonEmptyString};
use runx_contracts::{
    Artifact as ArtifactDescriptor, ArtifactLabels, ArtifactSchema, Digest, JsonObject, JsonValue,
    ProviderOperationPacket,
};
use serde::Deserialize;

use crate::error::{Contradiction, Mismatch};
use crate::hosted_api::HostedApiOperationError;
use crate::http::RuntimeHttpError;
use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Check, Driver, Observed, Unconfirmed, Unperformed};
use crate::kernel::operation::hosted::HostedSession;

const ALLOCATE: &str = "artifact.allocate";
const INSPECT: &str = "artifact.inspect";
/// The durable run a hosted worker executes; its gateway refuses any other.
pub(crate) const TRUSTED_RUN_ID_ENV: &str = "RUNX_HOSTED_ARTIFACT_TRUSTED_RUN_ID";
/// What a readback compares of every artifact runx holds, and the account it
/// holds it for.
const EVIDENCE: [&str; 5] = [
    "artifact.ref",
    "artifact.digest",
    "artifact.media_type",
    "artifact.size_bytes",
    "principal_uri",
];

/// An artifact as runx's hosted store states it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostedArtifact {
    #[serde(rename = "ref")]
    reference: NonEmptyString,
    digest: Digest,
    size_bytes: u64,
    media_type: NonEmptyString,
    #[serde(default)]
    created_at: Option<IsoDateTime>,
}

/// What confirms an allocation: reading its reference back shows the same
/// bytes, media type and owner the write reported.
pub(crate) fn readback_matches() -> Vec<ReadbackMatch> {
    EVIDENCE
        .iter()
        .map(|field| ReadbackMatch {
            expected: ReadbackExpected::Acknowledgement {
                path: (*field).to_owned().into(),
            },
            observed: (*field).to_owned().into(),
        })
        .collect()
}

/// The call input, as the connector's input schema admits it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AllocateInput {
    /// The target: what the run stores the artifact for. It is bound into
    /// the intent, and so into the key.
    #[serde(rename = "purpose")]
    _purpose: String,
    media_type: String,
    #[serde(default)]
    value: Option<JsonValue>,
    #[serde(default)]
    data_base64: Option<String>,
}

pub(crate) struct Artifact<'a> {
    pub session: &'a HostedSession,
    pub call: &'a AdmittedCall<'a>,
}

impl Artifact<'_> {
    fn allocate(&self) -> Result<JsonObject, Unperformed> {
        if self.call.operation != ALLOCATE {
            return Err(not_sent(format!(
                "the artifact implementation has no operation {}",
                self.call.operation
            )));
        }
        let input = self
            .call
            .arguments
            .clone()
            .deserialize_into::<AllocateInput>()
            .map_err(|error| not_sent(format!("the call input is invalid: {error}")))?;
        let bytes = bytes(&input)?;
        let media_type = media_type(&input.media_type)?;
        let key = self
            .call
            .idempotency_key
            .ok_or_else(|| not_sent("an artifact write has no idempotency key".to_owned()))?;
        let digest = Digest::of(&bytes);
        let answer = self.send(
            ALLOCATE,
            JsonObject::from([
                ("idempotency_key".to_owned(), text(key)),
                (
                    "data_base64".to_owned(),
                    text(&base64::engine::general_purpose::STANDARD.encode(&bytes)),
                ),
                ("digest".to_owned(), text(digest.as_str())),
                ("media_type".to_owned(), text(media_type)),
            ]),
        )?;
        if answer.operation != ALLOCATE {
            return Err(malformed("runx answered for another artifact operation"));
        }
        let principal_uri = self.session.principal_uri.as_str();
        if answer.principal_uri.as_deref() != Some(principal_uri) {
            return Err(malformed(
                "runx acted for another principal than the signed-in account",
            ));
        }
        if answer.idempotency_key.as_deref() != Some(key) {
            return Err(malformed("runx stored the artifact under another key"));
        }
        let artifact = self.artifact(answer)?;
        if artifact.digest != digest
            || artifact.media_type.as_str() != media_type
            || u64::try_from(bytes.len()).ok() != Some(artifact.size_bytes)
        {
            return Err(malformed("runx holds other bytes than the call stored"));
        }
        held(&artifact, Some(principal_uri))
    }

    /// What runx holds under `reference`, and the account it states it
    /// holds it for, which the kernel compares with the allocation's.
    fn inspect(&self, reference: &str) -> Result<JsonObject, Unconfirmed> {
        let answer = self.send(
            INSPECT,
            JsonObject::from([("ref".to_owned(), text(reference))]),
        )?;
        if answer.operation != INSPECT {
            return Err(Unconfirmed::Contradicted(Contradiction {
                mismatches: vec![Mismatch {
                    field: "operation".to_owned(),
                    asked: text(INSPECT),
                    held: text(&answer.operation),
                }],
            }));
        }
        let principal_uri = answer.principal_uri.clone();
        held(&self.artifact(answer)?, principal_uri.as_deref()).map_err(Unconfirmed::from)
    }

    fn send(
        &self,
        operation: &'static str,
        input: JsonObject,
    ) -> Result<ProviderOperationPacket, Unperformed> {
        let body = JsonObject::from([
            ("operation".to_owned(), text(operation)),
            ("run_id".to_owned(), text(run_id(self.call)?)),
            ("input".to_owned(), JsonValue::Object(input)),
        ]);
        let body = serde_json::to_string(&body)
            .map_err(|error| not_sent(format!("the request cannot be encoded: {error}")))?;
        let answer: ProviderOperationPacket = self
            .session
            .send(operation, "/v1/artifact-operations", body)
            .map_err(unperformed)?;
        if answer.status != "success" {
            return Err(malformed("runx did not answer the artifact operation"));
        }
        Ok(answer)
    }

    /// The artifact an answer states.
    fn artifact(&self, answer: ProviderOperationPacket) -> Result<ArtifactDescriptor, Unperformed> {
        let hosted = answer
            .result
            .deserialize_into::<HostedArtifact>()
            .map_err(|_| malformed("runx stated no artifact"))?;
        if !valid_artifact_ref(&hosted.reference) || hosted.reference.as_str() != answer.target {
            return Err(malformed("runx stated no valid artifact reference"));
        }
        Ok(ArtifactDescriptor {
            schema: ArtifactSchema::V1,
            reference: hosted.reference,
            digest: hosted.digest,
            size_bytes: hosted.size_bytes,
            media_type: hosted.media_type,
            created_at: hosted.created_at,
            produced_by: None,
            path: None,
            summary: None,
            sources: Vec::new(),
            labels: ArtifactLabels::CONFIGURED_MODEL,
        })
    }
}

impl Driver for Artifact<'_> {
    fn perform(&self, _item: Option<usize>) -> Result<Observed, Unperformed> {
        self.allocate().map(observed)
    }

    fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, Unconfirmed> {
        match check {
            Check::Readback { .. } => {
                let reference = reference(write).ok_or("the acknowledgement names no artifact")?;
                self.inspect(reference).map(observed)
            }
            Check::Precondition(_) => Err("an artifact call has no precondition".into()),
        }
    }

    fn result(
        &self,
        write: &JsonValue,
        _confirmed: &[JsonValue],
    ) -> Result<(JsonValue, Option<String>), String> {
        let reference = reference(write).map(str::to_owned);
        Ok((write.clone(), reference))
    }
}

/// The call's result: the artifact, and the account runx states it holds it
/// for.
fn held(
    artifact: &ArtifactDescriptor,
    principal_uri: Option<&str>,
) -> Result<JsonObject, Unperformed> {
    let artifact = JsonValue::from_serialize(artifact)
        .map_err(|_| malformed("the artifact cannot be stated"))?;
    let mut held = JsonObject::from([("artifact".to_owned(), artifact)]);
    if let Some(principal_uri) = principal_uri {
        held.insert("principal_uri".to_owned(), text(principal_uri));
    }
    Ok(held)
}

/// The bytes the call stores: exactly one of a JSON value or canonical
/// base64, never empty.
fn bytes(input: &AllocateInput) -> Result<Vec<u8>, Unperformed> {
    let bytes = match (&input.value, &input.data_base64) {
        (Some(value), None) => serde_json::to_vec(value)
            .map_err(|error| not_sent(format!("the value cannot be encoded: {error}")))?,
        (None, Some(encoded)) => {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| not_sent("data_base64 is not base64".to_owned()))?;
            if base64::engine::general_purpose::STANDARD.encode(&decoded) != *encoded {
                return Err(not_sent(
                    "data_base64 must use canonical padded base64".to_owned(),
                ));
            }
            decoded
        }
        _ => {
            return Err(not_sent(
                "exactly one of value or data_base64 is required".to_owned(),
            ));
        }
    };
    if bytes.is_empty() {
        return Err(not_sent("an artifact holds at least one byte".to_owned()));
    }
    Ok(bytes)
}

fn media_type(value: &str) -> Result<&str, Unperformed> {
    let valid = !value.is_empty()
        && value.len() <= 200
        && value.trim() == value
        && value.contains('/')
        && !value.chars().any(char::is_control);
    if valid {
        Ok(value)
    } else {
        Err(not_sent("media_type is invalid".to_owned()))
    }
}

/// The run every artifact call names: the worker's durable run, else the
/// runtime's own.
fn run_id<'a>(call: &'a AdmittedCall<'_>) -> Result<&'a str, Unperformed> {
    call.env
        .get(TRUSTED_RUN_ID_ENV)
        .or_else(|| call.env.get(crate::execution::engine::RUNX_RUN_ID_ENV))
        .map(String::as_str)
        .filter(|run_id| !run_id.trim().is_empty())
        .ok_or_else(|| not_sent("the call runs in no run".to_owned()))
}

fn reference(write: &JsonValue) -> Option<&str> {
    write
        .as_object()
        .and_then(|write| write.get("artifact"))
        .and_then(JsonValue::as_object)
        .and_then(|artifact| artifact.get("ref"))
        .and_then(JsonValue::as_str)
}

fn valid_artifact_ref(value: &str) -> bool {
    value
        .strip_prefix("runx:artifact:sha256:")
        .is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
}

fn unperformed(error: HostedApiOperationError) -> Unperformed {
    let refused = |status: u16, reason: String| match status {
        429 => Unperformed::Halted {
            code: "rate_limited",
            reason,
            retry_at: None,
            partial: false,
        },
        400..=499 => Unperformed::Refused {
            code: "artifact_refused",
            reason,
            answer: None,
        },
        _ => Unperformed::MaybeSent {
            code: "artifact_unavailable",
            reason,
        },
    };
    match error {
        HostedApiOperationError::Api { status, .. }
        | HostedApiOperationError::HttpStatus { status, .. } => refused(status, error.to_string()),
        HostedApiOperationError::InvalidRequest { .. } => not_sent(error.to_string()),
        HostedApiOperationError::RuntimeHttp(
            RuntimeHttpError::InvalidUrl(_)
            | RuntimeHttpError::UnsupportedUrlScheme { .. }
            | RuntimeHttpError::PrivateNetworkUrl { .. }
            | RuntimeHttpError::InvalidHeaderName { .. }
            | RuntimeHttpError::InvalidHeaderValue { .. },
        ) => not_sent(error.to_string()),
        HostedApiOperationError::RuntimeHttp(_) | HostedApiOperationError::InvalidJson { .. } => {
            Unperformed::MaybeSent {
                code: "artifact_unavailable",
                reason: error.to_string(),
            }
        }
    }
}

fn observed(value: JsonObject) -> Observed {
    let value = JsonValue::Object(value);
    let evidence = serde_json::to_vec(&value)
        .map(|bytes| runx_contracts::sha256_prefixed(&bytes))
        .unwrap_or_default();
    Observed {
        value,
        evidence: vec![evidence],
    }
}

fn text(value: &str) -> JsonValue {
    JsonValue::String(value.to_owned())
}

fn not_sent(reason: String) -> Unperformed {
    Unperformed::NotSent {
        code: "invalid_request",
        reason,
    }
}

/// runx answered, so the write may have landed, but not as the call asked.
fn malformed(reason: &str) -> Unperformed {
    Unperformed::MaybeSent {
        code: "malformed_answer",
        reason: reason.to_owned(),
    }
}
