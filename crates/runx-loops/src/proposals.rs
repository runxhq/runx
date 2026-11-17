//! What a run proposes to its loop (§8), checked before it's recorded, and a
//! person's answer to a change or a new loop. A run never changes its loop:
//! only a checked proposal reaches the record, and only a person's accept
//! writes the loop's file.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::file::{Calls, LoopFile, LoopName, digest};
use crate::records::{Action, Proposal, RecordEvent, Writes};
use crate::{LoopError, Loops};

/// A method's `loop` output, `runx/loop.result.v1`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoopResult {
    #[serde(default = "yes")]
    changed: bool,
    #[serde(default)]
    wake: Option<Wake>,
    #[serde(default)]
    notice: Option<Notice>,
    #[serde(default)]
    done: Option<Why>,
    #[serde(default)]
    change: Option<Change>,
    #[serde(default)]
    new_loop: Option<NewLoop>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wake {
    at: String,
    why: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Notice {
    text: String,
    #[serde(default)]
    action: Option<Action>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Why {
    why: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    #[serde(default)]
    brief: Option<String>,
    #[serde(default)]
    inputs: Option<Map<String, Value>>,
    why: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NewLoop {
    calls: String,
    name: String,
    brief: String,
    #[serde(default)]
    inputs: Map<String, Value>,
    #[serde(default)]
    on: Vec<Value>,
    why: String,
}

const fn yes() -> bool {
    true
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The `loop` output a run's result holds, a packet's envelope or plain.
fn loop_output(result: &Value) -> Option<&Value> {
    let value = result.get("loop")?;
    Some(match value.get("data") {
        Some(data) if value.get("schema").is_some() => data,
        _ => value,
    })
}

/// What a run's result says to its loop: whether it changed anything, and
/// each proposal, checked and recorded.
pub(crate) struct Checked {
    pub changed: bool,
    pub events: Vec<RecordEvent>,
}

/// Each proposal `run`'s result holds, checked. A run that declares no loop
/// output proposes nothing, and is taken to have changed something.
pub(crate) fn checked(
    loops: &Loops,
    run: &str,
    file: &LoopFile,
    result: &Value,
    now: DateTime<Utc>,
) -> Result<Checked, LoopError> {
    let Some(output) = loop_output(result) else {
        return Ok(Checked {
            changed: true,
            events: Vec::new(),
        });
    };
    let proposed =
        |kind: &str, proposal: Proposal, refused: Option<String>| RecordEvent::Proposed {
            run: run.to_owned(),
            id: format!("{run}/{kind}"),
            proposal,
            refused,
            at: stamp(now),
        };
    let parsed: LoopResult = match serde_json::from_value(output.clone()) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Ok(Checked {
                changed: true,
                events: vec![proposed(
                    "result",
                    Proposal::Notice {
                        text: format!("Its loop output isn't runx/loop.result.v1: {error}"),
                        action: None,
                    },
                    Some(format!("the loop output doesn't validate: {error}")),
                )],
            });
        }
    };
    let mut events = Vec::new();
    if let Some(wake) = parsed.wake {
        // The loop's min_interval paces its wakes: one asked for sooner is
        // the soonest it allows. Only one past its horizon is refused.
        let earliest = now + file.policy.min_interval.0;
        let parsed_at = DateTime::parse_from_rfc3339(&wake.at).map(|at| at.with_timezone(&Utc));
        let (at, refused) = match parsed_at {
            Err(_) => {
                let refused = format!("{} isn't a time with its offset", wake.at);
                (wake.at, Some(refused))
            }
            Ok(at) if at > now + file.policy.horizon.0 => (
                wake.at,
                Some(format!(
                    "it's further than the loop's horizon of {}",
                    file.policy.horizon
                )),
            ),
            Ok(at) => (
                at.max(earliest)
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                None,
            ),
        };
        events.push(proposed(
            "wake",
            Proposal::Wake { at, why: wake.why },
            refused,
        ));
    }
    if let Some(notice) = parsed.notice {
        let refused = match &notice.action {
            Some(action) => check_action(loops, file, action).err(),
            None => None,
        };
        events.push(proposed(
            "notice",
            Proposal::Notice {
                text: notice.text,
                action: notice.action,
            },
            refused,
        ));
    }
    if let Some(change) = parsed.change {
        let refused = check_change(loops, file, &change).err();
        events.push(proposed(
            "change",
            Proposal::Change {
                brief: change.brief,
                inputs: change.inputs,
                why: change.why,
            },
            refused,
        ));
    }
    if let Some(new) = parsed.new_loop {
        let refused = check_new_loop(loops, &new).err();
        events.push(proposed(
            "new_loop",
            Proposal::NewLoop {
                calls: new.calls,
                name: new.name,
                brief: new.brief,
                inputs: new.inputs,
                on: new.on,
                why: new.why,
            },
            refused,
        ));
    }
    if let Some(done) = parsed.done {
        // Stopping only narrows, so it takes effect at once.
        events.push(proposed(
            "done",
            Proposal::Done {
                why: done.why.clone(),
            },
            None,
        ));
        events.push(RecordEvent::Stopped {
            by: format!("run {run}"),
            why: done.why,
            at: stamp(now),
        });
    }
    Ok(Checked {
        changed: parsed.changed,
        events,
    })
}

fn object(inputs: &Map<String, Value>) -> Result<runx_contracts::JsonObject, String> {
    match serde_json::from_value(Value::Object(inputs.clone())) {
        Ok(runx_contracts::JsonValue::Object(object)) => Ok(object),
        _ => Err("inputs aren't a map".to_owned()),
    }
}

/// A notice's action is a method of the loop's skill, taking the loop's
/// inputs it declares with the action's over them.
fn check_action(loops: &Loops, file: &LoopFile, action: &Action) -> Result<(), String> {
    let calls = Calls {
        skill: file.calls.skill.clone(),
        method: action.method.clone(),
    };
    let skill = loops.skill_path(&calls)?;
    let declared = crate::pass::declared_inputs(&skill, &calls.method)?;
    let mut inputs = file
        .inputs
        .iter()
        .filter(|(key, _)| declared.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    inputs.extend(action.inputs.clone());
    runx_runtime::check_method_inputs(&skill, &calls.method, &object(&inputs)?)
        .map_err(|why| format!("its action's inputs don't validate: {why}"))
}

/// A change touches only the brief and the inputs, and the inputs it gives
/// are what the loop's method takes.
fn check_change(loops: &Loops, file: &LoopFile, change: &Change) -> Result<(), String> {
    if change
        .brief
        .as_deref()
        .is_some_and(|brief| brief.trim().is_empty())
    {
        return Err("its brief is empty".to_owned());
    }
    if change.brief.is_none() && change.inputs.is_none() {
        return Err("it changes nothing".to_owned());
    }
    if let Some(inputs) = &change.inputs {
        let skill = loops.skill_path(&file.calls)?;
        runx_runtime::check_method_inputs(&skill, &file.calls.method, &object(inputs)?)
            .map_err(|why| format!("its inputs don't validate: {why}"))?;
    }
    Ok(())
}

/// A new loop's file validates, names no authority or policy, and takes a
/// name no loop here has.
fn check_new_loop(loops: &Loops, new: &NewLoop) -> Result<(), String> {
    let name = LoopName::new(&new.name).map_err(|error| error.to_string())?;
    if name.path(loops.cwd()).exists()
        || loops
            .state(&name)
            .map_err(|error| error.to_string())?
            .version
            > 0
    {
        return Err(format!("a loop named {name} exists"));
    }
    let file = LoopFile::parse(&new_loop_bytes(new, None)?).map_err(|error| error.to_string())?;
    let skill = loops.skill_path(&file.calls)?;
    runx_runtime::check_method_inputs(&skill, &file.calls.method, &object(&file.inputs)?)
        .map_err(|why| format!("its inputs don't validate: {why}"))
}

fn new_loop_bytes(new: &NewLoop, authority: Option<&str>) -> Result<Vec<u8>, String> {
    let mut written = Map::new();
    written.insert("calls".to_owned(), Value::String(new.calls.clone()));
    written.insert("brief".to_owned(), Value::String(new.brief.clone()));
    if !new.inputs.is_empty() {
        written.insert("inputs".to_owned(), Value::Object(new.inputs.clone()));
    }
    if !new.on.is_empty() {
        written.insert("on".to_owned(), Value::Array(new.on.clone()));
    }
    if let Some(authority) = authority {
        written.insert("authority".to_owned(), Value::String(authority.to_owned()));
    }
    serde_norway::to_string(&Value::Object(written))
        .map(String::into_bytes)
        .map_err(|error| error.to_string())
}

impl Loops {
    /// A person accepts proposal `id` of loop `name`. A change writes the
    /// loop's file as its next revision; a new loop writes its file, under
    /// the `authority` the person chooses, never started.
    pub fn accept(
        &self,
        name: &LoopName,
        id: &str,
        by: &str,
        authority: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), LoopError> {
        let state = self.read(name)?;
        let pending =
            state.proposals.get(id).cloned().ok_or_else(|| {
                LoopError::Refused(format!("loop {name} has no open proposal {id}"))
            })?;
        // The answer is committed first, with the file it writes; the file is
        // written after, and a pass writes any the process stopped before.
        let (writes, revision) = match pending.proposal {
            Proposal::Change { brief, inputs, .. } => {
                let path = name.path(self.cwd());
                let bytes = std::fs::read_to_string(&path)
                    .map_err(|error| LoopError::io(format!("reading {}", path.display()), error))?;
                let base = digest(bytes.as_bytes());
                // A change was proposed against the file the loop runs; one
                // edited since isn't what was proposed against.
                if state.active.as_deref() != Some(base.as_str()) {
                    return Err(LoopError::Refused(format!(
                        "loop {name}'s file changed since proposal {id} was made: decline it, and its next run proposes against the file as it is"
                    )));
                }
                let mut written: Value = serde_norway::from_str(&bytes)
                    .map_err(|error| LoopError::Invalid(format!("the loop file: {error}")))?;
                let Some(map) = written.as_object_mut() else {
                    return Err(LoopError::Invalid("the loop file isn't a map".to_owned()));
                };
                if let Some(brief) = brief {
                    map.insert("brief".to_owned(), Value::String(brief));
                }
                if let Some(inputs) = inputs {
                    map.insert("inputs".to_owned(), Value::Object(inputs));
                }
                let next = serde_norway::to_string(&written)
                    .map_err(|error| LoopError::Invalid(error.to_string()))?;
                LoopFile::parse(next.as_bytes())?;
                let revision = RecordEvent::Revision {
                    digest: digest(next.as_bytes()),
                    bytes: next.clone(),
                    at: stamp(now),
                };
                (
                    Writes {
                        target: None,
                        bytes: next,
                        base: Some(base),
                    },
                    Some(revision),
                )
            }
            Proposal::NewLoop {
                calls,
                name: new_name,
                brief,
                inputs,
                on,
                why,
            } => {
                let new = NewLoop {
                    calls,
                    name: new_name,
                    brief,
                    inputs,
                    on,
                    why,
                };
                check_new_loop(self, &new).map_err(LoopError::Refused)?;
                LoopName::new(&new.name)?;
                let bytes =
                    String::from_utf8(new_loop_bytes(&new, authority).map_err(LoopError::Invalid)?)
                        .map_err(|error| {
                            LoopError::Invalid(format!("the new loop's file: {error}"))
                        })?;
                (
                    Writes {
                        target: Some(new.name),
                        bytes,
                        base: None,
                    },
                    None,
                )
            }
            _ => {
                return Err(LoopError::Refused(format!("proposal {id} takes no answer")));
            }
        };
        let mut record = vec![RecordEvent::Answered {
            id: id.to_owned(),
            accepted: true,
            by: by.to_owned(),
            words: None,
            writes: Some(writes),
            at: stamp(now),
        }];
        record.extend(revision);
        self.commit(name, &state, &record, None)?;
        self.write_accepted(name, now)
    }

    /// Write each file an accepted proposal writes that isn't on disk yet,
    /// only over the file it was accepted against, or where a new loop's
    /// isn't, and mark each written. A file the person edited since stands.
    pub(crate) fn write_accepted(
        &self,
        name: &LoopName,
        now: DateTime<Utc>,
    ) -> Result<(), LoopError> {
        let state = self.read(name)?;
        if state.unwritten.is_empty() {
            return Ok(());
        }
        let mut record = Vec::new();
        for writes in &state.unwritten {
            let target = match &writes.target {
                Some(other) => LoopName::new(other)?,
                None => name.clone(),
            };
            let path = target.path(self.cwd());
            let current = match std::fs::read(&path) {
                Ok(bytes) => Some(digest(&bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(LoopError::io(format!("reading {}", path.display()), error));
                }
            };
            let over = match &writes.base {
                Some(base) => current.as_deref() == Some(base.as_str()),
                None => current.is_none(),
            };
            if over {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| LoopError::io("making loops/", error))?;
                }
                std::fs::write(&path, &writes.bytes)
                    .map_err(|error| LoopError::io(format!("writing {}", path.display()), error))?;
            }
            record.push(RecordEvent::Written {
                target: writes.target.clone(),
                at: stamp(now),
            });
        }
        self.commit(name, &state, &record, None)?;
        Ok(())
    }

    /// A person declines proposal `id` of loop `name`, in their words, which
    /// the record keeps.
    pub fn decline(
        &self,
        name: &LoopName,
        id: &str,
        by: &str,
        words: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), LoopError> {
        let state = self.read(name)?;
        if !state.proposals.contains_key(id) {
            return Err(LoopError::Refused(format!(
                "loop {name} has no open proposal {id}"
            )));
        }
        self.commit(
            name,
            &state,
            &[RecordEvent::Answered {
                id: id.to_owned(),
                accepted: false,
                by: by.to_owned(),
                words: words.map(str::to_owned),
                writes: None,
                at: stamp(now),
            }],
            None,
        )?;
        Ok(())
    }
}
