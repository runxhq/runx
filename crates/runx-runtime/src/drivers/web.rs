//! The `web` driver: the runtime's guarded public web read.
//!
//! One URL is fetched with GET through a transport that refuses private
//! networks. Every redirect is admitted against the call's host allowlist
//! before it is followed, the page is read whole, and it is extracted as text,
//! metadata, links or one JSON document. A refused URL, a transport failure, a
//! non-2xx answer or a page past the operator's ceiling is a rejection: no
//! result stands for a page that was not read in full.
//!
//! The URL is the caller's choice, so this is the one read with a byte
//! ceiling: [`MAX_BYTES_ENV`], set by the operator, never by a skill.

use std::collections::BTreeMap;

use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use serde::Deserialize;
use url::Url;

use crate::http::{
    HttpMethod, RuntimeHttpError, RuntimeHttpHeader, RuntimeHttpRequest, RuntimeHttpResponse,
    RuntimeHttpTransport,
};
use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Check, Driver, Observed, Unperformed};

mod extract;
mod policy;

use policy::{host_allowed, normalize_allowlist, normalized_host, parse_web_url};

const MAX_REDIRECTS: usize = 10;
/// The operator setting that raises or lowers the ceiling on one page.
pub(crate) const MAX_BYTES_ENV: &str = "RUNX_WEB_READ_MAX_BYTES";
const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExtractMode {
    #[default]
    Text,
    Metadata,
    Links,
    /// The body as one JSON document.
    Json,
}

impl ExtractMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Metadata => "metadata",
            Self::Links => "links",
            Self::Json => "json",
        }
    }
}

/// The call input, as the connector's input schema admits it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchInput {
    url: String,
    #[serde(default = "any_public_host")]
    allowlist: Vec<String>,
    #[serde(default)]
    extract: ExtractMode,
}

fn any_public_host() -> Vec<String> {
    vec!["*".to_owned()]
}

fn max_bytes(env: &BTreeMap<String, String>) -> Result<usize, Unperformed> {
    let Some(value) = env.get(MAX_BYTES_ENV) else {
        return Ok(DEFAULT_MAX_BYTES);
    };
    value
        .parse::<usize>()
        .ok()
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| {
            not_sent(
                "invalid_configuration",
                format!("{MAX_BYTES_ENV} must be a positive byte count, got {value:?}"),
            )
        })
}

pub(crate) struct Web<'a> {
    pub transport: &'a dyn RuntimeHttpTransport,
    pub call: &'a AdmittedCall<'a>,
}

impl Driver for Web<'_> {
    fn perform(&self, _item: Option<usize>) -> Result<Observed, Unperformed> {
        fetch(self.transport, self.call.env, self.call.arguments)
    }

    fn check(&self, check: Check, _write: &JsonValue) -> Result<Observed, String> {
        Err(format!("a web read has no {check:?}"))
    }

    fn result(
        &self,
        write: &JsonValue,
        _confirmed: &[JsonValue],
    ) -> Result<(JsonValue, Option<String>), String> {
        Ok((write.clone(), None))
    }
}

/// Fetch and extract the page `arguments` names.
fn fetch(
    transport: &dyn RuntimeHttpTransport,
    env: &BTreeMap<String, String>,
    arguments: &JsonValue,
) -> Result<Observed, Unperformed> {
    let input = arguments
        .clone()
        .deserialize_into::<FetchInput>()
        .map_err(|error| not_sent("invalid_input", error.to_string()))?;
    let allowlist = normalize_allowlist(&input.allowlist)
        .map_err(|reason| not_sent("invalid_input", reason))?;
    let max_bytes = max_bytes(env)?;
    let start =
        parse_web_url(input.url.trim()).map_err(|reason| not_sent("invalid_input", reason))?;
    admit_host(&start, &allowlist)?;
    let (final_url, response, redirects) = follow(transport, start, &allowlist, max_bytes)?;
    if !(200..300).contains(&response.status) {
        return Err(Unperformed::Refused {
            code: "provider_error",
            reason: format!("{final_url} answered HTTP {}", response.status),
            answer: None,
        });
    }
    let content_type = header(&response, "content-type")
        .unwrap_or_default()
        .to_owned();
    let extracted =
        extract::extract_content(&response.body, input.extract, &final_url, &content_type)
            .map_err(|reason| Unperformed::Refused {
                code: "invalid_response",
                reason,
                answer: None,
            })?;
    let result = JsonObject::from([
        (
            "final_url".to_owned(),
            JsonValue::String(final_url.to_string()),
        ),
        (
            "status".to_owned(),
            JsonValue::Number(JsonNumber::from(u64::from(response.status))),
        ),
        (
            "content_digest".to_owned(),
            JsonValue::String(response.body_digest.clone()),
        ),
        (
            "extract_mode".to_owned(),
            JsonValue::String(input.extract.as_str().to_owned()),
        ),
        ("extracted".to_owned(), extracted),
        (
            "provenance".to_owned(),
            JsonValue::Object(JsonObject::from([
                (
                    "fetched_at".to_owned(),
                    JsonValue::String(crate::time::now_iso8601()),
                ),
                ("redirects".to_owned(), JsonValue::Array(redirects)),
                (
                    "bytes".to_owned(),
                    JsonValue::Number(JsonNumber::from(response.body_bytes as u64)),
                ),
            ])),
        ),
    ]);
    Ok(Observed {
        value: JsonValue::Object(result),
        evidence: vec![response.body_digest],
    })
}

fn follow(
    transport: &dyn RuntimeHttpTransport,
    start: Url,
    allowlist: &[String],
    max_bytes: usize,
) -> Result<(Url, RuntimeHttpResponse, Vec<JsonValue>), Unperformed> {
    let mut current = start;
    let mut redirects = Vec::new();
    for _ in 0..=MAX_REDIRECTS {
        let response = transport
            .send_limited(
                RuntimeHttpRequest {
                    method: HttpMethod::Get,
                    url: current.to_string(),
                    headers: vec![RuntimeHttpHeader::new(
                        "accept",
                        "text/html, text/plain, application/json;q=0.9, */*;q=0.1",
                    )],
                    body: None,
                },
                max_bytes,
            )
            .map_err(|error| match error {
                RuntimeHttpError::PrivateNetworkUrl { host } => {
                    not_sent("policy_denied", format!("host {host:?} is not publicly routable"))
                }
                RuntimeHttpError::ResponseBodyTooLarge { limit } => Unperformed::Refused {
                    code: "response_too_large",
                    reason: format!("{current} is larger than the {limit}-byte web read ceiling; {MAX_BYTES_ENV} raises it"),
                    answer: None,
                },
                other => Unperformed::MaybeSent {
                    code: "provider_error",
                    reason: other.to_string(),
                },
            })?;
        if !(300..400).contains(&response.status) {
            return Ok((current, response, redirects));
        }
        let location = header(&response, "location").ok_or_else(|| Unperformed::Refused {
            code: "provider_error",
            reason: format!("HTTP {} redirect without a location", response.status),
            answer: None,
        })?;
        let next = current
            .join(location)
            .map_err(|error| error.to_string())
            .and_then(|next| parse_web_url(next.as_str()))
            .map_err(|reason| Unperformed::Refused {
                code: "provider_error",
                reason: format!("invalid redirect URL: {reason}"),
                answer: None,
            })?;
        admit_host(&next, allowlist)?;
        redirects.push(JsonValue::Object(JsonObject::from([
            (
                "status".to_owned(),
                JsonValue::Number(JsonNumber::from(u64::from(response.status))),
            ),
            ("from".to_owned(), JsonValue::String(current.to_string())),
            ("to".to_owned(), JsonValue::String(next.to_string())),
        ])));
        current = next;
    }
    Err(Unperformed::Refused {
        code: "provider_error",
        reason: format!("more than {MAX_REDIRECTS} redirects"),
        answer: None,
    })
}

fn admit_host(url: &Url, allowlist: &[String]) -> Result<(), Unperformed> {
    let host = normalized_host(url).unwrap_or_default();
    if host_allowed(&host, allowlist) {
        return Ok(());
    }
    Err(not_sent(
        "policy_denied",
        format!("host {host:?} is not allowlisted"),
    ))
}

fn header<'a>(response: &'a RuntimeHttpResponse, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn not_sent(code: &'static str, reason: impl Into<String>) -> Unperformed {
    Unperformed::NotSent {
        code,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{
        HarnessExchanges, NativeHttpTransport, RuntimeHarnessHttpExchange,
        RuntimeHarnessHttpRequestBody,
    };

    const URL: &str = "https://fixture.runx.invalid/page";

    fn read(env: &BTreeMap<String, String>) -> Result<Observed, Unperformed> {
        let exchanges = HarnessExchanges::new(vec![RuntimeHarnessHttpExchange {
            method: crate::http::HttpMethod::Get,
            url: URL.to_owned(),
            body: RuntimeHarnessHttpRequestBody::None,
            response: RuntimeHttpResponse::new(200, "0123456789"),
        }]);
        let transport = NativeHttpTransport::new(Some(&exchanges))
            .map_err(|error| not_sent("test", error.to_string()))?;
        let input = JsonValue::Object(JsonObject::from([(
            "url".to_owned(),
            JsonValue::String(URL.to_owned()),
        )]));
        fetch(&transport, env, &input)
    }

    fn code(outcome: Result<Observed, Unperformed>) -> Option<&'static str> {
        match outcome {
            Err(
                Unperformed::NotSent { code, .. }
                | Unperformed::Refused { code, .. }
                | Unperformed::Halted { code, .. }
                | Unperformed::MaybeSent { code, .. },
            ) => Some(code),
            Ok(_) => None,
        }
    }

    #[test]
    fn a_page_is_read_whole_or_refused_past_the_operator_ceiling() {
        let whole = read(&BTreeMap::new()).ok().map(|observed| observed.value);
        let bytes = whole
            .as_ref()
            .and_then(JsonValue::as_object)
            .and_then(|result| result.get("provenance"))
            .and_then(JsonValue::as_object)
            .and_then(|provenance| provenance.get("bytes"))
            .cloned();
        assert_eq!(bytes, Some(JsonValue::Number(JsonNumber::from(10_u64))));

        let ceiling = |value: &str| BTreeMap::from([(MAX_BYTES_ENV.to_owned(), value.to_owned())]);
        assert!(read(&ceiling("10")).is_ok());
        assert_eq!(code(read(&ceiling("9"))), Some("response_too_large"));
        assert_eq!(code(read(&ceiling("0"))), Some("invalid_configuration"));
        assert_eq!(code(read(&ceiling("lots"))), Some("invalid_configuration"));
    }
}
