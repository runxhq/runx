//! Spawning a process's supervisor: the runx binary that is running now,
//! given the child's environment and nothing else, detached from its caller.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use super::supervisor::SUPERVISOR_FLAG;

pub(super) fn supervisor(
    project: &Path,
    id: &str,
    env: &BTreeMap<String, String>,
) -> io::Result<()> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(SUPERVISOR_FLAG)
        .arg(project)
        .arg(id)
        .env_clear()
        .envs(env)
        .current_dir(project)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        // Out of the host's kill-on-close job, which lets it break away.
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        command.creation_flags(
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
        );
    }
    let mut child = crate::process::with_spawn_lock(|| command.spawn())?;
    // Reaped when it exits, so a long-lived caller collects no zombies.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
