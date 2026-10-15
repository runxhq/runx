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
            kind: runx_contracts::ProcessKind::Detached,
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

    /// A command its driver runs, followed as a process of its call.
    fn followed_command(
        processes: Processes,
        key: &str,
        script: &str,
    ) -> Result<
        (
            std::sync::Arc<super::super::Followed>,
            crate::process::ProcessSpec,
        ),
        Box<dyn std::error::Error>,
    > {
        let call = crate::tool_catalogs::ToolCall {
            key: key.to_owned(),
            actor: None,
        };
        let followed = processes
            .follow(super::Start {
                call: &call,
                command: "sh".to_owned(),
                args: vec!["-c".to_owned(), script.to_owned()],
                cwd: std::env::temp_dir(),
                env: std::collections::BTreeMap::new(),
                tty: None,
                max_seconds: 60,
            })?
            .map(std::sync::Arc::new)
            .ok_or("a command of a new call is followed")?;
        let spec = crate::process::ProcessSpec::new("followed-test", "/bin/sh", 4096)
            .args(vec!["-c".to_owned(), script.to_owned()])
            .env(std::collections::BTreeMap::from([(
                "PATH".to_owned(),
                std::env::var("PATH").unwrap_or_default(),
            )]))
            .timeout(Some(Duration::from_secs(60)))
            .follower(Some(
                followed.clone() as std::sync::Arc<dyn crate::process::Follower>
            ));
        Ok((followed, spec))
    }

    #[test]
    fn a_commands_output_is_read_while_it_runs_and_its_exit_recorded_when_it_settles() -> TestResult
    {
        let project = project()?;
        let reader = Processes::at(&project.processes.project)?;
        let id = super::process_id("run#command@1");
        let (followed, spec) = followed_command(
            Processes::at(&project.processes.project)?,
            "run#command@1",
            "echo one; sleep 1; echo two",
        )?;
        let running = thread::spawn(move || crate::process::run_process(spec));

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let read = reader.read(&id, Reach::Person, 0, 1024)?;
            if read.chunks.iter().any(|chunk| chunk.text.contains("one")) {
                assert_eq!(read.state, ProcessState::Running, "{read:?}");
                break;
            }
            assert!(Instant::now() < deadline, "the output arrived while it ran");
            thread::sleep(Duration::from_millis(20));
        }
        assert!(matches!(
            reader.write(&id, Reach::Person, None, "x\n".to_owned()),
            Err(ProcessError::Refused(_))
        ));

        let outcome = running.join().map_err(|_| "the command panicked")??;
        followed.settle(Ok(&outcome))?;
        assert_eq!(String::from_utf8_lossy(&outcome.stdout.bytes), "one\ntwo\n");
        let process = reader.list(Reach::Person)?.processes;
        let [process] = process.as_slice() else {
            return Err("one process".into());
        };
        assert_eq!(process.kind, runx_contracts::ProcessKind::Command);
        let exit = process.exit.as_ref().ok_or("an exit")?;
        assert_eq!(
            (exit.reason, exit.code),
            (ProcessExitReason::Exited, Some(0))
        );
        Ok(())
    }

    #[test]
    fn a_stop_sent_to_a_commands_process_stops_the_command() -> TestResult {
        let project = project()?;
        let reader = Processes::at(&project.processes.project)?;
        let id = super::process_id("run#serve@1");
        let (followed, spec) = followed_command(
            Processes::at(&project.processes.project)?,
            "run#serve@1",
            "sleep 30",
        )?;
        let running = thread::spawn(move || crate::process::run_process(spec));
        until(&reader, &id, |record| record.confirmed.is_some())?;

        super::super::control::send(
            &reader.directory(&id)?,
            &super::super::control::Control::Stop,
        )?;
        let outcome = running.join().map_err(|_| "the command panicked")??;
        assert!(outcome.stopped);
        followed.settle(Ok(&outcome))?;
        assert_eq!(
            reader.known(&id)?.exit.map(|exit| exit.reason),
            Some(ProcessExitReason::Stopped)
        );
        Ok(())
    }
}
