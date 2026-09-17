//! Effect-free x402 v2 presentation and Runx invocation binding.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use runx_contracts::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use runx_contracts::{
    JsonObject, JsonValue, PaidInvocationPaymentChallenge, RUNX_INVOCATION_EXTENSION_KEY,
    Reference, RunxX402InvocationExtension, RunxX402InvocationExtensionInfo, SettlementFamily,
    X402_PAYMENT_REQUIRED_HEADER, X402_PAYMENT_RESPONSE_HEADER, X402_PAYMENT_SIGNATURE_HEADER,
    X402AcceptedRequirements, X402PaymentPayload, X402PaymentRequired, X402PaymentRequirements,
    X402ResourceInfo, X402SettleResponse, X402Version2, parse_runx_invocation_extension,
    runx_invocation_extension_value, sha256_prefixed,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

pub const X402_JSON_MEDIA_TYPE: &str = "application/json";
pub const MAX_X402_HEADER_BYTES: usize = 65_536;
pub const MAX_X402_DECODED_BYTES: usize = 49_152;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum X402Header {
    PaymentRequired,
    PaymentSignature,
    PaymentResponse,
}

impl X402Header {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PaymentRequired => X402_PAYMENT_REQUIRED_HEADER,
            Self::PaymentSignature => X402_PAYMENT_SIGNATURE_HEADER,
            Self::PaymentResponse => X402_PAYMENT_RESPONSE_HEADER,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum X402PresentationError {
    #[error("x402 header exceeds the configured bound")]
    HeaderTooLarge,
    #[error("x402 header is not standard base64")]
    InvalidBase64,
    #[error("x402 header JSON is malformed or violates its contract")]
    InvalidPayload,
    #[error("x402 value could not be encoded")]
    EncodingFailed,
    #[error("runx.invocation is reserved and cannot be supplied by a vendor")]
    ReservedExtension,
    #[error("runx.invocation is absent")]
    MissingRunxInvocation,
    #[error("runx.invocation does not match its published v1 schema")]
    RunxInvocationSchemaMismatch,
    #[error("the retry resource does not match the challenge")]
    ResourceMismatch,
    #[error("the retry selected requirements are not an exact offered requirement")]
    RequirementMismatch,
    #[error("the retry runx.invocation declaration changed")]
    RunxInvocationMismatch,
    #[error("the rail-neutral challenge is not an x402 v2 JSON challenge")]
    ChallengeKindMismatch,
    #[error("the rail-neutral challenge payload digest does not match")]
    ChallengeDigestMismatch,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedX402Retry {
    pub requirement_index: usize,
    pub invocation: RunxX402InvocationExtensionInfo,
}

/// The schema a `runx.invocation` declaration advertises: the published v1
/// document by its resolvable `$id`. The external `{ info, schema }` form is
/// kept, the five-kilobyte document is not; a challenge has to fit one
/// portable HTTP header next to the vendor's own discovery declaration.
pub fn runx_invocation_schema_reference() -> Result<JsonObject, X402PresentationError> {
    let schema = RunxX402InvocationExtensionInfo::json_schema();
    let id = schema
        .get("$id")
        .and_then(serde_json::Value::as_str)
        .ok_or(X402PresentationError::EncodingFailed)?;
    json_object_from_serializable(&serde_json::json!({ "$ref": id }))
}

/// Assemble the whole external challenge. Vendors may supply other declared
/// extensions as data, but cannot supply or overwrite `runx.invocation`.
pub fn assemble_payment_required(
    resource: X402ResourceInfo,
    accepts: X402AcceptedRequirements,
    invocation: RunxX402InvocationExtensionInfo,
    error: Option<String>,
    mut extensions: JsonObject,
) -> Result<X402PaymentRequired, X402PresentationError> {
    if extensions.contains_key(RUNX_INVOCATION_EXTENSION_KEY) {
        return Err(X402PresentationError::ReservedExtension);
    }
    let declaration = RunxX402InvocationExtension {
        info: invocation,
        schema: runx_invocation_schema_reference()?,
    };
    let extension_value = runx_invocation_extension_value(&declaration)
        .map_err(|_| X402PresentationError::EncodingFailed)?;
    extensions.insert(RUNX_INVOCATION_EXTENSION_KEY.to_owned(), extension_value);

    Ok(X402PaymentRequired {
        x402_version: X402Version2,
        error,
        resource,
        accepts,
        extensions: Some(extensions),
        additional: JsonObject::new(),
    })
}

/// Project a complete challenge into the existing rail-neutral aggregate.
/// The digest covers canonical Runx JSON for the whole external declaration.
pub fn bind_payment_required_challenge(
    payment_required: &X402PaymentRequired,
    quote_ref: Reference,
    quote_expires_at: IsoDateTime,
) -> Result<PaidInvocationPaymentChallenge, X402PresentationError> {
    let payload = json_value_from_serializable(payment_required)?;
    let canonical =
        serde_json::to_vec(&payload).map_err(|_| X402PresentationError::EncodingFailed)?;
    let settlement_family =
        SettlementFamily::new("x402").ok_or(X402PresentationError::EncodingFailed)?;
    let protocol_version = NonEmptyString::new("2").ok_or(X402PresentationError::EncodingFailed)?;
    let media_type =
        NonEmptyString::new(X402_JSON_MEDIA_TYPE).ok_or(X402PresentationError::EncodingFailed)?;
    let payload_digest = runx_contracts::Sha256Digest::new(sha256_prefixed(&canonical))
        .ok_or(X402PresentationError::EncodingFailed)?;

    Ok(PaidInvocationPaymentChallenge {
        settlement_family,
        protocol_version,
        media_type,
        payload,
        payload_digest,
        quote_ref,
        quote_expires_at,
    })
}

/// Recover and verify the external declaration carried by a rail-neutral
/// challenge without logging the opaque payload.
pub fn payment_required_from_challenge(
    challenge: &PaidInvocationPaymentChallenge,
) -> Result<X402PaymentRequired, X402PresentationError> {
    if challenge.settlement_family.as_str() != "x402"
        || challenge.protocol_version.as_str() != "2"
        || challenge.media_type.as_str() != X402_JSON_MEDIA_TYPE
    {
        return Err(X402PresentationError::ChallengeKindMismatch);
    }
    let canonical = serde_json::to_vec(&challenge.payload)
        .map_err(|_| X402PresentationError::InvalidPayload)?;
    if sha256_prefixed(&canonical) != challenge.payload_digest.as_str() {
        return Err(X402PresentationError::ChallengeDigestMismatch);
    }
    challenge
        .payload
        .clone()
        .deserialize_into()
        .map_err(|_| X402PresentationError::InvalidPayload)
}

/// Validate only immutable presentation commitments. This performs no payment
/// verification and returns no payment material.
pub fn validate_payment_retry(
    challenge: &X402PaymentRequired,
    retry: &X402PaymentPayload,
) -> Result<ValidatedX402Retry, X402PresentationError> {
    if retry.resource.as_ref() != Some(&challenge.resource) {
        return Err(X402PresentationError::ResourceMismatch);
    }
    let requirement_index = challenge
        .accepts
        .as_slice()
        .iter()
        .position(|candidate| candidate == &retry.accepted)
        .ok_or(X402PresentationError::RequirementMismatch)?;

    let declared = parse_runx_invocation_declaration(challenge.extensions.as_ref())?;
    let echoed = parse_runx_invocation_declaration(retry.extensions.as_ref())?;
    if echoed != declared {
        return Err(X402PresentationError::RunxInvocationMismatch);
    }

    Ok(ValidatedX402Retry {
        requirement_index,
        invocation: declared.info,
    })
}

pub fn encode_payment_required_header(
    value: &X402PaymentRequired,
) -> Result<String, X402PresentationError> {
    encode_header(value)
}

pub fn decode_payment_required_header(
    value: &str,
) -> Result<X402PaymentRequired, X402PresentationError> {
    decode_header(value)
}

pub fn encode_payment_signature_header(
    value: &X402PaymentPayload,
) -> Result<String, X402PresentationError> {
    encode_header(value)
}

pub fn decode_payment_signature_header(
    value: &str,
) -> Result<X402PaymentPayload, X402PresentationError> {
    decode_header(value)
}

pub fn encode_payment_response_header(
    value: &X402SettleResponse,
) -> Result<String, X402PresentationError> {
    encode_header(value)
}

pub fn decode_payment_response_header(
    value: &str,
) -> Result<X402SettleResponse, X402PresentationError> {
    decode_header(value)
}

fn encode_header<T: Serialize>(value: &T) -> Result<String, X402PresentationError> {
    let bytes = serde_json::to_vec(value).map_err(|_| X402PresentationError::EncodingFailed)?;
    if bytes.len() > MAX_X402_DECODED_BYTES {
        return Err(X402PresentationError::HeaderTooLarge);
    }
    let encoded = STANDARD.encode(bytes);
    if encoded.len() > MAX_X402_HEADER_BYTES {
        return Err(X402PresentationError::HeaderTooLarge);
    }
    Ok(encoded)
}

fn decode_header<T: DeserializeOwned>(value: &str) -> Result<T, X402PresentationError> {
    if value.len() > MAX_X402_HEADER_BYTES {
        return Err(X402PresentationError::HeaderTooLarge);
    }
    if !is_standard_base64(value) {
        return Err(X402PresentationError::InvalidBase64);
    }
    let normalized = padded_base64(value);
    let bytes = STANDARD
        .decode(normalized.as_bytes())
        .map_err(|_| X402PresentationError::InvalidBase64)?;
    if bytes.len() > MAX_X402_DECODED_BYTES {
        return Err(X402PresentationError::HeaderTooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| X402PresentationError::InvalidPayload)
}

fn is_standard_base64(value: &str) -> bool {
    let padding = value.bytes().rev().take_while(|byte| *byte == b'=').count();
    if padding > 2 || (padding > 0 && !value.len().is_multiple_of(4)) {
        return false;
    }
    if padding == 0 && value.len() % 4 == 1 {
        return false;
    }
    let data_len = value.len().saturating_sub(padding);
    value.bytes().enumerate().all(|(index, byte)| {
        if index >= data_len {
            byte == b'='
        } else {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/')
        }
    })
}

fn padded_base64(value: &str) -> String {
    if value.ends_with('=') || value.len().is_multiple_of(4) {
        return value.to_owned();
    }
    let mut normalized = String::with_capacity(value.len() + 2);
    normalized.push_str(value);
    match value.len() % 4 {
        2 => normalized.push_str("=="),
        3 => normalized.push('='),
        _ => {}
    }
    normalized
}

/// Read and normalize the `runx.invocation` declaration under `extensions`.
/// The advertised schema must be the published v1 schema, by reference or as
/// the inline document challenges carried before the reference form; both
/// name the same schema, so the declaration is returned in the reference
/// form and two echoes of one challenge compare equal whichever way it was
/// assembled. Inline acceptance retires once every emitter is on the
/// reference form.
pub fn parse_runx_invocation_declaration(
    extensions: Option<&JsonObject>,
) -> Result<RunxX402InvocationExtension, X402PresentationError> {
    let value = extensions
        .and_then(|values| values.get(RUNX_INVOCATION_EXTENSION_KEY))
        .cloned()
        .ok_or(X402PresentationError::MissingRunxInvocation)?;
    let declaration = parse_runx_invocation_extension(value)
        .map_err(|_| X402PresentationError::InvalidPayload)?;
    let reference = runx_invocation_schema_reference()?;
    if declaration.schema != reference
        && declaration.schema
            != json_object_from_serializable(&RunxX402InvocationExtensionInfo::json_schema())?
    {
        return Err(X402PresentationError::RunxInvocationSchemaMismatch);
    }
    Ok(RunxX402InvocationExtension {
        info: declaration.info,
        schema: reference,
    })
}

fn json_object_from_serializable<T: Serialize>(
    value: &T,
) -> Result<JsonObject, X402PresentationError> {
    match json_value_from_serializable(value)? {
        JsonValue::Object(object) => Ok(object),
        _ => Err(X402PresentationError::EncodingFailed),
    }
}

fn json_value_from_serializable<T: Serialize>(
    value: &T,
) -> Result<JsonValue, X402PresentationError> {
    let value = serde_json::to_value(value).map_err(|_| X402PresentationError::EncodingFailed)?;
    serde_json::from_value(value).map_err(|_| X402PresentationError::EncodingFailed)
}

/// Exact selected requirement returned after retry validation.
#[must_use]
pub fn selected_requirement<'a>(
    challenge: &'a X402PaymentRequired,
    validated: &ValidatedX402Retry,
) -> Option<&'a X402PaymentRequirements> {
    challenge
        .accepts
        .as_slice()
        .get(validated.requirement_index)
}
