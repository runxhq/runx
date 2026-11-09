//! The Postgres backend. Each append runs in one transaction under an
//! advisory lock on its key, so concurrent first appends to a new key are
//! ordered too. The connection is TLS unless the URL says `sslmode=disable`.
//! It keeps `store`'s streams; a `private` instance is refused here when it's
//! configured, because this backend can't search.

use std::sync::Arc;

use postgres::{Client, Config, GenericClient, NoTls, Row, Transaction};
use runx_contracts::JsonObject;

use super::model::{self, Appended, EventRecord, KeyState, Listed, Writer};
use super::request::{Append, Key, ListKeys, ReadEvents};
use super::{Backend, StoreError, Streams, append_in};

/// 0.12 raised it from 2, so an older runx refuses the store when it opens it,
/// and 0.12 never reads a store from before it.
const SCHEMA_VERSION: i32 = 3;

const VERSION_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS runx_store_schema (
  version INTEGER NOT NULL
);
"#;

const SCHEMA: &str = r#"
CREATE TABLE store_events (
  stream TEXT NOT NULL,
  key TEXT NOT NULL,
  version BIGINT NOT NULL,
  idempotency_key TEXT NOT NULL,
  id TEXT NOT NULL,
  type TEXT NOT NULL,
  digest TEXT NOT NULL,
  data TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  writer_package TEXT NOT NULL,
  writer_digest TEXT NOT NULL,
  PRIMARY KEY (stream, key, version),
  UNIQUE (stream, key, idempotency_key)
);
CREATE TABLE store_keys (
  stream TEXT NOT NULL,
  key TEXT NOT NULL,
  version BIGINT NOT NULL,
  id TEXT NOT NULL,
  type TEXT NOT NULL,
  digest TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  data TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  writer_package TEXT NOT NULL,
  writer_digest TEXT NOT NULL,
  chain_digest TEXT NOT NULL,
  PRIMARY KEY (stream, key)
);
CREATE INDEX store_keys_recent ON store_keys (stream, committed_at DESC, key ASC);
CREATE INDEX store_keys_type_recent ON store_keys (stream, type, committed_at DESC, key ASC);
"#;

const EVENT_COLUMNS: &str =
    "id, version, type, digest, idempotency_key, committed_at, data, writer_package, writer_digest";

pub(super) struct Postgres {
    client: Client,
}

impl Postgres {
    pub(super) fn connect(url: &str, password: Option<&str>) -> Result<Self, StoreError> {
        let mut config: Config = url
            .parse()
            .map_err(|error| StoreError::invalid(format!("the postgres url: {error}")))?;
        if let Some(password) = password {
            config.password(password);
        }
        let plaintext = url
            .split_once('?')
            .is_some_and(|(_, query)| query.split('&').any(|pair| pair == "sslmode=disable"));
        let client = if plaintext {
            config.connect(NoTls)
        } else {
            config.connect(tls()?)
        }
        .map_err(|error| StoreError::unavailable(format!("connecting to postgres: {error}")))?;
        let mut store = Self { client };
        store.ensure_schema()?;
        Ok(store)
    }

    fn ensure_schema(&mut self) -> Result<(), StoreError> {
        let _open = crate::work_counts::OpenTransaction::begin();
        let mut transaction = self
            .client
            .transaction()
            .map_err(|error| database("starting the schema", &error))?;
        transaction
            .batch_execute("SELECT pg_advisory_xact_lock(hashtextextended('runx_store_schema', 0))")
            .map_err(|error| database("locking the schema", &error))?;
        transaction
            .batch_execute(VERSION_TABLE)
            .map_err(|error| database("creating the schema", &error))?;
        let versions = transaction
            .query("SELECT version FROM runx_store_schema", &[])
            .map_err(|error| database("reading the schema version", &error))?;
        let versions = versions
            .iter()
            .map(|row| row.get::<_, i32>(0))
            .collect::<Vec<_>>();
        match versions.as_slice() {
            [] => {
                transaction
                    .batch_execute(SCHEMA)
                    .map_err(|error| database("creating the schema", &error))?;
                transaction
                    .execute(
                        "INSERT INTO runx_store_schema (version) VALUES ($1)",
                        &[&SCHEMA_VERSION],
                    )
                    .map_err(|error| database("versioning the schema", &error))?;
            }
            [SCHEMA_VERSION] => {}
            [1 | 2] => {
                return Err(StoreError::invalid(
                    "the database holds a store from before runx 0.12, which 0.12 doesn't read; move it aside, or name another database, to start a new store",
                ));
            }
            _ => {
                return Err(StoreError::corrupt(format!(
                    "the store's schema is not version {SCHEMA_VERSION}"
                )));
            }
        }
        transaction
            .commit()
            .map_err(|error| database("committing the schema", &error))
    }
}

fn tls() -> Result<tokio_postgres_rustls::MakeRustlsConnect, StoreError> {
    let mut roots = rustls::RootCertStore::empty();
    for certificate in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        roots.add(certificate.clone()).map_err(|error| {
            StoreError::unavailable(format!("loading root certificates: {error}"))
        })?;
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| StoreError::unavailable(format!("configuring TLS: {error}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

impl Backend for Postgres {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn append(&mut self, request: &Append<'_>, committed_at: &str) -> Result<Appended, StoreError> {
        let _open = crate::work_counts::OpenTransaction::begin();
        let mut transaction = self
            .client
            .transaction()
            .map_err(|error| database("starting the append", &error))?;
        let lock = serde_json::to_string(&(request.at.stream, request.at.key))
            .map_err(|error| StoreError::invalid(format!("encoding the key: {error}")))?;
        transaction
            .execute(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&lock],
            )
            .map_err(|error| database("locking the key", &error))?;
        let mut streams = PostgresStreams(transaction);
        let appended = append_in(&mut streams, request, committed_at)?;
        // A commit the server does not confirm may still have applied.
        streams.0.commit().map_err(|error| {
            StoreError::uncertain(format!("postgres did not confirm the append: {error}"))
        })?;
        Ok(appended)
    }

    fn state(&mut self, at: Key<'_>) -> Result<KeyState, StoreError> {
        state(&mut self.client, at)
    }

    fn events(&mut self, request: &ReadEvents<'_>) -> Result<(Vec<EventRecord>, bool), StoreError> {
        let at = request.at;
        let after = integer(request.after_version.unwrap_or(0))?;
        let (order, limit) = match request.after_version {
            Some(_) => ("ASC", request.limit + 1),
            None => ("DESC", request.limit),
        };
        let rows = self
            .client
            .query(
                &format!(
                    "SELECT {EVENT_COLUMNS} FROM store_events WHERE stream = $1 AND key = $2 AND version > $3 ORDER BY version {order} LIMIT $4"
                ),
                &[&at.stream, &at.key, &after, &integer(limit as u64)?],
            )
            .map_err(|error| database("reading events", &error))?;
        let mut events = rows
            .iter()
            .map(|row| record_at(row, 0))
            .collect::<Result<Vec<_>, _>>()?;
        if request.after_version.is_none() {
            events.reverse();
        }
        let has_more = request.after_version.is_some() && events.len() > request.limit;
        events.truncate(request.limit);
        Ok((events, has_more))
    }

    fn keys(&mut self, request: &ListKeys<'_>) -> Result<(Vec<Listed>, bool), StoreError> {
        let cursor = model::decode_cursor(request.cursor)?;
        let types = request
            .types
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let (cursor_at, cursor_key) = cursor.map_or((None, None), |cursor| {
            (Some(cursor.committed_at), Some(cursor.key))
        });
        let rows = self
            .client
            .query(
                &format!(
                    "SELECT key, {EVENT_COLUMNS} FROM store_keys WHERE stream = $1 AND (cardinality($2::text[]) = 0 OR type = ANY($2)) AND ($3::text IS NULL OR committed_at < $3 OR (committed_at = $3 AND key > $4)) ORDER BY committed_at DESC, key ASC LIMIT $5"
                ),
                &[
                    &request.stream,
                    &types,
                    &cursor_at,
                    &cursor_key,
                    &integer(request.limit as u64 + 1)?,
                ],
            )
            .map_err(|error| database("reading keys", &error))?;
        let mut keys = rows
            .iter()
            .map(|row| {
                Ok(Listed {
                    key: row.get(0),
                    last: record_at(row, 1)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let has_more = keys.len() > request.limit;
        keys.truncate(request.limit);
        Ok((keys, has_more))
    }

    fn landed(&mut self, at: Key<'_>, idempotency_key: &str) -> Result<Option<String>, StoreError> {
        Ok(existing(&mut self.client, at, idempotency_key)?.map(|record| record.digest))
    }
}

struct PostgresStreams<'a>(Transaction<'a>);

impl Streams for PostgresStreams<'_> {
    fn state(&mut self, at: Key<'_>) -> Result<KeyState, StoreError> {
        state(&mut self.0, at)
    }

    fn existing(
        &mut self,
        at: Key<'_>,
        idempotency_key: &str,
    ) -> Result<Option<EventRecord>, StoreError> {
        existing(&mut self.0, at, idempotency_key)
    }

    fn commit_event(
        &mut self,
        at: Key<'_>,
        record: &EventRecord,
        state: &KeyState,
    ) -> Result<(), StoreError> {
        insert_event(&mut self.0, at, record)?;
        upsert_key(&mut self.0, at, record, state)
    }
}

fn data_json(record: &EventRecord) -> Result<String, StoreError> {
    serde_json::to_string(&record.data)
        .map_err(|error| StoreError::invalid(format!("encoding the event: {error}")))
}

fn insert_event(
    client: &mut impl GenericClient,
    at: Key<'_>,
    record: &EventRecord,
) -> Result<(), StoreError> {
    let (package, digest) = writer_columns(record);
    client
        .execute(
            "INSERT INTO store_events (stream, key, version, idempotency_key, id, type, digest, data, committed_at, writer_package, writer_digest) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            &[
                &at.stream,
                &at.key,
                &integer(record.version)?,
                &record.idempotency_key,
                &record.id,
                &record.event_type,
                &record.digest,
                &data_json(record)?,
                &record.committed_at,
                &package,
                &digest,
            ],
        )
        .map_err(|error| database("inserting the event", &error))?;
    Ok(())
}

fn upsert_key(
    client: &mut impl GenericClient,
    at: Key<'_>,
    record: &EventRecord,
    state: &KeyState,
) -> Result<(), StoreError> {
    let (package, digest) = writer_columns(record);
    client
        .execute(
            "INSERT INTO store_keys (stream, key, version, id, type, digest, idempotency_key, data, committed_at, writer_package, writer_digest, chain_digest) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) ON CONFLICT (stream, key) DO UPDATE SET version = excluded.version, id = excluded.id, type = excluded.type, digest = excluded.digest, idempotency_key = excluded.idempotency_key, data = excluded.data, committed_at = excluded.committed_at, writer_package = excluded.writer_package, writer_digest = excluded.writer_digest, chain_digest = excluded.chain_digest",
            &[
                &at.stream,
                &at.key,
                &integer(record.version)?,
                &record.id,
                &record.event_type,
                &record.digest,
                &record.idempotency_key,
                &data_json(record)?,
                &record.committed_at,
                &package,
                &digest,
                &state.chain_digest,
            ],
        )
        .map_err(|error| database("moving the key", &error))?;
    Ok(())
}

fn writer_columns(record: &EventRecord) -> (String, String) {
    (record.writer.package.clone(), record.writer.digest.clone())
}

fn state(client: &mut impl GenericClient, at: Key<'_>) -> Result<KeyState, StoreError> {
    let row = client
        .query_opt(
            &format!(
                "SELECT chain_digest, {EVENT_COLUMNS} FROM store_keys WHERE stream = $1 AND key = $2"
            ),
            &[&at.stream, &at.key],
        )
        .map_err(|error| database("reading the key", &error))?;
    let Some(row) = row else {
        return model::empty_state();
    };
    let last = record_at(&row, 1)?;
    Ok(KeyState {
        version: last.version,
        last: Some(last),
        chain_digest: row.get(0),
    })
}

fn existing(
    client: &mut impl GenericClient,
    at: Key<'_>,
    idempotency_key: &str,
) -> Result<Option<EventRecord>, StoreError> {
    client
        .query_opt(
            &format!(
                "SELECT {EVENT_COLUMNS} FROM store_events WHERE stream = $1 AND key = $2 AND idempotency_key = $3"
            ),
            &[&at.stream, &at.key, &idempotency_key],
        )
        .map_err(|error| database("reading the event under the key", &error))?
        .map(|row| record_at(&row, 0))
        .transpose()
}

fn record_at(row: &Row, offset: usize) -> Result<EventRecord, StoreError> {
    let data: String = row.get(offset + 6);
    let writer = Writer {
        package: row.get(offset + 7),
        digest: row.get(offset + 8),
    };
    Ok(EventRecord {
        id: row.get(offset),
        version: u64::try_from(row.get::<_, i64>(offset + 1))
            .map_err(|_| StoreError::corrupt("an event has a negative version"))?,
        event_type: row.get(offset + 2),
        digest: row.get(offset + 3),
        idempotency_key: row.get(offset + 4),
        committed_at: row.get(offset + 5),
        data: serde_json::from_str::<JsonObject>(&data)
            .map_err(|error| StoreError::corrupt(format!("a stored event is not JSON: {error}")))?,
        writer,
    })
}

fn integer(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::invalid("a number exceeds the store's range"))
}

fn database(operation: &str, error: &postgres::Error) -> StoreError {
    let detail = error
        .as_db_error()
        .map_or_else(|| error.to_string(), |db| db.message().to_owned());
    StoreError::unavailable(format!("the postgres store failed {operation}: {detail}"))
}
