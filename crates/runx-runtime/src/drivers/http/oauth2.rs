//! OAuth2 app authentication inside credential custody. No token is an
//! operation answer, evidence blob, template input or durable session.
use std::cell::RefCell;
use std::time::{Duration, Instant};

use base64::Engine as _;
use runx_contracts::connector::{HttpAuth, TemplateInputs};
use runx_contracts::schema::NonEmptyString;

use crate::credentials::SecretString;
use crate::http::{HttpMethod, RuntimeHttpHeader, RuntimeHttpRequest, RuntimeHttpTransport};
use crate::kernel::operation::credentials::CredentialSecret;

pub(super) struct Token {
    value: SecretString,
    refresh_at: Instant,
}

pub(super) fn authorization(
    presentation: &HttpAuth,
    credential: &CredentialSecret,
    transport: &(dyn RuntimeHttpTransport + Send + Sync),
    cached: &RefCell<Option<Token>>,
    inputs: &TemplateInputs<'_>,
    allowed_hosts: &[NonEmptyString],
) -> Result<Option<RuntimeHttpHeader>, String> {
    let HttpAuth::Oauth2ClientCredentials {
        token_url,
        client_id,
        client_secret,
        scope,
    } = presentation
    else {
        return Ok(None);
    };
    let endpoint = token_url.render_string(inputs).map_err(|_| unavailable())?;
    let url = url::Url::parse(&endpoint).map_err(|_| unavailable())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port().is_some()
        || !url
            .host_str()
            .is_some_and(|host| allowed_hosts.iter().any(|allowed| allowed.as_str() == host))
    {
        return Err(unavailable());
    }
    let now = Instant::now();
    if !cached
        .borrow()
        .as_ref()
        .is_some_and(|token| token.refresh_at > now)
    {
        // RFC 6749 2.3.1: form-encode each component before Basic encoding.
        let basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!(
                "{}:{}",
                form_component(credential.field(client_id.as_str())?),
                form_component(credential.field(client_secret.as_str())?),
            ))
        );
        credential.presented(&basic);
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("grant_type", "client_credentials");
        if let Some(scope) = scope {
            form.append_pair("scope", scope.as_str());
        }
        let response = transport
            .send(RuntimeHttpRequest {
                method: HttpMethod::Post,
                url: url.to_string(),
                headers: vec![
                    RuntimeHttpHeader::secret("authorization", basic),
                    RuntimeHttpHeader::new("content-type", "application/x-www-form-urlencoded"),
                    RuntimeHttpHeader::new("accept", "application/json"),
                ],
                body: Some(form.finish()),
            })
            .map_err(|_| unavailable())?;
        if !(200..300).contains(&response.status) {
            return Err(unavailable());
        }
        let body: serde_json::Value =
            serde_json::from_str(&response.body).map_err(|_| unavailable())?;
        let value = body
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .filter(|token| !token.is_empty() && token.bytes().all(|b| (0x21..=0x7e).contains(&b)))
            .ok_or_else(unavailable)?;
        credential.presented(value);
        let kind = body
            .get("token_type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(unavailable)?;
        if !kind.eq_ignore_ascii_case("bearer") {
            return Err(unavailable());
        }
        let expires = body
            .get("expires_in")
            .and_then(serde_json::Value::as_u64)
            .filter(|seconds| *seconds > 0)
            .ok_or_else(unavailable)?;
        let lifetime = Duration::from_secs(expires);
        // Refresh 10% early, capped at sixty seconds for long-lived tokens.
        let refresh_at = now
            .checked_add(lifetime.saturating_sub((lifetime / 10).min(Duration::from_secs(60))))
            .ok_or_else(unavailable)?;
        *cached.borrow_mut() = Some(Token {
            value: SecretString::new(value),
            refresh_at,
        });
    }
    let cache = cached.borrow();
    let token = cache.as_ref().ok_or_else(unavailable)?;
    let header = format!("Bearer {}", token.value.expose());
    credential.presented(&header);
    Ok(Some(RuntimeHttpHeader::secret("authorization", header)))
}

fn unavailable() -> String {
    "the OAuth2 app credential could not obtain a valid bearer token; no provider operation was sent".to_owned()
}

fn form_component(value: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("", value)
        .finish()[1..]
        .to_owned()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::http::{RuntimeHttpError, RuntimeHttpResponse};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    fn test_authorization(
        presentation: &HttpAuth,
        credential: &CredentialSecret,
        transport: &(dyn RuntimeHttpTransport + Send + Sync),
        cached: &RefCell<Option<Token>>,
    ) -> Result<Option<RuntimeHttpHeader>, String> {
        super::authorization(
            presentation,
            credential,
            transport,
            cached,
            &TemplateInputs::default(),
            &["auth.example.test".into()],
        )
    }

    struct Provider {
        answer: RuntimeHttpResponse,
        calls: Mutex<Vec<RuntimeHttpRequest>>,
    }
    impl RuntimeHttpTransport for Provider {
        fn send(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
            self.calls.lock().unwrap().push(request);
            Ok(self.answer.clone())
        }
    }

    #[test]
    fn token_endpoint_choices_obey_the_shared_corpus_before_credentials_leave() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../fixtures/contracts/connector-oauth2.json"
        ))
        .unwrap();
        for case in corpus["endpoint_cases"].as_array().unwrap() {
            let mut presentation = corpus["presentation"].clone();
            presentation["token_url"] = case["template"].clone();
            let presentation: HttpAuth = serde_json::from_value(presentation).unwrap();
            let arguments = case
                .get("arguments")
                .cloned()
                .map(|value| serde_json::from_value::<runx_contracts::JsonValue>(value).unwrap());
            let parameters = case
                .get("parameters")
                .cloned()
                .map(|value| serde_json::from_value::<runx_contracts::JsonObject>(value).unwrap());
            let hosts: Vec<NonEmptyString> =
                serde_json::from_value(case["allowed_hosts"].clone()).unwrap();
            let credential = CredentialSecret::new(
                runx_contracts::connector::CredentialName::new("app").unwrap(),
                serde_json::from_value(corpus["credential"].clone()).unwrap(),
            );
            let transport = Provider {
                answer: RuntimeHttpResponse::new(
                    200,
                    r#"{"access_token":"private-token","token_type":"Bearer","expires_in":60}"#,
                ),
                calls: Mutex::new(Vec::new()),
            };
            let cached = RefCell::new(None);
            let result = super::authorization(
                &presentation,
                &credential,
                &transport,
                &cached,
                &TemplateInputs {
                    arguments: arguments.as_ref(),
                    parameters: parameters.as_ref(),
                    ..TemplateInputs::default()
                },
                &hosts,
            );
            let sent = transport.calls.lock().unwrap();
            if let Some(expected) = case.get("expected").and_then(serde_json::Value::as_str) {
                assert!(result.is_ok(), "{}", case["name"]);
                assert_eq!(sent.len(), 1);
                assert_eq!(sent[0].url, expected);
            } else {
                assert!(result.is_err(), "{}", case["name"]);
                assert!(sent.is_empty(), "invalid endpoint sent an app credential");
            }
        }
    }

    #[test]
    fn app_auth_obeys_the_shared_corpus_without_exposing_tokens_or_repeating_auth() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../fixtures/contracts/connector-oauth2.json"
        ))
        .unwrap();
        let presentation = serde_json::from_value(corpus["presentation"].clone()).unwrap();
        for case in corpus["cases"].as_array().unwrap() {
            let credential = CredentialSecret::new(
                runx_contracts::connector::CredentialName::new("app").unwrap(),
                serde_json::from_value::<BTreeMap<String, String>>(corpus["credential"].clone())
                    .unwrap(),
            );
            let transport = Provider {
                answer: RuntimeHttpResponse::new(
                    case["status"].as_u64().unwrap() as u16,
                    case["body"].to_string(),
                ),
                calls: Mutex::new(Vec::new()),
            };
            let cached = RefCell::new(None);
            let result = test_authorization(&presentation, &credential, &transport, &cached);
            assert_eq!(
                result.is_ok(),
                case["accept"].as_bool().unwrap(),
                "{}",
                case["name"]
            );
            if case["accept"] == true {
                let header = result.unwrap().unwrap();
                assert_eq!(header.value, "Bearer private-access-token");
                assert!(!format!("{header:?}").contains("private-access-token"));
                assert!(
                    !credential
                        .scrub_text("private-access-token")
                        .contains("private-access-token")
                );
                test_authorization(&presentation, &credential, &transport, &cached).unwrap();
            } else {
                assert!(!result.unwrap_err().contains("private"));
            }
            let sent = transport.calls.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].method.as_str(), corpus["request"]["method"]);
            assert_eq!(sent[0].url, corpus["request"]["url"]);
            assert_eq!(sent[0].body.as_deref().unwrap(), corpus["request"]["body"]);
            let basic = sent[0]
                .headers
                .iter()
                .find(|h| h.name == "authorization")
                .unwrap();
            assert_eq!(
                basic.value,
                format!(
                    "Basic {}",
                    base64::engine::general_purpose::STANDARD
                        .encode(corpus["request"]["basic_plaintext"].as_str().unwrap())
                )
            );
            assert!(!format!("{:?}", sent[0]).contains("private"));
        }
    }

    #[test]
    fn expired_tokens_are_replaced_without_retrying_any_provider_operation() {
        let presentation: HttpAuth = serde_json::from_value(serde_json::json!({"type":"oauth2_client_credentials", "token_url":"https://auth.example.test/token", "client_id":"id", "client_secret":"secret"})).unwrap();
        let credential = CredentialSecret::new(
            runx_contracts::connector::CredentialName::new("app").unwrap(),
            BTreeMap::from([
                ("id".to_owned(), "client".to_owned()),
                ("secret".to_owned(), "private-secret".to_owned()),
            ]),
        );
        let transport = Provider {
            answer: RuntimeHttpResponse::new(
                200,
                r#"{"access_token":"new-token","token_type":"Bearer","expires_in":3600}"#,
            ),
            calls: Mutex::new(Vec::new()),
        };
        let cached = RefCell::new(Some(Token {
            value: SecretString::new("expired-token"),
            refresh_at: Instant::now(),
        }));
        assert_eq!(
            test_authorization(&presentation, &credential, &transport, &cached)
                .unwrap()
                .unwrap()
                .value,
            "Bearer new-token"
        );
        assert_eq!(transport.calls.lock().unwrap().len(), 1);
    }
}
