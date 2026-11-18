//! A loop's feeds (the loop contract's E4). A page of a connector's feed,
//! read after the loop's cursor with no run around it, goes into the loop's
//! inbox: each event once, under the provider's id for it, a gap when the
//! provider can't replay from the cursor, and then the cursor. A page read
//! again after a crash appends nothing twice, and a poll that finds nothing
//! appends nothing at all.

use std::time::Duration;

use chrono::Utc;
use runx_contracts::JsonValue;
use runx_runtime::FeedRead;

use crate::file::{LoopFile, LoopName, Trigger};
use crate::pass::stamp;
use crate::records::{InboxEvent, RecordEvent};
use crate::state::Status;
use crate::store::Committed;
use crate::{LoopError, Loops};

/// How many times a cursor's move is tried while the loop's record moves.
const CURSOR_TRIES: usize = 5;

/// One of a loop's feeds, as its file names it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Feed {
    pub connector: String,
    pub feed: String,
}

impl Feed {
    /// The feed's key in the loop's record, `<connector>/<feed>`.
    fn key(&self) -> String {
        format!("{}/{}", self.connector, self.feed)
    }
}

impl std::fmt::Display for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.connector, self.feed)
    }
}

/// What one poll took into the inbox.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Polled {
    /// The events new to the inbox, a gap among them.
    pub taken: usize,
    /// More events follow at once.
    pub more: bool,
}

impl Loops {
    /// The feeds loop `name` takes events from: those its revision names,
    /// while it runs or waits paused, since what arrives meanwhile is still
    /// what wakes it. A stopped loop takes none.
    pub fn feeds(&self, name: &LoopName) -> Result<Vec<Feed>, LoopError> {
        let state = self.read(name)?;
        if !matches!(state.status, Status::Running | Status::Paused { .. }) {
            return Ok(Vec::new());
        }
        let Some(file) = state.active_file().transpose()? else {
            return Ok(Vec::new());
        };
        Ok(file
            .on
            .iter()
            .filter_map(|trigger| match trigger {
                Trigger::Feed { connector, feed } => Some(Feed {
                    connector: connector.clone(),
                    feed: feed.clone(),
                }),
                _ => None,
            })
            .collect())
    }

    /// Each feed `file` names, one its connector here offers.
    pub(crate) fn check_feeds(&self, file: &LoopFile) -> Result<(), LoopError> {
        for trigger in &file.on {
            if let Trigger::Feed { connector, feed } = trigger {
                runx_runtime::feed_offered(&self.env, &self.cwd, connector, feed).map_err(|why| {
                    LoopError::Refused(format!(
                        "it can't start until its feed {connector}/{feed} is one a connector here offers: {why}"
                    ))
                })?;
            }
        }
        Ok(())
    }

    /// Read one page of loop `name`'s `feed` after its cursor, held open as
    /// long as `wait` while nothing has arrived, under the loop's authority,
    /// and take it into the inbox.
    pub fn poll(&self, name: &LoopName, feed: &Feed, wait: Duration) -> Result<Polled, LoopError> {
        let state = self.read(name)?;
        let file = state
            .active_file()
            .transpose()?
            .ok_or_else(|| LoopError::Refused(format!("loop {name} hasn't started")))?;
        let key = feed.key();
        let cursor = state
            .cursors
            .get(&key)
            .map(|text| serde_json::from_str::<JsonValue>(text))
            .transpose()
            .map_err(|error| LoopError::json("a feed's cursor", &error))?;
        let authority = self
            .authority(&file, 0, Utc::now())
            .map_err(LoopError::Refused)?;
        let page = self
            .orchestrator
            .read_feed(&FeedRead {
                connector: &feed.connector,
                feed: &feed.feed,
                cursor: cursor.as_ref(),
                wait: Some(wait),
                authority: authority.as_ref(),
                env: &self.env,
                workspace: &self.cwd,
            })
            .map_err(|why| LoopError::Runtime(format!("feed {feed}: {why}")))?;
        let mut taken = 0;
        for event in page.events {
            let data = serde_json::to_value(&event.data)
                .map_err(|error| LoopError::json("a feed's event", &error))?;
            let event = InboxEvent::Feed {
                identity: format!("feed/{key}/{}", event.id),
                connector: feed.connector.clone(),
                feed: feed.feed.clone(),
                event_id: event.id,
                data,
            };
            if self.put(name, &event)? == Committed::Done {
                taken += 1;
            }
        }
        let from = state.cursors.get(&key).cloned();
        if page.gap {
            let event = InboxEvent::Gap {
                identity: format!("gap/{key}/{}", from.as_deref().unwrap_or("start")),
                connector: feed.connector.clone(),
                feed: feed.feed.clone(),
                cursor: from.clone(),
            };
            if self.put(name, &event)? == Committed::Done {
                taken += 1;
            }
        }
        if let Some(next) = page.cursor {
            let next = serde_json::to_string(&next)
                .map_err(|error| LoopError::json("a feed's cursor", &error))?;
            if from.as_ref() != Some(&next) {
                self.move_cursor(name, key, next)?;
            }
        }
        Ok(Polled {
            taken,
            more: page.more,
        })
    }

    /// Record that `key`'s events are taken up to `cursor`, as the loop's
    /// record stands when it's written.
    fn move_cursor(&self, name: &LoopName, key: String, cursor: String) -> Result<(), LoopError> {
        let moved = RecordEvent::Cursor {
            feed: key,
            cursor,
            at: stamp(Utc::now()),
        };
        for _ in 0..CURSOR_TRIES {
            let state = self.read(name)?;
            match self.commit(name, &state, std::slice::from_ref(&moved), None) {
                Err(LoopError::Moved(_)) => {}
                committed => return committed.map(|_| ()),
            }
        }
        Err(LoopError::Moved(name.to_string()))
    }
}
