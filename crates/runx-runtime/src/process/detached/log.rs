//! A process's output log. Output is appended as frames, each one stream's
//! bytes as `[stream][length: u32 big-endian][bytes]`, to segment files
//! named by the log offset they start at; only the newest few segments are
//! kept. A cursor is the offset of a frame boundary in the whole log, so it
//! stays meaningful as old segments are dropped, and one writer appends while
//! any number of readers read.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use runx_contracts::ProcessStream;

/// When a segment reaches this size, the next frame starts a new one.
const SEGMENT_BYTES: u64 = 1024 * 1024;
/// How many segments a log keeps: its newest output, about 4 MiB.
const SEGMENTS: usize = 4;
const HEADER_BYTES: usize = 5;
/// The most one frame carries.
const FRAME_BYTES: usize = 64 * 1024;

/// The one writer of a process's log.
pub(super) struct LogWriter {
    directory: PathBuf,
    segment: Option<(u64, File)>,
    offset: u64,
}

impl LogWriter {
    /// Start an empty log in `directory`.
    pub(super) fn create(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            segment: None,
            offset: 0,
        })
    }

    /// Append `bytes` written to `stream`.
    pub(super) fn append(&mut self, stream: ProcessStream, bytes: &[u8]) -> io::Result<()> {
        for payload in bytes.chunks(FRAME_BYTES) {
            let file = self.segment_for_next_frame()?;
            let mut frame = Vec::with_capacity(HEADER_BYTES + payload.len());
            frame.push(stream_byte(stream));
            frame.extend_from_slice(
                &u32::try_from(payload.len())
                    .unwrap_or(u32::MAX)
                    .to_be_bytes(),
            );
            frame.extend_from_slice(payload);
            file.write_all(&frame)?;
            self.offset += frame.len() as u64;
        }
        Ok(())
    }

    fn segment_for_next_frame(&mut self) -> io::Result<&mut File> {
        let full = self
            .segment
            .as_ref()
            .is_none_or(|(start, _)| self.offset - start >= SEGMENT_BYTES);
        if full {
            let file = OpenOptions::new()
                .create_new(true)
                .append(true)
                .open(segment_path(&self.directory, self.offset))?;
            self.segment = Some((self.offset, file));
            for (_, old) in segments(&self.directory)?.into_iter().rev().skip(SEGMENTS) {
                let _ = fs::remove_file(old);
            }
        }
        self.segment
            .as_mut()
            .map(|(_, file)| file)
            .ok_or_else(|| io::Error::other("the log has no segment"))
    }
}

/// One stream's output on its way to the log, held back while it ends
/// partway into a UTF-8 character, so each frame is whole text.
#[derive(Debug, Default)]
pub(super) struct Pending(Vec<u8>);

impl Pending {
    /// What of `bytes`, after what was held back, ends on a character
    /// boundary; the rest is held for the next.
    pub(super) fn whole(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.0.extend_from_slice(bytes);
        let whole = match std::str::from_utf8(&self.0) {
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            _ => self.0.len(),
        };
        self.0.drain(..whole).collect()
    }

    /// Whatever was held back, at the stream's end.
    pub(super) fn rest(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

/// What a read of a log returned.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct LogRead {
    /// Where the read began.
    pub(super) from: u64,
    /// The cursor after the last frame read.
    pub(super) next: u64,
    /// Log bytes between the cursor asked for and `from`, no longer kept.
    pub(super) dropped: u64,
    /// The output read, consecutive frames of one stream joined.
    pub(super) chunks: Vec<(ProcessStream, Vec<u8>)>,
}

/// Read frames after `after` until about `max_bytes` of output. A frame the
/// writer has not finished is left for a later read.
pub(super) fn read(directory: &Path, after: u64, max_bytes: usize) -> io::Result<LogRead> {
    // A segment dropped between listing and opening it is read again from
    // the new listing.
    for _ in 0..3 {
        match read_once(directory, after, max_bytes) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            read => return read,
        }
    }
    read_once(directory, after, max_bytes)
}

fn read_once(directory: &Path, after: u64, max_bytes: usize) -> io::Result<LogRead> {
    let segments = match segments(directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        segments => segments?,
    };
    let Some(oldest) = segments.first().map(|(start, _)| *start) else {
        return Ok(LogRead {
            from: after,
            next: after,
            ..LogRead::default()
        });
    };
    let from = after.max(oldest);
    let mut read = LogRead {
        from,
        next: from,
        dropped: from - after,
        chunks: Vec::new(),
    };
    let first = segments
        .iter()
        .rposition(|(start, _)| *start <= from)
        .unwrap_or(0);
    let mut bytes = 0_usize;
    for (start, path) in &segments[first..] {
        let mut file = File::open(path)?;
        let mut position = *start;
        // Walk to the cursor frame by frame: a cursor this log never
        // returned lands inside a frame and is refused.
        while position < read.next {
            let Some((_, length)) = header(&mut file)? else {
                return Ok(read);
            };
            file.seek(SeekFrom::Current(length as i64))?;
            position += (HEADER_BYTES + length) as u64;
        }
        if position != read.next {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the cursor is not one this log returned",
            ));
        }
        loop {
            if bytes >= max_bytes && !read.chunks.is_empty() {
                return Ok(read);
            }
            let Some((stream, length)) = header(&mut file)? else {
                break;
            };
            let mut payload = vec![0; length];
            if file.read_exact(&mut payload).is_err() {
                return Ok(read);
            }
            bytes += length;
            read.next += (HEADER_BYTES + length) as u64;
            match read.chunks.last_mut() {
                Some((last, text)) if *last == stream => text.extend_from_slice(&payload),
                _ => read.chunks.push((stream, payload)),
            }
        }
    }
    Ok(read)
}

/// The next frame's stream and length; none at the end of what is written.
fn header(file: &mut File) -> io::Result<Option<(ProcessStream, usize)>> {
    let mut header = [0_u8; HEADER_BYTES];
    let mut filled = 0;
    while filled < HEADER_BYTES {
        match file.read(&mut header[filled..])? {
            0 => return Ok(None),
            count => filled += count,
        }
    }
    let stream = match header[0] {
        b'o' => ProcessStream::Stdout,
        b'e' => ProcessStream::Stderr,
        b't' => ProcessStream::Terminal,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the log holds a frame of no stream",
            ));
        }
    };
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    Ok(Some((stream, length)))
}

fn stream_byte(stream: ProcessStream) -> u8 {
    match stream {
        ProcessStream::Stdout => b'o',
        ProcessStream::Stderr => b'e',
        ProcessStream::Terminal => b't',
    }
}

/// The log's segments, oldest first.
fn segments(directory: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
    let mut segments = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if let Some(start) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| u64::from_str_radix(name, 16).ok())
        {
            segments.push((start, path));
        }
    }
    segments.sort();
    Ok(segments)
}

fn segment_path(directory: &Path, start: u64) -> PathBuf {
    directory.join(format!("{start:016x}"))
}

#[cfg(test)]
mod tests;
