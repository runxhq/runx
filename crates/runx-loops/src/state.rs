//! A loop's state, a fold of its record (§5.1). Nothing about a loop is
//! kept anywhere else: not in a model's context, not in the UI.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::records::{Ending, Proposal, RecordEvent};

/// Whether a loop runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Made, never started.
    #[default]
    New,
    Running,
    Paused {
        why: String,
    },
    Stopped {
        why: String,
        by: String,
    },
}

/// The wake whose run works the loop's inbox: its events `from` to
/// `through`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub run: String,
    pub from: u64,
    pub through: u64,
    pub revision: String,
    pub at: String,
}

/// A person's run on the loop (§10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonRun {
    pub run: String,
    pub method: String,
    pub by: String,
    pub revision: String,
}

/// How a run last settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Last {
    pub run: String,
    pub ending: Ending,
    pub at: String,
}

/// A clock trigger's frontier: its occurrences are covered up to
/// `covered_until`, counted from `anchor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frontier {
    pub revision: String,
    pub anchor: DateTime<Utc>,
    pub covered_until: DateTime<Utc>,
}

/// A wake a run proposed whose time hasn't come.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedWake {
    pub identity: String,
    pub at: DateTime<Utc>,
    pub why: String,
    pub run: String,
}

/// Something a run told the person.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: String,
    pub run: String,
    pub text: String,
    pub action: Option<crate::records::Action>,
    pub at: String,
}

/// A change or a new loop waiting on a person's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub id: String,
    pub run: String,
    pub proposal: Proposal,
    pub at: String,
}

/// A person's answer to a change or a new loop a run proposed.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Answer {
    pub id: String,
    pub proposal: Proposal,
    pub accepted: bool,
    pub by: String,
    pub words: Option<String>,
    pub at: String,
}

/// A run's share of a day's budget, outstanding until it settles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub day: String,
    pub micro_usd: u64,
}

/// The most notices a loop's state keeps: the record keeps them all.
const NOTICES: usize = 20;

/// A loop, as its record folds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoopState {
    /// The record's version this fold reached.
    pub version: u64,
    pub status: Status,
    /// Every revision of the loop's file, by digest: its bytes, and whether
    /// it widened the revision before it.
    pub revisions: BTreeMap<String, (String, bool)>,
    /// The revision the loop runs.
    pub active: Option<String>,
    /// A revision that widens what the loop may do, waiting for a start.
    pub pending: Option<String>,
    /// The inbox version the loop has answered for.
    pub position: u64,
    pub live: Option<Live>,
    pub people: BTreeMap<String, PersonRun>,
    /// Failures in a row, and when the wake is tried again.
    pub failures: u32,
    pub retry_at: Option<DateTime<Utc>>,
    /// Why the loop last couldn't run, and how many times in a row, until a
    /// run starts.
    pub couldnt: Option<String>,
    pub couldnts: u32,
    /// Wakes in a row whose model was unavailable, which space the next try;
    /// a wake that answers or is closed clears them.
    pub outages: u32,
    /// The end of the batch a wake that didn't move the loop held, which a
    /// person's skip moves past.
    pub held_through: Option<u64>,
    pub last: Option<Last>,
    /// When a run last settled with an answer of its own.
    pub last_answered_at: Option<String>,
    pub frontiers: BTreeMap<String, Frontier>,
    pub wakes: BTreeMap<String, ProposedWake>,
    pub notices: Vec<Notice>,
    pub proposals: BTreeMap<String, Pending>,
    /// How a person answered the loop's latest proposals, newest last.
    pub answers: Vec<Answer>,
    pub reservations: BTreeMap<String, Reservation>,
    /// What settled runs spent, by day.
    pub spent: BTreeMap<String, u64>,
    pub cursors: BTreeMap<String, String>,
    /// Each webhook's token digest, by its name.
    pub tokens: BTreeMap<String, String>,
}

fn instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

impl LoopState {
    /// Fold `event`, the record's `version`th.
    pub fn apply(&mut self, version: u64, event: &RecordEvent) {
        self.version = version;
        match event {
            RecordEvent::Revision { digest, bytes, .. } => {
                let widens = self.widens(bytes);
                self.revisions
                    .insert(digest.clone(), (bytes.clone(), widens));
                if self.active.is_none() || !widens {
                    // A revision that doesn't widen what the loop may do is
                    // the one it runs from here; one that does waits.
                    self.active = Some(digest.clone());
                    self.pending = None;
                } else {
                    self.pending = Some(digest.clone());
                }
            }
            RecordEvent::Started { revision, .. } => {
                self.status = Status::Running;
                self.active = Some(revision.clone());
                self.pending = None;
                self.failures = 0;
                self.outages = 0;
                self.retry_at = None;
                self.couldnt = None;
                self.couldnts = 0;
            }
            RecordEvent::Stopped { why, by, .. } => {
                self.status = Status::Stopped {
                    why: why.clone(),
                    by: by.clone(),
                };
            }
            RecordEvent::Resumed { .. } => {
                self.status = Status::Running;
                self.failures = 0;
                self.outages = 0;
                self.retry_at = None;
                self.couldnt = None;
                self.couldnts = 0;
            }
            RecordEvent::Paused { why, .. } => {
                self.status = Status::Paused { why: why.clone() };
            }
            RecordEvent::Woke {
                run,
                from,
                through,
                revision,
                reserved_micro_usd,
                day,
                at,
            } => {
                self.live = Some(Live {
                    run: run.clone(),
                    from: *from,
                    through: *through,
                    revision: revision.clone(),
                    at: at.clone(),
                });
                self.couldnt = None;
                self.couldnts = 0;
                self.retry_at = None;
                self.reserve(run, day, *reserved_micro_usd);
            }
            RecordEvent::Ran {
                run,
                method,
                by,
                revision,
                reserved_micro_usd,
                day,
                ..
            } => {
                self.people.insert(
                    method.clone(),
                    PersonRun {
                        run: run.clone(),
                        method: method.clone(),
                        by: by.clone(),
                        revision: revision.clone(),
                    },
                );
                self.reserve(run, day, *reserved_micro_usd);
            }
            RecordEvent::Settled {
                run,
                ending,
                through,
                spent_micro_usd,
                retry_at,
                at,
                ..
            } => {
                if let Some(reservation) = self.reservations.remove(run) {
                    *self.spent.entry(reservation.day).or_default() += spent_micro_usd;
                }
                self.people.retain(|_, person| person.run != *run);
                if let Some(live) = self.live.take_if(|live| live.run == *run) {
                    if ending.moves() {
                        if let Some(through) = through {
                            self.position = self.position.max(*through);
                        }
                        self.failures = 0;
                        self.outages = 0;
                        self.retry_at = None;
                        self.held_through = None;
                    } else {
                        self.held_through = Some(live.through);
                        match ending {
                            Ending::Failed { .. } => {
                                self.failures += 1;
                                self.retry_at = retry_at.as_deref().and_then(instant);
                            }
                            Ending::Unavailable { why, .. } => {
                                self.outages += 1;
                                self.couldnt =
                                    Some(why.clone().unwrap_or_else(|| {
                                        "its model couldn't be reached".to_owned()
                                    }));
                                self.retry_at = retry_at.as_deref().and_then(instant);
                            }
                            _ => {}
                        }
                    }
                }
                if matches!(ending, Ending::Answered { .. }) {
                    self.last_answered_at = Some(at.clone());
                }
                self.last = Some(Last {
                    run: run.clone(),
                    ending: ending.clone(),
                    at: at.clone(),
                });
            }
            RecordEvent::Proposed {
                run,
                id,
                proposal,
                refused,
                at,
            } => {
                if refused.is_some() {
                    return;
                }
                match proposal {
                    Proposal::Wake { at: due, why } => {
                        if let Some(due) = instant(due) {
                            let identity = wake_identity(run, &due);
                            self.wakes.insert(
                                identity.clone(),
                                ProposedWake {
                                    identity,
                                    at: due,
                                    why: why.clone(),
                                    run: run.clone(),
                                },
                            );
                        }
                    }
                    Proposal::Notice { text, action } => {
                        self.notices.push(Notice {
                            id: id.clone(),
                            run: run.clone(),
                            text: text.clone(),
                            action: action.clone(),
                            at: at.clone(),
                        });
                        let excess = self.notices.len().saturating_sub(NOTICES);
                        self.notices.drain(..excess);
                    }
                    Proposal::Done { .. } => {}
                    Proposal::Change { .. } | Proposal::NewLoop { .. } => {
                        self.proposals.insert(
                            id.clone(),
                            Pending {
                                id: id.clone(),
                                run: run.clone(),
                                proposal: proposal.clone(),
                                at: at.clone(),
                            },
                        );
                    }
                }
            }
            RecordEvent::Answered {
                id,
                accepted,
                by,
                words,
                at,
            } => {
                if let Some(pending) = self.proposals.remove(id) {
                    self.answers.push(Answer {
                        id: id.clone(),
                        proposal: pending.proposal,
                        accepted: *accepted,
                        by: by.clone(),
                        words: words.clone(),
                        at: at.clone(),
                    });
                    let excess = self.answers.len().saturating_sub(NOTICES);
                    self.answers.drain(..excess);
                }
            }
            RecordEvent::Frontier {
                trigger,
                revision,
                anchor,
                covered_until,
                ..
            } => {
                if let (Some(anchor), Some(covered_until)) =
                    (instant(anchor), instant(covered_until))
                {
                    self.frontiers.insert(
                        trigger.clone(),
                        Frontier {
                            revision: revision.clone(),
                            anchor,
                            covered_until,
                        },
                    );
                }
            }
            RecordEvent::Skipped { through, .. } => {
                self.position = self.position.max(*through);
                self.held_through = None;
                self.failures = 0;
                self.retry_at = None;
            }
            RecordEvent::Fired { identity } => {
                self.wakes.remove(identity);
            }
            RecordEvent::Cursor { feed, cursor, .. } => {
                self.cursors.insert(feed.clone(), cursor.clone());
            }
            RecordEvent::Couldnt { why, retry_at, .. } => {
                self.couldnt = Some(why.clone());
                self.couldnts += 1;
                self.retry_at = instant(retry_at);
            }
            RecordEvent::Token { name, digest, .. } => {
                self.tokens.insert(name.clone(), digest.clone());
            }
        }
    }

    fn reserve(&mut self, run: &str, day: &str, micro_usd: u64) {
        self.reservations.insert(
            run.to_owned(),
            Reservation {
                day: day.to_owned(),
                micro_usd,
            },
        );
    }

    /// Whether a revision with `bytes` widens the one the loop runs: another
    /// method, authority or policy. A first revision widens nothing.
    fn widens(&self, bytes: &str) -> bool {
        let Some((active, _)) = self
            .active
            .as_ref()
            .and_then(|digest| self.revisions.get(digest))
        else {
            return false;
        };
        match (
            crate::file::LoopFile::parse(active.as_bytes()),
            crate::file::LoopFile::parse(bytes.as_bytes()),
        ) {
            (Ok(active), Ok(next)) => active.widened_by(&next),
            // A record holds only revisions that validated; one that no
            // longer does is held for a start rather than trusted.
            _ => true,
        }
    }

    /// The file of the revision the loop runs.
    pub fn active_file(&self) -> Option<Result<crate::file::LoopFile, crate::LoopError>> {
        let digest = self.active.as_ref()?;
        let (bytes, _) = self.revisions.get(digest)?;
        Some(crate::file::LoopFile::parse(bytes.as_bytes()))
    }

    /// What the day's budget has left: the budget less every reservation
    /// still outstanding on `day` and what settled runs spent on it.
    #[must_use]
    pub fn left_today(&self, budget_micro_usd: u64, day: &str) -> u64 {
        let reserved: u64 = self
            .reservations
            .values()
            .filter(|reservation| reservation.day == day)
            .map(|reservation| reservation.micro_usd)
            .sum();
        let spent = self.spent.get(day).copied().unwrap_or_default();
        budget_micro_usd.saturating_sub(reserved.saturating_add(spent))
    }
}

/// A proposed wake's identity in the inbox: its run and its instant.
#[must_use]
pub fn wake_identity(run: &str, at: &DateTime<Utc>) -> String {
    format!(
        "wake/{run}/{}",
        at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    )
}
