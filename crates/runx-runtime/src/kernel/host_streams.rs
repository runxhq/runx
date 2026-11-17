//! A host's own streams in the workspace's store (the loop contract's E1).
//!
//! A host that keeps a record beside its runs, such as the loop layer, keeps
//! it here: streams under the kind `host:<name>`, appended in batches at the
//! versions the host read, every append or none, and read back checked
//! against each stream's chain. No host reads or writes a kernel stream; a
//! run's start carries a host's appends in its own commit
//! ([`HostEvents`] on the run's request).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use runx_contracts::JsonValue;

use super::store::{Expected, Head, KernelStore, KernelStoreError};
use crate::paths::RunxPaths;

/// A host's name: lowercase letters, digits and hyphens, starting with a
/// letter, at most 64 characters. It names the kind its streams are kept
/// under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostName(String);

impl HostName {
    pub fn new(name: &str) -> Result<Self, HostStreamError> {
        let named = name.len() <= 64
            && name.starts_with(|first: char| first.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !named {
            return Err(HostStreamError::InvalidName {
                name: name.to_owned(),
            });
        }
        Ok(Self(name.to_owned()))
    }

    fn kind(&self) -> String {
        format!("host:{}", self.0)
    }
}

/// One event a host appends: to `key`, at the version it read (`None` when
/// the stream must not exist yet), of `event_type`, holding `data`.
#[derive(Clone, Debug, PartialEq)]
pub struct HostAppend {
    pub key: String,
    pub expected: Option<u64>,
    pub event_type: String,
    pub data: JsonValue,
}

/// A host's appends that commit with a run's start, in the one commit that
/// starts it: a loop's claim and the run it starts are one.
#[derive(Clone, Debug, PartialEq)]
pub struct HostEvents {
    pub host: HostName,
    pub appends: Vec<HostAppend>,
}

impl HostEvents {
    pub(crate) fn into_appends(self) -> Result<Vec<super::store::Append>, KernelStoreError> {
        let kind = self.host.kind();
        self.appends
            .into_iter()
            .map(|append| append.into_store(kind.clone()))
            .collect()
    }
}

impl HostAppend {
    fn into_store(self, kind: String) -> Result<super::store::Append, KernelStoreError> {
        let data = serde_json::to_value(&self.data).map_err(KernelStoreError::encode)?;
        super::store::Append::host(
            kind,
            self.key,
            self.expected.map_or(Expected::Absent, Expected::Version),
            self.event_type,
            &data,
        )
    }
}

/// One event of a host's stream, as it was appended.
#[derive(Clone, Debug, PartialEq)]
pub struct HostRecord {
    pub version: u64,
    pub committed_at: String,
    pub event_type: String,
    pub data: JsonValue,
}

/// Why a host's read or append failed. A conflict says which stream moved,
/// so the host reads it again and decides afresh.
#[derive(Debug, thiserror::Error)]
pub enum HostStreamError {
    #[error(
        "{name} isn't a host name: lowercase letters, digits and hyphens, starting with a letter, at most 64 characters"
    )]
    InvalidName { name: String },
    #[error("{key} moved: it's at version {actual}, not the {expected} its writer read")]
    Conflict {
        key: String,
        expected: u64,
        actual: u64,
    },
    #[error(transparent)]
    Store(#[from] KernelStoreError),
}

/// The streams one host keeps in a workspace's store.
pub struct HostStreams {
    store: Arc<KernelStore>,
    host: HostName,
}

impl HostStreams {
    /// `host`'s streams in the store of the workspace at `cwd`.
    pub fn open(
        env: &BTreeMap<String, String>,
        cwd: &Path,
        host: HostName,
    ) -> Result<Self, HostStreamError> {
        let paths = RunxPaths::resolve(env, cwd);
        Ok(Self {
            store: KernelStore::for_project(&paths)?,
            host,
        })
    }

    /// The version `key` is at: 0 when it holds nothing.
    pub fn version(&self, key: &str) -> Result<u64, HostStreamError> {
        Ok(self.store.version_of(&self.host.kind(), key)?)
    }

    /// At most `limit` events of `key` after version `after`, in order.
    pub fn read_after(
        &self,
        key: &str,
        after: u64,
        limit: usize,
    ) -> Result<Vec<HostRecord>, HostStreamError> {
        self.store
            .read_rows(&self.host.kind(), key, after, Some(limit))?
            .into_iter()
            .map(|row| {
                Ok(HostRecord {
                    version: row.version,
                    committed_at: row.committed_at,
                    event_type: row.event_type,
                    data: serde_json::from_slice(&row.data).map_err(KernelStoreError::encode)?,
                })
            })
            .collect()
    }

    /// The host's streams, in key order after `after`, at most `limit`.
    pub fn heads(&self, after: Option<&str>, limit: usize) -> Result<Vec<Head>, HostStreamError> {
        Ok(self.store.heads_of(&self.host.kind(), after, limit)?)
    }

    /// Append every event or none, durably, each at the version its writer
    /// read. One stream that moved refuses the whole batch, naming it.
    pub fn append(&self, appends: Vec<HostAppend>) -> Result<Vec<u64>, HostStreamError> {
        let batch = HostEvents {
            host: self.host.clone(),
            appends,
        }
        .into_appends()?;
        self.store.append(batch).map_err(|error| match error {
            KernelStoreError::Conflict {
                key,
                expected,
                actual,
                ..
            } => HostStreamError::Conflict {
                key,
                expected,
                actual,
            },
            other => HostStreamError::Store(other),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{HostAppend, HostName, HostStreamError, HostStreams};

    fn append(key: &str, expected: Option<u64>, n: u64) -> Result<HostAppend, serde_json::Error> {
        Ok(HostAppend {
            key: key.to_owned(),
            expected,
            event_type: "woke".to_owned(),
            data: serde_json::from_value(serde_json::json!({"n": n}))?,
        })
    }

    #[test]
    fn a_host_appends_at_the_version_it_read_and_reads_back_in_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let env = std::collections::BTreeMap::from([(
            "RUNX_HOME".to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        )]);
        let streams = HostStreams::open(&env, temp.path(), HostName::new("loops")?)?;
        assert_eq!(streams.version("acme/record")?, 0);
        streams.append(vec![
            append("acme/record", None, 1)?,
            append("acme/inbox", None, 1)?,
        ])?;
        streams.append(vec![append("acme/record", Some(1), 2)?])?;
        let read = streams.read_after("acme/record", 0, 10)?;
        assert_eq!(
            read.iter().map(|record| record.version).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(streams.read_after("acme/record", 1, 10)?.len(), 1);
        // A writer that read an old version is refused, the whole batch with it.
        let stale = streams.append(vec![
            append("acme/inbox", Some(1), 2)?,
            append("acme/record", Some(1), 3)?,
        ]);
        assert!(matches!(
            stale,
            Err(HostStreamError::Conflict { ref key, expected: 1, actual: 2 }) if key == "acme/record"
        ));
        assert_eq!(streams.version("acme/inbox")?, 1);
        assert!(HostName::new("Loops").is_err());
        Ok(())
    }
}
