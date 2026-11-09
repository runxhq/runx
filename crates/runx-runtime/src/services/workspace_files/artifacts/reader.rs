//! Bounded reads from an immutable snapshot, checked against its bound chunks.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use super::storage::SNAPSHOT_CHUNK_BYTES;

/// The chunks a reader keeps once it has read and checked them, the most
/// recent last: the largest page's, and one either side, so a page's range
/// read again, or the next page's first chunk, is never read from the store
/// twice.
const RETAINED_CHUNKS: usize =
    super::MAX_ARTIFACT_PAGE_BYTES / super::storage::SNAPSHOT_CHUNK_BYTES + 2;

pub(super) struct SnapshotReader {
    file: File,
    bytes: u64,
    chunks: Vec<String>,
    position: u64,
    cached: VecDeque<(usize, Vec<u8>)>,
}

impl SnapshotReader {
    pub(super) fn new(file: File, bytes: u64, chunks: Vec<String>) -> std::io::Result<Self> {
        if file.metadata()?.len() != bytes {
            return Err(integrity_error());
        }
        Ok(Self {
            file,
            bytes,
            chunks,
            position: 0,
            cached: VecDeque::new(),
        })
    }

    /// Chunk `index`, read from the store and checked the first time, and
    /// kept.
    fn load_chunk(&mut self, index: usize) -> std::io::Result<&[u8]> {
        if let Some(at) = self.cached.iter().position(|(cached, _)| *cached == index) {
            let kept = self.cached.remove(at).ok_or_else(integrity_error)?;
            self.cached.push_back(kept);
            return self.newest();
        }
        let start = u64::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(SNAPSHOT_CHUNK_BYTES as u64))
            .ok_or_else(integrity_error)?;
        let length = usize::try_from(
            self.bytes
                .saturating_sub(start)
                .min(SNAPSHOT_CHUNK_BYTES as u64),
        )
        .map_err(|_| integrity_error())?;
        self.file.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![0_u8; length];
        self.file.read_exact(&mut bytes)?;
        crate::work_counts::count_artifact_bytes(
            crate::work_counts::ArtifactCopy::StoreRead,
            length as u64,
        );
        let expected = self.chunks.get(index).ok_or_else(integrity_error)?;
        if &runx_contracts::sha256_prefixed(&bytes) != expected {
            return Err(integrity_error());
        }
        if self.cached.len() >= RETAINED_CHUNKS {
            self.cached.pop_front();
        }
        self.cached.push_back((index, bytes));
        self.newest()
    }

    fn newest(&self) -> std::io::Result<&[u8]> {
        self.cached
            .back()
            .map(|(_, bytes)| bytes.as_slice())
            .ok_or_else(integrity_error)
    }
}

impl Read for SnapshotReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() || self.position >= self.bytes {
            return Ok(0);
        }
        let index = usize::try_from(self.position / SNAPSHOT_CHUNK_BYTES as u64)
            .map_err(|_| integrity_error())?;
        let within = usize::try_from(self.position % SNAPSHOT_CHUNK_BYTES as u64)
            .map_err(|_| integrity_error())?;
        let bytes = self.load_chunk(index)?;
        let remaining = bytes.get(within..).ok_or_else(integrity_error)?;
        let length = output.len().min(remaining.len());
        output[..length].copy_from_slice(&remaining[..length]);
        self.position += length as u64;
        Ok(length)
    }
}

impl Seek for SnapshotReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let position = match position {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::End(offset) => i128::from(self.bytes) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
        };
        self.position = u64::try_from(position).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid snapshot offset")
        })?;
        Ok(self.position)
    }
}

fn integrity_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "local artifact snapshot integrity mismatch",
    )
}
