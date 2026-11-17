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
        self.take_revision(name, now)?;
        let mut state = self.read(name)?;
        // A stopped or paused loop still settles what its runs finish with.
        self.settle_live(name, &mut state, now, &mut passed)?;
        self.settle_people(name, &mut state, now, &mut passed)?;
        if state.status == Status::Running
            && let Some(file) = state.active_file().transpose()?
        {
            self.fire(name, &mut state, &file, now)?;
            if state.live.is_none() {
                self.wake(name, &mut state, &file, now, &mut passed)?;
            }
            passed.next = self.next_due(name, &state, &file, now)?;
        }
        if let Some(Waiting::Until(at)) = passed.waiting {
            passed.next = Some(passed.next.map_or(at, |next| next.min(at)));
        }
        Ok(passed)
    }

    /// The loop's file as written, kept as a revision when it validates and
    /// is new. One that doesn't validate is the loop's "couldn't run" until
    /// it does.
    fn take_revision(&self, name: &LoopName, now: DateTime<Utc>) -> Result<(), LoopError> {
        let Some(crate::Written {
            bytes,
            file: parsed,
        }) = self.written(name)?
        else {
            return Ok(());
        };
        let state = self.read(name)?;
        let file_digest = digest(bytes.as_bytes());
        if state.revisions.contains_key(&file_digest) {
            return Ok(());
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
                    return Ok(());
                }
                RecordEvent::Couldnt {
                    why,
                    at: stamp(now),
                    retry_at: stamp(now),
                }
            }
        };
        self.commit(name, &state, &[event], None)?;
        Ok(())
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
        if state.retry_at.is_some_and(|retry| retry > now) {
            return Ok(());
        }
        let events = self.inbox_after(
            name,
            state.position,
            file.policy.batch,
            file.policy.batch_bytes,
        )?;
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
        let current_context = vec![
            context::wake(
                name.as_str(),
                &file.brief,
                &revision,
                &now_text,
                state.last_answered_at.as_deref(),
                late,
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
        let bound = self.bind(&file.calls).map_err(LoopError::Runtime)?;
        let mut judged = self.judge(&live.run, &bound, &file.inputs)?;
        if let Judged::Open(waiting) = &judged {
            let drive_on = match waiting {
                Waiting::Until(at) => *at <= now,
                Waiting::Orphaned => true,
                _ => false,
            };
            if drive_on {
                self.drive_on(&live.run, &bound, &file.inputs, waiting)?;
                judged = self.judge(&live.run, &bound, &file.inputs)?;
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
                    retry_at: matches!(ending, Ending::Failed { .. })
                        .then(|| stamp(now + file.policy.retry_after(state.failures + 1))),
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
                if let Some(why) = pause {
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
            let bound = self.bind(&Calls {
                skill: file.calls.skill.clone(),
                method: person.method.clone(),
            });
            let Ok(bound) = bound else { continue };
            if let Judged::Ended {
                ending,
                result,
                spent,
            } = self.judge(&person.run, &bound, &file.inputs)?
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
    fn judge(
        &self,
        run: &str,
        bound: &Bound,
        inputs: &Map<String, Value>,
    ) -> Result<Judged, LoopError> {
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
                let sealed = self.restore(run, bound, inputs)?;
                let output = serde_json::to_value(&sealed.output)
                    .map_err(|error| LoopError::json("a run's output", &error))?;
                let reason = output
                    .get("seal_reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
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
                let result = output.get("result").cloned().unwrap_or(Value::Null);
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
    fn drive_on(
        &self,
        run: &str,
        bound: &Bound,
        inputs: &Map<String, Value>,
        waiting: &Waiting,
    ) -> Result<(), LoopError> {
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
                &self.request(bound, run, inputs, Vec::new(), None)?,
                Some(&bound.method),
                None,
                None,
            ),
        };
        result
            .map(|_| ())
            .map_err(|error| LoopError::Runtime(error.to_string()))
    }

    /// A sealed run's result, from its record.
    fn restore(
        &self,
        run: &str,
        bound: &Bound,
        inputs: &Map<String, Value>,
    ) -> Result<RunResult, LoopError> {
        self.orchestrator
            .run_skill_with_binding(
                &self.request(bound, run, inputs, Vec::new(), None)?,
                Some(&bound.method),
                None,
                None,
            )
            .map_err(|error| LoopError::Runtime(error.to_string()))
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

    /// The request that starts run `run` of a loop's skill, or continues it.
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
        let state = self.read(name)?;
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
        inputs.extend(given);
        if let Some(live) = state.people.get(method) {
            // The live one, opened where it waits.
            return self
                .orchestrator
                .run_skill_with_binding(
                    &self.request(&bound, &live.run, &inputs, Vec::new(), None)?,
                    Some(method),
                    None,
                    None,
                )
                .map_err(|error| LoopError::Runtime(error.to_string()));
        }
        let day = file.day_zone().day_of(now);
        let reserved = state.left_today(file.policy.spend.per_day_micro_usd(), &day);
        let authority = self
            .authority(&file, reserved, now)
            .map_err(LoopError::Refused)?;
        let run = runx_runtime::run_record::mint_run_id(&file.calls.skill)
            .map_err(|error| LoopError::Runtime(error.to_string()))?;
        let revision = match &state.active {
            Some(active) => active.clone(),
            None => {
                // A loop that isn't ready keeps its file's revision first.
                let bytes = self
                    .written(name)?
                    .ok_or_else(|| LoopError::Unknown(name.to_string()))?
                    .bytes;
                let revision = digest(bytes.as_bytes());
                self.commit(
                    name,
                    &state,
                    &[RecordEvent::Revision {
                        digest: revision.clone(),
                        bytes,
                        at: stamp(now),
                    }],
                    None,
                )?;
                revision
            }
        };
        let state = self.read(name)?;
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
        let current_context = vec![context::wake(
            name.as_str(),
            &file.brief,
            state.active.as_deref().unwrap_or_default(),
            &now_text,
            state.last_answered_at.as_deref(),
            None,
        )?];
        let request = self.request(&bound, &run, &inputs, current_context, authority)?;
        let claim = HostEvents {
            host: HostName::new(crate::records::HOST)?,
            appends: Self::record_append(name, &state, &[ran])?
                .into_iter()
                .collect(),
        };
        let scope = StreamScope::new(&bound.owner, name.as_str()).map_err(LoopError::Invalid)?;
        let result = self
            .orchestrator
            .run_claimed(&request, Some(method), (claim, Some(scope)))
            .map_err(|error| LoopError::Runtime(error.to_string()))?;
        let mut state = self.read(name)?;
        let mut passed = Passed::default();
        self.settle_people(name, &mut state, now, &mut passed)?;
        Ok(result)
    }

    /// The package a loop's `calls` names, as a run resolves it.
    pub(crate) fn skill_path(&self, calls: &Calls) -> Result<PathBuf, String> {
        self.bind(calls).map(|bound| bound.skill_path)
    }
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
