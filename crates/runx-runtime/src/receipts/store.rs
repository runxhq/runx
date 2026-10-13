// Module rationale: local store read/write/index semantics stay
// together.
use std::collections::BTreeMap;
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

use super::paths::{
    ReceiptStoreLabel, ReceiptStorePublicProjection, safe_receipt_store_projection,
};
use super::seal::{RuntimeReceiptProofContextProvider, RuntimeReceiptSignaturePolicy};
use crate::paths::RunxPaths;

const RECEIPT_STORE_INDEX_SCHEMA: &str = "runx.receipt_store_index.v1";
const INDEX_FILE_NAME: &str = "index.json";
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

    #[must_use]
    pub fn public_projection(&self, paths: &RunxPaths) -> ReceiptStorePublicProjection {
        safe_receipt_store_projection(&self.root, paths)
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
            receipts.push(read_receipt_file(&path, &receipt_id, signature_policy)?);
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
            receipts.push(read_receipt_file_without_proof(&path, &receipt_id)?);
        }
        receipts.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(receipts)
    }

    pub(crate) fn read_exact_without_proof_for_history(
        &self,
        receipt_id: &str,
    ) -> Result<Receipt, ReceiptStoreError> {
        let file_path = self.receipt_path(receipt_id)?;
        self.ensure_store_dir()?;
        read_receipt_file_without_proof(&file_path, receipt_id)
    }

    pub fn load_index(
        &self,
        signature_policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<ReceiptStoreIndex, ReceiptStoreError> {
        self.ensure_store_dir()?;
        let _lock = self.lock_mutations()?;
        let index_path = self.index_path();
        let contents = match fs::read_to_string(&index_path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return self.rebuild_index_from_receipts(self.list(signature_policy)?);
            }
            Err(source) => {
                return Err(ReceiptStoreError::StoreUnreadable {
                    path: index_path,
                    source,
                });
            }
        };
        let index = parse_index(&contents, &index_path)?;
        self.verify_index(&index, signature_policy)?;
        Ok(index)
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

    fn rebuild_index_from_receipts(
        &self,
        receipts: Vec<Receipt>,
    ) -> Result<ReceiptStoreIndex, ReceiptStoreError> {
        let entries = receipts
            .into_iter()
            .map(|receipt| index_entry(&receipt))
            .collect::<Result<Vec<_>, ReceiptStoreError>>()?;
        let index = ReceiptStoreIndex {
            schema: RECEIPT_STORE_INDEX_SCHEMA.to_owned(),
            generated_at: generated_at_nanos(),
            entries,
        };
        self.write_index(&index)?;
        Ok(index)
    }

    fn verify_index(
        &self,
        index: &ReceiptStoreIndex,
        signature_policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<(), ReceiptStoreError> {
        let listed = self.list(signature_policy)?;
        let listed_entries = listed
            .iter()
            .map(index_entry)
            .collect::<Result<Vec<_>, ReceiptStoreError>>()?;
        if listed_entries != index.entries {
            return Err(ReceiptStoreError::ReceiptIndexStale {
                path: self.index_path(),
                message: "index entries do not match receipt JSON files".to_owned(),
            });
        }
        Ok(())
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
            // listings, index loads, and audits.
            Err(_) => match self.rebuild_index_projection() {
                Ok(_) => Ok(()),
                Err(error) => Err(ReceiptStoreError::ReceiptIndexStale {
                    path: self.index_path(),
                    message: error.to_string(),
                }),
            },
        }
    }

    fn append_index_entries(
        &self,
        entries: &[ReceiptStoreIndexEntry],
    ) -> Result<(), ReceiptStoreError> {
        let receipt_count = self.receipt_file_count()?;
        let mut index = match self.read_index_without_verification() {
            Ok(index) => index,
            Err(ReceiptStoreError::StoreUnreadable { source, .. })
                if source.kind() == ErrorKind::NotFound && receipt_count == entries.len() =>
            {
                ReceiptStoreIndex {
                    schema: RECEIPT_STORE_INDEX_SCHEMA.to_owned(),
                    generated_at: generated_at_nanos(),
                    entries: Vec::new(),
                }
            }
            Err(error) => return Err(error),
        };
        ensure_index_shape_for_append(&index)?;
        let (merged, changed) = merge_index_entries(std::mem::take(&mut index.entries), entries)?;
        if receipt_count != merged.len() {
            return Err(ReceiptStoreError::ReceiptIndexStale {
                path: self.index_path(),
                message: "index entries do not cover every receipt JSON file".to_owned(),
            });
        }
        if !changed {
            return Ok(());
        }
        index.entries = merged;
        index.generated_at = generated_at_nanos();
        self.write_index(&index)
    }

    fn read_index_without_verification(&self) -> Result<ReceiptStoreIndex, ReceiptStoreError> {
        let index_path = self.index_path();
        let contents = fs::read_to_string(&index_path).map_err(|source| {
            ReceiptStoreError::StoreUnreadable {
                path: index_path.clone(),
                source,
            }
        })?;
        parse_index(&contents, &index_path)
    }

    fn receipt_file_count(&self) -> Result<usize, ReceiptStoreError> {
        let mut count = 0usize;
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
            if is_receipt_json_path(&path) {
                count += 1;
            }
        }
        Ok(count)
    }

    fn write_index(&self, index: &ReceiptStoreIndex) -> Result<(), ReceiptStoreError> {
        let contents =
            serde_json::to_vec(index).map_err(|source| ReceiptStoreError::MalformedIndex {
                path: self.index_path(),
                message: source.to_string(),
            })?;
        // `index.json` is a recoverable projection of receipt files. Receipt
        // writes remain durable; index writes stay atomic but skip fsync so an
        // append does not pay the full durability cost twice.
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptStoreIndex {
    pub schema: String,
    pub generated_at: String,
    pub entries: Vec<ReceiptStoreIndexEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// A store's receipt trees, each verified, and the files that are not
/// readable receipts.
#[derive(Clone, Debug)]
pub struct StoreTreeVerification {
    pub trees: Vec<StoredReceiptTree>,
    pub unreadable: Vec<UnreadableReceiptFile>,
}

impl StoreTreeVerification {
    /// Every tree verifies and every receipt file reads.
    #[must_use]
    pub fn valid(&self) -> bool {
        self.unreadable.is_empty() && self.trees.iter().all(|tree| tree.verification.valid)
    }
}

impl LocalReceiptStore {
    /// Every receipt tree in the store, or each one holding `receipt_id`,
    /// verified under `signature_policy`. A file that does not read as a
    /// receipt is reported, not fatal: one bad file never hides the rest.
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
        let mut receipts = Vec::new();
        let mut unreadable = Vec::new();
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
                Ok(receipt) => receipts.push(receipt),
                Err(error) => unreadable.push(UnreadableReceiptFile {
                    file: path,
                    message: error.to_string(),
                }),
            }
        }
        receipts.sort_by(|left, right| left.id.cmp(&right.id));
        let groups = runx_receipts::group_receipt_trees(&receipts)
            .into_iter()
            .filter(|group| receipt_id.is_none_or(|id| group.member_ids.contains(id)))
            .collect::<Vec<_>>();
        if let Some(receipt_id) = receipt_id
            && groups.is_empty()
        {
            return Err(ReceiptStoreError::ReceiptNotInStore {
                receipt_id: receipt_id.to_owned(),
                path: self.root.clone(),
            });
        }
        let trees = groups
            .into_iter()
            .map(|group| {
                // The root anchors the traversal; it is never its own child.
                let children = receipts
                    .iter()
                    .filter(|receipt| {
                        receipt.id != group.root.id
                            && group.member_ids.contains(receipt.id.as_str())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                StoredReceiptTree {
                    root_receipt_id: group.root.id.to_string(),
                    receipt_count: group.member_ids.len(),
                    verification: super::tree::verify_runtime_receipt_tree(
                        &group.root,
                        children,
                        runx_receipts::ReceiptTreeConfig::default(),
                        signature_policy,
                    ),
                }
            })
            .collect();
        Ok(StoreTreeVerification { trees, unreadable })
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

fn parse_index(contents: &str, path: &Path) -> Result<ReceiptStoreIndex, ReceiptStoreError> {
    let index = serde_json::from_str::<ReceiptStoreIndex>(contents).map_err(|source| {
        ReceiptStoreError::MalformedIndex {
            path: path.to_path_buf(),
            message: source.to_string(),
        }
    })?;
    if index.schema != RECEIPT_STORE_INDEX_SCHEMA {
        return Err(ReceiptStoreError::MalformedIndex {
            path: path.to_path_buf(),
            message: format!("unsupported index schema {}", index.schema),
        });
    }
    Ok(index)
}

fn ensure_index_shape_for_append(index: &ReceiptStoreIndex) -> Result<(), ReceiptStoreError> {
    let mut previous_id: Option<&str> = None;
    for entry in &index.entries {
        let expected_file_name = receipt_file_name(&entry.receipt_id)?;
        if entry.file_name != expected_file_name {
            return Err(ReceiptStoreError::ReceiptIndexStale {
                path: PathBuf::from(INDEX_FILE_NAME),
                message: "index file name does not match receipt id".to_owned(),
            });
        }
        if previous_id.is_some_and(|previous| previous >= entry.receipt_id.as_str()) {
            return Err(ReceiptStoreError::ReceiptIndexStale {
                path: PathBuf::from(INDEX_FILE_NAME),
                message: "index receipt ids must be sorted and unique".to_owned(),
            });
        }
        previous_id = Some(entry.receipt_id.as_str());
    }
    Ok(())
}

fn merge_index_entries(
    existing: Vec<ReceiptStoreIndexEntry>,
    additions: &[ReceiptStoreIndexEntry],
) -> Result<(Vec<ReceiptStoreIndexEntry>, bool), ReceiptStoreError> {
    let mut additions = additions.to_vec();
    additions.sort_by(|left, right| left.receipt_id.cmp(&right.receipt_id));
    if additions
        .windows(2)
        .any(|pair| pair[0].receipt_id == pair[1].receipt_id)
    {
        return Err(ReceiptStoreError::ReceiptIndexStale {
            path: PathBuf::from(INDEX_FILE_NAME),
            message: "new receipt ids must be unique".to_owned(),
        });
    }

    let mut existing = existing.into_iter().peekable();
    let mut additions = additions.into_iter().peekable();
    let mut merged = Vec::with_capacity(existing.len().saturating_add(additions.len()));
    let mut changed = false;
    loop {
        match (existing.peek(), additions.peek()) {
            (Some(left), Some(right)) => match left.receipt_id.cmp(&right.receipt_id) {
                std::cmp::Ordering::Less => {
                    let entry = existing.next().ok_or_else(|| {
                        index_merge_error("existing index merge iterator changed unexpectedly")
                    })?;
                    merged.push(entry);
                }
                std::cmp::Ordering::Greater => {
                    let entry = additions.next().ok_or_else(|| {
                        index_merge_error("new index merge iterator changed unexpectedly")
                    })?;
                    merged.push(entry);
                    changed = true;
                }
                std::cmp::Ordering::Equal => {
                    let existing_entry = existing.next().ok_or_else(|| {
                        index_merge_error("existing index merge iterator changed unexpectedly")
                    })?;
                    let addition = additions.next().ok_or_else(|| {
                        index_merge_error("new index merge iterator changed unexpectedly")
                    })?;
                    if existing_entry != addition {
                        return Err(ReceiptStoreError::ReceiptIndexStale {
                            path: PathBuf::from(INDEX_FILE_NAME),
                            message: "index entry conflicts with receipt projection".to_owned(),
                        });
                    }
                    merged.push(existing_entry);
                }
            },
            (Some(_), None) => {
                merged.extend(existing);
                break;
            }
            (None, Some(_)) => {
                merged.extend(additions);
                changed = true;
                break;
            }
            (None, None) => break,
        }
    }
    Ok((merged, changed))
}

fn index_merge_error(message: &str) -> ReceiptStoreError {
    ReceiptStoreError::ReceiptIndexStale {
        path: PathBuf::from(INDEX_FILE_NAME),
        message: message.to_owned(),
    }
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
    let receipt = runx_receipts::decode_receipt_value(value).map_err(|source| {
        ReceiptStoreError::MalformedReceipt {
            path: path.to_path_buf(),
            message: source.to_string(),
        }
    })?;
    if receipt.id != expected_id {
        return Err(ReceiptStoreError::IdFilenameMismatch {
            path: path.to_path_buf(),
            receipt_id: receipt.id.into_string(),
            file_stem: expected_id.to_owned(),
        });
    }
    Ok(receipt)
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

fn generated_at_nanos() -> String {
    unix_nanos().to_string()
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}
