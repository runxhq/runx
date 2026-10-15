//! A process's mailbox: what callers send its supervisor, one JSON line
//! each, appended to the `control` file in the process's directory. The
//! supervisor reads what was appended since it last looked.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

const MAILBOX: &str = "control";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Control {
    /// Input for its stdin or terminal. A write sent again under the same
    /// `call` is applied once.
    Write { call: Option<String>, input: String },
    /// A new terminal size.
    Resize { rows: u16, cols: u16 },
    /// Terminate it, then kill it.
    Stop,
}

/// Send `control` to the supervisor of the process in `directory`.
pub(super) fn send(directory: &Path, control: &Control) -> io::Result<()> {
    let mut line = serde_json::to_vec(control).map_err(io::Error::other)?;
    line.push(b'\n');
    // One append per message, so concurrent senders never interleave.
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(MAILBOX))?
        .write_all(&line)
}

/// The supervisor's end of the mailbox.
pub(super) struct Mailbox {
    file: File,
    offset: u64,
    partial: Vec<u8>,
}

impl Mailbox {
    pub(super) fn open(directory: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(directory.join(MAILBOX))?;
        Ok(Self {
            file,
            offset: 0,
            partial: Vec::new(),
        })
    }

    /// What was sent since the last take. A line that is not a message is
    /// skipped.
    pub(super) fn take(&mut self) -> io::Result<Vec<Control>> {
        self.file.seek(SeekFrom::Start(self.offset))?;
        let mut read = Vec::new();
        self.offset += self.file.read_to_end(&mut read)? as u64;
        self.partial.extend_from_slice(&read);
        let Some(end) = self.partial.iter().rposition(|byte| *byte == b'\n') else {
            return Ok(Vec::new());
        };
        let complete = self.partial.drain(..=end).collect::<Vec<_>>();
        Ok(complete
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_slice(line).ok())
            .collect())
    }
}
