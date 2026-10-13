//! The artifact connector's native implementation: one bounded value stored
//! in the artifact space of the runx account the runtime is signed in as.
//!
//! The bytes are the call's `value` as JSON, or its canonical `data_base64`,
//! stored under the media type the call names and the idempotency key the
//! kernel derives from the call's intent, so the same call lands once in any
//! run. runx answers with the artifact's reference and what it holds, for
//! the principal it stored it for, which must be the principal the session
//! authenticated. The kernel confirms the write by reading the reference
//! back.
//!
//! A hosted worker runs its runtime against a gateway bound to one durable
//! run, which every artifact call names; anywhere else the call names the
//! runtime's own run.

use base64::Engine as _;
use runx_contracts::connector::{ReadbackExpected, ReadbackMatch};
use runx_contracts::{JsonNumber, JsonObject, JsonValue, ProviderOperationPacket, sha256_prefixed};
use serde::Deserialize;

use crate::hosted_api::HostedApiOperationError;
use crate::http::RuntimeHttpError;
use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Check, Driver, Observed, Unperformed};
use crate::kernel::operation::hosted::HostedSession;

const ALLOCATE: &str = "artifact.allocate";
const INSPECT: &str = "artifact.inspect";
/// The durable run a hosted worker executes; its gateway refuses any other.
pub(crate) const TRUSTED_RUN_ID_ENV: &str = "RUNX_HOSTED_ARTIFACT_TRUSTED_RUN_ID";
/// What runx states of every artifact it holds.
const EVIDENCE: [&str; 5] = [
    "artifact_ref",
    "content_digest",
    "media_type",
    "size_bytes",
    "principal_ref",
];

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
        let content_digest = sha256_prefixed(&bytes);
        let answer = self.send(
            ALLOCATE,
            JsonObject::from([
                ("idempotency_key".to_owned(), text(key)),
                (
                    "data_base64".to_owned(),
                    text(&base64::engine::general_purpose::STANDARD.encode(&bytes)),
                ),
                ("content_digest".to_owned(), text(&content_digest)),
                ("media_type".to_owned(), text(media_type)),
            ]),
        )?;
        if answer.idempotency_key.as_deref() != Some(key) {
            return Err(malformed("runx stored the artifact under another key"));
        }
        let evidence = self.evidence(answer)?;
        if evidence.get("content_digest") != Some(&text(&content_digest))
            || evidence.get("media_type") != Some(&text(media_type))
            || evidence.get("size_bytes").and_then(unsigned) != u64::try_from(bytes.len()).ok()
        {
            return Err(malformed("runx holds other bytes than the call stored"));
        }
        Ok(evidence)
    }

    /// What runx holds under `artifact_ref`.
    fn inspect(&self, artifact_ref: &str) -> Result<JsonObject, Unperformed> {
        let answer = self.send(
            INSPECT,
            JsonObject::from([("artifact_ref".to_owned(), text(artifact_ref))]),
        )?;
        self.evidence(answer)
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
        if answer.status != "success" || answer.operation != operation {
            return Err(malformed("runx answered for another artifact operation"));
        }
        if answer.principal_ref.as_deref() != Some(self.session.principal_ref.as_str()) {
            return Err(malformed(
                "runx acted for another principal than the signed-in account",
            ));
        }
        Ok(answer)
    }

    /// The artifact evidence an answer states, with the principal it holds
    /// the artifact for.
    fn evidence(&self, answer: ProviderOperationPacket) -> Result<JsonObject, Unperformed> {
        let JsonValue::Object(mut evidence) = answer.result else {
            return Err(malformed("runx stated no artifact"));
        };
        let artifact_ref = evidence.get("artifact_ref").and_then(JsonValue::as_str);
        if !artifact_ref.is_some_and(valid_artifact_ref) || artifact_ref != Some(&answer.target) {
            return Err(malformed("runx stated no valid artifact reference"));
        }
        evidence.insert(
            "principal_ref".to_owned(),
            text(&self.session.principal_ref),
        );
        Ok(evidence)
    }
}

impl Driver for Artifact<'_> {
    fn perform(&self, _item: Option<usize>) -> Result<Observed, Unperformed> {
        self.allocate().map(observed)
    }

    fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, String> {
        match check {
            Check::Readback { .. } => {
                let artifact_ref =
                    artifact_ref(write).ok_or("the acknowledgement names no artifact")?;
                self.inspect(artifact_ref)
                    .map(observed)
                    .map_err(Unperformed::reason)
            }
            Check::Precondition(_) => Err("an artifact call has no precondition".to_owned()),
        }
    }

    fn result(
        &self,
        write: &JsonValue,
        _confirmed: &[JsonValue],
    ) -> Result<(JsonValue, Option<String>), String> {
        let artifact_ref = artifact_ref(write).map(str::to_owned);
        Ok((write.clone(), artifact_ref))
    }
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
        .or_else(|| call.env.get(crate::execution::runner::RUNX_RUN_ID_ENV))
        .map(String::as_str)
        .filter(|run_id| !run_id.trim().is_empty())
        .ok_or_else(|| not_sent("the call runs in no run".to_owned()))
}

fn artifact_ref(write: &JsonValue) -> Option<&str> {
    write
        .as_object()
        .and_then(|write| write.get("artifact_ref"))
        .and_then(JsonValue::as_str)
}

fn unsigned(value: &JsonValue) -> Option<u64> {
    match value {
        JsonValue::Number(JsonNumber::U64(value)) => Some(*value),
        JsonValue::Number(JsonNumber::I64(value)) => u64::try_from(*value).ok(),
        _ => None,
    }
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
        .map(|bytes| sha256_prefixed(&bytes))
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
