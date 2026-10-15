//! The `http` driver: the manifest's requests, sent to its declared hosts with
//! the instance's credential.
//!
//! A read sends its requests, repeating one per item where it declares
//! `each`, and decodes the answers. A write sends its one request. Checking
//! reads are sent as declared and decoded, or reduced to their status where
//! the manifest expects one. Every request is rendered from typed templates
//! straight into the transport request.

use std::cell::OnceCell;
use std::collections::BTreeMap;

use runx_contracts::connector::{
    DecodeStep, HttpImplementation, HttpMethod as ManifestMethod, HttpPages, HttpReadback,
    HttpRequest, RateLimit, TemplateError, TemplateInputs,
};
use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use url::Url;

use crate::http::{
    HttpMethod, RuntimeHttpHeader, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
};
use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Check, Driver, Observed, Unperformed, at_path};
use crate::kernel::operation::hosted::Gateway;

pub(crate) mod auth;

/// The status a status-only checking read reports, for the kernel to match.
pub(crate) const STATUS_FIELD: &str = "http_status";

/// The ceilings of one paged read, whatever limit the call states.
const MAX_PAGES: usize = 100;
const MAX_PAGED_ITEMS: usize = 10_000;
const MAX_PAGED_BYTES: usize = 32 * 1024 * 1024;
const MAX_PAGED_DURATION: std::time::Duration = std::time::Duration::from_secs(120);

/// What the kernel renders a request's paid invocation as when the request
/// goes through runx Connect. There the kernel renders only to check the
/// request before anything leaves; Connect renders it again with the
/// invocation it alone holds, so this stand-in never reaches a provider.
const CONNECT_INVOCATION: &str = "runx-connect-invocation";

/// Where a rendered request goes.
pub(crate) enum Egress<'a> {
    /// Straight to the provider, with the instance's credential.
    Direct(&'a (dyn RuntimeHttpTransport + Send + Sync)),
    /// Through runx Connect, which renders the same declared request and
    /// presents the grant's credential.
    Connect(&'a Gateway<'a>),
}

pub(crate) struct Http<'a> {
    egress: Egress<'a>,
    call: &'a AdmittedCall<'a>,
    spec: &'a HttpImplementation,
    /// A repeated write's items, rendered once.
    items: OnceCell<Option<Vec<JsonValue>>>,
}

impl<'a> Http<'a> {
    pub(crate) fn new(
        egress: Egress<'a>,
        call: &'a AdmittedCall<'a>,
        spec: &'a HttpImplementation,
    ) -> Self {
        Self {
            egress,
            call,
            spec,
            items: OnceCell::new(),
        }
    }
}

impl Driver for Http<'_> {
    fn items(&self) -> Result<Option<Vec<JsonValue>>, String> {
        if let Some(items) = self.items.get() {
            return Ok(items.clone());
        }
        let each = match (self.call.mutation(), self.spec.requests.as_slice()) {
            (true, [request]) => request.each.as_ref(),
            _ => None,
        };
        let items = match each.map(|each| each.render(&self.inputs(None, None))) {
            None => None,
            Some(Ok(JsonValue::Array(items))) => Some(items),
            Some(Ok(_)) => return Err("each must render to an array".to_owned()),
            Some(Err(error)) => return Err(error.to_string()),
        };
        Ok(self.items.get_or_init(|| items).clone())
    }

    fn perform(&self, item: Option<usize>) -> Result<Observed, Unperformed> {
        let spec = self.spec;
        let mutation = self.call.mutation();
        if let Some(index) = item {
            let items = self.items().map_err(not_sent)?.unwrap_or_default();
            let item = items
                .get(index)
                .ok_or_else(|| not_sent(format!("the batch has no item {index}")))?;
            let request = spec
                .requests
                .as_slice()
                .first()
                .ok_or_else(|| not_sent("the write declares no request".to_owned()))?;
            let response = self.send(request, Some((index, item)), None, None)?;
            return Ok(Observed {
                value: self.answered(&response, true)?,
                evidence: vec![response.body_digest],
            });
        }
        let mut evidence = Vec::with_capacity(spec.requests.len());
        let mut decoded = Vec::with_capacity(spec.requests.len());
        // A read's later requests render from its earlier answers.
        let mut answers = BTreeMap::new();
        for request in spec.requests.iter() {
            let earlier = (!request.responses().is_empty()).then_some(&answers);
            let value = match (&request.each, &request.pages) {
                (Some(each), _) => {
                    let items = match each.render(&self.inputs(None, earlier)) {
                        Ok(JsonValue::Array(items)) => items,
                        Ok(_) => return Err(not_sent("each must render to an array".to_owned())),
                        Err(error) => return Err(not_sent(error.to_string())),
                    };
                    let mut repeated = Vec::with_capacity(items.len());
                    for item in items.iter().enumerate() {
                        let response = self.send(request, Some(item), earlier, None)?;
                        evidence.push(response.body_digest.clone());
                        repeated.push(self.answered(&response, false)?);
                    }
                    JsonValue::Array(repeated)
                }
                (None, Some(pages)) => self.pages(request, pages, earlier, &mut evidence)?,
                (None, None) => {
                    let response = self.send(request, None, earlier, None)?;
                    evidence.push(response.body_digest.clone());
                    self.answered(&response, mutation)?
                }
            };
            answers.insert(
                request.id.as_str().to_owned(),
                JsonObject::from([("json".to_owned(), value.clone())]),
            );
            decoded.push(value);
        }
        let value = match <[JsonValue; 1]>::try_from(decoded) {
            Ok([single]) => single,
            Err(many) => JsonValue::Array(many),
        };
        Ok(Observed { value, evidence })
    }

    fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, String> {
        let (readback, responses, item) = match check {
            Check::Precondition(index) => (self.spec.preconditions.get(index), None, None),
            Check::Readback { readback, item } => (
                self.spec.readbacks.get(readback),
                Some(BTreeMap::from([(
                    "write".to_owned(),
                    JsonObject::from([("json".to_owned(), write.clone())]),
                )])),
                match item {
                    Some(index) => Some((
                        index,
                        self.items()?
                            .and_then(|items| items.get(index).cloned())
                            .ok_or_else(|| format!("the batch has no item {index}"))?,
                    )),
                    None => None,
                },
            ),
        };
        let readback =
            readback.ok_or_else(|| format!("the implementation declares no {check:?}"))?;
        let response = self
            .send(
                &readback.request,
                item.as_ref().map(|(index, item)| (*index, item)),
                responses.as_ref(),
                None,
            )
            .map_err(Unperformed::reason)?;
        let value = match readback.expected_status {
            Some(_) => JsonValue::Object(JsonObject::from([(
                STATUS_FIELD.to_owned(),
                JsonValue::Number(JsonNumber::from(u64::from(response.status))),
            )])),
            None => decode(
                &response,
                readback.decode.as_deref().unwrap_or(&self.spec.decode),
            )?,
        };
        Ok(Observed {
            value,
            evidence: vec![response.body_digest],
        })
    }

    fn result(
        &self,
        write: &JsonValue,
        confirmed: &[JsonValue],
    ) -> Result<(JsonValue, Option<String>), String> {
        let readbacks = if self.call.mutation() {
            confirmed.to_vec()
        } else {
            vec![write.clone()]
        };
        let source = JsonValue::Object(JsonObject::from([
            ("acknowledgement".to_owned(), write.clone()),
            ("readbacks".to_owned(), JsonValue::Array(readbacks)),
        ]));
        let result = self
            .spec
            .result_paths
            .iter()
            .map(|(field, path)| {
                at_path(&source, path)
                    .cloned()
                    .map(|value| (field.clone(), value))
                    .ok_or_else(|| format!("result path {path:?} is missing"))
            })
            .collect::<Result<JsonObject, _>>()?;
        // A write the provider gives no id, a batch or an acknowledgement
        // without one, is identified by the key it was sent with, which is
        // stable for its intent. Its answer cannot identify it: two
        // different writes may be answered alike.
        let operation_id = match self.spec.operation_id_path.as_deref() {
            None if self.call.mutation() => Some(format!(
                "http:{}",
                self.call
                    .idempotency_key
                    .ok_or("a write is sent with an idempotency key")?
            )),
            Some(path) => Some(
                at_path(write, path)
                    .and_then(scalar_string)
                    .ok_or_else(|| format!("operation id path {path:?} is missing"))?,
            ),
            None => None,
        };
        Ok((JsonValue::Object(result), operation_id))
    }
}

/// The status-only expectation of a checking read, as a match on what
/// [`Http::check`] reports.
pub(crate) fn status_match(
    readback: &HttpReadback,
) -> Option<runx_contracts::connector::ReadbackMatch> {
    readback
        .expected_status
        .map(|status| runx_contracts::connector::ReadbackMatch {
            expected: runx_contracts::connector::ReadbackExpected::Literal {
                value: JsonValue::Number(JsonNumber::from(u64::from(status))),
            },
            observed: STATUS_FIELD.to_owned().into(),
        })
}

impl Http<'_> {
    fn inputs<'a>(
        &'a self,
        item: Option<&'a JsonValue>,
        responses: Option<&'a BTreeMap<String, JsonObject>>,
    ) -> TemplateInputs<'a> {
        TemplateInputs {
            arguments: Some(self.call.arguments),
            parameters: Some(self.call.parameters),
            responses,
            item,
            idempotency_key: self.call.idempotency_key,
            // Sent directly there is no paid invocation, so a request that
            // sends one fails closed.
            invocation_id: match self.egress {
                Egress::Connect(_) => Some(CONNECT_INVOCATION),
                Egress::Direct(_) => None,
            },
            ..TemplateInputs::default()
        }
    }

    /// Render one declared request, for `item` (its position and value) of a
    /// repeated request or at the `cursor` of a paged one, and send it. The
    /// kernel renders on both placements, so every rendering error stops a
    /// request before anything leaves.
    fn send(
        &self,
        request: &HttpRequest,
        item: Option<(usize, &JsonValue)>,
        responses: Option<&BTreeMap<String, JsonObject>>,
        cursor: Option<&str>,
    ) -> Result<RuntimeHttpResponse, Unperformed> {
        let rendered = self
            .render(request, item.map(|(_, item)| item), responses, cursor)
            .map_err(not_sent)?;
        let transport = match self.egress {
            Egress::Direct(transport) => transport,
            Egress::Connect(gateway) => {
                return gateway.send(request.id.as_str(), item.map(|(index, _)| index), responses);
            }
        };
        // Sent once: a read's cost is not declared, so repeating it is not
        // free to assume. The transport repeats only what HTTP defines as
        // safe to repeat.
        transport
            .send(rendered)
            .map_err(|error| Unperformed::MaybeSent {
                code: "transport_failed",
                reason: error.to_string(),
            })
    }

    fn render(
        &self,
        request: &HttpRequest,
        item: Option<&JsonValue>,
        responses: Option<&BTreeMap<String, JsonObject>>,
        cursor: Option<&str>,
    ) -> Result<RuntimeHttpRequest, String> {
        let instance = match self.egress {
            Egress::Direct(_) => Some(self),
            Egress::Connect(_) => None,
        };
        render_request(
            self.spec,
            request,
            &self.inputs(item, responses),
            cursor,
            instance,
        )
    }

    /// A call's answer, decoded. A rate limit, a refused credential or a
    /// billing hold halts the caller. For a write, any other 4xx but a
    /// timeout is the provider refusing to act, with its answer; any other
    /// answer that cannot be decoded may have acted.
    fn answered(
        &self,
        response: &RuntimeHttpResponse,
        mutation: bool,
    ) -> Result<JsonValue, Unperformed> {
        if let Some(halted) = halted(response, self.spec.rate_limit.as_ref()) {
            return Err(halted);
        }
        decode(response, &self.spec.decode).map_err(|reason| match response.status {
            400..=499 if mutation && response.status != 408 => Unperformed::Refused {
                code: "provider_rejected",
                reason,
                answer: serde_json::from_str::<JsonValue>(&response.body).ok(),
            },
            _ => Unperformed::MaybeSent {
                code: "invalid_response",
                reason,
            },
        })
    }

    /// A paged read: each page in turn, until a page names no cursor, holds
    /// no items, or the items reach the call's limit. Every page is whole, so
    /// the last can carry the items past the limit; a cursor the provider
    /// names twice fails the read. A read is finite whatever the call asks:
    /// it stops at the paged read's ceilings and returns the provider's next
    /// cursor with what it read.
    fn pages(
        &self,
        request: &HttpRequest,
        pages: &HttpPages,
        responses: Option<&BTreeMap<String, JsonObject>>,
        evidence: &mut Vec<String>,
    ) -> Result<JsonValue, Unperformed> {
        let limit = match pages
            .limit
            .as_ref()
            .map(|limit| limit.render(&self.inputs(None, responses)))
        {
            None
            | Some(Err(TemplateError::Unavailable {
                binding: "$argument",
                ..
            })) => None,
            Some(Ok(JsonValue::Number(JsonNumber::I64(limit)))) => Some(
                usize::try_from(limit)
                    .map_err(|_| not_sent("a page limit is a whole number".to_owned()))?,
            ),
            Some(Ok(_)) => return Err(not_sent("a page limit is a whole number".to_owned())),
            Some(Err(error)) => return Err(not_sent(error.to_string())),
        };
        let mut items = Vec::new();
        let mut cursor = None::<String>;
        let mut cursors = std::collections::BTreeSet::new();
        let started = std::time::Instant::now();
        let (mut read_pages, mut read_bytes) = (0_usize, 0_usize);
        loop {
            let response = self.send(request, None, responses, cursor.as_deref())?;
            evidence.push(response.body_digest.clone());
            read_pages += 1;
            read_bytes = read_bytes.saturating_add(response.body.len());
            let page = self.answered(&response, false)?;
            // A provider omits the items of an empty page.
            let found = match at_path(&page, pages.items.as_str()) {
                None | Some(JsonValue::Null) => Vec::new(),
                Some(JsonValue::Array(found)) => found.clone(),
                Some(_) => {
                    return Err(Unperformed::MaybeSent {
                        code: "invalid_response",
                        reason: format!("page items at {} are not an array", pages.items),
                    });
                }
            };
            let empty = found.is_empty();
            items.extend(found);
            cursor = at_path(&page, pages.cursor.as_str()).and_then(scalar_string);
            // A provider that names a cursor it already answered would page
            // forever; the read fails rather than loop.
            if let Some(next) = &cursor
                && !cursors.insert(next.clone())
            {
                return Err(Unperformed::MaybeSent {
                    code: "invalid_response",
                    reason: format!("the provider repeated page cursor {next}"),
                });
            }
            if cursor.is_none() || empty || limit.is_some_and(|limit| items.len() >= limit) {
                break;
            }
            if read_pages >= MAX_PAGES
                || items.len() >= MAX_PAGED_ITEMS
                || read_bytes >= MAX_PAGED_BYTES
                || started.elapsed() >= MAX_PAGED_DURATION
            {
                break;
            }
        }
        Ok(JsonValue::Object(JsonObject::from([
            ("items".to_owned(), JsonValue::Array(items)),
            (
                "next".to_owned(),
                cursor.map_or(JsonValue::Null, JsonValue::String),
            ),
        ])))
    }

    fn authorization(
        &self,
        method: HttpMethod,
        url: &Url,
    ) -> Result<Option<RuntimeHttpHeader>, String> {
        if self.spec.auth.is_empty() {
            return Ok(None);
        }
        let secret = self
            .call
            .credential
            .ok_or("the implementation authenticates but the instance delivered no credential")?;
        let presentation = self.spec.auth.get(&secret.name).ok_or_else(|| {
            format!(
                "the implementation does not say how credential {} authenticates",
                secret.name
            )
        })?;
        auth::credential_header(presentation, secret, method, url)
    }

    /// Place the instance's credential in the request body, when its
    /// implementation presents it there.
    fn credential_body(&self, body: Option<&mut JsonValue>) -> Result<(), String> {
        let Some(secret) = self.call.credential else {
            return Ok(());
        };
        match self.spec.auth.get(&secret.name) {
            Some(presentation) => auth::credential_body(presentation, secret, body),
            None => Ok(()),
        }
    }

    /// The URL the instance's credential stands for, when its implementation
    /// presents the credential as the request URL.
    fn credential_url(&self, declared: &Url) -> Result<Option<Url>, String> {
        let Some(secret) = self.call.credential else {
            return Ok(None);
        };
        match self.spec.auth.get(&secret.name) {
            Some(presentation) => auth::credential_url(presentation, secret, declared),
            None => Ok(None),
        }
    }
}

/// One declared request as runx Connect is asked to send it: by its id, with
/// the element of a repeated write by position and, for a write's checking
/// read, the write's answers.
pub(crate) struct DeclaredCall<'a> {
    pub request: &'a str,
    pub arguments: &'a JsonValue,
    pub parameters: Option<&'a JsonObject>,
    pub idempotency_key: Option<&'a str>,
    /// The paid invocation Connect holds for the call's run.
    pub invocation_id: Option<&'a str>,
    pub item: Option<usize>,
    pub responses: Option<&'a BTreeMap<String, JsonObject>>,
}

/// Render a declared request as runx Connect does, after
/// `renderDeclaredRequest` in `packages/contracts/src/connector.ts`. It
/// carries no credential: Connect's gateway presents the grant's.
pub(crate) fn render_declared(
    spec: &HttpImplementation,
    call: &DeclaredCall<'_>,
) -> Result<RuntimeHttpRequest, String> {
    let checks = || spec.preconditions.iter().chain(&spec.readbacks);
    let request = spec
        .requests
        .iter()
        .chain(checks().map(|check| &check.request))
        .find(|candidate| candidate.id.as_str() == call.request)
        .ok_or_else(|| format!("the implementation declares no request {}", call.request))?;
    let base = TemplateInputs {
        arguments: Some(call.arguments),
        parameters: call.parameters,
        idempotency_key: call.idempotency_key,
        invocation_id: call.invocation_id,
        ..TemplateInputs::default()
    };
    let item = call
        .item
        .map(|index| {
            // A checking read of a repeated write binds the write's item.
            let each = request
                .each
                .as_ref()
                .or_else(|| {
                    checks()
                        .any(|check| check.request.id == request.id)
                        .then(|| spec.requests.iter().find_map(|write| write.each.as_ref()))
                        .flatten()
                })
                .ok_or_else(|| format!("request {} does not repeat", request.id))?;
            match each.render(&base).map_err(|error| error.to_string())? {
                JsonValue::Array(mut items) if index < items.len() => Ok(items.swap_remove(index)),
                JsonValue::Array(_) => Err(format!("the batch has no item {index}")),
                _ => Err("each must render to an array".to_owned()),
            }
        })
        .transpose()?;
    let inputs = TemplateInputs {
        item: item.as_ref(),
        responses: call.responses,
        ..base
    };
    render_request(spec, request, &inputs, None, None)
}

/// Render one declared request from `inputs`: its URL, checked against the
/// implementation's hosts, its query, headers and body. Sent directly, the
/// `instance` places its credential where the implementation presents it;
/// through runx Connect there is none, since Connect's gateway presents the
/// grant's.
fn render_request(
    spec: &HttpImplementation,
    request: &HttpRequest,
    inputs: &TemplateInputs<'_>,
    cursor: Option<&str>,
    instance: Option<&Http<'_>>,
) -> Result<RuntimeHttpRequest, String> {
    let mut url = Url::parse(
        &request
            .url
            .render_string(inputs)
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("request {} has an invalid URL: {error}", request.id))?;
    // A credential that is the endpoint itself replaces the declared URL,
    // which it must extend; the checks below hold for it as for any URL.
    if let Some(instance) = instance
        && let Some(credential_url) = instance.credential_url(&url)?
    {
        url = credential_url;
    }
    // A fragment never reaches the server: a URL carrying one names a
    // request other than the one sent.
    if url.fragment().is_some() {
        return Err(format!("request {} has a URL fragment", request.id));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if url.scheme() != "https"
        || !spec
            .allowed_hosts
            .iter()
            .any(|allowed| runx_contracts::connector::host_allowed(allowed.as_str(), &host))
    {
        return Err(format!(
            "request {} targets a host the implementation does not allow",
            request.id
        ));
    }
    // A page after the first carries the cursor in place of whatever the
    // call started from.
    let paged = cursor.and_then(|cursor| {
        request
            .pages
            .as_ref()
            .map(|pages| (pages.param.as_str(), cursor))
    });
    // A query parameter is optional: one whose argument the call does not
    // carry is not sent, and a request that sends none has no query.
    let mut pairs = Vec::with_capacity(request.query.len() + 1);
    let mut cursor_sent = false;
    for (name, template) in &request.query {
        if let Some((param, cursor)) = paged
            && param == name
        {
            pairs.push((param.to_owned(), cursor.to_owned()));
            cursor_sent = true;
            continue;
        }
        let value = match template.render(inputs) {
            Ok(value) => value,
            Err(TemplateError::Unavailable {
                binding: "$argument",
                ..
            }) => continue,
            Err(error) => return Err(error.to_string()),
        };
        if let Some(value) = scalar_string(&value) {
            pairs.push((name.clone(), value));
        }
    }
    if let Some((param, cursor)) = paged
        && !cursor_sent
    {
        pairs.push((param.to_owned(), cursor.to_owned()));
    }
    if !pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(pairs);
    }
    let method = match request.method {
        ManifestMethod::Get => HttpMethod::Get,
        ManifestMethod::Post => HttpMethod::Post,
        ManifestMethod::Put => HttpMethod::Put,
        ManifestMethod::Patch => HttpMethod::Patch,
        ManifestMethod::Delete => HttpMethod::Delete,
    };
    let mut headers = Vec::with_capacity(request.headers.len() + 2);
    for (name, template) in &request.headers {
        let value = template
            .render_string(inputs)
            .map_err(|error| error.to_string())?;
        headers.push(RuntimeHttpHeader::new(name.clone(), value));
    }
    let mut body = request
        .body
        .as_ref()
        .map(|template| template.render(inputs))
        .transpose()
        .map_err(|error| error.to_string())?;
    if let Some(instance) = instance {
        instance.credential_body(body.as_mut())?;
    }
    let body = match body {
        Some(value) => {
            headers.push(RuntimeHttpHeader::new("content-type", "application/json"));
            Some(serde_json::to_string(&value).map_err(|error| error.to_string())?)
        }
        None => None,
    };
    if let Some(instance) = instance
        && let Some(header) = instance.authorization(method, &url)?
    {
        headers.push(header);
    }
    Ok(RuntimeHttpRequest {
        method,
        url: url.to_string(),
        headers,
        body,
    })
}

/// The answer of a request whose status refuses the caller rather than the
/// request: a rate limit, a credential the provider does not accept, or an
/// account on a billing hold.
fn halted(response: &RuntimeHttpResponse, rate_limit: Option<&RateLimit>) -> Option<Unperformed> {
    match response.status {
        429 => {
            let retry_at = rate_limit
                .and_then(|rate_limit| header(response, rate_limit.reset_header.as_str()))
                .and_then(|value| value.trim().parse::<i64>().ok())
                .filter(|seconds| *seconds > 0)
                .map(crate::time::iso8601_from_unix_seconds);
            Some(Unperformed::Halted {
                code: "rate_limited",
                reason: match &retry_at {
                    Some(at) => format!("the provider rate limited the request until {at}"),
                    None => "the provider rate limited the request".to_owned(),
                },
                retry_at,
            })
        }
        401 => Some(Unperformed::Halted {
            code: "unauthorized",
            reason: "the provider did not accept the credential (HTTP 401)".to_owned(),
            retry_at: None,
        }),
        402 => Some(Unperformed::Halted {
            code: "billing_required",
            reason: "the provider holds the account until its billing is settled (HTTP 402)"
                .to_owned(),
            retry_at: None,
        }),
        _ => None,
    }
}

fn header<'a>(response: &'a RuntimeHttpResponse, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn decode(response: &RuntimeHttpResponse, steps: &[DecodeStep]) -> Result<JsonValue, String> {
    if !(200..300).contains(&response.status) {
        return Err(format!("the provider returned HTTP {}", response.status));
    }
    let mut value = match steps.first() {
        Some(DecodeStep::Status) => JsonValue::Object(JsonObject::from([(
            "status".to_owned(),
            JsonValue::Number(JsonNumber::from(u64::from(response.status))),
        )])),
        Some(DecodeStep::Text) => JsonValue::String(response.body.clone()),
        Some(DecodeStep::AutoJson) => serde_json::from_str::<JsonValue>(&response.body)
            .map_err(|_| "the provider response is not valid JSON".to_owned())?,
        _ => return Err("the decoder has no source".to_owned()),
    };
    for step in steps.iter().skip(1) {
        value = match step {
            DecodeStep::AutoJson | DecodeStep::Text | DecodeStep::Status => {
                return Err("a decoder source may only begin a decode chain".to_owned());
            }
            DecodeStep::Field { path } => at_path(&value, path)
                .cloned()
                .ok_or_else(|| format!("decoded field {path} is missing"))?,
            DecodeStep::ParseJson => {
                serde_json::from_str::<JsonValue>(value.as_str().ok_or("parse_json needs text")?)
                    .map_err(|_| "the provider text is not valid JSON".to_owned())?
            }
            DecodeStep::RejectTrue { path } => {
                if at_path(&value, path).and_then(JsonValue::as_bool) == Some(true) {
                    return Err("the provider reported an operation error".to_owned());
                }
                value
            }
            DecodeStep::RejectPresent { path } => {
                if at_path(&value, path).is_some_and(|value| !matches!(value, JsonValue::Null)) {
                    return Err("the provider reported an operation error".to_owned());
                }
                value
            }
        };
    }
    Ok(value)
}

fn scalar_string(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(value) if !value.is_empty() => Some(value.clone()),
        JsonValue::Number(value) => Some(value.to_string()),
        JsonValue::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn not_sent(reason: String) -> Unperformed {
    Unperformed::NotSent {
        code: "invalid_request",
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &str) -> RuntimeHttpResponse {
        RuntimeHttpResponse::new(status, body)
    }

    #[test]
    fn an_empty_write_and_a_json_readback_decode_independently() -> Result<(), String> {
        let write = decode(&response(204, ""), &[DecodeStep::Status])?;
        let observed = decode(
            &response(200, r#"{"id":"resource-1"}"#),
            &[DecodeStep::AutoJson],
        )?;
        assert_eq!(
            at_path(&write, "status"),
            Some(&JsonValue::Number(JsonNumber::from(204_u64)))
        );
        assert_eq!(
            at_path(&observed, "id"),
            Some(&JsonValue::String("resource-1".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn failures_and_errors_inside_a_success_status_refuse() {
        assert!(decode(&response(500, "{}"), &[DecodeStep::AutoJson]).is_err());
        assert!(
            decode(
                &response(200, r#"{"error":true}"#),
                &[
                    DecodeStep::AutoJson,
                    DecodeStep::RejectTrue {
                        path: "error".to_owned().into()
                    }
                ]
            )
            .is_err()
        );
    }

    const CONNECT_BASE_URL: &str = "https://connect.example.test";

    /// runx Connect, as far as opening a session asks of it.
    struct ConnectSession;

    impl RuntimeHttpTransport for ConnectSession {
        fn send(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, crate::http::RuntimeHttpError> {
            Ok(if request.url == format!("{CONNECT_BASE_URL}/v1/me") {
                response(
                    200,
                    r#"{"status":"success","principal":{"principal_id":"operator-1"}}"#,
                )
            } else {
                response(404, r#"{"message":"Not Found"}"#)
            })
        }
    }

    struct Items;

    impl RuntimeHttpTransport for Items {
        fn send(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, crate::http::RuntimeHttpError> {
            Ok(match (request.method, request.url.as_str()) {
                (HttpMethod::Patch, "https://api.example.test/items/1")
                | (HttpMethod::Get, "https://api.example.test/items/1") => {
                    response(200, r#"{"id":"1","title":"first"}"#)
                }
                _ => response(404, r#"{"message":"Not Found"}"#),
            })
        }
    }

    #[test]
    fn a_repeated_write_sends_each_item_and_reads_each_back() -> Result<(), String> {
        let spec: HttpImplementation = serde_json::from_value(serde_json::json!({
            "allowed_hosts": ["api.example.test"],
            "requests": [{
                "id": "update",
                "each": {"$argument": "/updates"},
                "method": "PATCH",
                "url": {"$concat": ["https://api.example.test/items/", {"$item": "/id"}]},
                "body": {"$item": "/patch"}
            }],
            "readbacks": [{
                "request": {
                    "id": "read",
                    "method": "GET",
                    "url": {"$concat": ["https://api.example.test/items/", {"$item": "/id"}]}
                },
                "matches": [{"expected": {"source": "item", "path": "patch.title"}, "observed": "title"}]
            }],
            "decode": [{"op": "auto_json"}],
            "result_paths": {"items": "acknowledgement"}
        }))
        .map_err(|error| error.to_string())?;
        let arguments = serde_json::from_value::<JsonValue>(serde_json::json!({
            "updates": [
                {"id": "1", "patch": {"title": "first"}},
                {"id": "2", "patch": {"title": "second"}}
            ]
        }))
        .map_err(|error| error.to_string())?;
        let (parameters, env) = (JsonObject::new(), BTreeMap::new());
        let call = AdmittedCall::for_tests(
            &arguments,
            &parameters,
            &env,
            runx_contracts::connector::OperationAccess::Mutate,
        );
        let driver = Http::new(Egress::Direct(&Items), &call, &spec);

        assert_eq!(driver.items()?.map(|items| items.len()), Some(2));
        let applied = driver.perform(Some(0)).map_err(Unperformed::reason)?;
        assert!(matches!(
            driver.perform(Some(1)),
            Err(Unperformed::Refused {
                code: "provider_rejected",
                ..
            })
        ));
        let read = driver.check(
            Check::Readback {
                readback: 0,
                item: Some(0),
            },
            &applied.value,
        )?;
        assert_eq!(
            at_path(&read.value, "title"),
            Some(&JsonValue::String("first".to_owned()))
        );
        Ok(())
    }

    /// An account at `/me` whose posts come two to a page, the third page
    /// rate limited until 1800000000.
    struct Pages;

    impl RuntimeHttpTransport for Pages {
        fn send(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, crate::http::RuntimeHttpError> {
            let mut answer = match request.url.as_str() {
                "https://api.example.test/me" => response(200, r#"{"data":{"id":"42"}}"#),
                "https://api.example.test/users/42/posts?size=2" => {
                    response(200, r#"{"data":[1,2],"meta":{"next":"c2"}}"#)
                }
                "https://api.example.test/users/42/posts?cursor=c2&size=2" => {
                    response(200, r#"{"data":[3,4],"meta":{"next":"c3"}}"#)
                }
                "https://api.example.test/users/42/posts?cursor=c3&size=2" => {
                    response(429, r#"{"title":"Too Many Requests"}"#)
                }
                _ => response(404, "{}"),
            };
            answer.headers.push(RuntimeHttpHeader::new(
                "X-Rate-Reset",
                "1800000000".to_owned(),
            ));
            Ok(answer)
        }
    }

    fn paged_spec() -> Result<HttpImplementation, String> {
        serde_json::from_value(serde_json::json!({
            "allowed_hosts": ["api.example.test"],
            "requests": [
                {"id": "me", "method": "GET", "url": "https://api.example.test/me"},
                {
                    "id": "posts",
                    "method": "GET",
                    "url": {"$concat": ["https://api.example.test/users/", {"$response": "me.json.data.id"}, "/posts"]},
                    "query": {"size": 2, "cursor": {"$argument": "/cursor"}},
                    "pages": {"param": "cursor", "cursor": "meta.next", "items": "data", "limit": {"$argument": "/limit"}}
                }
            ],
            "decode": [{"op": "auto_json"}],
            "result_paths": {"posts": "acknowledgement.1.items"},
            "rate_limit": {"reset_header": "x-rate-reset"}
        }))
        .map_err(|error| error.to_string())
    }

    #[test]
    fn a_paged_read_renders_from_earlier_answers_and_stops_at_its_limit() -> Result<(), String> {
        let spec = paged_spec()?;
        let arguments = serde_json::from_value::<JsonValue>(serde_json::json!({"limit": 3}))
            .map_err(|error| error.to_string())?;
        let (parameters, env) = (JsonObject::new(), BTreeMap::new());
        let call = AdmittedCall::for_tests(
            &arguments,
            &parameters,
            &env,
            runx_contracts::connector::OperationAccess::Read,
        );
        let read = Http::new(Egress::Direct(&Pages), &call, &spec)
            .perform(None)
            .map_err(Unperformed::reason)?;
        let expected = serde_json::from_value::<JsonValue>(serde_json::json!({
            "items": [1, 2, 3, 4],
            "next": "c3"
        }))
        .map_err(|error| error.to_string())?;
        assert_eq!(at_path(&read.value, "1"), Some(&expected));
        assert_eq!(read.evidence.len(), 3, "the account and two pages");
        Ok(())
    }

    /// A provider whose every page names a new cursor.
    struct EndlessFeed;

    impl RuntimeHttpTransport for EndlessFeed {
        fn send(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, crate::http::RuntimeHttpError> {
            if request.url == "https://api.example.test/me" {
                return Ok(response(200, r#"{"data":{"id":"42"}}"#));
            }
            let page = request
                .url
                .split_once("cursor=c")
                .and_then(|(_, rest)| rest.split('&').next())
                .and_then(|page| page.parse::<u64>().ok())
                .unwrap_or(0);
            Ok(response(
                200,
                &format!(r#"{{"data":[{page}],"meta":{{"next":"c{}"}}}}"#, page + 1),
            ))
        }
    }

    #[test]
    fn a_paged_read_without_a_limit_stops_at_its_ceiling() -> Result<(), String> {
        let spec = paged_spec()?;
        let arguments = JsonValue::Object(JsonObject::new());
        let (parameters, env) = (JsonObject::new(), BTreeMap::new());
        let call = AdmittedCall::for_tests(
            &arguments,
            &parameters,
            &env,
            runx_contracts::connector::OperationAccess::Read,
        );
        let read = Http::new(Egress::Direct(&EndlessFeed), &call, &spec)
            .perform(None)
            .map_err(Unperformed::reason)?;
        assert_eq!(read.evidence.len(), 1 + super::MAX_PAGES);
        let page = at_path(&read.value, "1").ok_or("the paged read returned nothing")?;
        assert_eq!(
            at_path(page, "next"),
            Some(&JsonValue::String(format!("c{}", super::MAX_PAGES))),
            "the read hands back where it stopped"
        );
        Ok(())
    }

    #[test]
    fn a_request_that_sends_no_query_parameter_has_no_query() -> Result<(), String> {
        // Its only parameter is an argument the call does not carry, so the
        // request goes to the bare URL, not one ending in `?`.
        let spec: HttpImplementation = serde_json::from_value(serde_json::json!({
            "allowed_hosts": ["api.example.test"],
            "requests": [{
                "id": "me",
                "method": "GET",
                "url": "https://api.example.test/me",
                "query": {"state": {"$argument": "/state"}}
            }],
            "decode": [{"op": "auto_json"}],
            "result_paths": {"me": "acknowledgement"}
        }))
        .map_err(|error| error.to_string())?;
        let arguments = JsonValue::Object(JsonObject::new());
        let (parameters, env) = (JsonObject::new(), BTreeMap::new());
        let call = AdmittedCall::for_tests(
            &arguments,
            &parameters,
            &env,
            runx_contracts::connector::OperationAccess::Read,
        );
        let read = Http::new(Egress::Direct(&Pages), &call, &spec)
            .perform(None)
            .map_err(Unperformed::reason)?;
        assert_eq!(read.evidence.len(), 1);
        Ok(())
    }

    #[test]
    fn a_rate_limit_halts_the_caller_until_the_time_the_provider_names() -> Result<(), String> {
        let spec = paged_spec()?;
        let arguments = JsonValue::Object(JsonObject::new());
        let (parameters, env) = (JsonObject::new(), BTreeMap::new());
        let call = AdmittedCall::for_tests(
            &arguments,
            &parameters,
            &env,
            runx_contracts::connector::OperationAccess::Read,
        );
        match Http::new(Egress::Direct(&Pages), &call, &spec).perform(None) {
            Err(Unperformed::Halted {
                code: "rate_limited",
                retry_at: Some(retry_at),
                ..
            }) => assert_eq!(retry_at, "2027-01-15T08:00:00Z"),
            Err(other) => return Err(format!("expected a rate limit, got {other:?}")),
            Ok(_) => return Err("expected a rate limit, got an answer".to_owned()),
        }
        Ok(())
    }

    #[test]
    fn the_instances_credential_picks_how_a_request_authenticates() -> Result<(), String> {
        let spec: HttpImplementation = serde_json::from_value(serde_json::json!({
            "allowed_hosts": ["api.example.test"],
            "auth": {"app": {"type": "bearer"}, "key": {"type": "header", "name": "authorization", "field": "api_key"}, "user": {"type": "oauth1"}},
            "requests": [{"id": "me", "method": "GET", "url": "https://api.example.test/me"}],
            "decode": [{"op": "auto_json"}],
            "result_paths": {"me": "acknowledgement"}
        }))
        .map_err(|error| error.to_string())?;
        let arguments = JsonValue::Object(JsonObject::new());
        let (parameters, env) = (JsonObject::new(), BTreeMap::new());
        let authorization = |name: &str, fields: &[(&str, &str)]| -> Result<String, String> {
            let secret = crate::kernel::operation::credentials::CredentialSecret::new(
                runx_contracts::connector::CredentialName::new(name).ok_or("a credential name")?,
                fields
                    .iter()
                    .map(|(field, value)| ((*field).to_owned(), (*value).to_owned()))
                    .collect(),
            );
            let mut call = AdmittedCall::for_tests(
                &arguments,
                &parameters,
                &env,
                runx_contracts::connector::OperationAccess::Read,
            );
            call.credential = Some(&secret);
            let request = Http::new(Egress::Direct(&Pages), &call, &spec).render(
                &spec.requests.as_slice()[0],
                None,
                None,
                None,
            )?;
            request
                .headers
                .into_iter()
                .find(|header| header.name == "authorization")
                .map(|header| header.value)
                .ok_or_else(|| "no authorization".to_owned())
        };
        assert_eq!(authorization("app", &[("token", "t-1")])?, "Bearer t-1");
        assert_eq!(authorization("key", &[("api_key", "k-1")])?, "k-1");
        assert!(authorization("key", &[("wrong_field", "k-1")]).is_err());
        assert!(
            authorization(
                "user",
                &[
                    ("consumer_key", "ck"),
                    ("consumer_secret", "cs"),
                    ("access_token", "at"),
                    ("access_secret", "as"),
                ]
            )?
            .starts_with("OAuth ")
        );
        assert!(authorization("other", &[("token", "t-1")]).is_err());
        Ok(())
    }

    #[test]
    fn a_body_credential_is_placed_at_its_pointer_and_nowhere_else() -> Result<(), String> {
        let spec: HttpImplementation = serde_json::from_value(serde_json::json!({
            "allowed_hosts": ["api.indexnow.test"],
            "auth": {"site": {"type": "body", "field": "key", "pointer": "/key"}},
            "requests": [{"id": "submit", "method": "POST", "url": "https://api.indexnow.test/indexnow",
                "body": {"host": "example.test", "urlList": {"$argument": "/urls"}}}],
            "decode": [{"op": "auto_json"}],
            "result_paths": {"submit": "acknowledgement"}
        }))
        .map_err(|error| error.to_string())?;
        let arguments: JsonValue =
            serde_json::from_value(serde_json::json!({"urls": ["https://example.test/a"]}))
                .map_err(|error| error.to_string())?;
        let (parameters, env) = (JsonObject::new(), BTreeMap::new());
        let secret = crate::kernel::operation::credentials::CredentialSecret::new(
            runx_contracts::connector::CredentialName::new("site").ok_or("a credential name")?,
            [("key".to_owned(), "site-key-1".to_owned())]
                .into_iter()
                .collect(),
        );
        let mut call = AdmittedCall::for_tests(
            &arguments,
            &parameters,
            &env,
            runx_contracts::connector::OperationAccess::Mutate,
        );
        call.credential = Some(&secret);
        let request = Http::new(Egress::Direct(&Pages), &call, &spec).render(
            &spec.requests.as_slice()[0],
            None,
            None,
            None,
        )?;
        let body: serde_json::Value =
            serde_json::from_str(request.body.as_deref().ok_or("a body")?)
                .map_err(|error| error.to_string())?;
        assert_eq!(
            body,
            serde_json::json!({"host": "example.test", "key": "site-key-1", "urlList": ["https://example.test/a"]})
        );
        assert!(
            request
                .headers
                .iter()
                .all(|header| !header.value.contains("site-key-1"))
        );
        Ok(())
    }

    /// The declared-request cases runx Connect's renderer answers too
    /// (`packages/contracts/src/connector.test.ts`), as the kernel renders
    /// each request and as Connect does.
    #[test]
    fn declared_requests_render_as_the_shared_cases_expect() -> Result<(), String> {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/contracts/connector-request/cases.json"
        ))
        .map_err(|error| error.to_string())?;
        let spec: HttpImplementation =
            serde_json::from_value(corpus["http"].clone()).map_err(|error| error.to_string())?;
        let env = BTreeMap::new();
        let connect_env = BTreeMap::from([
            (
                crate::HOSTED_API_BASE_URL_ENV.to_owned(),
                CONNECT_BASE_URL.to_owned(),
            ),
            (
                crate::HOSTED_API_TOKEN_ENV.to_owned(),
                "rxk_test".to_owned(),
            ),
        ]);
        let session = crate::kernel::operation::hosted::HostedClient::with_transport(
            std::sync::Arc::new(ConnectSession),
        )
        .session(&connect_env, std::path::Path::new("/"))?;
        let binding = crate::kernel::connectors::source_scope()
            .connector("slack")
            .map_err(|error| error.to_string())?
            .binding();
        for case in corpus["cases"].as_array().ok_or("cases")? {
            let name = case["name"].as_str().ok_or("name")?;
            let decode = |field: &str, default: serde_json::Value| {
                serde_json::from_value::<JsonValue>(case.get(field).cloned().unwrap_or(default))
                    .map_err(|error| error.to_string())
            };
            let arguments = decode("arguments", serde_json::json!({}))?;
            let JsonValue::Object(parameters) = decode("parameters", serde_json::json!({}))? else {
                return Err(format!("{name}: parameters must be an object"));
            };
            let responses = case
                .get("responses")
                .map(|responses| {
                    serde_json::from_value::<BTreeMap<String, JsonObject>>(responses.clone())
                })
                .transpose()
                .map_err(|error| error.to_string())?;
            let call = AdmittedCall::for_tests(
                &arguments,
                &parameters,
                &env,
                runx_contracts::connector::OperationAccess::Mutate,
            );
            let driver = Http::new(Egress::Direct(&Items), &call, &spec);
            let request = spec
                .requests
                .iter()
                .chain(spec.readbacks.iter().map(|readback| &readback.request))
                .find(|request| Some(request.id.as_str()) == case["request"].as_str())
                .ok_or_else(|| format!("{name}: no such request"))?;
            let rendered = (|| {
                // A readback of a repeated write binds the write's item.
                let each = request
                    .each
                    .as_ref()
                    .or_else(|| spec.requests.iter().find_map(|write| write.each.as_ref()));
                let item = match (case["item"].as_u64(), each) {
                    (Some(index), Some(each)) => match each.render(&driver.inputs(None, None)) {
                        Ok(JsonValue::Array(items)) => Some(
                            usize::try_from(index)
                                .ok()
                                .and_then(|index| items.get(index).cloned())
                                .ok_or("no such item")?,
                        ),
                        _ => return Err("each must render to an array".to_owned()),
                    },
                    _ => None,
                };
                driver.render(request, item.as_ref(), responses.as_ref(), None)
            })();
            // As runx Connect renders it, from the same call its renderer's
            // shared-case test makes.
            let declared = render_declared(
                &spec,
                &DeclaredCall {
                    request: case["request"].as_str().ok_or("request")?,
                    arguments: &arguments,
                    parameters: Some(&parameters),
                    idempotency_key: corpus["idempotency_key"].as_str(),
                    invocation_id: case["invocation_id"].as_str(),
                    item: case["item"]
                        .as_u64()
                        .map(|item| usize::try_from(item).map_err(|error| error.to_string()))
                        .transpose()?,
                    responses: responses.as_ref(),
                },
            );
            if case["error"].as_bool() == Some(true) {
                assert!(rendered.is_err(), "{name}: expected a refusal");
                assert!(declared.is_err(), "{name}: expected Connect to refuse");
                continue;
            }
            let expected = &case["expected"];
            let assert_expected = |rendered: &RuntimeHttpRequest, by: &str| {
                let headers = rendered
                    .headers
                    .iter()
                    .map(|header| {
                        (
                            header.name.clone(),
                            serde_json::Value::from(header.value.clone()),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>();
                let body = rendered
                    .body
                    .as_deref()
                    .map(serde_json::from_str::<serde_json::Value>)
                    .transpose()
                    .map_err(|error| error.to_string())?;
                assert_eq!(rendered.method.as_str(), expected["method"], "{name} {by}");
                assert_eq!(rendered.url, expected["url"], "{name} {by}");
                assert_eq!(
                    serde_json::Value::Object(headers),
                    expected["headers"],
                    "{name} {by}"
                );
                assert_eq!(body.as_ref(), expected.get("body"), "{name} {by}");
                Ok::<_, String>(())
            };
            assert_expected(
                &declared.map_err(|error| format!("{name}: as Connect renders it: {error}"))?,
                "as Connect renders it",
            )?;
            // Only runx Connect supplies the paid invocation; the kernel never
            // holds it, so a request that sends it never renders to send
            // directly. Through Connect the kernel's rendering checks it, with
            // a stand-in Connect replaces.
            if case.get("invocation_id").is_some() {
                assert!(
                    rendered.is_err(),
                    "{name}: the kernel holds no paid invocation"
                );
                let gateway = Gateway {
                    connector_binding: &binding,
                    session: &session,
                    grant_id: "grant_sales",
                    operation: "sale.create",
                    operation_digest: "sha256:operation",
                    implementation_digest: "sha256:implementation",
                    input: &JsonObject::new(),
                    idempotency_key: None,
                };
                let checked = Http::new(Egress::Connect(&gateway), &call, &spec)
                    .render(request, None, responses.as_ref(), None)
                    .map_err(|error| format!("{name}: through Connect: {error}"))?;
                let body = checked
                    .body
                    .as_deref()
                    .map(serde_json::from_str::<serde_json::Value>)
                    .transpose()
                    .map_err(|error| error.to_string())?;
                let expected = serde_json::to_string(&case["expected"]["body"])
                    .map_err(|error| error.to_string())?
                    .replace(
                        &case["invocation_id"].to_string(),
                        &serde_json::Value::from(CONNECT_INVOCATION).to_string(),
                    );
                assert_eq!(
                    body,
                    Some(serde_json::from_str(&expected).map_err(|error| error.to_string())?),
                    "{name}"
                );
                continue;
            }
            assert_expected(
                &rendered.map_err(|error| format!("{name}: {error}"))?,
                "as the kernel renders it",
            )?;
        }
        Ok(())
    }

    /// The decode chains, case by case.
    #[test]
    fn decode_chains_answer_as_their_cases_expect() -> Result<(), String> {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/contracts/connector-request/decode.json"
        ))
        .map_err(|error| error.to_string())?;
        for case in corpus["cases"].as_array().ok_or("cases")? {
            let name = case["name"].as_str().ok_or("name")?;
            let steps: Vec<DecodeStep> =
                serde_json::from_value(case["steps"].clone()).map_err(|error| error.to_string())?;
            let status = u16::try_from(case["status"].as_u64().ok_or("status")?)
                .map_err(|error| error.to_string())?;
            let decoded = decode(
                &response(status, case["body"].as_str().ok_or("body")?),
                &steps,
            );
            if case["error"].as_bool() == Some(true) {
                assert!(decoded.is_err(), "{name}: expected a refusal");
                continue;
            }
            let decoded = decoded.map_err(|error| format!("{name}: {error}"))?;
            let expected: JsonValue = serde_json::from_value(case["expected"].clone())
                .map_err(|error| error.to_string())?;
            assert_eq!(decoded, expected, "{name}");
        }
        Ok(())
    }
}
