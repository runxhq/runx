// Module rationale: local store read/write/index semantics stay
// together.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use runx_contracts::{RECEIPT_SCHEMA, Receipt};
use runx_receipts::{
    ReceiptProofContextProvider, content_addressed_receipt_id, verify_receipt_proof,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::paths::ReceiptStoreLabel;
use super::seal::{RuntimeReceiptProofContextProvider, RuntimeReceiptSignaturePolicy};

/// The receipt index: a log of every receipt the store holds, one JSON entry
/// per line after a header that names it, appended to and never rewritten.
const RECEIPT_STORE_INDEX_SCHEMA: &str = "runx.receipt_index.v1";
const INDEX_FILE_NAME: &str = "index.jsonl";
pub(crate) const STORE_LOCK_FILE_NAME: &str = ".receipt-store.lock";
const SHA256_RECEIPT_ID_PREFIX: &str = "sha256:";
const SHA256_RECEIPT_FILE_PREFIX: &str = "sha256-";

#[derive(Clone, Debug)]
pub struct LocalReceiptStore {
    root: PathBuf,
}

impl LocalReceiptStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn receipt_path(&self, receipt_id: &str) -> Result<PathBuf, ReceiptStoreError> {
        Ok(self.root.join(receipt_file_name(receipt_id)?))
    }

    pub fn read_exact(
        &self,
        receipt_id: &str,
        signature_policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<Receipt, ReceiptStoreError> {
        let file_path = self.receipt_path(receipt_id)?;
        self.ensure_store_dir()?;
        read_receipt_file(&file_path, receipt_id, signature_policy)
    }

    pub fn write_receipt(
        &self,
        receipt: &Receipt,
        signature_policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<(), ReceiptStoreError> {
        let file_name = receipt_file_name(&receipt.id)?;
        self.ensure_or_create_store_dir()?;
        let _lock = self.lock_mutations()?;
        let file_path = self.root.join(&file_name);
        let contents =
            serde_json::to_vec(receipt).map_err(|source| ReceiptStoreError::MalformedReceipt {
                path: file_path.clone(),
                message: source.to_string(),
            })?;

        if file_path.exists() {
            let existing =
                fs::read(&file_path).map_err(|source| ReceiptStoreError::ReceiptUnreadable {
                    path: file_path.clone(),
                    source,
                })?;
            if existing == contents {
                verify_stored_receipt_proof(&file_path, receipt, signature_policy)?;
                return self.update_index_after_write(receipt);
            }
            return Err(ReceiptStoreError::ReceiptAlreadyExists {
                receipt_id: receipt.id.to_string(),
            });
        }

        verify_stored_receipt_proof(&file_path, receipt, signature_policy)?;
        write_atomic(&self.root, &file_name, &contents)?;
        self.update_index_after_write(receipt)
    }

    pub fn write_receipts<'a>(
        &self,
        receipts: impl IntoIterator<Item = &'a Receipt>,
        signature_policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<(), ReceiptStoreError> {
        let mut receipts = receipts.into_iter().peekable();
        if receipts.peek().is_none() {
            return Ok(());
        }
        self.ensure_or_create_store_dir()?;
        let _lock = self.lock_mutations()?;
        let mut unique_by_id = BTreeMap::<String, usize>::new();
        let mut unique = Vec::<(&Receipt, String, String, PathBuf, Vec<u8>)>::new();
        for receipt in receipts {
            let receipt_id = receipt.id.to_string();
            let file_name = receipt_file_name(&receipt_id)?;
            let file_path = self.root.join(&file_name);
            let contents = serde_json::to_vec(receipt).map_err(|source| {
                ReceiptStoreError::MalformedReceipt {
                    path: file_path.clone(),
                    message: source.to_string(),
                }
            })?;
            if let Some(index) = unique_by_id.get(&receipt_id).copied() {
                if unique[index].4 != contents {
                    return Err(ReceiptStoreError::ReceiptAlreadyExists { receipt_id });
                }
                continue;
            }
            unique_by_id.insert(receipt_id.clone(), unique.len());
            unique.push((receipt, receipt_id, file_name, file_path, contents));
        }

        let mut pending = Vec::new();
        let mut index_entries = Vec::new();
        for (receipt, receipt_id, file_name, file_path, contents) in unique {
            if file_path.exists() {
                let existing = fs::read(&file_path).map_err(|source| {
                    ReceiptStoreError::ReceiptUnreadable {
                        path: file_path.clone(),
                        source,
                    }
                })?;
                if existing != contents {
                    return Err(ReceiptStoreError::ReceiptAlreadyExists { receipt_id });
                }
                verify_stored_receipt_proof(&file_path, receipt, signature_policy)?;
            } else {
                verify_stored_receipt_proof(&file_path, receipt, signature_policy)?;
                pending.push((file_name, contents));
            }
            index_entries.push(index_entry(receipt)?);
        }
        if !pending.is_empty() {
            write_atomic_batch(&self.root, &pending)?;
        }
        self.update_index_after_writes(&index_entries)
    }

    pub fn list(
        &self,
        signature_policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<Vec<Receipt>, ReceiptStoreError> {
        self.ensure_store_dir()?;
        let mut receipts = Vec::new();
        for entry in
            fs::read_dir(&self.root).map_err(|source| ReceiptStoreError::StoreUnreadable {
                path: self.root.clone(),
                source,
            })?
        {
            let entry = entry.map_err(|source| ReceiptStoreError::StoreUnreadable {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            if !is_receipt_json_path(&path) {
                continue;
            }
            let Some(receipt_id) = path
                .file_stem()
                .and_then(OsStr::to_str)
                .and_then(receipt_id_from_file_stem)
            else {
                continue;
            };
            match read_receipt_file(&path, &receipt_id, signature_policy) {
                Ok(receipt) => receipts.push(receipt),
                // An earlier runx's receipt is that runx's to read.
                Err(ReceiptStoreError::ReceiptBeforeThisRunx { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        receipts.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(receipts)
    }

    pub(crate) fn list_without_proof_for_history(&self) -> Result<Vec<Receipt>, ReceiptStoreError> {
        self.ensure_store_dir()?;
        let mut receipts = Vec::new();
        for entry in
            fs::read_dir(&self.root).map_err(|source| ReceiptStoreError::StoreUnreadable {
                path: self.root.clone(),
                source,
            })?
        {
            let entry = entry.map_err(|source| ReceiptStoreError::StoreUnreadable {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            if !is_receipt_json_path(&path) {
                continue;
            }
            let Some(receipt_id) = path
                .file_stem()
                .and_then(OsStr::to_str)
                .and_then(receipt_id_from_file_stem)
            else {
                continue;
            };
            match read_receipt_file_without_proof(&path, &receipt_id) {
                Ok(receipt) => receipts.push(receipt),
                // An earlier runx's receipt is that runx's to read.
                Err(ReceiptStoreError::ReceiptBeforeThisRunx { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        receipts.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(receipts)
    }

    pub fn read_exact_without_proof(&self, receipt_id: &str) -> Result<Receipt, ReceiptStoreError> {
        let file_path = self.receipt_path(receipt_id)?;
        self.ensure_store_dir()?;
        read_receipt_file_without_proof(&file_path, receipt_id)
    }

    pub fn rebuild_index(
        &self,
        signature_policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<ReceiptStoreIndex, ReceiptStoreError> {
        self.ensure_store_dir()?;
        let _lock = self.lock_mutations()?;
        self.rebuild_index_from_receipts(self.list(signature_policy)?)
    }

    fn rebuild_index_projection(&self) -> Result<ReceiptStoreIndex, ReceiptStoreError> {
        self.rebuild_index_from_receipts(self.list_without_proof_for_history()?)
    }

    /// The index as the log holds it: each receipt once, in id order. Its
    /// coverage of the directory is checked here, where the index is read,
    /// never on an append.
    pub fn index(&self) -> Result<ReceiptStoreIndex, ReceiptStoreError> {
        self.ensure_store_dir()?;
        let index_path = self.index_path();
        let contents = fs::read_to_string(&index_path).map_err(|source| {
            ReceiptStoreError::StoreUnreadable {
                path: index_path.clone(),
                source,
            }
        })?;
        let index = parse_index(&contents, &index_path)?;
        if !self.indexes_every_receipt_it_reads(&index)? {
            return Err(ReceiptStoreError::ReceiptIndexStale {
                path: index_path,
                message: "index entries do not cover every receipt JSON file".to_owned(),
            });
        }
        Ok(index)
    }

    fn rebuild_index_from_receipts(
        &self,
        receipts: Vec<Receipt>,
    ) -> Result<ReceiptStoreIndex, ReceiptStoreError> {
        let mut entries = receipts
            .into_iter()
            .map(|receipt| index_entry(&receipt))
            .collect::<Result<Vec<_>, ReceiptStoreError>>()?;
        entries.sort_by(|left, right| left.receipt_id.cmp(&right.receipt_id));
        let index = ReceiptStoreIndex {
            schema: RECEIPT_STORE_INDEX_SCHEMA.to_owned(),
            entries,
        };
        self.write_index(&index)?;
        Ok(index)
    }

    fn update_index_after_write(&self, receipt: &Receipt) -> Result<(), ReceiptStoreError> {
        self.update_index_after_writes(&[index_entry(receipt)?])
    }

    fn update_index_after_writes(
        &self,
        entries: &[ReceiptStoreIndexEntry],
    ) -> Result<(), ReceiptStoreError> {
        match self.append_index_entries(entries) {
            Ok(()) => Ok(()),
            // The index is a recoverable structural projection, not proof that
            // every historical receipt verifies under the caller's current
            // keyring. The new receipt was verified before it was written;
            // unrelated historical proof remains fail-closed on exact reads,
            // listings, index rebuilds, and audits.
            Err(_) => match self.rebuild_index_projection() {
                Ok(_) => Ok(()),
                Err(error) => Err(ReceiptStoreError::ReceiptIndexStale {
                    path: self.index_path(),
                    message: error.to_string(),
                }),
            },
        }
    }

    /// Add `entries` to the end of the index, reading none of it. A store
    /// with no index yet, or one whose index was lost, writes it whole from
    /// its receipts, and so does one whose last line an earlier append left
    /// torn; either happens once, not on every append.
    fn append_index_entries(
        &self,
        entries: &[ReceiptStoreIndexEntry],
    ) -> Result<(), ReceiptStoreError> {
        let index_path = self.index_path();
        let mut file = match OpenOptions::new().read(true).append(true).open(&index_path) {
            Ok(file) => file,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return self.rebuild_index_projection().map(|_| ());
            }
            Err(source) => {
                return Err(ReceiptStoreError::StoreUnreadable {
                    path: index_path,
                    source,
                });
            }
        };
        let unreadable = |source| ReceiptStoreError::StoreUnreadable {
            path: self.index_path(),
            source,
        };
        if !ends_a_line(&mut file).map_err(unreadable)? {
            return Err(ReceiptStoreError::ReceiptIndexStale {
                path: self.index_path(),
                message: "the index ends partway through an entry".to_owned(),
            });
        }
        let mut lines = Vec::new();
        for entry in entries {
            serde_json::to_writer(&mut lines, entry).map_err(|source| {
                ReceiptStoreError::MalformedIndex {
                    path: self.index_path(),
                    message: source.to_string(),
                }
            })?;
            lines.push(b'\n');
        }
        // A recoverable projection of the receipt files, which stay durable:
        // an append skips fsync so a receipt doesn't pay for durability twice.
        crate::work_counts::count_many(
            crate::work_counts::Work::ReceiptIndexEntryWritten,
            entries.len(),
        );
        file.write_all(&lines).map_err(unreadable)
    }

    /// Whether `index` names every receipt file, and only those, but an
    /// earlier runx's, which a rebuild leaves for that runx. Only a file the
    /// index doesn't name is read.
    fn indexes_every_receipt_it_reads(
        &self,
        index: &ReceiptStoreIndex,
    ) -> Result<bool, ReceiptStoreError> {
        let indexed = index
            .entries
            .iter()
            .map(|entry| entry.file_name.as_str())
            .collect::<BTreeSet<_>>();
        let mut found = 0usize;
        for entry in
            fs::read_dir(&self.root).map_err(|source| ReceiptStoreError::StoreUnreadable {
                path: self.root.clone(),
                source,
            })?
        {
            let entry = entry.map_err(|source| ReceiptStoreError::StoreUnreadable {
                path: self.root.clone(),
                source,
            })?;
            crate::work_counts::count(crate::work_counts::Work::ReceiptDirectoryEntryRead);
            let path = entry.path();
            let Some(receipt_id) = path
                .file_stem()
                .and_then(OsStr::to_str)
                .filter(|_| is_receipt_json_path(&path))
                .and_then(receipt_id_from_file_stem)
            else {
                continue;
            };
            if path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| indexed.contains(name))
            {
                found += 1;
                continue;
            }
            if !matches!(
                read_receipt_file_without_proof(&path, &receipt_id),
                Err(ReceiptStoreError::ReceiptBeforeThisRunx { .. })
            ) {
                return Ok(false);
            }
        }
        Ok(found == indexed.len())
    }

    /// Write the whole index, header and entries, in place of whatever was
    /// there: a rebuild's, never an append's.
    fn write_index(&self, index: &ReceiptStoreIndex) -> Result<(), ReceiptStoreError> {
        let malformed = |source: serde_json::Error| ReceiptStoreError::MalformedIndex {
            path: self.index_path(),
            message: source.to_string(),
        };
        let mut contents = Vec::new();
        serde_json::to_writer(
            &mut contents,
            &IndexHeader {
                schema: index.schema.clone(),
            },
        )
        .map_err(malformed)?;
        contents.push(b'\n');
        for entry in &index.entries {
            serde_json::to_writer(&mut contents, entry).map_err(malformed)?;
            contents.push(b'\n');
        }
        crate::work_counts::count_many(
            crate::work_counts::Work::ReceiptIndexEntryWritten,
            index.entries.len(),
        );
        write_atomic_cache(&self.root, INDEX_FILE_NAME, &contents)
    }

    fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE_NAME)
    }

    fn lock_mutations(&self) -> Result<ReceiptStoreMutationLock, ReceiptStoreError> {
        let path = self.root.join(STORE_LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| ReceiptStoreError::StoreUnreadable {
                path: path.clone(),
                source,
            })?;
        file.lock_exclusive()
            .map_err(|source| ReceiptStoreError::StoreUnreadable {
                path: path.clone(),
                source,
            })?;
        Ok(ReceiptStoreMutationLock { file })
    }

    fn ensure_store_dir(&self) -> Result<(), ReceiptStoreError> {
        match fs::metadata(&self.root) {
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(_) => Err(ReceiptStoreError::StoreNotDirectory {
                path: self.root.clone(),
            }),
            Err(source) if source.kind() == ErrorKind::NotFound => {
                Err(ReceiptStoreError::MissingStore {
                    path: self.root.clone(),
                })
            }
            Err(source) => Err(ReceiptStoreError::StoreUnreadable {
                path: self.root.clone(),
                source,
            }),
        }
    }

    fn ensure_or_create_store_dir(&self) -> Result<(), ReceiptStoreError> {
        match fs::metadata(&self.root) {
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(_) => Err(ReceiptStoreError::StoreNotDirectory {
                path: self.root.clone(),
            }),
            Err(source) if source.kind() == ErrorKind::NotFound => fs::create_dir_all(&self.root)
                .map_err(|source| ReceiptStoreError::StoreUnreadable {
                    path: self.root.clone(),
                    source,
                }),
            Err(source) => Err(ReceiptStoreError::StoreUnreadable {
                path: self.root.clone(),
                source,
            }),
        }
    }
}

struct ReceiptStoreMutationLock {
    file: File,
}

impl Drop for ReceiptStoreMutationLock {
    fn drop(&mut self) {
        let _ignored = FileExt::unlock(&self.file);
    }
}

/// The receipt index: each receipt the store holds once, in id order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptStoreIndex {
    pub schema: String,
    pub entries: Vec<ReceiptStoreIndexEntry>,
}

/// The index's first line, which names it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexHeader {
    schema: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptStoreIndexEntry {
    pub receipt_id: String,
    pub file_name: String,
    pub created_at: String,
}

fn index_entry(receipt: &Receipt) -> Result<ReceiptStoreIndexEntry, ReceiptStoreError> {
    let receipt_id = receipt.id.to_string();
    Ok(ReceiptStoreIndexEntry {
        file_name: receipt_file_name(&receipt_id)?,
        receipt_id,
        created_at: receipt.created_at.to_string(),
    })
}

#[derive(Debug, Error)]
pub enum ReceiptStoreError {
    #[error("receipt store is missing")]
    MissingStore { path: PathBuf },
    #[error("receipt store path is not a directory")]
    StoreNotDirectory { path: PathBuf },
    #[error("receipt store is unreadable: {source}")]
    StoreUnreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("receipt id is invalid for local store lookup: {receipt_id}")]
    InvalidReceiptId { receipt_id: String },
    #[error("receipt is missing")]
    MissingReceipt { path: PathBuf },
    #[error("receipt is unreadable: {source}")]
    ReceiptUnreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("receipt JSON is malformed: {message}")]
    MalformedJson { path: PathBuf, message: String },
    #[error("receipt has unsupported schema: {schema}")]
    WrongSchema { path: PathBuf, schema: String },
    #[error("receipt shape is invalid: {message}")]
    MalformedReceipt { path: PathBuf, message: String },
    #[error("receipt id '{receipt_id}' does not match file name '{file_stem}'")]
    IdFilenameMismatch {
        path: PathBuf,
        receipt_id: String,
        file_stem: String,
    },
    #[error("receipt proof is invalid for {receipt_id}: {message}")]
    ReceiptProofInvalid {
        path: PathBuf,
        receipt_id: String,
        message: String,
    },
    #[error("receipt already exists with different content: {receipt_id}")]
    ReceiptAlreadyExists { receipt_id: String },
    #[error("receipt store index is malformed: {message}")]
    MalformedIndex { path: PathBuf, message: String },
    #[error("receipt store index is stale: {message}")]
    ReceiptIndexStale { path: PathBuf, message: String },
    #[error("receipt store path cannot be projected safely: {reason}")]
    UnsafePathProjection { reason: String },
    #[error("receipt {receipt_id} was not found in {}", path.display())]
    ReceiptNotInStore { receipt_id: String, path: PathBuf },
    #[error(
        "receipt {receipt_id} was written by runx {runx_version}, a runx before 0.12; verify it with that version"
    )]
    ReceiptBeforeThisRunx {
        path: PathBuf,
        receipt_id: String,
        runx_version: String,
    },
}

impl ReceiptStoreError {
    #[must_use]
    pub fn public_message(&self, store_label: &ReceiptStoreLabel) -> String {
        match self {
            Self::MissingStore { .. } => format!("receipt store {store_label} is missing"),
            Self::StoreNotDirectory { .. } => {
                format!("receipt store {store_label} is not a directory")
            }
            Self::StoreUnreadable { .. } => format!("receipt store {store_label} is unreadable"),
            Self::InvalidReceiptId { .. } => {
                "receipt id is invalid for local store lookup".to_owned()
            }
            Self::MissingReceipt { .. } => format!("receipt is missing in store {store_label}"),
            Self::ReceiptUnreadable { .. } => {
                format!("receipt is unreadable in store {store_label}")
            }
            Self::MalformedJson { .. } => {
                format!("receipt JSON is malformed in store {store_label}")
            }
            Self::WrongSchema { schema, .. } => {
                format!("receipt has unsupported schema in store {store_label}: {schema}")
            }
            Self::MalformedReceipt { .. } => {
                format!("receipt shape is invalid in store {store_label}")
            }
            Self::IdFilenameMismatch { .. } => {
                format!("receipt id does not match file name in store {store_label}")
            }
            Self::ReceiptProofInvalid { .. } => {
                format!("receipt proof is invalid in store {store_label}")
            }
            Self::ReceiptAlreadyExists { .. } => {
                format!("receipt already exists with different content in store {store_label}")
            }
            Self::MalformedIndex { .. } => {
                format!("receipt store index is malformed in store {store_label}")
            }
            Self::ReceiptIndexStale { .. } => {
                format!("receipt store index is stale in store {store_label}")
            }
            Self::UnsafePathProjection { .. } => {
                "receipt store path cannot be projected safely".to_owned()
            }
            Self::ReceiptNotInStore { receipt_id, .. } => {
                format!("receipt {receipt_id} was not found in store {store_label}")
            }
            Self::ReceiptBeforeThisRunx { .. } => self.to_string(),
        }
    }
}

fn receipt_file_name(receipt_id: &str) -> Result<String, ReceiptStoreError> {
    if let Some(digest) = receipt_id.strip_prefix(SHA256_RECEIPT_ID_PREFIX) {
        if is_sha256_hex_digest(digest) {
            return Ok(format!("{SHA256_RECEIPT_FILE_PREFIX}{digest}.json"));
        }
    } else if is_safe_literal_receipt_file_stem(receipt_id) {
        return Ok(format!("{receipt_id}.json"));
    }
    Err(ReceiptStoreError::InvalidReceiptId {
        receipt_id: receipt_id.to_owned(),
    })
}

/// One receipt tree of a store, verified.
#[derive(Clone, Debug)]
pub struct StoredReceiptTree {
    pub root_receipt_id: String,
    pub receipt_count: usize,
    pub verification: runx_receipts::ReceiptVerification,
}

/// A file named as a receipt that does not read as one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnreadableReceiptFile {
    pub file: PathBuf,
    pub message: String,
}

/// A store's receipt trees, each verified, the files that are not readable
/// receipts, and the receipts an earlier runx wrote, each that runx's to
/// verify.
#[derive(Clone, Debug)]
pub struct StoreTreeVerification {
    pub trees: Vec<StoredReceiptTree>,
    pub unreadable: Vec<UnreadableReceiptFile>,
    pub earlier: Vec<UnreadableReceiptFile>,
}

impl StoreTreeVerification {
    /// Every tree verifies and every receipt file reads, but an earlier
    /// runx's.
    #[must_use]
    pub fn valid(&self) -> bool {
        self.unreadable.is_empty() && self.trees.iter().all(|tree| tree.verification.valid)
    }
}

impl LocalReceiptStore {
    /// Every receipt tree in the store, or each one holding `receipt_id`,
    /// verified under `signature_policy`. Unreadable files are reported for
    /// the selected trees, including their signed child references. A whole-store
    /// audit reports every unreadable receipt file. The store is grouped from
    /// each receipt's links alone, and each tree is verified as its receipts
    /// are read, so the audit holds the path a walk is on, never a tree.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be listed, or when no tree
    /// holds `receipt_id`.
    pub fn verify_trees(
        &self,
        receipt_id: Option<&str>,
        signature_policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<StoreTreeVerification, ReceiptStoreError> {
        let unreadable_store = |source| ReceiptStoreError::StoreUnreadable {
            path: self.root.clone(),
            source,
        };
        let mut links = Vec::new();
        let mut unreadable = Vec::new();
        let mut earlier = Vec::new();
        // The receipt asked for, when an earlier runx wrote it.
        let mut asked_earlier = None;
        for entry in fs::read_dir(&self.root).map_err(unreadable_store)? {
            let path = entry.map_err(unreadable_store)?.path();
            let Some(expected_id) = path
                .file_stem()
                .and_then(OsStr::to_str)
                .filter(|_| is_receipt_json_path(&path))
                .and_then(receipt_id_from_file_stem)
            else {
                continue;
            };
            match read_receipt_file_without_proof(&path, &expected_id) {
                Ok(receipt) => links.push(runx_receipts::ReceiptLinks::of(&receipt)),
                Err(error @ ReceiptStoreError::ReceiptBeforeThisRunx { .. }) => {
                    earlier.push(UnreadableReceiptFile {
                        file: path,
                        message: error.to_string(),
                    });
                    if receipt_id == Some(expected_id.as_str()) {
                        asked_earlier = Some(error);
                    }
                }
                Err(error) => unreadable.push(UnreadableReceiptFile {
                    file: path,
                    message: error.to_string(),
                }),
            }
        }
        links.sort_by(|left, right| left.id.cmp(&right.id));
        let groups = runx_receipts::group_receipt_trees(&links)
            .into_iter()
            .filter(|group| receipt_id.is_none_or(|id| group.member_ids.contains(id)))
            .collect::<Vec<_>>();
        if let Some(receipt_id) = receipt_id
            && groups.is_empty()
        {
            if let Some(error) = asked_earlier {
                return Err(error);
            }
            return Err(ReceiptStoreError::ReceiptNotInStore {
                receipt_id: receipt_id.to_owned(),
                path: self.root.clone(),
            });
        }
        if receipt_id.is_some() {
            let member_ids = groups
                .iter()
                .flat_map(|group| group.member_ids.iter().map(String::as_str))
                .collect::<BTreeSet<_>>();
            let mut relevant = member_ids
                .iter()
                .map(|id| super::archive::receipt_uri(id))
                .collect::<BTreeSet<_>>();
            relevant.extend(
                links
                    .iter()
                    .filter(|receipt| member_ids.contains(receipt.id.as_str()))
                    .flat_map(|receipt| receipt.children.iter())
                    .map(|child| super::archive::receipt_uri(child)),
            );
            let relevant_file = |issue: &UnreadableReceiptFile| {
                issue
                    .file
                    .file_stem()
                    .and_then(OsStr::to_str)
                    .and_then(receipt_id_from_file_stem)
                    .is_some_and(|id| relevant.contains(&super::archive::receipt_uri(&id)))
            };
            unreadable.retain(relevant_file);
            earlier.retain(relevant_file);
        }
        drop(links);
        let read = |id: &str| {
            let path = self.receipt_path(id).ok()?;
            read_receipt_file_without_proof(&path, id).ok()
        };
        let mut trees = Vec::with_capacity(groups.len());
        for group in groups {
            // The root anchors the walk; it is read again, as its children are.
            let Some(root) = read(&group.root_id) else {
                unreadable.push(UnreadableReceiptFile {
                    file: self.receipt_path(&group.root_id)?,
                    message: "the receipt could not be read again to verify its tree".to_owned(),
                });
                continue;
            };
            let verification = super::tree::verify_runtime_receipt_tree_reading(
                &root,
                |child_id| {
                    group
                        .member_ids
                        .contains(child_id)
                        .then(|| read(child_id))
                        .flatten()
                },
                signature_policy,
            )
            .verification;
            trees.push(StoredReceiptTree {
                root_receipt_id: group.root_id,
                receipt_count: group.member_ids.len(),
                verification,
            });
        }
        Ok(StoreTreeVerification {
            trees,
            unreadable,
            earlier,
        })
    }
}

fn is_receipt_json_path(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("json"))
        && path
            .file_name()
            .is_some_and(|file_name| file_name != OsStr::new(INDEX_FILE_NAME))
        && path
            .file_stem()
            .and_then(OsStr::to_str)
            .and_then(receipt_id_from_file_stem)
            .is_some()
}

fn receipt_id_from_file_stem(stem: &str) -> Option<String> {
    if let Some(digest) = stem.strip_prefix(SHA256_RECEIPT_FILE_PREFIX) {
        return is_sha256_hex_digest(digest).then(|| format!("{SHA256_RECEIPT_ID_PREFIX}{digest}"));
    };
    None
}

fn is_sha256_hex_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_safe_literal_receipt_file_stem(stem: &str) -> bool {
    !(stem.is_empty() || stem == "." || stem == "..")
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn read_receipt_file(
    path: &Path,
    expected_id: &str,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Receipt, ReceiptStoreError> {
    let contents = fs::read_to_string(path).map_err(|source| {
        if source.kind() == ErrorKind::NotFound {
            ReceiptStoreError::MissingReceipt {
                path: path.to_path_buf(),
            }
        } else {
            ReceiptStoreError::ReceiptUnreadable {
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    parse_receipt_contents(&contents, path, expected_id, signature_policy)
}

fn read_receipt_file_without_proof(
    path: &Path,
    expected_id: &str,
) -> Result<Receipt, ReceiptStoreError> {
    let contents = fs::read_to_string(path).map_err(|source| {
        if source.kind() == ErrorKind::NotFound {
            ReceiptStoreError::MissingReceipt {
                path: path.to_path_buf(),
            }
        } else {
            ReceiptStoreError::ReceiptUnreadable {
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    parse_receipt_contents_without_proof(&contents, path, expected_id)
}

/// The index a log holds: its header, then its entries. An entry a retried
/// write appended again is the same entry; one that differs is a stale index.
fn parse_index(contents: &str, path: &Path) -> Result<ReceiptStoreIndex, ReceiptStoreError> {
    let malformed = |message: String| ReceiptStoreError::MalformedIndex {
        path: path.to_path_buf(),
        message,
    };
    let stale = |message: &str| ReceiptStoreError::ReceiptIndexStale {
        path: path.to_path_buf(),
        message: message.to_owned(),
    };
    if !contents.ends_with('\n') {
        return Err(stale("the index ends partway through an entry"));
    }
    let mut lines = contents.lines();
    let header: IndexHeader = lines
        .next()
        .ok_or_else(|| malformed("the index names no schema".to_owned()))
        .and_then(|line| {
            serde_json::from_str(line).map_err(|error| malformed(error.to_string()))
        })?;
    if header.schema != RECEIPT_STORE_INDEX_SCHEMA {
        return Err(malformed(format!(
            "unsupported index schema {}",
            header.schema
        )));
    }
    let mut entries = BTreeMap::<String, ReceiptStoreIndexEntry>::new();
    let mut read = 0usize;
    for line in lines {
        read += 1;
        let entry: ReceiptStoreIndexEntry =
            serde_json::from_str(line).map_err(|error| malformed(error.to_string()))?;
        if entry.file_name != receipt_file_name(&entry.receipt_id)? {
            return Err(stale("index file name does not match receipt id"));
        }
        match entries.get(&entry.receipt_id) {
            Some(known) if *known != entry => {
                return Err(stale("index entry conflicts with receipt projection"));
            }
            Some(_) => {}
            None => {
                entries.insert(entry.receipt_id.clone(), entry);
            }
        }
    }
    crate::work_counts::count_many(crate::work_counts::Work::ReceiptIndexEntryRead, read);
    Ok(ReceiptStoreIndex {
        schema: header.schema,
        entries: entries.into_values().collect(),
    })
}

/// Whether `file` ends with a whole line, so an append can't join a torn
/// entry.
fn ends_a_line(file: &mut File) -> Result<bool, std::io::Error> {
    use std::io::{Read, Seek, SeekFrom};
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
}

fn parse_receipt_contents(
    contents: &str,
    path: &Path,
    expected_id: &str,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Receipt, ReceiptStoreError> {
    let receipt = parse_receipt_contents_without_proof(contents, path, expected_id)?;
    verify_stored_receipt_proof(path, &receipt, signature_policy)?;
    Ok(receipt)
}

fn parse_receipt_contents_without_proof(
    contents: &str,
    path: &Path,
    expected_id: &str,
) -> Result<Receipt, ReceiptStoreError> {
    let value = serde_json::from_str::<serde_json::Value>(contents).map_err(|source| {
        ReceiptStoreError::MalformedJson {
            path: path.to_path_buf(),
            message: source.to_string(),
        }
    })?;
    let schema = value
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<missing>");
    if schema != RECEIPT_SCHEMA {
        return Err(ReceiptStoreError::WrongSchema {
            path: path.to_path_buf(),
            schema: schema.to_owned(),
        });
    }
    let receipt =
        runx_receipts::decode_receipt_value(value).map_err(
            |source| match written_before_this_runx(contents, expected_id) {
                Some(runx_version) => ReceiptStoreError::ReceiptBeforeThisRunx {
                    path: path.to_path_buf(),
                    receipt_id: expected_id.to_owned(),
                    runx_version,
                },
                None => ReceiptStoreError::MalformedReceipt {
                    path: path.to_path_buf(),
                    message: source.to_string(),
                },
            },
        )?;
    if receipt.id != expected_id {
        return Err(ReceiptStoreError::IdFilenameMismatch {
            path: path.to_path_buf(),
            receipt_id: receipt.id.into_string(),
            file_stem: expected_id.to_owned(),
        });
    }
    Ok(receipt)
}

/// The runx release that wrote the receipt in `contents`, when it is one
/// before 0.12 and the receipt is whole: it commits its own content address,
/// `expected_id`, as that runx wrote it. Receipts keep their words, so this
/// runx names that one rather than read them.
fn written_before_this_runx(contents: &str, expected_id: &str) -> Option<String> {
    let receipt = serde_json::from_str::<serde_json::Value>(contents).ok()?;
    let runx_version = receipt.get("issuer")?.get("runx_version")?.as_str()?;
    let mut release = runx_version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u64>().ok());
    let (major, minor) = (release.next()??, release.next()??);
    let whole = receipt.get("id").and_then(serde_json::Value::as_str) == Some(expected_id)
        && runx_receipts::content_addressed_receipt_json_id(&receipt)
            .ok()
            .as_deref()
            == Some(expected_id);
    ((major, minor) < (0, 12) && whole).then(|| runx_version.to_owned())
}

fn verify_stored_receipt_proof(
    path: &Path,
    receipt: &Receipt,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<(), ReceiptStoreError> {
    let expected_id = content_addressed_receipt_id(receipt).map_err(|error| {
        ReceiptStoreError::ReceiptProofInvalid {
            path: path.to_path_buf(),
            receipt_id: receipt.id.to_string(),
            message: format!("receipt content address could not be recomputed: {error}"),
        }
    })?;
    if receipt.id != expected_id {
        return Err(ReceiptStoreError::ReceiptProofInvalid {
            path: path.to_path_buf(),
            receipt_id: receipt.id.to_string(),
            message: format!(
                "receipt id must match content address: expected {expected_id}, got {}",
                receipt.id
            ),
        });
    }
    let proof_contexts = RuntimeReceiptProofContextProvider::new(signature_policy);
    let context = proof_contexts.proof_context(receipt);
    let verification = verify_receipt_proof(receipt, &context);
    if verification.valid {
        Ok(())
    } else {
        Err(ReceiptStoreError::ReceiptProofInvalid {
            path: path.to_path_buf(),
            receipt_id: receipt.id.to_string(),
            message: format!("{:?}", verification.findings),
        })
    }
}

pub(crate) fn write_atomic(
    dir: &Path,
    file_name: &str,
    contents: &[u8],
) -> Result<(), ReceiptStoreError> {
    write_atomic_with(dir, file_name, contents, true)
}

fn write_atomic_cache(
    dir: &Path,
    file_name: &str,
    contents: &[u8],
) -> Result<(), ReceiptStoreError> {
    write_atomic_with(dir, file_name, contents, false)
}

fn write_atomic_batch(dir: &Path, entries: &[(String, Vec<u8>)]) -> Result<(), ReceiptStoreError> {
    let mut staged = Vec::with_capacity(entries.len());
    for (file_name, contents) in entries {
        let temp_path = dir.join(temp_file_name(file_name));
        if let Err(source) = write_temp_file(&temp_path, contents, true) {
            for path in &staged {
                let _ignored = fs::remove_file(path);
            }
            return Err(ReceiptStoreError::StoreUnreadable {
                path: dir.join(file_name),
                source,
            });
        }
        staged.push(temp_path);
    }
    for ((file_name, _), temp_path) in entries.iter().zip(&staged) {
        if let Err(source) = fs::rename(temp_path, dir.join(file_name)) {
            for path in &staged {
                let _ignored = fs::remove_file(path);
            }
            return Err(ReceiptStoreError::StoreUnreadable {
                path: dir.join(file_name),
                source,
            });
        }
    }
    sync_directory(dir).map_err(|source| ReceiptStoreError::StoreUnreadable {
        path: dir.to_path_buf(),
        source,
    })
}

fn write_atomic_with(
    dir: &Path,
    file_name: &str,
    contents: &[u8],
    durable: bool,
) -> Result<(), ReceiptStoreError> {
    let temp_name = temp_file_name(file_name);
    let temp_path = dir.join(&temp_name);
    let final_path = dir.join(file_name);
    let write_result = write_temp_file(&temp_path, contents, durable)
        .and_then(|()| fs::rename(&temp_path, &final_path))
        .and_then(|()| if durable { sync_directory(dir) } else { Ok(()) });
    if let Err(source) = write_result {
        let _ignored = fs::remove_file(&temp_path);
        return Err(ReceiptStoreError::StoreUnreadable {
            path: final_path,
            source,
        });
    }
    Ok(())
}

fn write_temp_file(path: &Path, contents: &[u8], durable: bool) -> Result<(), std::io::Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.flush()?;
    if durable {
        file.sync_all()?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    // On Windows, opening a directory handle and calling sync_all fails with
    // ERROR_ACCESS_DENIED (os error 5). Receipt bytes are already durable via
    // file.sync_all() in write_temp_file; skip directory fsync on Windows.
    #[cfg(windows)]
    {
        let _ = path;
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        File::open(path)?.sync_all()
    }
}

fn temp_file_name(file_name: &str) -> String {
    format!(".{file_name}.tmp.{}-{}", std::process::id(), unix_nanos())
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::adapter::{InvocationOutput, InvocationStatus};
    use crate::receipts::{RunScope, RuntimeReceiptSignatureConfig, step_receipt};

    /// `count` distinct sealed receipts.
    fn receipts(signature: &RuntimeReceiptSignatureConfig, count: usize) -> Vec<Receipt> {
        let output = InvocationOutput::process(
            InvocationStatus::Success,
            "ok".to_owned(),
            String::new(),
            Some(0),
            1,
        );
        (0..count)
            .map(|index| {
                step_receipt(
                    RunScope {
                        run_id: "run_index",
                        workflow_name: "index",
                    },
                    &format!("step-{index}"),
                    1,
                    &output,
                    &runx_contracts::JsonObject::new(),
                    "2026-11-09T00:00:00Z",
                    signature.signature_policy(),
                )
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn a_receipt_append_adds_to_the_index_and_reads_none_of_it() {
        let test = concat!(
            module_path!(),
            "::a_receipt_append_adds_to_the_index_and_reads_none_of_it"
        );
        // What one receipt's append lists of the directory, reads of the
        // index and writes to it, after `history` receipts.
        let (small, large) = crate::work_counts::alone(test, || {
            let signature = RuntimeReceiptSignatureConfig::fixture().unwrap();
            let append = |history: usize| -> (u64, u64, u64) {
                let directory = tempfile::tempdir().unwrap();
                let store = LocalReceiptStore::new(directory.path().join("receipts"));
                let receipts = receipts(&signature, history + 1);
                store
                    .write_receipts(&receipts[..history], signature.signature_policy())
                    .unwrap();
                let before = crate::work_counts::snapshot();
                store
                    .write_receipt(&receipts[history], signature.signature_policy())
                    .unwrap();
                let after = crate::work_counts::snapshot();
                (
                    after.receipt_directory_entries_read - before.receipt_directory_entries_read,
                    after.receipt_index_entries_read - before.receipt_index_entries_read,
                    after.receipt_index_entries_written - before.receipt_index_entries_written,
                )
            };
            (append(10), append(1_000))
        });
        assert_eq!(small, large, "an append's work doesn't grow with the store");
        assert_eq!(small, (0, 0, 1));
    }
}
