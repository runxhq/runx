//! One bounded native page loop, with independent cursors for repeated items.

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

impl Http<'_> {
    /// Preserve whole pages and the exact next cursor. A spent shared budget
    /// leaves later items explicitly unstarted, rather than claiming no matches.
    pub(super) fn pages(
        &self,
        request: &HttpRequest,
        pages: &HttpPages,
        item: Option<(usize, &JsonValue)>,
        responses: Option<&BTreeMap<String, JsonObject>>,
        budget: &mut PageBudget,
        evidence: &mut Vec<String>,
    ) -> Result<JsonValue, Unperformed> {
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
        let mut items = Vec::new();
        let mut cursor = None::<JsonValue>;
        let mut cursors = BTreeSet::new();
        let mut started = false;
        while !budget.exhausted() {
            let response = self.send(request, item, responses, cursor.as_ref())?;
            let page = self.answered(&response)?;
            evidence.push(response.body_digest.clone());
            started = true;
            budget.pages += 1;
            budget.bytes = budget.bytes.saturating_add(response.body.len());
            let found = match at_path(&page, pages.items.as_str()) {
                None | Some(JsonValue::Null) => Vec::new(),
                Some(JsonValue::Array(found)) => found.clone(),
                Some(_) => return Err(invalid_page("page items are not an array")),
            };
            let empty = found.is_empty();
            budget.items = budget.items.saturating_add(found.len());
            items.extend(found);
            cursor = match at_path(&page, pages.cursor.as_str()) {
                None | Some(JsonValue::Null) => None,
                Some(JsonValue::String(value)) if value.is_empty() => None,
                Some(
                    value @ (JsonValue::String(_) | JsonValue::Number(_) | JsonValue::Bool(_)),
                ) => Some(value.clone()),
                Some(_) => return Err(invalid_page("page cursor is not a scalar")),
            };
            if let Some(next) = &cursor {
                let identity = match pages.location {
                    HttpPageCursorLocation::Query => super::scalar_string(next)
                        .ok_or_else(|| invalid_page("page cursor cannot be encoded"))?,
                    HttpPageCursorLocation::Body => serde_json::to_string(next)
                        .map_err(|_| invalid_page("page cursor cannot be encoded"))?,
                };
                if !cursors.insert(identity) {
                    return Err(invalid_page("the provider repeated a page cursor"));
                }
            }
            if cursor.is_none() || empty || limit.is_some_and(|limit| items.len() >= limit) {
                break;
            }
        }
        let mut output = JsonObject::from([
            ("items".to_owned(), JsonValue::Array(items)),
            ("next".to_owned(), cursor.clone().unwrap_or(JsonValue::Null)),
        ]);
        // A repeated request may exhaust the shared budget before this item.
        // Keep the established nonrepeated pager wire shape unchanged.
        if item.is_some() {
            output.extend([
                (
                    "complete".to_owned(),
                    JsonValue::Bool(started && cursor.is_none()),
                ),
                ("started".to_owned(), JsonValue::Bool(started)),
            ]);
        }
        Ok(JsonValue::Object(output))
    }
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
