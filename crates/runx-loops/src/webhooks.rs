//! A loop's webhooks (the loop contract §5.2): a request to the loop's
//! address with its token puts its payload in the loop's inbox once, before
//! runx answers. The payload is outside input, never anyone's authority. A
//! webhook's token is kept by its digest alone, and rotating it replaces the
//! one before, so only the newest address is answered.

use chrono::Utc;

use crate::file::{LoopFile, LoopName, Trigger, digest};
use crate::pass::stamp;
use crate::records::{InboxEvent, RecordEvent};
use crate::state::{LoopState, Status};
use crate::store::Committed;
use crate::{LoopError, Loops};

/// The names of the webhooks `file` takes requests on.
fn webhooks(file: &LoopFile) -> Vec<&str> {
    file.on
        .iter()
        .filter_map(|trigger| match trigger {
            Trigger::Webhook { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// The header webhook `webhook` of `file` knows each delivery by, when its
/// trigger names one.
fn delivery_header<'file>(file: &'file LoopFile, webhook: &str) -> Option<&'file str> {
    file.on.iter().find_map(|trigger| match trigger {
        Trigger::Webhook { name, delivery } if name == webhook => delivery.as_deref(),
        _ => None,
    })
}

/// The most characters a delivery's id may hold.
const DELIVERY_MOST: usize = 200;

/// Whether loop `state` takes requests on its webhook `webhook` now: it runs
/// or waits paused, and its revision names it.
fn takes(state: &LoopState, webhook: &str) -> Result<bool, LoopError> {
    if !matches!(state.status, Status::Running | Status::Paused { .. }) {
        return Ok(false);
    }
    Ok(state
        .active_file()
        .transpose()?
        .is_some_and(|file| webhooks(&file).contains(&webhook)))
}

impl Loops {
    /// Keep the token digesting to `token_digest` as the token of loop
    /// `name`'s webhook `webhook`, or of its one webhook when none is named,
    /// in place of the one before. Returns the webhook's name.
    pub fn rotate_token(
        &self,
        name: &LoopName,
        webhook: Option<&str>,
        token_digest: &str,
    ) -> Result<String, LoopError> {
        let file = self
            .file(name)?
            .ok_or_else(|| LoopError::Unknown(name.to_string()))?;
        let named = webhooks(&file);
        let webhook = match (webhook, named.as_slice()) {
            (Some(webhook), _) if named.contains(&webhook) => webhook.to_owned(),
            (None, [only]) => (*only).to_owned(),
            (Some(webhook), _) => {
                return Err(LoopError::Refused(format!(
                    "loop {name} takes no webhook {webhook}; its file names {}",
                    listed(&named)
                )));
            }
            (None, _) => {
                return Err(LoopError::Refused(format!(
                    "loop {name}'s file names {}: say which, `runx loop token {name} <webhook>`",
                    listed(&named)
                )));
            }
        };
        let token = RecordEvent::Token {
            name: webhook.clone(),
            digest: token_digest.to_owned(),
            at: stamp(Utc::now()),
        };
        let state = self.read(name)?;
        self.commit(name, &state, std::slice::from_ref(&token), None)?;
        Ok(webhook)
    }

    /// The loop and webhook whose token digests to `token_digest`, among the
    /// loops here taking requests on it.
    pub fn webhook_of(&self, token_digest: &str) -> Result<Option<(LoopName, String)>, LoopError> {
        for name in self.names()? {
            let state = self.read(&name)?;
            let found = state
                .tokens
                .iter()
                .find(|(_, held)| held.as_str() == token_digest)
                .map(|(webhook, _)| webhook.clone());
            if let Some(webhook) = found
                && takes(&state, &webhook)?
            {
                return Ok(Some((name, webhook)));
            }
        }
        Ok(None)
    }

    /// A request to loop `name`'s webhook `webhook` carrying `body`, a JSON
    /// payload, with its `headers`: put in the inbox once, by the delivery id
    /// its trigger's header gives, else by the body's digest. A trigger that
    /// names a header takes no request without it, since its sender sends it
    /// with every delivery, a retry included. Returns whether it was new.
    pub fn take_webhook(
        &self,
        name: &LoopName,
        webhook: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<bool, LoopError> {
        let state = self.read(name)?;
        if !takes(&state, webhook)? {
            return Err(LoopError::Refused(format!(
                "loop {name} takes no requests on webhook {webhook} now"
            )));
        }
        let file = state
            .active_file()
            .transpose()?
            .ok_or_else(|| LoopError::Unknown(name.to_string()))?;
        let delivery = match delivery_header(&file, webhook) {
            None => digest(body),
            Some(header) => {
                let id = headers
                    .iter()
                    .find(|(named, _)| named.eq_ignore_ascii_case(header))
                    .map(|(_, value)| value.trim())
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        LoopError::Refused(format!(
                            "webhook {webhook} knows each delivery by its {header} header, which this request doesn't carry"
                        ))
                    })?;
                if id.len() > DELIVERY_MOST || !id.bytes().all(|byte| byte.is_ascii_graphic()) {
                    return Err(LoopError::Refused(format!(
                        "webhook {webhook}'s {header} header isn't a delivery id: at most {DELIVERY_MOST} visible characters"
                    )));
                }
                id.to_owned()
            }
        };
        let payload = serde_json::from_slice(body)
            .map_err(|error| LoopError::Invalid(format!("a webhook's payload is JSON: {error}")))?;
        let event = InboxEvent::Webhook {
            identity: format!("webhook/{webhook}/{delivery}"),
            name: webhook.to_owned(),
            payload,
        };
        Ok(self.put(name, &event)? == Committed::Done)
    }
}

fn listed(webhooks: &[&str]) -> String {
    match webhooks {
        [] => "no webhook".to_owned(),
        names => format!("webhook {}", names.join(", ")),
    }
}
