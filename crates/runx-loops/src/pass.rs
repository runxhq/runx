//! One pass over a loop (§5 to §7): its file's revision taken in, its live
//! runs judged and settled, its due times put in its inbox, and, when it's
//! running with nothing live and events waiting, a wake claimed and run.
//! Every step commits at the record's version it read.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use runx_contracts::{JsonValue, ResolutionRequest, RunOutcome, RunState};
use runx_runtime::orchestrator::{DrivePolicy, ResumeRequest, RunResult, SkillRunRequest};
use runx_runtime::registry::resolve::{
    AbsentPins, Holder, SkillResolverOptions, resolve_skill_ref_details,
};
use runx_runtime::{HostEvents, HostName, RunAuthority, StreamScope};
use serde_json::{Map, Value, json};

use crate::clock;
use crate::file::{Calls, Late, LoopFile, LoopName, digest};
use crate::records::{Ending, InboxEvent, RecordEvent};
use crate::state::{LoopState, Status};
use crate::{LoopError, Loops, context};

/// What an open run that holds a loop's events waits on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Waiting {
    /// A person's answer: an approval, a question, a purchase.
    Person,
    /// A time runx keeps: a script's wait or a bounded retry.
    Until(DateTime<Utc>),
    /// An effect whose end is unknown, until custody settles it.
    Reconciling,
    /// Its driver is working it.
    Driving,
    /// No driver, and it never suspended: its process died mid-run.
    Orphaned,
}

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Passed {
    /// The run a wake started.
    pub woke: Option<String>,
    /// Each run that settled, and how.
    pub settled: Vec<(String, Ending)>,
    /// Why the loop couldn't run, when it couldn't.
    pub couldnt: Option<String>,
    /// What the live wake waits on, while it's open.
    pub waiting: Option<Waiting>,
    /// The earliest time something is next due: a clock time, a proposed
    /// wake, a retry, or the time a live run waits for.
    pub next: Option<DateTime<Utc>>,
}

/// How a run's end was judged.
enum Judged {
    Open(Waiting),
    Ended {
        ending: Ending,
        result: Value,
        spent: u64,
    },
}

/// A loop's run as runx starts it: its skill, owner and method.
struct Bound {
    skill_path: PathBuf,
    owner: String,
    method: String,
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// A fresh identity under `kind`, unique per call.
pub(crate) fn fresh(kind: &str) -> Result<String, LoopError> {
    let id = runx_runtime::run_record::mint_run_id(kind)
        .map_err(|error| LoopError::Runtime(error.to_string()))?;
    Ok(format!("{kind}/{id}"))
}

fn to_store(value: Value) -> Result<JsonValue, LoopError> {
    serde_json::from_value(value).map_err(|error| LoopError::json("a loop's inputs", &error))
}

impl Loops {
    /// One pass over loop `name` at `now`.
    pub fn pass(&self, name: &LoopName, now: DateTime<Utc>) -> Result<Passed, LoopError> {
        let mut passed = Passed::default();
        // What an accepted proposal writes is on disk before the file is read.
        self.write_accepted(name, now)?;
        let valid = self.take_revision(name, now)?;
        let mut state = self.read(name)?;
        // A stopped or paused loop still settles what its runs finish with.
        self.settle_live(name, &mut state, now, &mut passed)?;
        self.settle_people(name, &mut state, now, &mut passed)?;
        // A file that doesn't validate starts nothing until it does: no wake
        // runs on the revision before it as if nothing were wrong.
        if !valid {
            passed.couldnt = state.couldnt.clone();
            return Ok(passed);
        }
        if let Some(file) = state.active_file().transpose()? {
            let running = state.status == Status::Running;
            if running {
                self.fire(name, &mut state, &file, now)?;
            }
            // The person's "run now" or message wakes the loop whatever its
            // state: their explicit act. The loop stays as it was.
            if state.live.is_none() && (running || self.person_waiting(name, &state)?) {
                self.wake(name, &mut state, &file, now, &mut passed)?;
            }
            if running {
                passed.next = self.next_due(name, &state, &file, now)?;
            }
        }
        if let Some(Waiting::Until(at)) = passed.waiting {
            passed.next = Some(passed.next.map_or(at, |next| next.min(at)));
        }
        Ok(passed)
    }

    /// The loop's file as written, taken as its revision when it validates
    /// and isn't the one the loop runs or holds pending, an earlier one it
    /// went back to among them. One that doesn't validate is the loop's
    /// "couldn't run" until it does. Whether the file validates.
    fn take_revision(&self, name: &LoopName, now: DateTime<Utc>) -> Result<bool, LoopError> {
        let Some(crate::Written {
            bytes,
            file: parsed,
        }) = self.written(name)?
        else {
            return Ok(true);
        };
        let state = self.read(name)?;
        let file_digest = digest(bytes.as_bytes());
        let valid = parsed.is_ok();
        if valid
            && !state.invalid_file
            && [state.active.as_deref(), state.pending.as_deref()]
                .contains(&Some(file_digest.as_str()))
        {
            return Ok(true);
        }
        let event = match parsed {
            Ok(_) => RecordEvent::Revision {
                digest: file_digest,
                bytes,
                at: stamp(now),
            },
            Err(error) => {
                let why = format!("its file doesn't validate: {error}");
                if state.couldnt.as_deref() == Some(why.as_str()) {
                    return Ok(false);
                }
                RecordEvent::Couldnt {
                    why,
                    at: stamp(now),
                    retry_at: stamp(now),
                    file: true,
                }
            }
        };
        self.commit(name, &state, &[event], None)?;
        Ok(valid)
    }

    /// Put each clock time that came due in the inbox, moving its frontier
    /// past it in the same commit, and each proposed wake whose time came.
    fn fire(
        &self,
        name: &LoopName,
        state: &mut LoopState,
        file: &LoopFile,
        now: DateTime<Utc>,
    ) -> Result<(), LoopError> {
        let revision = state.active.clone().unwrap_or_default();
        for trigger in file.on.iter().filter(|trigger| trigger.is_clock()) {
            let id = trigger.id();
            let Some(frontier) = state.frontiers.get(&id).cloned() else {
                // A trigger's times count from the revision that activated
                // it: none before now.
                self.commit(
                    name,
                    state,
                    &[RecordEvent::Frontier {
                        trigger: id,
                        revision: revision.clone(),
                        anchor: stamp(now),
                        covered_until: stamp(now),
                        skipped: 0,
                    }],
                    None,
                )?;
                *state = self.read(name)?;
                continue;
            };
            let Some(due) = clock::due(trigger, frontier.anchor, frontier.covered_until, now)
            else {
                continue;
            };
            let behind = now - due.latest;
            let late = due.count > 1 || behind > file.policy.grace.0;
            let skip = file.policy.late == Late::Skip && behind > file.policy.grace.0;
            let record = [RecordEvent::Frontier {
                trigger: id.clone(),
                revision: frontier.revision.clone(),
                anchor: stamp(frontier.anchor),
                covered_until: stamp(now),
                skipped: if skip { due.count } else { due.count - 1 },
            }];
            let inbox = if skip {
                None
            } else {
                Some(InboxEvent::Clock {
                    identity: format!(
                        "clock/{id}/{}",
                        due.latest.to_rfc3339_opts(SecondsFormat::Secs, true)
                    ),
                    due: stamp(due.latest),
                    late: late.then_some(due.count),
                    why: Some(trigger.to_string()),
                })
            };
            self.commit(name, state, &record, inbox.as_ref())?;
            *state = self.read(name)?;
        }
        let due_wakes = state
            .wakes
            .values()
            .filter(|wake| wake.at <= now)
            .cloned()
            .collect::<Vec<_>>();
        for wake in due_wakes {
            self.commit(
                name,
                state,
                &[RecordEvent::Fired {
                    identity: wake.identity.clone(),
                }],
                Some(&InboxEvent::Clock {
                    identity: wake.identity.clone(),
                    due: stamp(wake.at),
                    late: None,
                    why: Some(wake.why.clone()),
                }),
            )?;
            *state = self.read(name)?;
        }
        Ok(())
    }

    /// The file loop `name` runs: its active revision's, else its file as
    /// written, when it validates.
    pub fn file(&self, name: &LoopName) -> Result<Option<LoopFile>, LoopError> {
        let state = self.read(name)?;
        if let Some(file) = state.active_file() {
            return file.map(Some);
        }
        Ok(self.written(name)?.and_then(|written| written.file.ok()))
    }

    /// When loop `name` is next due, when it runs: its next clock time, a
    /// proposed wake, or a retry of events waiting.
    pub fn next(
        &self,
        name: &LoopName,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, LoopError> {
        let state = self.read(name)?;
        if state.status != Status::Running {
            return Ok(None);
        }
        match state.active_file().transpose()? {
            Some(file) => self.next_due(name, &state, &file, now),
            None => Ok(None),
        }
    }

    /// Whether the person asked for a wake the loop hasn't answered, since
    /// any batch that failed.
    fn person_waiting(&self, name: &LoopName, state: &LoopState) -> Result<bool, LoopError> {
        let file = state.active_file().transpose()?;
        let limit = file.as_ref().map_or(100, |file| file.policy.batch);
        Ok(self
            .inbox_after(name, state.position, limit, u64::MAX)?
            .iter()
            .any(|(version, event)| is_persons(event) && Some(*version) > state.held_through))
    }

    /// When something of the loop is next due.
    fn next_due(
        &self,
        name: &LoopName,
        state: &LoopState,
        file: &LoopFile,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, LoopError> {
        let mut next: Option<DateTime<Utc>> = None;
        let mut consider = |at: DateTime<Utc>| next = Some(next.map_or(at, |next| next.min(at)));
        for trigger in file.on.iter().filter(|trigger| trigger.is_clock()) {
            if let Some(frontier) = state.frontiers.get(&trigger.id())
                && let Some(at) =
                    clock::next_after(trigger, frontier.anchor, frontier.covered_until)
            {
                consider(at);
            }
        }
        for wake in state.wakes.values() {
            consider(wake.at);
        }
        if state.live.is_none() && self.inbox_version(name)? > state.position {
            consider(state.retry_at.unwrap_or(now).max(now));
        }
        Ok(next)
    }

    /// Claim the inbox's events past the loop's position and start the run
    /// that works them, in one commit, then settle it if it ended.
    fn wake(
        &self,
        name: &LoopName,
        state: &mut LoopState,
        file: &LoopFile,
        now: DateTime<Utc>,
        passed: &mut Passed,
    ) -> Result<(), LoopError> {
        let events = self.inbox_after(
            name,
            state.position,
            file.policy.batch,
            file.policy.batch_bytes,
        )?;
        // A retry waits for its time, unless the person asked for a wake
        // since the batch that failed.
        let asked = events
            .iter()
            .any(|(version, event)| is_persons(event) && Some(*version) > state.held_through);
        if !asked && state.retry_at.is_some_and(|retry| retry > now) {
            return Ok(());
        }
        let (Some((from, _)), Some((through, _))) = (events.first(), events.last()) else {
            return Ok(());
        };
        let (from, through) = (*from, *through);
        let bound = match self
            .zero_price()
            .and_then(|()| self.bind(&file.calls))
            .and_then(|bound| {
                let inputs = to_store(Value::Object(file.inputs.clone()))
                    .map_err(|error| error.to_string())?;
                let JsonValue::Object(inputs) = inputs else {
                    return Err("the loop's inputs aren't a map".to_owned());
                };
                runx_runtime::check_method_inputs(&bound.skill_path, &bound.method, &inputs)
                    .map_err(|why| format!("its inputs don't validate: {why}"))?;
                Ok(bound)
            }) {
            Ok(bound) => bound,
            Err(why) => return self.couldnt(name, state, file, now, why, passed),
        };
        let day = file.day_zone().day_of(now);
        let reserved = state.left_today(file.policy.spend.per_day_micro_usd(), &day);
        let authority = match self.authority(file, reserved, now) {
            Ok(authority) => authority,
            Err(why) => return self.couldnt(name, state, file, now, why, passed),
        };
        let run = runx_runtime::run_record::mint_run_id(&file.calls.skill)
            .map_err(|error| LoopError::Runtime(error.to_string()))?;
        let revision = state.active.clone().unwrap_or_default();
        let woke = RecordEvent::Woke {
            run: run.clone(),
            from,
            through,
            revision: revision.clone(),
            reserved_micro_usd: reserved,
            day,
            at: stamp(now),
        };
        let late = events.iter().find_map(|(_, event)| match event {
            InboxEvent::Clock {
                late: Some(count),
                due,
                ..
            } => Some(json!({
                "occurrences": count,
                "seconds": instant(due).map(|due| (now - due).num_seconds()),
            })),
            _ => None,
        });
        let batch = events
            .iter()
            .map(|(version, event)| {
                let mut value = serde_json::to_value(event)
                    .map_err(|error| LoopError::json("a loop's batch", &error))?;
                let mut entry = value
                    .get_mut("data")
                    .map(Value::take)
                    .unwrap_or(Value::Null);
                if let Some(object) = entry.as_object_mut() {
                    object.insert("kind".to_owned(), json!(event.kind()));
                    object.insert("version".to_owned(), json!(version));
                }
                Ok(entry)
            })
            .collect::<Result<Vec<_>, LoopError>>()?;
        let now_text = stamp(now);
        let (date, start, end) = file.day_zone().day_bounds(now);
        let current_context = vec![
            context::wake(
                name.as_str(),
                &file.brief,
                &revision,
                &now_text,
                (state.last_answered_at.as_deref(), late),
                &state.answers,
                (&date, &stamp(start), &stamp(end)),
            )?,
            context::batch(name.as_str(), batch, (from, through), &now_text)?,
        ];
        let request = self.request(&bound, &run, &file.inputs, current_context, authority)?;
        let claim = HostEvents {
            host: HostName::new(crate::records::HOST)?,
            appends: Self::record_append(name, state, &[woke])?
                .into_iter()
                .collect(),
        };
        let scope = StreamScope::new(&bound.owner, name.as_str()).map_err(LoopError::Invalid)?;
        let started =
            self.orchestrator
                .run_claimed(&request, Some(&bound.method), (claim, Some(scope)));
        *state = self.read(name)?;
        let ours = state.live.as_ref().is_some_and(|live| live.run == run);
        match started {
            Err(error) if !ours => {
                // Another wake claimed first, or the start was refused before
                // its commit: no run started.
                if state.live.is_none() {
                    return self.couldnt(name, state, file, now, error.to_string(), passed);
                }
                return Ok(());
            }
            _ => {}
        }
        passed.woke = Some(run);
        self.settle_live(name, state, now, passed)
    }

    /// Record that the loop couldn't run, and when it's tried again.
    fn couldnt(
        &self,
        name: &LoopName,
        state: &mut LoopState,
        file: &LoopFile,
        now: DateTime<Utc>,
        why: String,
        passed: &mut Passed,
    ) -> Result<(), LoopError> {
        let tries = state.couldnts.saturating_add(1);
        let retry_at = now + file.policy.retry_after(tries);
        self.commit(
            name,
            state,
            &[RecordEvent::Couldnt {
                why: why.clone(),
                at: stamp(now),
                retry_at: stamp(retry_at),
                file: false,
            }],
            None,
        )?;
        *state = self.read(name)?;
        passed.couldnt = Some(why);
        Ok(())
    }

    /// Judge the live wake, drive it on when its time came or its process
    /// died, and settle it when it ended.
    fn settle_live(
        &self,
        name: &LoopName,
        state: &mut LoopState,
        now: DateTime<Utc>,
        passed: &mut Passed,
    ) -> Result<(), LoopError> {
        let Some(live) = state.live.clone() else {
            return Ok(());
        };
        let Some(file) = state
            .revisions
            .get(&live.revision)
            .map(|(bytes, _)| LoopFile::parse(bytes.as_bytes()))
            .transpose()?
        else {
            return Err(LoopError::Runtime(format!(
                "loop {name}'s live wake names a revision its record doesn't hold"
            )));
        };
        let mut judged = self.judge(&live.run)?;
        if let Judged::Open(waiting) = &judged {
            let drive_on = match waiting {
                Waiting::Until(at) => *at <= now,
                Waiting::Orphaned => true,
                _ => false,
            };
            if drive_on {
                let bound = self.bind(&file.calls).map_err(LoopError::Runtime)?;
                self.drive_on(&live.run, &bound, waiting)?;
                judged = self.judge(&live.run)?;
            }
        }
        match judged {
            Judged::Open(waiting) => {
                passed.waiting = Some(waiting);
                Ok(())
            }
            Judged::Ended {
                ending,
                result,
                spent,
            } => {
                let proposals = crate::proposals::checked(self, &live.run, &file, &result, now)?;
                let mut record = vec![RecordEvent::Settled {
                    run: live.run.clone(),
                    ending: ending.clone(),
                    through: ending.moves().then_some(live.through),
                    changed: proposals.changed,
                    spent_micro_usd: spent,
                    retry_at: match ending {
                        Ending::Failed { .. } => {
                            Some(stamp(now + file.policy.retry_after(state.failures + 1)))
                        }
                        Ending::Unavailable { .. } => {
                            Some(stamp(now + file.policy.retry_after(state.outages + 1)))
                        }
                        _ => None,
                    },
                    at: stamp(now),
                }];
                let pause = match &ending {
                    Ending::Refused { reason, .. } => {
                        Some(format!("admission refused its run: {reason}"))
                    }
                    Ending::FailedAfterChange { why, changed, .. } => Some(format!(
                        "its run changed {changed} thing(s) and then failed{}: retry or skip it",
                        why.as_deref()
                            .map(|why| format!(": {why}"))
                            .unwrap_or_default()
                    )),
                    Ending::Failed { why, .. } => file
                        .policy
                        .on_failure
                        .pause_after
                        .filter(|after| state.failures + 1 >= *after)
                        .map(|after| {
                            format!(
                                "{after} failures in a row{}",
                                why.as_deref()
                                    .map(|why| format!(", the last: {why}"))
                                    .unwrap_or_default()
                            )
                        }),
                    _ => None,
                };
                // A loop the person stopped stays stopped: a pause is for
                // one that runs, and the stop outranks it.
                if let Some(why) = pause.filter(|_| !matches!(state.status, Status::Stopped { .. }))
                {
                    record.push(RecordEvent::Paused {
                        why,
                        at: stamp(now),
                        run: Some(live.run.clone()),
                    });
                }
                record.extend(proposals.events);
                self.commit(name, state, &record, None)?;
                *state = self.read(name)?;
                passed.settled.push((live.run, ending));
                Ok(())
            }
        }
    }

    /// Settle each person's run on the loop that ended.
    fn settle_people(
        &self,
        name: &LoopName,
        state: &mut LoopState,
        now: DateTime<Utc>,
        passed: &mut Passed,
    ) -> Result<(), LoopError> {
        for person in state.people.values().cloned().collect::<Vec<_>>() {
            let Some(file) = state
                .revisions
                .get(&person.revision)
                .map(|(bytes, _)| LoopFile::parse(bytes.as_bytes()))
                .transpose()?
            else {
                continue;
            };
            if let Judged::Ended {
                ending,
                result,
                spent,
            } = self.judge(&person.run)?
            {
                let proposals = crate::proposals::checked(self, &person.run, &file, &result, now)?;
                let mut record = vec![RecordEvent::Settled {
                    run: person.run.clone(),
                    ending: ending.clone(),
                    through: None,
                    changed: proposals.changed,
                    spent_micro_usd: spent,
                    retry_at: None,
                    at: stamp(now),
                }];
                record.extend(proposals.events);
                self.commit(name, state, &record, None)?;
                *state = self.read(name)?;
                passed.settled.push((person.run, ending));
            }
        }
        Ok(())
    }

    /// How run `run` stands: open and on what it waits, or how it ended, by
    /// its state, what it changed and who ended it (§7).
    fn judge(&self, run: &str) -> Result<Judged, LoopError> {
        let read = runx_runtime::run_record::read_run(&self.env, &self.cwd, run, None)
            .map_err(|error| LoopError::Runtime(error.to_string()))?;
        match read.run.state {
            RunState::Running => Ok(Judged::Open(if read.run.driven {
                Waiting::Driving
            } else {
                Waiting::Orphaned
            })),
            RunState::Paused => {
                let mut waiting = None;
                for waited in &read.waiting {
                    let this = match &waited.request {
                        ResolutionRequest::Wait { until, .. } => {
                            DateTime::from_timestamp_millis(*until)
                                .map_or(Waiting::Person, Waiting::Until)
                        }
                        ResolutionRequest::Retry {
                            retry_after_seconds: 0,
                            ..
                        } => Waiting::Reconciling,
                        ResolutionRequest::Retry {
                            retry_after_seconds,
                            ..
                        } => instant(read.run.updated_at.as_str())
                            .and_then(|at| {
                                at.checked_add_signed(Duration::seconds(
                                    i64::try_from(*retry_after_seconds).unwrap_or(i64::MAX),
                                ))
                            })
                            .map_or(Waiting::Person, Waiting::Until),
                        _ => Waiting::Person,
                    };
                    // A person's question outranks a reconciliation, which
                    // outranks a time.
                    waiting = Some(match (waiting, this) {
                        (Some(Waiting::Person), _) | (_, Waiting::Person) => Waiting::Person,
                        (Some(Waiting::Reconciling), _) | (_, Waiting::Reconciling) => {
                            Waiting::Reconciling
                        }
                        (Some(Waiting::Until(left)), Waiting::Until(right)) => {
                            Waiting::Until(left.min(right))
                        }
                        (_, this) => this,
                    });
                }
                Ok(Judged::Open(waiting.unwrap_or(Waiting::Person)))
            }
            RunState::Sealed => {
                let changes = runx_runtime::run_record::changes(&self.env, &self.cwd, run)
                    .map_err(|error| LoopError::Runtime(error.to_string()))?;
                if changes.unknown > 0 {
                    return Ok(Judged::Open(Waiting::Reconciling));
                }
                let outcome = read.run.outcome.unwrap_or(RunOutcome::Failed);
                // Read from the record and the receipt alone, so a run whose
                // skill changed since still settles.
                let reason =
                    self.seal_reason(read.run.receipt_id.as_ref().map(|id| id.as_str()))?;
                let why = read.run.why.as_ref().map(|why| why.as_str().to_owned());
                let label = outcome.label().to_owned();
                let ending = match (outcome, reason.as_deref()) {
                    (_, Some("host_closed" | "approval_declined")) => {
                        Ending::Closed { outcome: label }
                    }
                    (
                        _,
                        Some(
                            reason @ ("authority_denied"
                            | "cost_limit_exceeded"
                            | "preflight_refused"
                            | "input_contract_invalid"),
                        ),
                    ) => Ending::Refused {
                        outcome: label,
                        reason: why.clone().unwrap_or_else(|| reason.replace('_', " ")),
                    },
                    // Its model was unavailable and it changed nothing: the
                    // loop couldn't run, no failure of its skill's.
                    (RunOutcome::Failed, Some("drive_provider_failed")) if changes.done == 0 => {
                        Ending::Unavailable {
                            outcome: label,
                            why,
                        }
                    }
                    (RunOutcome::Failed | RunOutcome::Killed | RunOutcome::TimedOut, _) => {
                        if changes.done > 0 {
                            Ending::FailedAfterChange {
                                outcome: label,
                                why,
                                changed: changes.done,
                            }
                        } else {
                            Ending::Failed {
                                outcome: label,
                                why,
                            }
                        }
                    }
                    _ => Ending::Answered { outcome: label },
                };
                let result = self.loop_output(run)?;
                let spent = runx_runtime::committed_spend(&self.env, &self.cwd, run)
                    .map_err(LoopError::Runtime)?;
                Ok(Judged::Ended {
                    ending,
                    result,
                    spent,
                })
            }
        }
    }

    /// Drive a waiting run on: at its time, resumed with nothing new; after
    /// its process died, started again by its id, which continues it from
    /// its record.
    fn drive_on(&self, run: &str, bound: &Bound, waiting: &Waiting) -> Result<(), LoopError> {
        let result = match waiting {
            Waiting::Until(_) => self.orchestrator.resume(&ResumeRequest {
                run_id: run.to_owned(),
                answers: runx_contracts::ResumeDocument::default(),
                receipt_dir: None,
                env: self.env.clone(),
                cwd: self.cwd.clone(),
                drive: DrivePolicy::Inline { max_rounds: None },
                package_digest: None,
                closure_digest: None,
            }),
            _ => self.orchestrator.run_skill_with_binding(
                &self.continuation(bound, run)?,
                Some(&bound.method),
                None,
                None,
            ),
        };
        result
            .map(|_| ())
            .map_err(|error| LoopError::Runtime(error.to_string()))
    }

    /// Why a sealed run's receipt says it sealed, when it says.
    fn seal_reason(&self, receipt: Option<&str>) -> Result<Option<String>, LoopError> {
        let Some(receipt) = receipt else {
            return Ok(None);
        };
        let paths = runx_runtime::RunxPaths::resolve(&self.env, &self.cwd);
        let store = runx_runtime::LocalReceiptStore::of_project(paths.receipts(), &paths);
        let receipt = store
            .read_exact_without_proof(receipt)
            .map_err(|error| LoopError::Runtime(error.to_string()))?;
        Ok(receipt.seal.reason.map(|reason| reason.label().to_owned()))
    }

    /// The run's declared `loop` output: the last one a step of the run
    /// itself recorded, as `{"loop": …}`, or nothing when none did.
    fn loop_output(&self, run: &str) -> Result<Value, LoopError> {
        let mut cursor: Option<String> = None;
        let mut found = Value::Null;
        loop {
            let page =
                runx_runtime::run_record::read_run(&self.env, &self.cwd, run, cursor.as_deref())
                    .map_err(|error| LoopError::Runtime(error.to_string()))?;
            for record in &page.events {
                if record.stream.as_str() != run {
                    continue;
                }
                if let runx_contracts::RunEventFact::Step {
                    succeeded: true,
                    output,
                    ..
                } = &record.event
                    && let Some(output) = output.get("loop")
                {
                    found = serde_json::to_value(output)
                        .map_err(|error| LoopError::json("a run's loop output", &error))?;
                }
            }
            if !page.more {
                break;
            }
            cursor = Some(page.cursor.as_str().to_owned());
        }
        Ok(if found.is_null() {
            Value::Null
        } else {
            json!({ "loop": found })
        })
    }

    /// Whether unattended wakes have a zero-price model: a local one, which
    /// nothing charges for.
    fn zero_price(&self) -> Result<(), String> {
        let config = runx_runtime::load_drive_config(&self.env, &self.cwd)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                "no model is set up for unattended wakes: set agent.provider and agent.endpoint_url to a local model with `runx config set`".to_owned()
            })?;
        let loopback = url::Url::parse(&config.endpoint_url)
            .ok()
            .is_some_and(|url| {
                matches!(
                    url.host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                )
            });
        if config.auth_mode == runx_runtime::DriveAuthMode::LocalNone || loopback {
            Ok(())
        } else {
            Err(format!(
                "unattended wakes run only on a zero-price model, and agent.provider {} at {} may charge: point it at a local model",
                config.provider.as_str(),
                config.endpoint_url
            ))
        }
    }

    /// The skill a loop's `calls` names, resolved as a run resolves it, and
    /// the owner its streams are kept under.
    fn bind(&self, calls: &Calls) -> Result<Bound, String> {
        let resolved = resolve_skill_ref_details(
            std::path::Path::new(&calls.skill),
            &self.cwd,
            SkillResolverOptions {
                env: &self.env,
                registry: None,
                expected_digest: None,
                absent_pins: AbsentPins::Fail,
                holder: Holder::Project,
            },
        )?;
        let owner = runx_runtime::stream_owner(&self.env, &resolved.runnable_path)?;
        Ok(Bound {
            skill_path: resolved.runnable_path,
            owner,
            method: calls.method.clone(),
        })
    }

    /// The authority a loop's runs act under: the saved authority its file
    /// names, else the workspace's standing one, its spend bounded by what
    /// the run reserved.
    fn authority(
        &self,
        file: &LoopFile,
        reserved: u64,
        now: DateTime<Utc>,
    ) -> Result<Option<RunAuthority>, String> {
        let paths = runx_runtime::RunxPaths::resolve(&self.env, &self.cwd);
        let mut authority = match &file.authority {
            Some(name) => runx_runtime::saved_authorities::named(&paths, name, &stamp(now))
                .map_err(|error| format!("its authority {name}: {error}"))?,
            None => runx_runtime::saved_authorities::standing(&paths)
                .map_err(|error| error.to_string())?
                .unwrap_or_default(),
        };
        authority.max_spend_micro_usd = Some(
            authority
                .max_spend_micro_usd
                .map_or(reserved, |limit| limit.min(reserved)),
        );
        Ok(Some(authority))
    }

    /// The request that continues run `run`, which takes its inputs and its
    /// context from its record.
    fn continuation(&self, bound: &Bound, run: &str) -> Result<SkillRunRequest, LoopError> {
        self.request(bound, run, &Map::new(), Vec::new(), None)
    }

    /// The request that starts run `run` of a loop's skill.
    fn request(
        &self,
        bound: &Bound,
        run: &str,
        inputs: &Map<String, Value>,
        current_context: Vec<runx_contracts::ContextEntry>,
        run_authority: Option<RunAuthority>,
    ) -> Result<SkillRunRequest, LoopError> {
        let inputs = inputs
            .iter()
            .map(|(key, value)| Ok((key.clone(), to_store(value.clone())?)))
            .collect::<Result<BTreeMap<_, _>, LoopError>>()?;
        Ok(SkillRunRequest {
            current_context,
            historical_context: Vec::new(),
            run_authority,
            act: None,
            skill_path: bound.skill_path.clone(),
            receipt_dir: None,
            run_id: Some(run.to_owned()),
            answers: None,
            inputs,
            env: self.env.clone(),
            cwd: self.cwd.clone(),
            drive: DrivePolicy::Inline { max_rounds: None },
        })
    }

    /// Start loop `name`: it takes the file's revision, a pending one
    /// included, and resumes when paused. The file must validate, and its
    /// inputs must be what its method takes.
    pub fn start(
        &self,
        name: &LoopName,
        by: &str,
        now: DateTime<Utc>,
    ) -> Result<LoopState, LoopError> {
        let Some(crate::Written {
            bytes,
            file: parsed,
        }) = self.written(name)?
        else {
            return Err(LoopError::Unknown(name.to_string()));
        };
        let file = parsed?;
        let bound = self.bind(&file.calls).map_err(LoopError::Refused)?;
        let JsonValue::Object(inputs) = to_store(Value::Object(file.inputs.clone()))? else {
            return Err(LoopError::Invalid("inputs: a map".to_owned()));
        };
        runx_runtime::check_method_inputs(&bound.skill_path, &bound.method, &inputs).map_err(
            |why| LoopError::Refused(format!("it can't start until its inputs validate: {why}")),
        )?;
        let state = self.read(name)?;
        let revision = digest(bytes.as_bytes());
        if state.status == Status::Running
            && state.active.as_deref() == Some(revision.as_str())
            && state.pending.is_none()
        {
            return Ok(state);
        }
        let mut record = Vec::new();
        if !state.revisions.contains_key(&revision) {
            record.push(RecordEvent::Revision {
                digest: revision.clone(),
                bytes,
                at: stamp(now),
            });
        }
        record.push(RecordEvent::Started {
            by: by.to_owned(),
            revision,
            at: stamp(now),
        });
        self.commit(name, &state, &record, None)?;
        self.read(name)
    }

    /// Stop loop `name`. A live wake goes on to its end, which settles; no
    /// wake follows.
    pub fn stop(
        &self,
        name: &LoopName,
        by: &str,
        why: &str,
        now: DateTime<Utc>,
    ) -> Result<LoopState, LoopError> {
        let state = self.read(name)?;
        if state.version == 0 {
            return Err(LoopError::Unknown(name.to_string()));
        }
        if matches!(state.status, Status::Stopped { .. }) {
            return Ok(state);
        }
        self.commit(
            name,
            &state,
            &[RecordEvent::Stopped {
                by: by.to_owned(),
                why: why.to_owned(),
                at: stamp(now),
            }],
            None,
        )?;
        self.read(name)
    }

    /// The person's "run now": one wake, whether the loop runs or not.
    pub fn run_now(&self, name: &LoopName, by: &str) -> Result<(), LoopError> {
        self.require(name)?;
        let identity = fresh("manual")?;
        self.take(
            name,
            InboxEvent::Manual {
                identity,
                by: by.to_owned(),
            },
            None,
        )
    }

    /// A message from the person, which wakes the loop.
    pub fn message(&self, name: &LoopName, by: &str, text: &str) -> Result<(), LoopError> {
        self.require(name)?;
        let identity = fresh("message")?;
        self.take(
            name,
            InboxEvent::Message {
                identity,
                by: by.to_owned(),
                text: text.to_owned(),
            },
            None,
        )
    }

    /// Put `event` in loop `name`'s inbox, once for its identity, with a
    /// feed's cursor moved in the same commit.
    pub fn take(
        &self,
        name: &LoopName,
        event: InboxEvent,
        cursor: Option<(String, String)>,
    ) -> Result<(), LoopError> {
        let state = self.read(name)?;
        let record = cursor
            .map(|(feed, cursor)| RecordEvent::Cursor {
                feed,
                cursor,
                at: stamp(Utc::now()),
            })
            .into_iter()
            .collect::<Vec<_>>();
        self.commit(name, &state, &record, Some(&event))?;
        Ok(())
    }

    /// After a failure that changed something, or a refusal: give the batch
    /// to a new attempt. The loop runs again.
    pub fn retry(
        &self,
        name: &LoopName,
        by: &str,
        now: DateTime<Utc>,
    ) -> Result<LoopState, LoopError> {
        let state = self.read(name)?;
        let Status::Paused { .. } = state.status else {
            return Err(LoopError::Refused(format!(
                "loop {name} isn't paused: nothing to retry"
            )));
        };
        self.commit(
            name,
            &state,
            &[RecordEvent::Resumed {
                by: by.to_owned(),
                at: stamp(now),
            }],
            None,
        )?;
        self.read(name)
    }

    /// After a failure, move past its batch: the person's skip. The loop
    /// runs again.
    pub fn skip(
        &self,
        name: &LoopName,
        by: &str,
        now: DateTime<Utc>,
    ) -> Result<LoopState, LoopError> {
        let state = self.read(name)?;
        if state.live.is_some() {
            return Err(LoopError::Refused(format!(
                "loop {name}'s wake is still open: answer or close its run with `runx resume`, then skip"
            )));
        }
        let Status::Paused { .. } = state.status else {
            return Err(LoopError::Refused(format!(
                "loop {name} isn't paused: nothing to skip"
            )));
        };
        let through = state.held_through.unwrap_or(state.position);
        self.commit(
            name,
            &state,
            &[
                RecordEvent::Skipped {
                    through,
                    by: by.to_owned(),
                    at: stamp(now),
                },
                RecordEvent::Resumed {
                    by: by.to_owned(),
                    at: stamp(now),
                },
            ],
            None,
        )?;
        self.read(name)
    }

    fn require(&self, name: &LoopName) -> Result<(), LoopError> {
        if self.written(name)?.is_none() && self.read(name)?.version == 0 {
            return Err(LoopError::Unknown(name.to_string()));
        }
        Ok(())
    }

    /// Start a person's run of `method` on loop `name` (§10): in the loop's
    /// scope, under its authority, with the loop's inputs the method
    /// declares and `given` over them. One is live per method: starting it
    /// again opens the live one.
    pub fn person_run(
        &self,
        name: &LoopName,
        method: &str,
        given: Map<String, Value>,
        by: &str,
        now: DateTime<Utc>,
    ) -> Result<RunResult, LoopError> {
        // Every check is made on the record the start commits at: one that
        // moved under the decision, another start among what moved it, is
        // decided again from what's there.
        for _ in 0..PERSON_RUN_TRIES {
            if let Some(result) = self.try_person_run(name, method, &given, by, now)? {
                return Ok(result);
            }
        }
        Err(LoopError::Refused(format!(
            "{name}'s record kept moving while {method} was being started; try again"
        )))
    }

    /// One decision to start, or open, the person's run of `method`, made on
    /// one reading of the record and committed at its version. None when the
    /// record moved under it.
    fn try_person_run(
        &self,
        name: &LoopName,
        method: &str,
        given: &Map<String, Value>,
        by: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<RunResult>, LoopError> {
        // One that ended since is settled first, so it isn't taken as open.
        let mut state = self.read(name)?;
        self.settle_people(name, &mut state, now, &mut Passed::default())?;
        let file = match state.active_file() {
            Some(file) => file?,
            None => {
                self.written(name)?
                    .ok_or_else(|| LoopError::Unknown(name.to_string()))?
                    .file?
            }
        };
        let bound = self
            .bind(&Calls {
                skill: file.calls.skill.clone(),
                method: method.to_owned(),
            })
            .map_err(LoopError::Refused)?;
        let declared = declared_inputs(&bound.skill_path, method).map_err(LoopError::Refused)?;
        let mut inputs = file
            .inputs
            .iter()
            .filter(|(key, _)| declared.contains(key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Map<_, _>>();
        inputs.extend(given.clone());
        if let Some(live) = state.people.get(method) {
            // The live one, opened where it waits.
            return self
                .orchestrator
                .run_skill_with_binding(
                    &self.continuation(&bound, &live.run)?,
                    Some(method),
                    None,
                    None,
                )
                .map(Some)
                .map_err(|error| LoopError::Runtime(error.to_string()));
        }
        let Some(revision) = state.active.clone() else {
            // A loop that isn't ready keeps its file's revision first, and
            // the start is decided again on the record that holds it.
            let bytes = self
                .written(name)?
                .ok_or_else(|| LoopError::Unknown(name.to_string()))?
                .bytes;
            self.commit(
                name,
                &state,
                &[RecordEvent::Revision {
                    digest: digest(bytes.as_bytes()),
                    bytes,
                    at: stamp(now),
                }],
                None,
            )?;
            return Ok(None);
        };
        let day = file.day_zone().day_of(now);
        let reserved = state.left_today(file.policy.spend.per_day_micro_usd(), &day);
        let authority = self
            .authority(&file, reserved, now)
            .map_err(LoopError::Refused)?;
        let run = runx_runtime::run_record::mint_run_id(&file.calls.skill)
            .map_err(|error| LoopError::Runtime(error.to_string()))?;
        let ran = RecordEvent::Ran {
            run: run.clone(),
            method: method.to_owned(),
            by: by.to_owned(),
            revision,
            reserved_micro_usd: reserved,
            day,
            at: stamp(now),
        };
        let now_text = stamp(now);
        let (date, start, end) = file.day_zone().day_bounds(now);
        let current_context = vec![context::wake(
            name.as_str(),
            &file.brief,
            state.active.as_deref().unwrap_or_default(),
            &now_text,
            (state.last_answered_at.as_deref(), None),
            &state.answers,
            (&date, &stamp(start), &stamp(end)),
        )?];
        let request = self.request(&bound, &run, &inputs, current_context, authority)?;
        let claim = HostEvents {
            host: HostName::new(crate::records::HOST)?,
            appends: Self::record_append(name, &state, &[ran])?
                .into_iter()
                .collect(),
        };
        let scope = StreamScope::new(&bound.owner, name.as_str()).map_err(LoopError::Invalid)?;
        let started = self
            .orchestrator
            .run_claimed(&request, Some(method), (claim, Some(scope)));
        let mut after = self.read(name)?;
        let ours = after.people.get(method).is_some_and(|live| live.run == run);
        let result = match started {
            // The record moved before the claim committed: no run started.
            Err(_) if !ours && after.version != state.version => return Ok(None),
            started => started.map_err(|error| LoopError::Runtime(error.to_string()))?,
        };
        let mut passed = Passed::default();
        self.settle_people(name, &mut after, now, &mut passed)?;
        Ok(Some(result))
    }

    /// The package a loop's `calls` names, as a run resolves it.
    pub(crate) fn skill_path(&self, calls: &Calls) -> Result<PathBuf, String> {
        self.bind(calls).map(|bound| bound.skill_path)
    }
}

/// How often a person's start is decided again on a record that keeps moving.
const PERSON_RUN_TRIES: u32 = 4;

/// Whether `event` is the person's own: their "run now" or a message.
fn is_persons(event: &InboxEvent) -> bool {
    matches!(
        event,
        InboxEvent::Manual { .. } | InboxEvent::Message { .. }
    )
}

/// The inputs `method` of the package at `skill_path` declares.
pub(crate) fn declared_inputs(
    skill_path: &std::path::Path,
    method: &str,
) -> Result<std::collections::BTreeSet<String>, String> {
    let loaded = runx_runtime::load_validated_skill_package(skill_path)
        .map_err(|error| error.to_string())?;
    loaded
        .package
        .profiles
        .values()
        .find_map(|profile| profile.methods.get(method))
        .map(|definition| definition.inputs.keys().cloned().collect())
        .ok_or_else(|| format!("{} has no method {method}", loaded.package.skill.name))
}
