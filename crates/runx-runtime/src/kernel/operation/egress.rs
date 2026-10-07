//! The egress guard a host sets for delegated runs: every outward call
//! needs exact approval, and no argument may carry local paths, credentials
//! or the operator's confidential terms. The assistant sets it for every run
//! it composes; the rule is the same one it applies before persisting intent.

use std::collections::BTreeMap;

use runx_contracts::JsonValue;

/// Set by a host that requires exact approval for every outward mutation.
pub const REQUIRE_MUTATION_APPROVAL_ENV: &str = "RUNX_REQUIRE_MUTATION_APPROVAL";
/// The operator's confidential terms, as a JSON array.
pub const CONFIDENTIAL_TERMS_ENV: &str = "RUNX_CONFIDENTIAL_TERMS_JSON";

/// Whether the guard applies, refusing a call whose arguments carry
/// protected material. Returns true when the guard requires approval.
pub(crate) fn admit(
    env: &BTreeMap<String, String>,
    arguments: &JsonValue,
    scopes: &[&str],
) -> Result<bool, String> {
    let Some(mode) = env.get(REQUIRE_MUTATION_APPROVAL_ENV) else {
        return Ok(false);
    };
    if mode != "required" {
        return Err("the host's mutation approval policy is invalid".to_owned());
    }
    let terms = confidential_terms(env)?;
    if !json_is_safe(arguments, &terms, env)
        || !scopes
            .iter()
            .all(|scope| outbound_text_is_safe(scope, &terms, env))
    {
        return Err("the call carries protected local or credential material".to_owned());
    }
    Ok(true)
}

/// Check a rendered provider request: instance parameters can add fields
/// after the arguments were admitted.
pub(crate) fn check_rendered(
    env: &BTreeMap<String, String>,
    rendered: &JsonValue,
) -> Result<(), String> {
    if !env.contains_key(REQUIRE_MUTATION_APPROVAL_ENV) {
        return Ok(());
    }
    let terms = confidential_terms(env)?;
    if json_is_safe(rendered, &terms, env) {
        Ok(())
    } else {
        Err(
            "the rendered provider request carries protected local or credential material"
                .to_owned(),
        )
    }
}

fn confidential_terms(env: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
    env.get(CONFIDENTIAL_TERMS_ENV)
        .and_then(|value| serde_json::from_str::<Vec<String>>(value).ok())
        .filter(|terms| {
            terms.len() <= 16
                && terms
                    .iter()
                    .all(|term| (3..=200).contains(&term.len()) && term.trim() == term)
        })
        .ok_or_else(|| "the confidential egress policy is missing or invalid".to_owned())
}

fn json_is_safe(value: &JsonValue, terms: &[String], env: &BTreeMap<String, String>) -> bool {
    match value {
        JsonValue::String(text) => outbound_text_is_safe(text, terms, env),
        JsonValue::Array(items) => items.iter().all(|item| json_is_safe(item, terms, env)),
        JsonValue::Object(fields) => fields.iter().all(|(key, value)| {
            outbound_text_is_safe(key, terms, env) && json_is_safe(value, terms, env)
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
pub fn outbound_text_is_safe(
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
