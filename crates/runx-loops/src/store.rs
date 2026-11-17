//! Reading a loop's record into its state, and committing what an act
//! decided at the versions it read: every append or none.

use runx_runtime::{HostAppend, HostStreamError};
use serde_json::Value;

use crate::file::LoopName;
use crate::records::{InboxEvent, Keys, RecordEvent, join, split};
use crate::state::LoopState;
use crate::{LoopError, Loops};

/// How many events one read of a stream takes.
const PAGE: usize = 500;

/// How often an append that only adds to the inbox is tried again when
/// another writer added first.
const INBOX_TRIES: usize = 8;

/// What a commit did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Committed {
    Done,
    /// An event's identity was in the inbox already: nothing was appended.
    Duplicate,
}

fn to_store(value: Value) -> Result<runx_contracts::JsonValue, LoopError> {
    serde_json::from_value(value).map_err(|error| LoopError::json("encoding a loop event", &error))
}

fn from_store(value: &runx_contracts::JsonValue) -> Result<Value, LoopError> {
    serde_json::to_value(value).map_err(|error| LoopError::json("reading a loop event", &error))
}

impl Loops {
    /// Loop `name`'s state, folded from its whole record.
    pub(crate) fn read(&self, name: &LoopName) -> Result<LoopState, LoopError> {
        let keys = Keys::of(name);
        let mut state = LoopState::default();
        let mut after = 0;
        loop {
            let page = self.streams.read_after(&keys.record, after, PAGE)?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.version;
            for record in &page {
                // An entry is one commit: its facts, in the order decided.
                let facts = from_store(&record.data)?
                    .get_mut("facts")
                    .map(Value::take)
                    .and_then(|facts| match facts {
                        Value::Array(facts) => Some(facts),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        LoopError::Runtime(format!(
                            "loop {name}'s record holds an entry with no facts at version {}",
                            record.version
                        ))
                    })?;
                for fact in facts {
                    let event: RecordEvent = serde_json::from_value(fact)
                        .map_err(|error| LoopError::json("reading a loop's record", &error))?;
                    state.apply(record.version, &event);
                }
            }
        }
        Ok(state)
    }

    /// The inbox's events after `after`: at most `limit` of them and
    /// `bytes` bytes, though always the first.
    pub(crate) fn inbox_after(
        &self,
        name: &LoopName,
        after: u64,
        limit: u32,
        bytes: u64,
    ) -> Result<Vec<(u64, InboxEvent)>, LoopError> {
        let keys = Keys::of(name);
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let page = self.streams.read_after(&keys.inbox, after, limit)?;
        let mut events = Vec::with_capacity(page.len());
        let mut taken = 0_u64;
        for record in page {
            let data = from_store(&record.data)?;
            let size = u64::try_from(data.to_string().len()).unwrap_or(u64::MAX);
            if !events.is_empty() && taken.saturating_add(size) > bytes {
                break;
            }
            taken = taken.saturating_add(size);
            let event: InboxEvent = join(&record.event_type, data)
                .map_err(|error| LoopError::json("reading a loop's inbox", &error))?;
            events.push((record.version, event));
        }
        Ok(events)
    }

    /// The inbox's version: how many events it holds.
    pub(crate) fn inbox_version(&self, name: &LoopName) -> Result<u64, LoopError> {
        Ok(self.streams.version(&Keys::of(name).inbox)?)
    }

    /// The append that puts `facts` on loop `name`'s record after the
    /// version `state` read: one entry, the commit's, typed by its first
    /// fact. None for no facts.
    pub(crate) fn record_append(
        name: &LoopName,
        state: &LoopState,
        facts: &[RecordEvent],
    ) -> Result<Option<HostAppend>, LoopError> {
        let Some(first) = facts.first() else {
            return Ok(None);
        };
        let (event_type, _) =
            split(first).map_err(|error| LoopError::json("encoding a loop event", &error))?;
        let facts = serde_json::to_value(facts)
            .map_err(|error| LoopError::json("encoding a loop event", &error))?;
        Ok(Some(HostAppend {
            key: Keys::of(name).record,
            expected: Some(state.version),
            event_type,
            data: to_store(serde_json::json!({ "facts": facts }))?,
        }))
    }

    /// Commit `record` after the version `state` read, and `inbox` after the
    /// inbox's version, once for its identity. A record that moved refuses
    /// the whole commit; an inbox another writer added to first is read
    /// again, when nothing goes on the record.
    pub(crate) fn commit(
        &self,
        name: &LoopName,
        state: &LoopState,
        record: &[RecordEvent],
        inbox: Option<&InboxEvent>,
    ) -> Result<Committed, LoopError> {
        let keys = Keys::of(name);
        for _ in 0..INBOX_TRIES {
            let mut appends = Self::record_append(name, state, record)?
                .into_iter()
                .collect::<Vec<_>>();
            if let Some(event) = inbox {
                let (event_type, data) = split(event)
                    .map_err(|error| LoopError::json("encoding an inbox event", &error))?;
                appends.push(HostAppend {
                    key: keys.inbox.clone(),
                    expected: Some(self.streams.version(&keys.inbox)?),
                    event_type,
                    data: to_store(data)?,
                });
                appends.push(HostAppend {
                    key: keys.seen(event.identity()),
                    expected: None,
                    event_type: "seen".to_owned(),
                    data: to_store(serde_json::json!({ "identity": event.identity() }))?,
                });
            }
            if appends.is_empty() {
                return Ok(Committed::Done);
            }
            match self.streams.append(appends) {
                Ok(_) => return Ok(Committed::Done),
                Err(HostStreamError::Conflict { key, .. }) if key.contains("/seen/") => {
                    return Ok(Committed::Duplicate);
                }
                Err(HostStreamError::Conflict { key, .. })
                    if key == keys.inbox && record.is_empty() => {}
                Err(HostStreamError::Conflict { .. }) => {
                    return Err(LoopError::Moved(name.to_string()));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(LoopError::Moved(name.to_string()))
    }
}
