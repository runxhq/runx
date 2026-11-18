//! The loop commands (the loop contract §16): `runx loops`, `runx loop
//! new|show|accept|decline|retry|skip|message|token`, and `runx start|stop|run`.
//! Each acts as the person at this terminal, then rings the always-on runx
//! so it acts on the change at once, and prints the loops afterwards.

use std::ffi::OsString;
use std::process::ExitCode;

use chrono::{DateTime, Local, Utc};
use runx_loops::{Ending, LoopFile, LoopName, LoopState, Loops, Status, Waiting};
use runx_runtime::WorkspaceEnv;
use serde_json::{Map, Value};

use crate::cli_args::{os_arg, split_flag};

const USAGE: &str = "runx loops | runx loop new <skill> <name> | runx loop show|retry|skip <loop> | runx loop accept|decline <loop> <proposal> | runx loop message <loop> <text> | runx loop token <loop> [webhook] | runx start|stop <loop> | runx run <loop> [method]";

#[derive(Clone, Debug, PartialEq)]
pub struct LoopsPlan {
    pub action: LoopAction,
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoopAction {
    List,
    New {
        skill: String,
        name: String,
    },
    Show {
        name: String,
    },
    Start {
        name: String,
    },
    Stop {
        name: String,
        why: Option<String>,
    },
    /// "Run now" with no method; a person's run of `method` with one.
    Run {
        name: String,
        method: Option<String>,
        inputs: Map<String, Value>,
    },
    Retry {
        name: String,
    },
    Skip {
        name: String,
    },
    Message {
        name: String,
        text: String,
    },
    /// A new token for one of the loop's webhooks, in place of the one before.
    Token {
        name: String,
        webhook: Option<String>,
    },
    Accept {
        name: String,
        proposal: String,
        authority: Option<String>,
    },
    Decline {
        name: String,
        proposal: String,
        why: Option<String>,
    },
}

/// Parse `runx loops`, `runx loop …`, `runx start|stop|run …`.
pub fn parse_loops_plan(args: &[OsString]) -> Result<LoopsPlan, String> {
    let mut words = Vec::new();
    let mut json = false;
    let mut why = None;
    let mut authority = None;
    let mut inputs = Map::new();
    let mut index = 0;
    while index < args.len() {
        let token = os_arg(args, index, "loop")?;
        let (flag, inline) = split_flag(token);
        let mut value = |name: &str| -> Result<String, String> {
            if let Some(inline) = inline {
                return Ok(inline.to_owned());
            }
            index += 1;
            os_arg(args, index, "loop")
                .map(str::to_owned)
                .map_err(|_| format!("{name} takes a value"))
        };
        match flag {
            "--json" | "-j" if inline.is_none() => json = true,
            "--why" => why = Some(value("--why")?),
            "--authority" => authority = Some(value("--authority")?),
            "--inputs" => {
                let given: Value = serde_json::from_str(&value("--inputs")?)
                    .map_err(|error| format!("--inputs takes a JSON object: {error}"))?;
                let Value::Object(given) = given else {
                    return Err("--inputs takes a JSON object".to_owned());
                };
                inputs.extend(given);
            }
            _ if token.starts_with("--") => return Err(format!("unknown loop argument {token}")),
            _ => words.push(token.to_owned()),
        }
        index += 1;
    }
    let words = words.iter().map(String::as_str).collect::<Vec<_>>();
    let owned = |text: &str| text.to_owned();
    let action = match words.as_slice() {
        ["loops"] => LoopAction::List,
        ["loop", "new", skill, name] => LoopAction::New {
            skill: owned(skill),
            name: owned(name),
        },
        ["loop", "show", name] => LoopAction::Show { name: owned(name) },
        ["loop", "retry", name] => LoopAction::Retry { name: owned(name) },
        ["loop", "skip", name] => LoopAction::Skip { name: owned(name) },
        ["loop", "message", name, text @ ..] if !text.is_empty() => LoopAction::Message {
            name: owned(name),
            text: text.join(" "),
        },
        ["loop", "token", name] => LoopAction::Token {
            name: owned(name),
            webhook: None,
        },
        ["loop", "token", name, webhook] => LoopAction::Token {
            name: owned(name),
            webhook: Some(owned(webhook)),
        },
        ["loop", "accept", name, proposal] => LoopAction::Accept {
            name: owned(name),
            proposal: owned(proposal),
            authority,
        },
        ["loop", "decline", name, proposal] => LoopAction::Decline {
            name: owned(name),
            proposal: owned(proposal),
            why,
        },
        ["start", name] => LoopAction::Start { name: owned(name) },
        ["stop", name] => LoopAction::Stop {
            name: owned(name),
            why,
        },
        ["run", name] => LoopAction::Run {
            name: owned(name),
            method: None,
            inputs,
        },
        ["run", name, method] => LoopAction::Run {
            name: owned(name),
            method: Some(owned(method)),
            inputs,
        },
        _ => return Err(format!("usage: {USAGE}")),
    };
    Ok(LoopsPlan { action, json })
}

/// Who acts from this terminal: the person, by their account's name.
pub(crate) fn person(workspace: &WorkspaceEnv) -> String {
    let user = workspace
        .env()
        .get("USER")
        .or_else(|| workspace.env().get("USERNAME"))
        .map_or("local", String::as_str);
    format!("person:{user}")
}

pub(crate) fn open(workspace: &WorkspaceEnv) -> Result<Loops, String> {
    let orchestrator =
        crate::runtime::local_orchestrator(workspace.env()).map_err(|error| error.to_string())?;
    Loops::open(workspace.env().clone(), workspace.cwd(), orchestrator)
        .map_err(|error| error.to_string())
}

pub fn run_native_loops(plan: LoopsPlan, workspace: &WorkspaceEnv) -> ExitCode {
    match execute(&plan, workspace) {
        Ok(Done::Text(output)) => crate::cli_io::write_stdout_code(&output, 0),
        Ok(Done::Run(result)) => crate::skill::write_run_result(
            &result,
            crate::skill::RunOutput {
                json: plan.json,
                diagnostics: false,
                receipt_dir: None,
                answers_path: None,
                artifacts: &workspace.paths().artifacts(),
            },
        ),
        Err(error) if plan.json => crate::cli_io::write_stdout_code(
            &crate::cli_error::json_failure_output(&error, "loop_failed"),
            1,
        ),
        Err(error) => crate::cli_io::write_stderr_code(&format!("runx: {error}\n"), 1),
    }
}

enum Done {
    Text(String),
    Run(Box<runx_runtime::RunResult>),
}

fn name(text: &str) -> Result<LoopName, String> {
    LoopName::new(text).map_err(|error| error.to_string())
}

fn execute(plan: &LoopsPlan, workspace: &WorkspaceEnv) -> Result<Done, String> {
    let loops = open(workspace)?;
    let by = person(workspace);
    let now = Utc::now();
    let fail = |error: runx_loops::LoopError| error.to_string();
    let said = match &plan.action {
        LoopAction::List => None,
        LoopAction::New {
            skill,
            name: loop_name,
        } => {
            let path = new_loop(&loops, workspace, skill, &name(loop_name)?)?;
            Some(format!(
                "made {}: fill in its inputs, then `runx start {loop_name}`\n",
                path.display()
            ))
        }
        LoopAction::Show { name: loop_name } => {
            let loop_name = name(loop_name)?;
            let state = loops.state(&loop_name).map_err(fail)?;
            return Ok(Done::Text(if plan.json {
                show_json(&loops, &loop_name, &state)?
            } else {
                show(&loops, &loop_name, &state)?
            }));
        }
        LoopAction::Start { name: loop_name } => {
            let loop_name = name(loop_name)?;
            loops.start(&loop_name, &by, now).map_err(fail)?;
            crate::always_on::serve_workspace(workspace)?;
            Some(format!("started {loop_name}\n"))
        }
        LoopAction::Stop {
            name: loop_name,
            why,
        } => {
            let loop_name = name(loop_name)?;
            loops
                .stop(
                    &loop_name,
                    &by,
                    why.as_deref().unwrap_or("the person stopped it"),
                    now,
                )
                .map_err(fail)?;
            Some(format!("stopped {loop_name}\n"))
        }
        LoopAction::Run {
            name: loop_name,
            method: None,
            ..
        } => {
            let loop_name = name(loop_name)?;
            loops.run_now(&loop_name, &by).map_err(fail)?;
            Some(format!("{loop_name} wakes now\n"))
        }
        LoopAction::Run {
            name: loop_name,
            method: Some(method),
            inputs,
        } => {
            let loop_name = name(loop_name)?;
            let result = loops
                .person_run(&loop_name, method, inputs.clone(), &by, now)
                .map_err(fail)?;
            crate::always_on::ring(workspace);
            return Ok(Done::Run(Box::new(result)));
        }
        LoopAction::Retry { name: loop_name } => {
            let loop_name = name(loop_name)?;
            loops.retry(&loop_name, &by, now).map_err(fail)?;
            Some(format!("{loop_name} tries its batch again\n"))
        }
        LoopAction::Skip { name: loop_name } => {
            let loop_name = name(loop_name)?;
            loops.skip(&loop_name, &by, now).map_err(fail)?;
            Some(format!("{loop_name} moved past its failed batch\n"))
        }
        LoopAction::Message {
            name: loop_name,
            text,
        } => {
            let loop_name = name(loop_name)?;
            loops.message(&loop_name, &by, text).map_err(fail)?;
            Some(format!("told {loop_name}\n"))
        }
        LoopAction::Token {
            name: loop_name,
            webhook,
        } => {
            let loop_name = name(loop_name)?;
            return token(&loops, workspace, &loop_name, webhook.as_deref(), plan.json)
                .map(Done::Text);
        }
        LoopAction::Accept {
            name: loop_name,
            proposal,
            authority,
        } => {
            let loop_name = name(loop_name)?;
            loops
                .accept(&loop_name, proposal, &by, authority.as_deref(), now)
                .map_err(fail)?;
            Some(format!("accepted {proposal}\n"))
        }
        LoopAction::Decline {
            name: loop_name,
            proposal,
            why,
        } => {
            let loop_name = name(loop_name)?;
            loops
                .decline(&loop_name, proposal, &by, why.as_deref(), now)
                .map_err(fail)?;
            Some(format!("declined {proposal}\n"))
        }
    };
    crate::always_on::ring(workspace);
    let listing = if plan.json {
        list_json(&loops)?
    } else {
        list(&loops, workspace)?
    };
    Ok(Done::Text(match said {
        Some(said) if !plan.json => format!("{said}\n{listing}"),
        _ => listing,
    }))
}

/// A new token for loop `name`'s webhook, kept by its digest alone, and the
/// address that carries it, shown this once.
fn token(
    loops: &Loops,
    workspace: &WorkspaceEnv,
    name: &LoopName,
    webhook: Option<&str>,
    json: bool,
) -> Result<String, String> {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0_u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "the system gave no randomness for a token".to_owned())?;
    let token = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let webhook = loops
        .rotate_token(name, webhook, &runx_loops::digest(token.as_bytes()))
        .map_err(|error| error.to_string())?;
    let path = format!("/webhook/{token}");
    let address = crate::always_on::address(workspace).map(|base| format!("{base}{path}"));
    if json {
        return serde_json::to_string_pretty(&serde_json::json!({
            "loop": name.as_str(),
            "webhook": webhook,
            "path": path,
            "address": address,
        }))
        .map(|text| text + "\n")
        .map_err(|error| error.to_string());
    }
    let at = address.unwrap_or_else(|| {
        format!(
            "http://127.0.0.1:<port>{path}, at the port runx listens on once it's on (`runx on`)"
        )
    });
    Ok(format!(
        "{name}'s webhook {webhook} takes a POST of a JSON payload at\n  {at}\nThe token is shown this once: `runx loop token {name} {webhook}` makes a new one, and the address before stops working. A sender on another machine needs a public HTTPS address in front of this one.\n"
    ))
}

/// Copy the skill's loop template to `loops/<name>.yaml`.
fn new_loop(
    loops: &Loops,
    workspace: &WorkspaceEnv,
    skill: &str,
    name: &LoopName,
) -> Result<std::path::PathBuf, String> {
    let target = name.path(loops.cwd());
    if target.exists() {
        return Err(format!("{} exists", target.display()));
    }
    let resolved = runx_runtime::registry::resolve::resolve_skill_ref_details(
        std::path::Path::new(skill),
        workspace.cwd(),
        runx_runtime::registry::resolve::SkillResolverOptions {
            env: workspace.env(),
            registry: None,
            expected_digest: None,
            absent_pins: runx_runtime::registry::resolve::AbsentPins::Fail,
            holder: runx_runtime::registry::resolve::Holder::Project,
        },
    )?;
    let loaded = runx_runtime::load_validated_skill_package(&resolved.runnable_path)
        .map_err(|error| error.to_string())?;
    let package = &loaded.package;
    let bytes = package
        .loop_template
        .as_deref()
        .and_then(|path| package.file_bytes(path))
        .ok_or_else(|| format!("{skill} ships no loop template"))?;
    LoopFile::parse(bytes)
        .map_err(|error| format!("{skill}'s loop template isn't a loop: {error}"))?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("making loops/: {error}"))?;
    }
    runx_runtime::durable::replace_file(&target, bytes)
        .map_err(|error| format!("writing {}: {error}", target.display()))?;
    Ok(target)
}

fn ago(at: &str, now: DateTime<Utc>) -> String {
    let Ok(at) = DateTime::parse_from_rfc3339(at) else {
        return at.to_owned();
    };
    let seconds = (now - at.with_timezone(&Utc)).num_seconds().max(0);
    match seconds {
        0..60 => format!("{seconds}s ago"),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

fn when(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%a %H:%M").to_string()
}

fn status(state: &LoopState) -> String {
    match &state.status {
        Status::New => "not started".to_owned(),
        Status::Running => "running".to_owned(),
        Status::Paused { why } => format!("paused: {why}"),
        Status::Stopped { why, .. } => format!("stopped: {why}"),
    }
}

fn ending(ending: &Ending) -> String {
    match ending {
        Ending::Answered { outcome } => outcome.clone(),
        Ending::Closed { outcome } => format!("closed by the person ({outcome})"),
        Ending::Refused { reason, .. } => format!("refused: {reason}"),
        Ending::Failed { why, .. } => {
            format!("failed: {}", why.as_deref().unwrap_or("no reason given"))
        }
        Ending::FailedAfterChange { why, changed, .. } => format!(
            "failed after {changed} change(s): {}",
            why.as_deref().unwrap_or("no reason given")
        ),
        Ending::Unavailable { why, .. } => format!(
            "couldn't run, its model unavailable: {}",
            why.as_deref().unwrap_or("no reason given")
        ),
    }
}

/// Every loop here, a few lines each: what it calls, its state, its last
/// run, its next wake, and what waits on the person.
fn list(loops: &Loops, workspace: &WorkspaceEnv) -> Result<String, String> {
    let names = loops.names().map_err(|error| error.to_string())?;
    if names.is_empty() {
        return Ok("no loops here: `runx loop new <skill> <name>` makes one\n".to_owned());
    }
    let now = Utc::now();
    let mut out = String::new();
    for name in names {
        let state = loops.state(&name).map_err(|error| error.to_string())?;
        let calls = loops.file(&name).ok().flatten().map_or_else(
            || "(its file doesn't validate)".to_owned(),
            |file| file.calls.to_string(),
        );
        out.push_str(&format!("{name}  {calls}  {}\n", status(&state)));
        if let Some(live) = live_line(loops, &state, now)? {
            out.push_str(&format!("  {live}\n"));
        }
        if let Some(last) = &state.last {
            out.push_str(&format!(
                "  last: {} {}\n",
                ending(&last.ending),
                ago(&last.at, now)
            ));
        }
        if let Some(why) = &state.couldnt {
            out.push_str(&format!("  couldn't run: {why}\n"));
        }
        if let Some(retry) = state.retry_at {
            out.push_str(&format!("  tries again: {}\n", when(retry)));
        }
        if let Some(next) = loops.next(&name, now).map_err(|error| error.to_string())? {
            out.push_str(&format!("  next due: {}\n", when(next)));
        }
        for person in state.people.values() {
            out.push_str(&format!(
                "  {} is open: {} (`runx resume {}`)\n",
                person.method, person.run, person.run
            ));
        }
        if let Some(notice) = state.notices.last() {
            let action = notice
                .action
                .as_ref()
                .map(|action| format!(" -> `runx run {name} {}`", action.method))
                .unwrap_or_default();
            out.push_str(&format!("  told you: {}{action}\n", notice.text));
        }
        for pending in state.proposals.values() {
            let answer = if state.outdated(pending) {
                format!(
                    "made against a revision the loop no longer runs: `runx loop decline {name} {}`",
                    pending.id
                )
            } else {
                format!("`runx loop accept {name} {}`", pending.id)
            };
            out.push_str(&format!(
                "  proposes: {} ({answer})\n",
                proposal_line(&pending.proposal)
            ));
        }
    }
    let serving = crate::always_on::status_line(workspace);
    out.push_str(&format!("\n{serving}\n"));
    Ok(out)
}

fn proposal_line(proposal: &runx_loops::Proposal) -> String {
    match proposal {
        runx_loops::Proposal::Change { why, .. } => format!("a change: {why}"),
        runx_loops::Proposal::NewLoop { name, why, .. } => format!("a new loop {name}: {why}"),
        runx_loops::Proposal::Wake { why, .. } => format!("a wake: {why}"),
        runx_loops::Proposal::Notice { text, .. } => text.clone(),
        runx_loops::Proposal::Done { why } => format!("done: {why}"),
    }
}

/// One loop in full: its state, its notices and what it proposes.
fn show(loops: &Loops, name: &LoopName, state: &LoopState) -> Result<String, String> {
    let now = Utc::now();
    let mut out = format!("{name}: {}\n", status(state));
    if let Some(file) = state.active_file().and_then(Result::ok) {
        out.push_str(&format!("calls {}\nbrief: {}\n", file.calls, file.brief));
        for trigger in &file.on {
            out.push_str(&format!(
                "wakes on {trigger}{}\n",
                taken(name, trigger, state)
            ));
        }
    }
    out.push_str(&format!(
        "answered for its inbox up to {}\n",
        state.position
    ));
    if let Some(live) = live_line(loops, state, now)? {
        out.push_str(&format!("{live}\n"));
    }
    if let Some(pending) = &state.pending {
        out.push_str(&format!(
            "a revision that widens it waits for a start: {pending}\n"
        ));
    }
    if let Some(last) = &state.last {
        out.push_str(&format!(
            "last run {}: {} {}\n",
            last.run,
            ending(&last.ending),
            ago(&last.at, now)
        ));
    }
    for notice in &state.notices {
        out.push_str(&format!(
            "notice {} {}: {}\n",
            notice.id,
            ago(&notice.at, now),
            notice.text
        ));
    }
    for pending in state.proposals.values() {
        out.push_str(&format!(
            "proposal {}{}: {}\n",
            pending.id,
            if state.outdated(pending) {
                ", made against a revision the loop no longer runs, so only declined"
            } else {
                ""
            },
            proposal_line(&pending.proposal)
        ));
        if let Ok(detail) = serde_json::to_string_pretty(&pending.proposal) {
            out.push_str(&format!("{detail}\n"));
        }
    }
    Ok(out)
}

/// How far a trigger's events are taken: a feed's cursor, or whether a
/// webhook has an address yet.
fn taken(name: &LoopName, trigger: &runx_loops::Trigger, state: &LoopState) -> String {
    match trigger {
        runx_loops::Trigger::Feed { connector, feed } => state
            .cursors
            .get(&format!("{connector}/{feed}"))
            .map_or_else(
                || ", read from its start".to_owned(),
                |cursor| format!(", read up to {cursor}"),
            ),
        runx_loops::Trigger::Webhook { name: webhook, .. }
            if !state.tokens.contains_key(webhook) =>
        {
            format!(", which has no address yet: `runx loop token {name} {webhook}` gives one")
        }
        _ => String::new(),
    }
}

/// What the loop's live wake waits on, or `None` when it has none, or its
/// run ended and no pass has settled it yet.
fn live_waiting(loops: &Loops, state: &LoopState) -> Result<Option<Waiting>, String> {
    state.live.as_ref().map_or(Ok(None), |live| {
        loops.waiting(&live.run).map_err(|error| error.to_string())
    })
}

/// The loop's live wake as the person reads it: one waiting on them says so,
/// and how to answer it.
fn live_line(
    loops: &Loops,
    state: &LoopState,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    let Some(live) = &state.live else {
        return Ok(None);
    };
    let since = ago(&live.at, now);
    Ok(Some(match live_waiting(loops, state)? {
        Some(Waiting::Person) => format!(
            "waits on you since {since}: {} (`runx resume {}`)",
            live.run, live.run
        ),
        Some(Waiting::Until(at)) => format!("waits until {}: {}", when(at), live.run),
        Some(Waiting::Reconciling) => format!(
            "waits while runx finds out how an effect ended: {}",
            live.run
        ),
        _ => format!("working: {} since {since}", live.run),
    }))
}

fn show_json(loops: &Loops, name: &LoopName, state: &LoopState) -> Result<String, String> {
    serde_json::to_string_pretty(&view(loops, name, state)?).map_err(|error| error.to_string())
}

fn view(loops: &Loops, name: &LoopName, state: &LoopState) -> Result<Value, String> {
    // The live wake, and what it waits on while it waits: the person, a
    // time, or how an effect ended.
    let waiting = live_waiting(loops, state)?;
    let live = state.live.as_ref().map(|live| {
        let mut view = Map::new();
        view.insert("run".to_owned(), Value::from(live.run.clone()));
        view.insert("since".to_owned(), Value::from(live.at.clone()));
        let waits_on = match &waiting {
            Some(Waiting::Person) => Some("person"),
            Some(Waiting::Until(at)) => {
                view.insert("until".to_owned(), Value::from(at.to_rfc3339()));
                Some("time")
            }
            Some(Waiting::Reconciling) => Some("reconciliation"),
            _ => None,
        };
        if let Some(waits_on) = waits_on {
            view.insert("waits_on".to_owned(), Value::from(waits_on));
        }
        Value::Object(view)
    });
    Ok(serde_json::json!({
        "loop": name.as_str(),
        "status": status(state),
        "calls": state.active_file().and_then(Result::ok).map(|file| file.calls.to_string()),
        "position": state.position,
        "live": live,
        "last": state.last.as_ref().map(|last| serde_json::json!({"run": last.run, "ending": last.ending, "at": last.at})),
        "couldnt": state.couldnt,
        "next_wake": state.wakes.values().map(|wake| wake.at).min(),
        "notices": state.notices.iter().map(|notice| serde_json::json!({"id": notice.id, "text": notice.text, "action": notice.action, "at": notice.at})).collect::<Vec<_>>(),
        "proposals": state.proposals.values().map(|pending| serde_json::json!({"id": pending.id, "proposal": pending.proposal, "revision": pending.revision, "outdated": state.outdated(pending)})).collect::<Vec<_>>(),
        "open": state.people.values().map(|person| serde_json::json!({"method": person.method, "run": person.run})).collect::<Vec<_>>(),
        "cursors": state.cursors,
        "webhooks_with_addresses": state.tokens.keys().collect::<Vec<_>>(),
    }))
}

fn list_json(loops: &Loops) -> Result<String, String> {
    let mut all = Vec::new();
    for name in loops.names().map_err(|error| error.to_string())? {
        let state = loops.state(&name).map_err(|error| error.to_string())?;
        all.push(view(loops, &name, &state)?);
    }
    serde_json::to_string_pretty(&serde_json::json!({"schema": "runx.loops.v1", "loops": all}))
        .map(|text| text + "\n")
        .map_err(|error| error.to_string())
}
