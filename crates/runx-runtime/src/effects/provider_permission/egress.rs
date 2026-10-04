use std::collections::BTreeMap;

use runx_contracts::{AuthorityVerb, JsonValue};

use super::{
    ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV, PROVIDER_PERMISSION_EFFECT_FAMILY,
    ProviderNativeAccess,
};
use crate::RuntimeEffectError;
use crate::effects::EffectStepRequest;

pub const ASSISTANT_CONFIDENTIAL_TERMS_ENV: &str = "RUNX_ASSISTANT_CONFIDENTIAL_TERMS_JSON";

pub(super) fn admit_assistant_egress(
    request: &EffectStepRequest<'_>,
    access: Option<ProviderNativeAccess>,
    required_scopes: &[String],
) -> Result<bool, RuntimeEffectError> {
    if access != Some(ProviderNativeAccess::Mutate) {
        return Ok(false);
    }
    let Some(mode) = request.env.get(ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV) else {
        return Ok(false);
    };
    if mode != "required" {
        return Err(denied("assistant provider mutation policy is invalid"));
    }
    let terms = request
        .env
        .get(ASSISTANT_CONFIDENTIAL_TERMS_ENV)
        .and_then(|value| serde_json::from_str::<Vec<String>>(value).ok())
        .filter(|terms| {
            terms.len() <= 16
                && terms
                    .iter()
                    .all(|term| (3..=200).contains(&term.len()) && term.trim() == term)
        })
        .ok_or_else(|| denied("assistant confidential egress policy is missing or invalid"))?;
    let safe = request.inputs.iter().all(|(key, value)| {
        assistant_text_is_safe(key, &terms, request.env) && json_is_safe(value, &terms, request.env)
    }) && required_scopes
        .iter()
        .all(|scope| assistant_text_is_safe(scope, &terms, request.env));
    if !safe {
        return Err(denied(
            "assistant provider mutation contains protected local or credential material",
        ));
    }
    Ok(true)
}

fn denied(message: &str) -> RuntimeEffectError {
    RuntimeEffectError::Denied {
        family: PROVIDER_PERMISSION_EFFECT_FAMILY.to_owned(),
        verb: AuthorityVerb::Write,
        message: message.to_owned(),
    }
}

fn json_is_safe(value: &JsonValue, terms: &[String], env: &BTreeMap<String, String>) -> bool {
    match value {
        JsonValue::String(text) => assistant_text_is_safe(text, terms, env),
        JsonValue::Array(items) => items.iter().all(|item| json_is_safe(item, terms, env)),
        JsonValue::Object(fields) => fields.iter().all(|(key, value)| {
            assistant_text_is_safe(key, terms, env) && json_is_safe(value, terms, env)
        }),
        _ => true,
    }
}

fn percent_decode_once(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            )
        {
            decoded.push(((high << 4) | low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn has_absolute_path_shape(text: &str) -> bool {
    let bytes = text.as_bytes();
    let separator = |byte| {
        matches!(
            byte,
            b' ' | b'\r' | b'\n' | b'\t' | b'=' | b'(' | b'[' | b'\'' | b'"' | b'`' | b',' | b':'
        )
    };
    bytes.iter().enumerate().any(|(index, byte)| {
        if *byte == b'/'
            && (index == 0 || separator(bytes[index - 1]))
            && bytes
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_alphanumeric() || matches!(*next, b'.' | b'_'))
        {
            return true;
        }
        index + 2 < bytes.len()
            && (index == 0 || separator(bytes[index - 1]))
            && byte.is_ascii_alphabetic()
            && bytes[index + 1] == b':'
            && matches!(bytes[index + 2], b'\\' | b'/')
    }) || text.contains("\\\\")
}

/// One content rule is used by the assistant before intent persistence and by
/// the native provider effect before any external mutation is admitted.
pub fn assistant_text_is_safe(
    text: &str,
    confidential_terms: &[String],
    env: &BTreeMap<String, String>,
) -> bool {
    let decoded = percent_decode_once(&percent_decode_once(text));
    let lower = decoded.to_ascii_lowercase();
    let blocked_shapes = [
        "/users/",
        "/home/",
        "/private/",
        "/var/folders/",
        "/opt/homebrew/",
        "/tmp/",
        "~/",
        "file://",
        "local://",
        "c:\\users\\",
        "-----begin private key-----",
        "bearer ",
        "ghp_",
        "gho_",
        "xoxb-",
        "xoxp-",
        "sk-proj-",
        "aws_secret_access_key",
        "$home",
        "$path",
        "$runx_",
        "$aws_",
        "%userprofile%",
        "%appdata%",
        "runx_agent_",
        "${",
        "$(",
        ".env",
        "runx:provider-credential:",
    ];
    if has_absolute_path_shape(&decoded)
        || blocked_shapes.iter().any(|shape| lower.contains(shape))
        || confidential_terms
            .iter()
            .any(|term| lower.contains(&term.to_ascii_lowercase()))
    {
        return false;
    }
    for (key, value) in env {
        let upper = key.to_ascii_uppercase();
        let protected = [
            "KEY",
            "TOKEN",
            "SECRET",
            "PASSWORD",
            "CREDENTIAL",
            "COOKIE",
            "SESSION",
        ]
        .iter()
        .any(|marker| upper.contains(marker));
        if protected
            && (lower.contains(&key.to_ascii_lowercase())
                || (value.len() >= 8 && decoded.contains(value)))
        {
            return false;
        }
    }
    true
}
