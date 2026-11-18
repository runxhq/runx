//! Loops (the loop contract, `plans/0.12/contracts/loop.md`): named,
//! continuing jobs a workspace keeps. A loop calls one entry method of a
//! skill each time events wake it, and keeps everything it knows in two host
//! streams, its inbox and its record.
//!
//! This crate is the loop model alone, built on the runtime's public API so
//! the engine learns no loop (rules 1 to 3): the loop file, the streams and
//! their fold, the clock's occurrences, the claim that starts a wake's run in
//! the same commit, how a run's end settles the loop, and what a run may
//! propose. The always-on runx that drives loops is a host in the CLI.

mod clock;
mod context;
mod feeds;
mod file;
mod pass;
mod proposals;
mod records;
mod state;
mod store;
mod webhooks;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use clock::{Due, next_after};
pub use feeds::{Feed, Polled};
pub use file::{
    Calls, DayZone, LOOPS_DIR, Late, LoopFile, LoopName, OnFailure, Policy, Span, Spend, Trigger,
    digest,
};
pub use pass::{Passed, Waiting};
pub use records::{Action, Ending, HOST, InboxEvent, Proposal, RecordEvent};
pub use state::{
    Answer, Frontier, Last, Live, LoopState, Notice, Pending, PersonRun, ProposedWake, Reservation,
    Status,
};

use runx_runtime::orchestrator::LocalOrchestrator;
use runx_runtime::{HostName, HostStreamError, HostStreams};

/// Why a loop act didn't happen.
#[derive(Debug, thiserror::Error)]
pub enum LoopError {
    /// Something the person wrote doesn't validate, in its own words.
    #[error("{0}")]
    Invalid(String),
    #[error("there's no loop named {0} here: `runx loops` lists them")]
    Unknown(String),
    /// The loop's record moved since it was read: read it again and decide
    /// afresh.
    #[error("loop {0} moved while this was decided: try again")]
    Moved(String),
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Streams(#[from] HostStreamError),
    #[error("{0}")]
    Runtime(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

impl LoopError {
    fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    fn json(context: &str, error: &serde_json::Error) -> Self {
        Self::Runtime(format!("{context}: {error}"))
    }
}

/// A loop's file as written: its bytes, and what they parse to.
struct Written {
    bytes: String,
    file: Result<LoopFile, LoopError>,
}

/// A host's word, asked at any moment, that it's going off.
pub type GoingOff = Arc<dyn Fn() -> bool + Send + Sync>;

/// The loops of one workspace, and the acts on them.
pub struct Loops {
    env: BTreeMap<String, String>,
    cwd: PathBuf,
    streams: HostStreams,
    orchestrator: LocalOrchestrator,
    going_off: Option<GoingOff>,
}

impl Loops {
    /// The loops of the workspace at `cwd`, whose runs `orchestrator` runs.
    pub fn open(
        env: BTreeMap<String, String>,
        cwd: &Path,
        orchestrator: LocalOrchestrator,
    ) -> Result<Self, LoopError> {
        let host = HostName::new(records::HOST)?;
        let streams = HostStreams::open(&env, cwd, host)?;
        Ok(Self {
            env,
            cwd: cwd.to_path_buf(),
            streams,
            orchestrator,
            going_off: None,
        })
    }

    /// These loops as a host serves them, `going_off` saying when it's
    /// going off: a pass then settles what its runs finish with and claims
    /// no new wake, which waits for the host's next start.
    #[must_use]
    pub fn served(mut self, going_off: GoingOff) -> Self {
        self.going_off = Some(going_off);
        self
    }

    fn going_off(&self) -> bool {
        self.going_off.as_ref().is_some_and(|going_off| going_off())
    }

    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Every loop here: each file in `loops/`, and each loop with a record
    /// whose file has gone, by name.
    pub fn names(&self) -> Result<Vec<LoopName>, LoopError> {
        let mut names = std::collections::BTreeSet::new();
        let dir = self.cwd.join(LOOPS_DIR);
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|error| LoopError::io("reading loops/", error))?;
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) == Some("yaml")
                        && let Some(name) = path
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .and_then(|stem| LoopName::new(stem).ok())
                    {
                        names.insert(name);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(LoopError::io("reading loops/", error)),
        }
        let mut after: Option<String> = None;
        loop {
            let heads = self.streams.heads(after.as_deref(), 500)?;
            let Some(last) = heads.last() else {
                break;
            };
            after = Some(last.key.clone());
            for head in &heads {
                if let Some(name) = head
                    .key
                    .strip_suffix("/record")
                    .and_then(|name| LoopName::new(name).ok())
                {
                    names.insert(name);
                }
            }
        }
        Ok(names.into_iter().collect())
    }

    /// Loop `name`'s state, folded from its record.
    pub fn state(&self, name: &LoopName) -> Result<LoopState, LoopError> {
        self.read(name)
    }

    /// The loop's file as written now, its bytes and what they parse to.
    fn written(&self, name: &LoopName) -> Result<Option<Written>, LoopError> {
        let path = name.path(&self.cwd);
        match std::fs::read_to_string(&path) {
            Ok(bytes) => {
                let file = LoopFile::parse(bytes.as_bytes());
                Ok(Some(Written { bytes, file }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(LoopError::io(format!("reading {}", path.display()), error)),
        }
    }
}
