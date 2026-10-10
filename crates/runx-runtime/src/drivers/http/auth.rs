//! Credential header presentation shared by HTTP and MCP; OAuth 1.0a signs
//! each HTTP request with HMAC-SHA1 (RFC 5849).

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use url::Url;

use crate::RuntimeError;
use crate::http::HttpMethod;
use crate::http::RuntimeHttpHeader;
use crate::kernel::operation::credentials::CredentialSecret;
use runx_contracts::connector::HttpAuth;

/// Present only the credential chosen by an admitted instance. HTTP and MCP
/// share this boundary; ordinary request templates never receive these fields.
pub(crate) fn credential_header(
    presentation: &HttpAuth,
    credential: &CredentialSecret,
    method: HttpMethod,
    url: &Url,
) -> Result<RuntimeHttpHeader, String> {
    let value = match presentation {
        HttpAuth::Bearer { field } => format!("Bearer {}", credential.field(field.as_str())?),
        HttpAuth::Header { field, .. } => credential.field(field.as_str())?.to_owned(),
        HttpAuth::Oauth1 => {
            let signed = oauth1_header(
                method,
                url,
                &OAuth1Credential {
                    consumer_key: credential.field("consumer_key")?.to_owned(),
                    consumer_secret: credential.field("consumer_secret")?.to_owned(),
                    access_token: credential.field("access_token")?.to_owned(),
                    access_secret: credential.field("access_secret")?.to_owned(),
                },
            )
            .map_err(|error| error.to_string())?;
            // The signature is derived material of its own: an echo of it
            // alone is redacted as the header is.
            credential.presented(&signed.signature);
            signed.header
        }
    };
    credential.presented(&value);
    Ok(RuntimeHttpHeader::secret(
        presentation.header_name().to_ascii_lowercase(),
        value,
    ))
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: "http".to_owned(),
        message: message.into(),
    }
}

/// OAuth 1.0a user-context material: a consumer and an access token.
pub(crate) struct OAuth1Credential {
    pub consumer_key: String,
    pub consumer_secret: String,
    pub access_token: String,
    pub access_secret: String,
}

/// One signed request: its `Authorization` header value and the signature
/// within it.
pub(crate) struct OAuth1Signed {
    pub(crate) header: String,
    pub(crate) signature: String,
}

/// Sign one request.
pub(crate) fn oauth1_header(
    method: HttpMethod,
    url: &Url,
    credential: &OAuth1Credential,
) -> Result<OAuth1Signed, RuntimeError> {
    let mut oauth = oauth1_parameters(credential)?;
    let parameter_string = signature_parameter_string(url, &oauth);
    let base_string = signature_base_string(method, url, &parameter_string);
    let signature = sign_oauth1(&base_string, credential);
    oauth.insert("oauth_signature".to_owned(), signature.clone());
    Ok(OAuth1Signed {
        header: render_oauth1_header(&oauth),
        signature,
    })
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn arbitrary_authentication_headers_are_secret_in_transport_debug() {
        let credential = CredentialSecret::new(
            runx_contracts::connector::CredentialName::new("key").unwrap(),
            BTreeMap::from([("api_key".to_owned(), "private-presented-key".to_owned())]),
        );
        let presentation = serde_json::from_value(serde_json::json!({
            "type": "header", "name": "X-Custom-Identity", "field": "api_key"
        }))
        .unwrap();
        let header = credential_header(
            &presentation,
            &credential,
            HttpMethod::Get,
            &Url::parse("https://api.example.com").unwrap(),
        )
        .unwrap();
        assert_eq!(header.name, "x-custom-identity");
        assert_eq!(header.value, "private-presented-key");
        assert!(header.is_sensitive());
        assert!(!format!("{header:?} {credential:?}").contains("private-presented-key"));
        let bearer =
            serde_json::from_value(serde_json::json!({"type": "bearer", "field": "api_key"}))
                .unwrap();
        let bearer = credential_header(
            &bearer,
            &credential,
            HttpMethod::Get,
            &Url::parse("https://api.example.com").unwrap(),
        )
        .unwrap();
        let echo = base64::engine::general_purpose::STANDARD.encode(&bearer.value);
        assert_eq!(credential.scrub_text(echo), "[redacted-credential]");
    }

    #[test]
    fn an_oauth1_signature_echoed_alone_is_redacted() {
        let credential = CredentialSecret::new(
            runx_contracts::connector::CredentialName::new("oauth").unwrap(),
            BTreeMap::from(
                [
                    ("consumer_key", "consumer"),
                    ("consumer_secret", "consumer-secret"),
                    ("access_token", "token"),
                    ("access_secret", "token-secret"),
                ]
                .map(|(field, value)| (field.to_owned(), value.to_owned())),
            ),
        );
        let presentation = serde_json::from_value(serde_json::json!({"type": "oauth1"})).unwrap();
        let header = credential_header(
            &presentation,
            &credential,
            HttpMethod::Post,
            &Url::parse("https://api.example.com/2/tweets?x=1").unwrap(),
        )
        .unwrap();
        let encoded = header
            .value
            .split("oauth_signature=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap()
            .to_owned();
        let signature = url::form_urlencoded::parse(format!("s={encoded}").as_bytes())
            .next()
            .unwrap()
            .1
            .into_owned();
        assert_ne!(
            signature, encoded,
            "the signature is percent-encoded in the header"
        );
        for echo in [signature, encoded] {
            assert_eq!(
                credential.scrub_text(format!("provider said {echo}")),
                "provider said [redacted-credential]"
            );
        }
    }
}
