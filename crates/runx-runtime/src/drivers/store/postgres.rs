//! The Postgres backend. Each append runs in one transaction under an
//! advisory lock on its stream, so concurrent first appends to a new stream
//! are ordered too. The connection is TLS unless the URL says
//! `sslmode=disable`.

use std::sync::Arc;

use postgres::{Client, Config, GenericClient, NoTls, Row, Transaction};
use runx_contracts::JsonObject;

use super::model::{self, Appended, EventRecord, Projection, StreamHead};
use super::request::{Append, ListHeads, ReadEvents, Source};
use super::{Backend, StoreError, Streams, append_in};

const SCHEMA_VERSION: i32 = 1;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS runx_store_schema (
  version INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS runx_store_events (
  data_source_ref TEXT NOT NULL,
  resource TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  version BIGINT NOT NULL,
  idempotency_key TEXT NOT NULL,
  event_ref TEXT NOT NULL,
  event_type TEXT NOT NULL,
  event_digest TEXT NOT NULL,
  event_json TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  PRIMARY KEY (data_source_ref, resource, aggregate_id, version),
  UNIQUE (data_source_ref, resource, aggregate_id, idempotency_key)
);
CREATE TABLE IF NOT EXISTS runx_store_heads (
  data_source_ref TEXT NOT NULL,
  resource TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  version BIGINT NOT NULL,
  event_ref TEXT NOT NULL,
  event_type TEXT NOT NULL,
  event_digest TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  event_json TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  projection_digest TEXT NOT NULL,
  PRIMARY KEY (data_source_ref, resource, aggregate_id)
);
CREATE INDEX IF NOT EXISTS runx_store_heads_recent
  ON runx_store_heads (data_source_ref, resource, committed_at DESC, aggregate_id ASC);
CREATE INDEX IF NOT EXISTS runx_store_heads_type_recent
  ON runx_store_heads (data_source_ref, resource, event_type, committed_at DESC, aggregate_id ASC);
"#;

const EVENT_COLUMNS: &str =
    "event_ref, version, event_type, event_digest, idempotency_key, committed_at, event_json";

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
        let mut transaction = self
            .client
            .transaction()
            .map_err(|error| database("starting the schema", &error))?;
        transaction
            .batch_execute("SELECT pg_advisory_xact_lock(hashtextextended('runx_store_schema', 0))")
            .map_err(|error| database("locking the schema", &error))?;
        transaction
            .batch_execute(SCHEMA)
            .map_err(|error| database("creating the schema", &error))?;
        let versions = transaction
            .query("SELECT version FROM runx_store_schema", &[])
            .map_err(|error| database("reading the schema version", &error))?;
        match versions.as_slice() {
            [] => {
                transaction
                    .execute(
                        "INSERT INTO runx_store_schema (version) VALUES ($1)",
                        &[&SCHEMA_VERSION],
                    )
                    .map_err(|error| database("versioning the schema", &error))?;
            }
            [row] if row.get::<_, i32>(0) == SCHEMA_VERSION => {}
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
        let mut transaction = self
            .client
            .transaction()
            .map_err(|error| database("starting the append", &error))?;
        let source = request.source;
        let stream =
            serde_json::to_string(&(source.data_source_ref, source.resource, source.aggregate_id))
                .map_err(|error| StoreError::invalid(format!("encoding the stream: {error}")))?;
        transaction
            .execute(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&stream],
            )
            .map_err(|error| database("locking the stream", &error))?;
        let mut streams = PostgresStreams(transaction);
        let appended = append_in(&mut streams, request, committed_at)?;
        // A commit the server does not confirm may still have applied.
        streams.0.commit().map_err(|error| {
            StoreError::uncertain(format!("postgres did not confirm the append: {error}"))
        })?;
        Ok(appended)
    }

    fn projection(&mut self, source: Source<'_>) -> Result<Projection, StoreError> {
        projection(&mut self.client, source)
    }

    fn events(&mut self, request: &ReadEvents<'_>) -> Result<(Vec<EventRecord>, bool), StoreError> {
        let source = request.source;
        let after = integer(request.after_version.unwrap_or(0))?;
        let (order, limit) = match request.after_version {
            Some(_) => ("ASC", request.limit + 1),
            None => ("DESC", request.limit),
        };
        let rows = self
            .client
            .query(
                &format!(
                    "SELECT {EVENT_COLUMNS} FROM runx_store_events WHERE data_source_ref = $1 AND resource = $2 AND aggregate_id = $3 AND version > $4 ORDER BY version {order} LIMIT $5"
                ),
                &[
                    &source.data_source_ref,
                    &source.resource,
                    &source.aggregate_id,
                    &after,
                    &integer(limit as u64)?,
                ],
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

    fn heads(&mut self, request: &ListHeads<'_>) -> Result<(Vec<StreamHead>, bool), StoreError> {
        let cursor = model::decode_cursor(request.cursor)?;
        let event_types = request
            .event_types
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let (cursor_at, cursor_id) = cursor.map_or((None, None), |cursor| {
            (Some(cursor.committed_at), Some(cursor.aggregate_id))
        });
        let rows = self
            .client
            .query(
                &format!(
                    "SELECT aggregate_id, {EVENT_COLUMNS} FROM runx_store_heads WHERE data_source_ref = $1 AND resource = $2 AND (cardinality($3::text[]) = 0 OR event_type = ANY($3)) AND ($4::text IS NULL OR committed_at < $4 OR (committed_at = $4 AND aggregate_id > $5)) ORDER BY committed_at DESC, aggregate_id ASC LIMIT $6"
                ),
                &[
                    &request.data_source_ref,
                    &request.resource,
                    &event_types,
                    &cursor_at,
                    &cursor_id,
                    &integer(request.limit as u64 + 1)?,
                ],
            )
            .map_err(|error| database("reading stream heads", &error))?;
        let mut heads = rows
            .iter()
            .map(|row| {
                Ok(StreamHead {
                    aggregate_id: row.get(0),
                    event: record_at(row, 1)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let has_more = heads.len() > request.limit;
        heads.truncate(request.limit);
        Ok((heads, has_more))
    }

    fn landed(
        &mut self,
        source: Source<'_>,
        idempotency_key: &str,
    ) -> Result<Option<String>, StoreError> {
        Ok(existing(&mut self.client, source, idempotency_key)?.map(|record| record.event_digest))
    }
}

struct PostgresStreams<'a>(Transaction<'a>);

impl Streams for PostgresStreams<'_> {
    fn projection(&mut self, source: Source<'_>) -> Result<Projection, StoreError> {
        projection(&mut self.0, source)
    }

    fn existing(
        &mut self,
        source: Source<'_>,
        idempotency_key: &str,
    ) -> Result<Option<EventRecord>, StoreError> {
        existing(&mut self.0, source, idempotency_key)
    }

    fn commit_event(
        &mut self,
        source: Source<'_>,
        record: &EventRecord,
        projection: &Projection,
    ) -> Result<(), StoreError> {
        let event_json = serde_json::to_string(&record.event)
            .map_err(|error| StoreError::invalid(format!("encoding the event: {error}")))?;
        let version = integer(record.version)?;
        self.0
            .execute(
                "INSERT INTO runx_store_events (data_source_ref, resource, aggregate_id, version, idempotency_key, event_ref, event_type, event_digest, event_json, committed_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                &[
                    &source.data_source_ref,
                    &source.resource,
                    &source.aggregate_id,
                    &version,
                    &record.idempotency_key,
                    &record.event_ref,
                    &record.event_type,
                    &record.event_digest,
                    &event_json,
                    &record.committed_at,
                ],
            )
            .map_err(|error| database("inserting the event", &error))?;
        self.0
            .execute(
                "INSERT INTO runx_store_heads (data_source_ref, resource, aggregate_id, version, event_ref, event_type, event_digest, idempotency_key, event_json, committed_at, projection_digest) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) ON CONFLICT (data_source_ref, resource, aggregate_id) DO UPDATE SET version = excluded.version, event_ref = excluded.event_ref, event_type = excluded.event_type, event_digest = excluded.event_digest, idempotency_key = excluded.idempotency_key, event_json = excluded.event_json, committed_at = excluded.committed_at, projection_digest = excluded.projection_digest",
                &[
                    &source.data_source_ref,
                    &source.resource,
                    &source.aggregate_id,
                    &version,
                    &record.event_ref,
                    &record.event_type,
                    &record.event_digest,
                    &record.idempotency_key,
                    &event_json,
                    &record.committed_at,
                    &projection.projection_digest,
                ],
            )
            .map_err(|error| database("moving the stream head", &error))?;
        Ok(())
    }
}

fn projection(
    client: &mut impl GenericClient,
    source: Source<'_>,
) -> Result<Projection, StoreError> {
    let row = client
        .query_opt(
            "SELECT version, event_ref, event_type, event_digest, projection_digest FROM runx_store_heads WHERE data_source_ref = $1 AND resource = $2 AND aggregate_id = $3",
            &[&source.data_source_ref, &source.resource, &source.aggregate_id],
        )
        .map_err(|error| database("reading the stream projection", &error))?;
    let Some(row) = row else {
        return model::empty_projection();
    };
    Ok(Projection {
        version: u64::try_from(row.get::<_, i64>(0))
            .map_err(|_| StoreError::corrupt("a stream head has a negative version"))?,
        last_event_ref: Some(row.get(1)),
        last_event_type: Some(row.get(2)),
        last_event_digest: Some(row.get(3)),
        projection_digest: row.get(4),
    })
}

fn existing(
    client: &mut impl GenericClient,
    source: Source<'_>,
    idempotency_key: &str,
) -> Result<Option<EventRecord>, StoreError> {
    client
        .query_opt(
            &format!(
                "SELECT {EVENT_COLUMNS} FROM runx_store_events WHERE data_source_ref = $1 AND resource = $2 AND aggregate_id = $3 AND idempotency_key = $4"
            ),
            &[
                &source.data_source_ref,
                &source.resource,
                &source.aggregate_id,
                &idempotency_key,
            ],
        )
        .map_err(|error| database("reading the event under the key", &error))?
        .map(|row| record_at(&row, 0))
        .transpose()
}

fn record_at(row: &Row, offset: usize) -> Result<EventRecord, StoreError> {
    let event_json: String = row.get(offset + 6);
    Ok(EventRecord {
        event_ref: row.get(offset),
        version: u64::try_from(row.get::<_, i64>(offset + 1))
            .map_err(|_| StoreError::corrupt("an event has a negative version"))?,
        event_type: row.get(offset + 2),
        event_digest: row.get(offset + 3),
        idempotency_key: row.get(offset + 4),
        committed_at: row.get(offset + 5),
        event: serde_json::from_str::<JsonObject>(&event_json)
            .map_err(|error| StoreError::corrupt(format!("a stored event is not JSON: {error}")))?,
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
