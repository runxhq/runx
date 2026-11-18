//! A loop's two streams (the loop contract §5): its inbox, what can wake it,
//! and its record, what happened to it. Both are host streams in the
//! workspace's store (E1), under the host `loops`, keyed by the loop's name.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::file::{LoopName, digest};

/// The host the loop layer's streams are kept under.
pub const HOST: &str = "loops";

/// The keys of one loop's streams.
#[derive(Clone, Debug)]
pub(crate) struct Keys {
    pub inbox: String,
    pub record: String,
    name: String,
}

impl Keys {
    pub(crate) fn of(name: &LoopName) -> Self {
        Self {
            inbox: format!("{name}/inbox"),
            record: format!("{name}/record"),
            name: name.as_str().to_owned(),
        }
    }

    /// The key that holds an inbox identity once it's in the inbox: appended
    /// only where it doesn't exist, in the commit that appends the event, so
    /// an identity is in the inbox at most once for the loop's life.
    pub(crate) fn seen(&self, identity: &str) -> String {
        format!("{}/seen/{}", self.name, &digest(identity.as_bytes())[7..39])
    }
}

/// What can wake a loop, each under its identity (§5.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum InboxEvent {
    /// A trigger's occurrence or a wake a run proposed: due at `due`, and
    /// late by `late` occurrences when runx was off past them.
    Clock {
        identity: String,
        due: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        late: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    /// The person's "run now".
    Manual { identity: String, by: String },
    /// A message from the person.
    Message {
        identity: String,
        by: String,
        text: String,
    },
    /// An event from a connector's feed (E4).
    Feed {
        identity: String,
        connector: String,
        feed: String,
        event_id: String,
        /// The event as the provider gave it: outside input, never anyone's
        /// authority.
        #[serde(default)]
        data: Value,
    },
    /// A feed that can't replay from its cursor: the method catches up.
    Gap {
        identity: String,
        connector: String,
        feed: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    /// A request to the loop's webhook. Its payload is outside input, never
    /// anyone's authority.
    Webhook {
        identity: String,
        name: String,
        payload: Value,
    },
}

impl InboxEvent {
    #[must_use]
    pub fn identity(&self) -> &str {
        match self {
            Self::Clock { identity, .. }
            | Self::Manual { identity, .. }
            | Self::Message { identity, .. }
            | Self::Feed { identity, .. }
            | Self::Gap { identity, .. }
            | Self::Webhook { identity, .. } => identity,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Clock { .. } => "clock",
            Self::Manual { .. } => "manual",
            Self::Message { .. } => "message",
            Self::Feed { .. } => "feed",
            Self::Gap { .. } => "gap",
            Self::Webhook { .. } => "webhook",
        }
    }
}

/// What a run may propose to its loop (§8), as its `loop` output holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Proposal {
    /// Wake again at `at`.
    Wake { at: String, why: String },
    /// Tell the person something once, with an action that starts a person's
    /// run of one of the loop's entry methods.
    Notice {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<Action>,
    },
    /// Stop the loop.
    Done { why: String },
    /// Change the loop's brief or inputs, when a person accepts it.
    Change {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        brief: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inputs: Option<Map<String, Value>>,
        why: String,
    },
    /// Make another loop, when a person accepts it and chooses its
    /// authority.
    NewLoop {
        calls: String,
        name: String,
        brief: String,
        #[serde(default)]
        inputs: Map<String, Value>,
        #[serde(default)]
        on: Vec<Value>,
        why: String,
    },
}

/// A notice's action: a person's run of `method` with `inputs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub method: String,
    #[serde(default)]
    pub inputs: Map<String, Value>,
    /// The button's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// How a run that held a loop's events ended, as §7 judges it: by its state,
/// what it changed and who ended it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "as", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ending {
    /// Sealed by its own answer: the loop moves past its batch.
    Answered { outcome: String },
    /// Sealed by the person: the loop moves past its batch, their skip.
    Closed { outcome: String },
    /// Sealed by admission's refusal: the loop pauses for the person.
    Refused { outcome: String, reason: String },
    /// Failed, killed or timed out having changed nothing: tried again on
    /// the retry ladder.
    Failed {
        outcome: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    /// Failed, killed or timed out after changing something: the loop pauses
    /// for the person to retry or skip.
    FailedAfterChange {
        outcome: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
        changed: u32,
    },
    /// Its model couldn't be reached or answered nothing usable, having
    /// changed nothing: the loop couldn't run, which is no failure of its
    /// skill, and it's tried again later on the batch it held.
    Unavailable {
        outcome: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
}

impl Ending {
    /// Whether the loop moves past the run's batch.
    #[must_use]
    pub const fn moves(&self) -> bool {
        matches!(self, Self::Answered { .. } | Self::Closed { .. })
    }

    /// The run's outcome label.
    #[must_use]
    pub fn outcome(&self) -> &str {
        match self {
            Self::Answered { outcome }
            | Self::Closed { outcome }
            | Self::Refused { outcome, .. }
            | Self::Failed { outcome, .. }
            | Self::FailedAfterChange { outcome, .. }
            | Self::Unavailable { outcome, .. } => outcome,
        }
    }
}

/// A file an accepted proposal writes: this loop's own, or a new loop's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Writes {
    /// The loop whose file it is, when it isn't this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub bytes: String,
    /// The digest of the file it replaces, written only over that; none for
    /// a new loop's file, written only where none is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

/// What happened to a loop (§5.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum RecordEvent {
    /// A version of the loop's file that validates, its bytes kept by digest.
    Revision {
        digest: String,
        bytes: String,
        at: String,
    },
    /// The loop started, on `revision`; a start also accepts a pending
    /// revision and resumes a paused loop.
    Started {
        by: String,
        revision: String,
        at: String,
    },
    Stopped {
        by: String,
        why: String,
        at: String,
    },
    /// The person resumed a paused loop on its revision: a retry of its
    /// batch, or after a skip. A pending revision still waits for a start.
    Resumed {
        by: String,
        at: String,
    },
    /// The loop paused itself, saying why.
    Paused {
        why: String,
        at: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run: Option<String>,
    },
    /// A wake claimed the inbox's events `from` to `through` for `run`, on
    /// `revision`, reserving its share of the day's budget. Its commit
    /// starts the run.
    Woke {
        run: String,
        from: u64,
        through: u64,
        revision: String,
        reserved_micro_usd: u64,
        day: String,
        at: String,
    },
    /// A person's run of `method` on the loop (§10), started in the same
    /// commit, reserving its share.
    Ran {
        run: String,
        method: String,
        by: String,
        revision: String,
        reserved_micro_usd: u64,
        day: String,
        at: String,
    },
    /// How `run` ended, once: a second append of a settlement is a replay.
    /// `through` is where the loop's position moved, when it moved.
    Settled {
        run: String,
        ending: Ending,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        through: Option<u64>,
        /// Whether the run said it changed anything; one that didn't is
        /// folded away in a listing.
        #[serde(default = "changed_default")]
        changed: bool,
        spent_micro_usd: u64,
        /// When a failed wake is tried again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_at: Option<String>,
        at: String,
    },
    /// What `run` proposed, checked: `refused` says why it wasn't taken.
    /// `revision` is the revision of the loop's file the run ran under, the
    /// loop a change was proposed against.
    Proposed {
        run: String,
        revision: String,
        id: String,
        proposal: Proposal,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refused: Option<String>,
        at: String,
    },
    /// A person's answer to a change or a new loop.
    Answered {
        id: String,
        accepted: bool,
        by: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        words: Option<String>,
        /// The file an accepted proposal writes, kept with the answer so
        /// it's written even when the process stops between the two.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writes: Option<Writes>,
        at: String,
    },
    /// The file an accepted proposal writes is on disk, or the person's own
    /// edit since stands in its place.
    Written {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        at: String,
    },
    /// A clock trigger's occurrences are covered up to `covered_until`,
    /// counted from `anchor`, the time `revision` activated it.
    Frontier {
        trigger: String,
        revision: String,
        anchor: String,
        covered_until: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        skipped: u32,
    },
    /// The person moved the loop past a failed batch, to `through`.
    Skipped {
        through: u64,
        by: String,
        at: String,
    },
    /// A proposed wake's time came: its clock event is in the inbox.
    Fired {
        identity: String,
    },
    /// Where a feed's events have been taken up to.
    Cursor {
        feed: String,
        cursor: String,
        at: String,
    },
    /// A feed couldn't be read, and why: said once for each reason, until
    /// it reads again.
    FeedFailing {
        feed: String,
        why: String,
        at: String,
    },
    /// A feed that couldn't be read reads again.
    FeedReads {
        feed: String,
        at: String,
    },
    /// The loop couldn't run: no run started. Tried again at `retry_at`.
    Couldnt {
        why: String,
        at: String,
        retry_at: String,
        /// Whether its file doesn't validate, which a valid file clears.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        file: bool,
    },
    /// A webhook's token, kept by digest only.
    Token {
        name: String,
        digest: String,
        at: String,
    },
}

const fn changed_default() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(count: &u32) -> bool {
    *count == 0
}

/// An event as its stream holds it: its type and its data apart.
pub(crate) fn split<E: Serialize>(event: &E) -> Result<(String, Value), serde_json::Error> {
    let mut whole = serde_json::to_value(event)?;
    let kind = whole
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let data = whole
        .as_object_mut()
        .and_then(|object| object.remove("data"))
        .unwrap_or(Value::Null);
    Ok((kind, data))
}

/// An event from its stream's type and data.
pub(crate) fn join<E: for<'de> Deserialize<'de>>(
    kind: &str,
    data: Value,
) -> Result<E, serde_json::Error> {
    serde_json::from_value(serde_json::json!({ "type": kind, "data": data }))
}
