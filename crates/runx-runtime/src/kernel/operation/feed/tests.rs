#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use runx_contracts::JsonValue;

use super::{FeedRead, read};
use crate::effects::RuntimeEffectRegistry;
use crate::http::{
    RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
};
use crate::kernel::operation::OperationEffect;

const MAIL: &str = r#"
schema: runx.connector.v1
name: example/mail
title: Example Mail
description: Mail with an inbox feed.
auth: {none: {}}
placements: {local: {auth: [none]}}
feeds:
  inbox:
    description: Replies, bounces and unsubscribes, oldest first.
    operation: inbox.read
    cursor: {input: after, result: cursor}
    events: {result: events, id: id}
    more: more
    wait: wait
    gap: gap
operations:
  inbox.read:
    description: Read what arrived after a cursor.
    verb: read
    scopes: [mail.read]
    target: {template: "https://mail.example.com/inbox"}
    input_schema:
      type: object
      additionalProperties: false
      properties:
        after: {type: string}
        wait: {type: integer, minimum: 0, maximum: 60}
    result_fields: [events, cursor, more, gap]
    cost: {usd_micro: 0, per: request, source: docs, source_url: "https://mail.example.com/pricing"}
    implementation:
      http:
        allowed_hosts: [mail.example.com]
        requests:
          - http_method: GET
            url: "https://mail.example.com/inbox"
            query: {after: {"$argument": "/after"}, wait: {"$argument": "/wait"}}
        result_paths: {events: acknowledgement.events, cursor: acknowledgement.cursor, more: acknowledgement.more, gap: acknowledgement.gap}
"#;

/// The provider's wire: it answers every request with `page` and keeps the
/// URL each was sent to.
#[derive(Clone)]
struct Wire {
    page: String,
    sent: Arc<Mutex<Vec<String>>>,
}

impl RuntimeHttpTransport for Wire {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        self.sent.lock().unwrap().push(request.url);
        Ok(RuntimeHttpResponse::new(200, self.page.clone()))
    }
}

struct Workspace {
    root: tempfile::TempDir,
    env: BTreeMap<String, String>,
    wire: Wire,
    effects: RuntimeEffectRegistry,
}

impl Workspace {
    /// A workspace whose own `example/mail` answers every read with `page`.
    fn answering(page: serde_json::Value) -> Self {
        let root = tempfile::tempdir().unwrap();
        let connector = root.path().join("connectors/example/mail");
        std::fs::create_dir_all(&connector).unwrap();
        std::fs::write(connector.join("connector.yaml"), MAIL).unwrap();
        let env = BTreeMap::from([
            (
                crate::paths::RUNX_HOME_ENV.to_owned(),
                root.path().join("home").to_string_lossy().into_owned(),
            ),
            (
                crate::paths::RUNX_CWD_ENV.to_owned(),
                root.path().to_string_lossy().into_owned(),
            ),
        ]);
        let wire = Wire {
            page: page.to_string(),
            sent: Arc::default(),
        };
        let effects =
            RuntimeEffectRegistry::with_effect(OperationEffect::with_http_transport(wire.clone()))
                .unwrap();
        Self {
            root,
            env,
            wire,
            effects,
        }
    }

    fn read(&self, cursor: Option<&JsonValue>) -> Result<super::FeedPage, String> {
        read(
            &self.effects,
            &FeedRead {
                connector: "example/mail",
                feed: "inbox",
                cursor,
                wait: Some(Duration::from_secs(300)),
                authority: None,
                env: &self.env,
                workspace: self.root.path(),
            },
        )
    }
}

/// Every file under `dir` and its bytes.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                files.insert(
                    path.to_string_lossy().into_owned(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

/// A feed's page is read through the connector's own driver, from the cursor
/// it was given and held open no longer than its read takes, and a poll
/// leaves nothing behind: no attempt, no use, no receipt.
#[test]
fn a_feed_s_page_is_read_after_its_cursor_and_leaves_nothing_behind() {
    let workspace = Workspace::answering(serde_json::json!({
        "events": [{"id": "e8", "type": "reply", "from": "ana@example.org"}, {"id": 9, "type": "bounce"}],
        "cursor": "9",
        "more": true,
        "gap": false,
    }));
    let cursor = JsonValue::String("7".to_owned());
    let page = workspace.read(Some(&cursor)).unwrap();
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        ["e8", "9"]
    );
    assert_eq!(
        page.events[0].data.as_object().unwrap()["from"],
        JsonValue::String("ana@example.org".to_owned())
    );
    assert_eq!(page.cursor, Some(JsonValue::String("9".to_owned())));
    assert!(page.more);
    assert!(!page.gap);
    let sent = workspace.wire.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("after=7"), "{}", sent[0]);
    assert!(
        sent[0].contains("wait=60"),
        "the wait is bounded by the read's: {}",
        sent[0]
    );

    let before = files(workspace.root.path());
    workspace.read(Some(&cursor)).unwrap();
    assert_eq!(
        files(workspace.root.path()),
        before,
        "a poll records nothing"
    );
}

/// A provider that can't replay from the cursor says so, and the page says
/// it; a feed the connector doesn't offer is refused by name.
#[test]
fn a_gap_is_the_provider_s_word_and_an_unknown_feed_is_refused() {
    let workspace = Workspace::answering(serde_json::json!({
        "events": [], "cursor": null, "more": false, "gap": true,
    }));
    let page = workspace.read(None).unwrap();
    assert!(page.gap);
    assert!(page.events.is_empty());
    assert_eq!(page.cursor, None);
    let sent = workspace.wire.sent.lock().unwrap().clone();
    assert!(
        !sent[0].contains("after="),
        "no cursor, from the feed's start: {}",
        sent[0]
    );

    let unknown = read(
        &workspace.effects,
        &FeedRead {
            connector: "example/mail",
            feed: "outbox",
            cursor: None,
            wait: None,
            authority: None,
            env: &workspace.env,
            workspace: workspace.root.path(),
        },
    )
    .unwrap_err();
    assert_eq!(unknown, "connector example/mail offers no feed outbox");
}

/// A page whose events carry no id can't be taken: nothing would dedupe it.
#[test]
fn an_event_with_no_id_is_no_page() {
    let workspace = Workspace::answering(serde_json::json!({
        "events": [{"type": "reply"}], "cursor": "1", "more": false, "gap": false,
    }));
    let refused = workspace.read(None).unwrap_err();
    assert_eq!(
        refused,
        "example/mail/inbox.read answered an event with no id"
    );
}

/// Polls aren't counted, so an authority that bounds how often the read may
/// be used can't serve the feed, and nothing is sent.
#[test]
fn an_authority_bounding_the_read_s_uses_can_t_serve_a_feed() {
    let workspace = Workspace::answering(serde_json::json!({
        "events": [], "cursor": "1", "more": false, "gap": false,
    }));
    let authority = crate::RunAuthority {
        delegations: vec![crate::Delegation {
            connector: runx_contracts::connector::ConnectorName::new("example/mail").unwrap(),
            resource_ref: None,
            principal_ref: None,
            expires_at: None,
            terms: vec![crate::DelegationTerm {
                verbs: vec![runx_contracts::AuthorityVerb::Read],
                bounds: runx_contracts::AuthorityBounds {
                    operations: vec!["example/mail/inbox.read".into()],
                    scopes: vec!["mail.read".into()],
                    targets: vec!["*".into()],
                    uses: Some(runx_contracts::AuthorityUseLimit {
                        total: Some(100),
                        per_day: None,
                    }),
                    ..runx_contracts::AuthorityBounds::default()
                },
            }]
            .into(),
        }],
        ..crate::RunAuthority::default()
    };
    let refused = read(
        &workspace.effects,
        &FeedRead {
            connector: "example/mail",
            feed: "inbox",
            cursor: None,
            wait: None,
            authority: Some(&authority),
            env: &workspace.env,
            workspace: workspace.root.path(),
        },
    )
    .unwrap_err();
    assert_eq!(
        refused,
        "the authority bounds how often example/mail/inbox.read may be used, and a feed's polls aren't counted, so it can't serve feed inbox"
    );
    assert!(workspace.wire.sent.lock().unwrap().is_empty());
}
