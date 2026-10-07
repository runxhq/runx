//! OAuth 1.0a user-context request signing (HMAC-SHA1, RFC 5849) for HTTP
//! implementations.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use url::Url;

use crate::RuntimeError;
use crate::http::HttpMethod;

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: "http".to_owned(),
        message: message.into(),
    }
}

/// OAuth 1.0a user-context material: a consumer and an access token.
#[derive(Debug)]
pub(crate) struct OAuth1Credential {
    pub consumer_key: String,
    pub consumer_secret: String,
    pub access_token: String,
    pub access_secret: String,
}

/// The `Authorization` header value for one signed request.
pub(crate) fn oauth1_header(
    method: HttpMethod,
    url: &Url,
    credential: &OAuth1Credential,
) -> Result<String, RuntimeError> {
    let mut oauth = oauth1_parameters(credential)?;
    let parameter_string = signature_parameter_string(url, &oauth);
    let base_string = signature_base_string(method, url, &parameter_string);
    oauth.insert(
        "oauth_signature".to_owned(),
        sign_oauth1(&base_string, credential),
    );
    Ok(render_oauth1_header(&oauth))
}

fn oauth1_parameters(
    credential: &OAuth1Credential,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    let mut nonce = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| invalid("generating OAuth1 nonce"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid("system clock is before the Unix epoch"))?
        .as_secs()
        .to_string();
    Ok(BTreeMap::from([
        (
            "oauth_consumer_key".to_owned(),
            credential.consumer_key.clone(),
        ),
        ("oauth_nonce".to_owned(), hex(&nonce)),
        ("oauth_signature_method".to_owned(), "HMAC-SHA1".to_owned()),
        ("oauth_timestamp".to_owned(), timestamp),
        ("oauth_token".to_owned(), credential.access_token.clone()),
        ("oauth_version".to_owned(), "1.0".to_owned()),
    ]))
}

fn signature_parameter_string(url: &Url, oauth: &BTreeMap<String, String>) -> String {
    let mut signature_params = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    signature_params.extend(
        oauth
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    signature_params.sort_by(|left, right| {
        let left = (percent_encode(&left.0), percent_encode(&left.1));
        let right = (percent_encode(&right.0), percent_encode(&right.1));
        left.cmp(&right)
    });
    signature_params
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn signature_base_string(method: HttpMethod, url: &Url, parameter_string: &str) -> String {
    let mut base_url = url.clone();
    base_url.set_query(None);
    base_url.set_fragment(None);
    format!(
        "{}&{}&{}",
        method.as_str(),
        percent_encode(base_url.as_str()),
        percent_encode(parameter_string)
    )
}

fn sign_oauth1(base_string: &str, credential: &OAuth1Credential) -> String {
    let signing_key = format!(
        "{}&{}",
        percent_encode(&credential.consumer_secret),
        percent_encode(&credential.access_secret)
    );
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, signing_key.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hmac::sign(&key, base_string.as_bytes()))
}

fn render_oauth1_header(oauth: &BTreeMap<String, String>) -> String {
    format!(
        "OAuth {}",
        oauth
            .iter()
            .map(|(key, value)| format!("{}=\"{}\"", percent_encode(key), percent_encode(value)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// RFC 3986 unreserved characters pass; every other byte is escaped.
pub(crate) fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
