//! What a store from before streams had owners becomes (§6.9, `store.md`).
//!
//! Before 0.12 a stream was addressed by its caller's own `data_source_uri`,
//! `resource` and `aggregate_id`, and no event recorded its writer. A
//! resource maps to an owner only if a first-party manifest named it, as the
//! stream that manifest now names; anything else moves to
//! `unowned/<resource>`, which no package can write and a read reaches only
//! by naming it. An aggregate becomes its stream's key. Every event keeps its
//! version, data, digest, idempotency key and commit time, takes the type it
//! was stored with and the id its new place gives it, and records no writer,
//! so it verifies as it was written. Each key's chain is the one it had.

use std::collections::BTreeMap;

use runx_contracts::JsonObject;

use super::StoreError;
use super::model::{self, EventRecord, KeyState};

/// The resources first-party manifests named, and the stream each names
/// now.
const OWNED: &[(&str, &str)] = &[
    (
        "contact_consent_events",
        "runx/contact-consent-review/consent",
    ),
    ("github_sync_events", "runx/github/sync_plans"),
    ("reddit_accounts", "runx/reddit/accounts"),
    ("reddit_contributions", "runx/reddit/contributions"),
    ("subject_requests", "runx/privacy-request-review/requests"),
    ("twitter_execution", "runx/twitter/execution"),
    ("twitter_plans", "runx/twitter/plans"),
];

/// The owner of a stream no first-party manifest named. No package is
/// named this, so none can write it.
pub(super) const UNOWNED: &str = "unowned";

/// The stream `resource` becomes.
pub(super) fn stream_of(resource: &str) -> String {
    OWNED
        .iter()
        .find(|(owned, _)| *owned == resource)
        .map_or_else(
            || format!("{UNOWNED}/{resource}"),
            |(_, stream)| (*stream).to_owned(),
        )
}

/// One event as a store before 0.12 kept it.
pub(super) struct OldEvent {
    pub data_source_uri: String,
    pub resource: String,
    pub aggregate_id: String,
    pub version: u64,
    pub event_type: String,
    pub data: JsonObject,
    pub digest: String,
    pub idempotency_key: String,
    pub committed_at: String,
}

/// One key as it migrates: its events in version order and its state.
pub(super) struct Migrated {
    pub stream: String,
    pub key: String,
    pub events: Vec<EventRecord>,
    pub state: KeyState,
}

/// An old stream's address: its data source, resource and aggregate.
pub(super) type OldAddress = (String, String, String);

/// Every key the old `events` become, each with the chain it had, which must
/// be the chain the old store holds for it in `chains`. Two histories that
/// would become one key, the same resource and aggregate under two data
/// sources, refuse the migration, so neither is lost or mixed.
///
/// # Errors
///
/// Returns why the events can't migrate: two histories for one key, a key's
/// versions not running from 1, or a chain that isn't the one held.
pub(super) fn migrate(
    events: Vec<OldEvent>,
    chains: &BTreeMap<OldAddress, String>,
) -> Result<Vec<Migrated>, StoreError> {
    let mut by_source = BTreeMap::<(String, String, String), Vec<OldEvent>>::new();
    for event in events {
        by_source
            .entry((
                event.data_source_uri.clone(),
                event.resource.clone(),
                event.aggregate_id.clone(),
            ))
            .or_default()
            .push(event);
    }
    let mut migrated = BTreeMap::<(String, String), (String, Migrated)>::new();
    for ((source, resource, aggregate), mut events) in by_source {
        events.sort_by_key(|event| event.version);
        let stream = stream_of(&resource);
        let mut state = model::empty_state()?;
        let mut records = Vec::with_capacity(events.len());
        for (index, event) in events.into_iter().enumerate() {
            if event.version != index as u64 + 1 {
                return Err(StoreError::corrupt(format!(
                    "{source} {resource} {aggregate} holds version {} where {} belongs",
                    event.version,
                    index + 1
                )));
            }
            let record = EventRecord {
                id: model::event_id(&stream, &aggregate, event.version),
                version: event.version,
                event_type: event.event_type,
                data: event.data,
                digest: event.digest,
                idempotency_key: event.idempotency_key,
                committed_at: event.committed_at,
                writer: None,
            };
            state = model::advance(&state, &record)?;
            records.push(record);
        }
        let address = (source.clone(), resource.clone(), aggregate.clone());
        if chains.get(&address) != Some(&state.chain_digest) {
            return Err(StoreError::corrupt(format!(
                "{source} {resource} {aggregate}'s events don't chain to the digest the store holds for it"
            )));
        }
        let entry = (stream.clone(), aggregate.clone());
        if let Some((other, _)) = migrated.get(&entry) {
            return Err(StoreError::corrupt(format!(
                "{other} and {source} both hold {resource} {aggregate}, which both become {stream} {aggregate}; move one aside before this runx opens the store"
            )));
        }
        migrated.insert(
            entry,
            (
                source,
                Migrated {
                    stream,
                    key: aggregate,
                    events: records,
                    state,
                },
            ),
        );
    }
    Ok(migrated.into_values().map(|(_, key)| key).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old(source: &str, resource: &str, aggregate: &str, version: u64) -> OldEvent {
        let data = JsonObject::from([("n".to_owned(), model::number(version))]);
        OldEvent {
            data_source_uri: source.to_owned(),
            resource: resource.to_owned(),
            aggregate_id: aggregate.to_owned(),
            version,
            event_type: "data.event".to_owned(),
            digest: model::digest(&runx_contracts::JsonValue::Object(data.clone()))
                .unwrap_or_default(),
            data,
            idempotency_key: format!("key-{version}"),
            committed_at: "2026-10-01T00:00:00.000Z".to_owned(),
        }
    }

    /// A first-party resource moves to the stream its manifest names, and
    /// anything else to `unowned/`; the chain is the one 0.11 computed.
    #[test]
    fn resources_move_to_their_owners_and_keep_their_chains() -> Result<(), StoreError> {
        assert_eq!(stream_of("reddit_accounts"), "runx/reddit/accounts");
        assert_eq!(stream_of("ops_projection"), "unowned/ops_projection");
        let events = || {
            vec![
                old("local://reddit", "reddit_accounts", "alice", 2),
                old("local://reddit", "reddit_accounts", "alice", 1),
                old("tenant://ops", "ops_projection", "workspace", 1),
            ]
        };
        let chains = held(&events())?;
        let migrated = migrate(events(), &chains)?;
        assert_eq!(migrated.len(), 2);
        let alice = &migrated[0];
        assert_eq!(
            (alice.stream.as_str(), alice.key.as_str()),
            ("runx/reddit/accounts", "alice")
        );
        assert_eq!(
            alice
                .events
                .iter()
                .map(|event| event.version)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(alice.events.iter().all(|event| event.writer.is_none()));
        let mut chain = model::empty_chain()?;
        for event in &alice.events {
            chain = model::chain(&chain, event.version, &event.digest)?;
            assert_eq!(
                model::event_digest(event)?,
                event.digest,
                "verifies as written"
            );
        }
        assert_eq!(alice.state.chain_digest, chain);
        assert_eq!(
            alice.events[0].id,
            model::event_id("runx/reddit/accounts", "alice", 1)
        );
        // A chain the old store doesn't hold refuses the migration.
        let mut forged = chains.clone();
        forged.values_mut().for_each(|chain| chain.push('0'));
        assert!(migrate(events(), &forged).is_err());
        Ok(())
    }

    /// The chain the old store held for each address of `events`, as it
    /// computed it.
    fn held(events: &[OldEvent]) -> Result<BTreeMap<OldAddress, String>, StoreError> {
        let mut chains = BTreeMap::<OldAddress, (u64, String)>::new();
        let mut sorted = events.iter().collect::<Vec<_>>();
        sorted.sort_by_key(|event| event.version);
        for event in sorted {
            let address = (
                event.data_source_uri.clone(),
                event.resource.clone(),
                event.aggregate_id.clone(),
            );
            let previous = match chains.get(&address) {
                Some((_, chain)) => chain.clone(),
                None => model::empty_chain()?,
            };
            chains.insert(
                address,
                (
                    event.version,
                    model::chain(&previous, event.version, &event.digest)?,
                ),
            );
        }
        Ok(chains
            .into_iter()
            .map(|(address, (_, chain))| (address, chain))
            .collect())
    }

    /// Two data sources holding one resource's aggregate would become one key:
    /// the migration refuses rather than mix their histories.
    #[test]
    fn two_histories_for_one_key_refuse_the_migration() {
        let events = || {
            vec![
                old("local://a", "notes", "k", 1),
                old("local://b", "notes", "k", 1),
            ]
        };
        let chains = held(&events()).unwrap_or_default();
        assert!(migrate(events(), &chains).is_err());
        let gap = || vec![old("local://a", "notes", "k", 2)];
        assert!(
            migrate(gap(), &held(&gap()).unwrap_or_default()).is_err(),
            "a key's versions run from 1"
        );
    }
}
