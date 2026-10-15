use runx_contracts::{ProcessExitReason, ProcessState};
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::record::{self, ProcessEvent, Started};
use super::{ProcessError, Processes, Reach, Start, process_id};
use crate::tool_catalogs::ToolCall;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Project {
    _directory: tempfile::TempDir,
    processes: Processes,
}

fn project() -> Result<Project, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let processes = Processes::at(&directory.path().join(".runx"))?;
    Ok(Project {
        _directory: directory,
        processes,
    })
}

/// A process recorded as its call started it, with no supervisor yet.
fn recorded(
    processes: &Processes,
    key: &str,
    actor: Option<&str>,
    script: &str,
    max_seconds: u64,
    created_at: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let id = process_id(key);
    std::fs::create_dir_all(processes.directory(&id)?)?;
    record::append(
        &processes.store,
        &id,
        0,
        &ProcessEvent::Started(Box::new(Started {
            call: key.to_owned(),
            actor: actor.map(str::to_owned),
            command: "sh".to_owned(),
            args: vec!["-c".to_owned(), script.to_owned()],
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            tty: None,
            max_seconds,
            created_at: created_at.to_owned(),
        })),
    )?;
    Ok(id)
}

#[test]
fn a_start_no_supervisor_confirmed_is_settled_by_its_lock_and_sent_again_to_the_same_process()
-> TestResult {
    let project = project()?;
    let processes = &project.processes;
    // A driver killed between recording the start and spawning its
    // supervisor, long enough ago.
    let id = recorded(
        processes,
        "run#start@1",
        None,
        "true",
        60,
        "2026-01-01T00:00:00.000Z",
    )?;
    let fresh = recorded(
        processes,
        "run#fresh@1",
        None,
        "true",
        60,
        &crate::time::now_iso8601(),
    )?;

    let listed = processes.list(Reach::Person)?.processes;
    let state = |id: &str| {
        listed
            .iter()
            .find(|process| process.process_id == id)
            .map(|process| (process.state, process.exit.as_ref().map(|exit| exit.reason)))
    };
    assert_eq!(
        state(&id),
        Some((ProcessState::Exited, Some(ProcessExitReason::Failed)))
    );
    assert_eq!(
        state(&fresh),
        Some((ProcessState::Starting, None)),
        "a start inside its confirmation window is still starting"
    );

    let call = ToolCall {
        key: "run#start@1".to_owned(),
        actor: None,
    };
    let again = processes.start(Start {
        call: &call,
        command: "sh".to_owned(),
        args: Vec::new(),
        cwd: PathBuf::from("."),
        env: BTreeMap::new(),
        tty: None,
        max_seconds: 60,
    })?;
    assert_eq!(again.process_id, id);
    assert_eq!(
        again.exit.map(|exit| exit.message.unwrap_or_default()),
        Some("its supervisor never started".to_owned())
    );
    Ok(())
}

#[test]
fn a_run_reaches_only_its_actors_processes() -> TestResult {
    let project = project()?;
    let processes = &project.processes;
    let now = crate::time::now_iso8601();
    let theirs = recorded(
        processes,
        "a#start@1",
        Some("runx:principal:a"),
        "true",
        60,
        &now,
    )?;
    let operators = recorded(processes, "b#start@1", None, "true", 60, &now)?;

    let listed = |reach| -> Result<Vec<String>, ProcessError> {
        Ok(processes
            .list(reach)?
            .processes
            .into_iter()
            .map(|process| process.process_id)
            .collect())
    };
    assert_eq!(
        listed(Reach::Actor(Some("runx:principal:a")))?,
        std::slice::from_ref(&theirs)
    );
    assert_eq!(
        listed(Reach::Actor(None))?,
        std::slice::from_ref(&operators)
    );
    assert_eq!(listed(Reach::Person)?.len(), 2);
    assert!(matches!(
        processes.stop(&theirs, Reach::Actor(None)),
        Err(ProcessError::Unknown(_))
    ));
    assert!(matches!(
        processes.read("../../kernel", Reach::Person, 0, 10),
        Err(ProcessError::InvalidId(_))
    ));
    Ok(())
}

/// Supervised in this process, as the detached supervisor would supervise
/// them, with a Unix shell.
#[cfg(unix)]
mod supervised {
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use runx_contracts::{ProcessExitReason, ProcessState, ProcessStream};

    use super::super::record::Record;
    use super::super::{ProcessError, Processes, Reach, supervisor};
    use super::{TestResult, project, recorded};

    /// Supervise `id` in this process, as the detached supervisor would.
    fn supervise(processes: &Processes, id: &str) -> JoinHandle<Result<(), String>> {
        let project = processes.project.clone();
        let id = id.to_owned();
        thread::spawn(move || supervisor::supervise(&project, &id))
    }

    fn until(
        processes: &Processes,
        id: &str,
        done: impl Fn(&Record) -> bool,
    ) -> Result<Record, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let record = processes.known(id)?;
            if done(&record) {
                return Ok(record);
            }
            if Instant::now() > deadline {
                return Err(format!("process {id} never got there: {record:?}").into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn running(record: &Record) -> bool {
        record.confirmed.is_some()
    }

    fn exited(record: &Record) -> bool {
        record.exit.is_some()
    }

    fn output(
        processes: &Processes,
        id: &str,
    ) -> Result<Vec<(ProcessStream, String)>, ProcessError> {
        Ok(processes
            .read(id, Reach::Person, 0, super::super::MAX_READ_BYTES)?
            .chunks
            .into_iter()
            .map(|chunk| (chunk.stream, chunk.text))
            .collect())
    }

    #[test]
    fn a_supervised_process_takes_input_once_per_call_and_records_its_exit() -> TestResult {
        let project = project()?;
        let processes = &project.processes;
        let now = crate::time::now_iso8601();
        let id = recorded(
            processes,
            "run#start@1",
            None,
            "echo out; echo err >&2; read a; read b; echo got:$a:$b; exit 3",
            60,
            &now,
        )?;
        let supervisor = supervise(processes, &id);
        until(processes, &id, running)?;

        // The same call sent again is applied once.
        processes.write(
            &id,
            Reach::Actor(None),
            Some("run#write@1"),
            "x\n".to_owned(),
        )?;
        processes.write(
            &id,
            Reach::Actor(None),
            Some("run#write@1"),
            "x\n".to_owned(),
        )?;
        processes.write(
            &id,
            Reach::Actor(None),
            Some("run#other@1"),
            "y\n".to_owned(),
        )?;
        let record = until(processes, &id, exited)?;
        supervisor.join().map_err(|_| "the supervisor panicked")??;

        let exit = record.exit.ok_or("an exit")?;
        assert_eq!(
            (exit.reason, exit.code),
            (ProcessExitReason::Exited, Some(3))
        );
        let output = output(processes, &id)?;
        assert!(
            output.contains(&(ProcessStream::Stderr, "err\n".to_owned())),
            "{output:?}"
        );
        let stdout = output
            .iter()
            .filter(|(stream, _)| *stream == ProcessStream::Stdout)
            .map(|(_, text)| text.as_str())
            .collect::<String>();
        assert_eq!(stdout, "out\ngot:x:y\n");
        assert!(matches!(
            processes.write(&id, Reach::Person, None, "late\n".to_owned()),
            Err(ProcessError::Exited(_))
        ));
        Ok(())
    }

    #[test]
    fn a_stop_terminates_the_process_and_records_it_stopped() -> TestResult {
        let project = project()?;
        let processes = &project.processes;
        let id = recorded(
            processes,
            "run#serve@1",
            None,
            "sleep 30",
            60,
            &crate::time::now_iso8601(),
        )?;
        let supervisor = supervise(processes, &id);
        until(processes, &id, running)?;
        assert!(matches!(
            processes.resize(
                &id,
                Reach::Person,
                runx_contracts::TerminalSize { rows: 10, cols: 10 }
            ),
            Err(ProcessError::NoTerminal(_))
        ));

        let started = Instant::now();
        let stopped = processes.stop(&id, Reach::Person)?;
        supervisor.join().map_err(|_| "the supervisor panicked")??;
        assert_eq!(stopped.state, ProcessState::Exited);
        let exit = stopped.exit.ok_or("an exit")?;
        assert_eq!(exit.reason, ProcessExitReason::Stopped);
        assert!(
            started.elapsed() < supervisor::STOP_GRACE,
            "a terminate was enough"
        );
        Ok(())
    }

    #[test]
    fn a_process_past_its_limit_is_stopped_by_its_supervisor() -> TestResult {
        let project = project()?;
        let processes = &project.processes;
        let id = recorded(
            processes,
            "run#slow@1",
            None,
            "sleep 30",
            1,
            &crate::time::now_iso8601(),
        )?;
        let supervisor = supervise(processes, &id);
        let record = until(processes, &id, exited)?;
        supervisor.join().map_err(|_| "the supervisor panicked")??;
        assert_eq!(
            record.exit.map(|exit| exit.reason),
            Some(ProcessExitReason::TimedOut)
        );
        Ok(())
    }
}
