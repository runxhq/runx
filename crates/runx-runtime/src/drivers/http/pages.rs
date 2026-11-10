//! One bounded native page loop, with independent cursors for repeated items,
//! each of which settles on its own.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use runx_contracts::connector::{
    HttpPageCursorLocation, HttpPages, HttpRequest, TemplateError, json_pointer_segments,
};
use runx_contracts::{JsonNumber, JsonObject, JsonValue};

use super::{Http, MAX_PAGED_BYTES, MAX_PAGED_DURATION, MAX_PAGED_ITEMS, MAX_PAGES};
use crate::kernel::operation::confirm::{Unperformed, at_path};

pub(super) struct PageBudget {
    pages: usize,
    items: usize,
    bytes: usize,
    started: Instant,
}

impl PageBudget {
    pub(super) fn new() -> Self {
        Self {
            pages: 0,
            items: 0,
            bytes: 0,
            started: Instant::now(),
        }
    }

    fn exhausted(&self) -> bool {
        self.pages >= MAX_PAGES
            || self.items >= MAX_PAGED_ITEMS
            || self.bytes >= MAX_PAGED_BYTES
            || self.started.elapsed() >= MAX_PAGED_DURATION
    }
}

/// How one repeated item of a paged read settled. A provider that refuses the
/// caller (a rate limit, a credential it doesn't accept, a billing hold)
/// halts the read: every later item stays unstarted, never "no matches".
pub(super) struct SettledItem {
    pub(super) value: JsonValue,
    pub(super) halted: bool,
}

impl Http<'_> {
    /// Preserve whole pages and the exact next cursor. A spent shared budget
    /// leaves later items explicitly unstarted, rather than claiming no matches.
    /// A read that isn't repeated fails as a whole.
    pub(super) fn pages(
        &self,
        request: &HttpRequest,
        pages: &HttpPages,
        responses: Option<&BTreeMap<String, JsonObject>>,
        budget: &mut PageBudget,
        evidence: &mut Vec<String>,
    ) -> Result<JsonValue, Unperformed> {
        let mut read = PageRead::default();
        let read_all = self.read_pages(request, pages, None, responses, budget, &mut read);
        evidence.append(&mut read.evidence);
        read_all?;
        Ok(JsonValue::Object(JsonObject::from([
            ("items".to_owned(), JsonValue::Array(read.items)),
            ("next".to_owned(), read.cursor.unwrap_or(JsonValue::Null)),
        ])))
    }

    /// One item of a repeated paged read, settled: its pages, whether it
    /// started and read to the end, and the error that stopped it, so one
    /// item's failure is that item's and the rest go on.
    pub(super) fn item_pages(
        &self,
        request: &HttpRequest,
        pages: &HttpPages,
        item: (usize, &JsonValue),
        responses: Option<&BTreeMap<String, JsonObject>>,
        budget: &mut PageBudget,
        evidence: &mut Vec<String>,
    ) -> SettledItem {
        let mut read = PageRead::default();
        let failed = self
            .read_pages(request, pages, Some(item), responses, budget, &mut read)
            .err();
        evidence.append(&mut read.evidence);
        let complete = failed.is_none() && read.started && read.cursor.is_none();
        let mut output = JsonObject::from([
            ("complete".to_owned(), JsonValue::Bool(complete)),
            ("items".to_owned(), JsonValue::Array(read.items)),
            ("next".to_owned(), read.cursor.unwrap_or(JsonValue::Null)),
            ("started".to_owned(), JsonValue::Bool(read.started)),
        ]);
        let halted = matches!(failed, Some(Unperformed::Halted { .. }));
        // Every settled item has the same fields, so a step can bind each.
        output.insert(
            "error".to_owned(),
            failed.map_or(JsonValue::Null, item_error),
        );
        SettledItem {
            value: JsonValue::Object(output),
            halted,
        }
    }

    /// Read pages into `read` until the end, the budget or a failure. What
    /// was read before a failure stays in `read`, and its cursor is the page
    /// that failed, where a further read starts.
    fn read_pages(
        &self,
        request: &HttpRequest,
        pages: &HttpPages,
        item: Option<(usize, &JsonValue)>,
        responses: Option<&BTreeMap<String, JsonObject>>,
        budget: &mut PageBudget,
        read: &mut PageRead,
    ) -> Result<(), Unperformed> {
        let limit = match pages
            .limit
            .as_ref()
            .map(|limit| limit.render(&self.inputs(item.map(|(_, item)| item), responses)))
        {
            None
            | Some(Err(TemplateError::Unavailable {
                binding: "$argument",
                ..
            })) => None,
            Some(Ok(JsonValue::Number(JsonNumber::I64(limit)))) => Some(
                usize::try_from(limit)
                    .map_err(|_| super::not_sent("a page limit is a whole number".to_owned()))?,
            ),
            Some(Ok(_)) => {
                return Err(super::not_sent("a page limit is a whole number".to_owned()));
            }
            Some(Err(error)) => return Err(super::not_sent(error.to_string())),
        };
        let mut cursors = BTreeSet::new();
        while !budget.exhausted() {
            let response = self.send(request, item, responses, read.cursor.as_ref())?;
            read.started = true;
            let page = self.answered(&response)?;
            read.evidence.push(response.body_digest.clone());
            budget.pages += 1;
            budget.bytes = budget.bytes.saturating_add(response.body.len());
            let found = match at_path(&page, pages.items.as_str()) {
                None | Some(JsonValue::Null) => Vec::new(),
                Some(JsonValue::Array(found)) => found.clone(),
                Some(_) => return Err(invalid_page("page items are not an array")),
            };
            let next = match at_path(&page, pages.next.cursor.as_str()) {
                None | Some(JsonValue::Null) => None,
                Some(JsonValue::String(value)) if value.is_empty() => None,
                Some(
                    value @ (JsonValue::String(_) | JsonValue::Number(_) | JsonValue::Bool(_)),
                ) => Some(value.clone()),
                Some(_) => return Err(invalid_page("page cursor is not a scalar")),
            };
            if let Some(next) = &next {
                let identity = match pages.next.location {
                    HttpPageCursorLocation::Query => super::scalar_string(next)
                        .ok_or_else(|| invalid_page("page cursor cannot be encoded"))?,
                    HttpPageCursorLocation::Body => serde_json::to_string(next)
                        .map_err(|_| invalid_page("page cursor cannot be encoded"))?,
                };
                if !cursors.insert(identity) {
                    return Err(invalid_page("the provider repeated a page cursor"));
                }
            }
            // The page is read: its items are kept and the cursor moves past it.
            let empty = found.is_empty();
            budget.items = budget.items.saturating_add(found.len());
            read.items.extend(found);
            read.cursor = next;
            if read.cursor.is_none()
                || empty
                || limit.is_some_and(|limit| read.items.len() >= limit)
            {
                break;
            }
        }
        Ok(())
    }
}

/// What a paged read gathered: its items in order, the cursor a further read
/// starts from, whether any request for it went out, and the digest of each
/// page it read.
#[derive(Default)]
struct PageRead {
    items: Vec<JsonValue>,
    cursor: Option<JsonValue>,
    started: bool,
    evidence: Vec<String>,
}

/// The error a settled item reports: what stopped it, and when the provider
/// said it would take requests again.
fn item_error(failure: Unperformed) -> JsonValue {
    let (code, message, retry_at) = match failure {
        Unperformed::Halted {
            code,
            reason,
            retry_at,
            ..
        } => (code, reason, retry_at),
        Unperformed::NotSent { code, reason }
        | Unperformed::Refused { code, reason, .. }
        | Unperformed::MaybeSent { code, reason } => (code, reason, None),
        Unperformed::Contradicted(contradiction) => ("contradicted", contradiction.reason(), None),
    };
    let mut error = JsonObject::from([
        ("code".to_owned(), JsonValue::String(code.to_owned())),
        ("message".to_owned(), JsonValue::String(message)),
    ]);
    if let Some(retry_at) = retry_at {
        error.insert("retry_at".to_owned(), JsonValue::String(retry_at));
    }
    JsonValue::Object(error)
}

/// An item a halted read never reached: not started, so never "no matches".
pub(super) fn unstarted() -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("complete".to_owned(), JsonValue::Bool(false)),
        ("error".to_owned(), JsonValue::Null),
        ("items".to_owned(), JsonValue::Array(Vec::new())),
        ("next".to_owned(), JsonValue::Null),
        ("started".to_owned(), JsonValue::Bool(false)),
    ]))
}

fn invalid_page(reason: &str) -> Unperformed {
    Unperformed::MaybeSent {
        code: "invalid_response",
        reason: reason.to_owned(),
    }
}

pub(super) fn place_body_cursor(
    body: Option<&mut JsonValue>,
    pointer: &str,
    cursor: &JsonValue,
) -> Result<(), String> {
    let segments =
        json_pointer_segments(pointer).ok_or("page cursor pointer names no object member")?;
    let (last, parent) = segments
        .split_last()
        .ok_or("page cursor pointer names no member")?;
    super::body_parent(body, parent)?.insert(last.clone(), cursor.clone());
    Ok(())
}
