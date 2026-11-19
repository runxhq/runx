//! Run one rehearsal pass with the normal scheduler and connector feeds.
//! The caller owns the clock and runs this in an isolated workspace.

use std::io::{self, Write};
use std::time::Duration;

use chrono::Utc;
use runx_loops::{LoopName, Loops, Passed, Waiting};
use runx_runtime::WorkspaceEnv;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(name), None) = (args.next(), args.next()) else {
        return Err("usage: loop-pass <loop>".into());
    };
    let name = LoopName::new(&name)?;
    let workspace = WorkspaceEnv::load_process(std::env::current_dir()?)?;
    let orchestrator = runx_cli::runtime::local_orchestrator(workspace.env())?;
    let loops = Loops::open(workspace.env().clone(), workspace.cwd(), orchestrator)?;
    let mut feed_errors = Vec::new();
    let mut taken = 0;
    for feed in loops.feeds(&name)? {
        loop {
            match loops.poll(&name, &feed, Duration::ZERO) {
                Ok(page) => {
                    taken += page.taken;
                    if !page.more {
                        break;
                    }
                }
                Err(error) => {
                    feed_errors.push(format!("{feed}: {error}"));
                    break;
                }
            }
        }
    }
    let now = Utc::now();
    let passed = if feed_errors.is_empty() {
        loops.pass(&name, now)?
    } else {
        Passed::default()
    };
    let waiting = match passed.waiting {
        Some(Waiting::Person) => Some("person"),
        Some(Waiting::Until(_)) => Some("time"),
        Some(Waiting::Reconciling) => Some("reconciliation"),
        Some(Waiting::Orphaned | Waiting::Driving | Waiting::Turn) => Some("working"),
        None => None,
    };
    serde_json::to_writer(
        io::stdout().lock(),
        &json!({
            "at": now,
            "taken": taken,
            "feed_errors": feed_errors,
            "started": passed.woke,
            "settled": passed.settled,
            "couldnt": passed.couldnt,
            "next_wake": passed.next,
            "waiting": waiting,
        }),
    )?;
    io::stdout().write_all(b"\n")?;
    Ok(())
}
