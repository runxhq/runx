// Module rationale: a runx home's own receipt identity. The private seed is
// created once, closed to other users, and never replaced: a key that is
// unreadable, malformed or open to others stops local signing.
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use runx_contracts::ReceiptIssuerType;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use super::signing::{
    Ed25519ReceiptSigner, RuntimeReceiptSigningError, TrustedReceiptKey, local_receipt_kid,
};
use crate::paths::RunxPaths;

pub const LOCAL_RECEIPT_KEY_SCHEMA: &str = "runx.local_receipt_key.v1";

/// A key document is a few hundred bytes; anything larger is not one.
const MAX_KEY_DOCUMENT_BYTES: u64 = 4096;

/// Why a local receipt key cannot be used.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum LocalReceiptKeyFault {
    #[error("is a link")]
    Link,
    #[error("is not a regular file")]
    NotAFile,
    #[error("is not owned by the current user")]
    Owner,
    #[error("is open to other users")]
    Permissions,
    #[error("is not a {LOCAL_RECEIPT_KEY_SCHEMA} document")]
    Malformed,
    #[error("names a public key its seed does not derive")]
    PublicKeyMismatch,
    #[error("cannot be restricted to its owner on this platform")]
    Unprotectable,
    #[error("is in {directory}, which {problem}")]
    Directory {
        directory: String,
        problem: KeyDirectoryProblem,
    },
    #[error("could not be read or written: {0}")]
    Io(String),
}

/// Why the directory that holds a local receipt key cannot hold one.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum KeyDirectoryProblem {
    #[error("is a link")]
    Link,
    #[error("is not a directory")]
    NotADirectory,
    #[error("is not owned by the current user")]
    Owner,
    #[error("other users can write to; restrict it with chmod 700")]
    Writable,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalReceiptKeyDocument {
    schema: String,
    seed: String,
    public_key: String,
}

/// The home's signer, its key created on first use. Two processes creating
/// it at once publish one key: each writes a private file and links it into
/// place without replacing, and the loser reads the winner's.
pub(crate) fn load_or_create_local_signer(
    paths: &RunxPaths,
) -> Result<Ed25519ReceiptSigner, RuntimeReceiptSigningError> {
    let path = paths.local_receipt_key();
    if let Some(seed) = read_seed(&path)? {
        return Ed25519ReceiptSigner::local(&seed);
    }
    create(&path).map_err(|fault| key_error(&path, fault))?;
    let seed = read_seed(&path)?.ok_or_else(|| {
        key_error(
            &path,
            LocalReceiptKeyFault::Io(io::ErrorKind::NotFound.to_string()),
        )
    })?;
    Ed25519ReceiptSigner::local(&seed)
}

/// The home key's public half, when the home has one. Reading trust never
/// creates a key.
pub(crate) fn load_local_trusted_key(
    paths: &RunxPaths,
) -> Result<Option<TrustedReceiptKey>, RuntimeReceiptSigningError> {
    let path = paths.local_receipt_key();
    let Some(seed) = read_seed(&path)? else {
        return Ok(None);
    };
    let signer = Ed25519ReceiptSigner::local(&seed)?;
    Ok(Some(TrustedReceiptKey::new(
        ReceiptIssuerType::Local,
        local_receipt_kid(signer.public_key()),
        signer.public_key().to_vec(),
    )))
}

fn read_seed(path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, RuntimeReceiptSigningError> {
    let Some(mut file) = platform::open_private(path).map_err(|fault| key_error(path, fault))?
    else {
        return Ok(None);
    };
    let mut bytes = Zeroizing::new(Vec::new());
    Read::take(&mut file, MAX_KEY_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| key_error(path, io_fault(&error)))?;
    if bytes.len() as u64 > MAX_KEY_DOCUMENT_BYTES {
        return Err(key_error(path, LocalReceiptKeyFault::Malformed));
    }
    parse(&bytes)
        .map(Some)
        .map_err(|fault| key_error(path, fault))
}

fn parse(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, LocalReceiptKeyFault> {
    let LocalReceiptKeyDocument {
        schema,
        seed,
        public_key,
    } = serde_json::from_slice(bytes).map_err(|_| LocalReceiptKeyFault::Malformed)?;
    let seed = Zeroizing::new(seed);
    if schema != LOCAL_RECEIPT_KEY_SCHEMA {
        return Err(LocalReceiptKeyFault::Malformed);
    }
    let seed = Zeroizing::new(
        STANDARD
            .decode(seed.as_bytes())
            .map_err(|_| LocalReceiptKeyFault::Malformed)?,
    );
    let public_key = STANDARD
        .decode(public_key.as_bytes())
        .map_err(|_| LocalReceiptKeyFault::Malformed)?;
    if seed.len() != 32 {
        return Err(LocalReceiptKeyFault::Malformed);
    }
    let key_pair =
        Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| LocalReceiptKeyFault::Malformed)?;
    if key_pair.public_key().as_ref() != public_key.as_slice() {
        return Err(LocalReceiptKeyFault::PublicKeyMismatch);
    }
    Ok(seed)
}

fn create(path: &Path) -> Result<(), LocalReceiptKeyFault> {
    let directory = path.parent().ok_or(LocalReceiptKeyFault::NotAFile)?;
    platform::ensure_private_directory(directory)?;
    let random = SystemRandom::new();
    let mut seed = Zeroizing::new([0_u8; 32]);
    random
        .fill(seed.as_mut())
        .map_err(|_| LocalReceiptKeyFault::Io("no system randomness".to_owned()))?;
    let key_pair = Ed25519KeyPair::from_seed_unchecked(seed.as_ref())
        .map_err(|_| LocalReceiptKeyFault::Malformed)?;
    let document = Zeroizing::new(
        serde_json::to_vec(&LocalReceiptKeyDocument {
            schema: LOCAL_RECEIPT_KEY_SCHEMA.to_owned(),
            seed: STANDARD.encode(seed.as_ref()),
            public_key: STANDARD.encode(key_pair.public_key().as_ref()),
        })
        .map_err(|_| LocalReceiptKeyFault::Malformed)?,
    );
    let mut nonce = [0_u8; 8];
    random
        .fill(&mut nonce)
        .map_err(|_| LocalReceiptKeyFault::Io("no system randomness".to_owned()))?;
    let staged = directory.join(format!(
        ".local-receipt-ed25519.{}.tmp",
        nonce
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ));
    let published = write_staged(&staged, &document).and_then(|()| {
        match fs::hard_link(&staged, path) {
            Ok(()) => Ok(()),
            // Another process published first; its key is the home's key.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(io_fault(&error)),
        }
    });
    let removed = match fs::remove_file(&staged) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_fault(&error)),
    };
    published?;
    removed?;
    platform::sync_directory(directory)
}

fn write_staged(path: &Path, document: &[u8]) -> Result<(), LocalReceiptKeyFault> {
    let mut file = platform::create_private(path)?;
    file.write_all(document)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_fault(&error))
}

fn io_fault(error: &io::Error) -> LocalReceiptKeyFault {
    LocalReceiptKeyFault::Io(error.kind().to_string())
}

fn key_error(path: &Path, fault: LocalReceiptKeyFault) -> RuntimeReceiptSigningError {
    RuntimeReceiptSigningError::LocalKey {
        path: path.display().to_string(),
        fault,
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    use std::path::Path;

    use super::{KeyDirectoryProblem, LocalReceiptKeyFault, io_fault};

    pub(super) fn ensure_private_directory(directory: &Path) -> Result<(), LocalReceiptKeyFault> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|error| io_fault(&error))?;
        check_private_directory(directory)
    }

    /// The key's directory is a real directory this user owns, and no one
    /// else may add or replace a file in it.
    fn check_private_directory(directory: &Path) -> Result<(), LocalReceiptKeyFault> {
        let metadata = fs::symlink_metadata(directory).map_err(|error| io_fault(&error))?;
        let refuse = |problem| {
            Err(LocalReceiptKeyFault::Directory {
                directory: directory.display().to_string(),
                problem,
            })
        };
        if metadata.file_type().is_symlink() {
            return refuse(KeyDirectoryProblem::Link);
        }
        if !metadata.is_dir() {
            return refuse(KeyDirectoryProblem::NotADirectory);
        }
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return refuse(KeyDirectoryProblem::Owner);
        }
        // Others may list the directory; none may add or replace a key in it.
        if metadata.mode() & 0o022 != 0 {
            return refuse(KeyDirectoryProblem::Writable);
        }
        Ok(())
    }

    pub(super) fn create_private(path: &Path) -> Result<File, LocalReceiptKeyFault> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| io_fault(&error))
    }

    /// Open the key without following a link, then check the opened file is
    /// the one the path names, owned by this user and closed to others, in a
    /// directory no one else can write.
    pub(super) fn open_private(path: &Path) -> Result<Option<File>, LocalReceiptKeyFault> {
        let named = match fs::symlink_metadata(path) {
            Ok(named) => named,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_fault(&error)),
        };
        if named.file_type().is_symlink() {
            return Err(LocalReceiptKeyFault::Link);
        }
        if !named.is_file() {
            return Err(LocalReceiptKeyFault::NotAFile);
        }
        check_private_directory(path.parent().ok_or(LocalReceiptKeyFault::NotAFile)?)?;
        let file = File::open(path).map_err(|error| io_fault(&error))?;
        let opened = file.metadata().map_err(|error| io_fault(&error))?;
        if (opened.dev(), opened.ino()) != (named.dev(), named.ino()) {
            return Err(LocalReceiptKeyFault::Link);
        }
        if opened.uid() != rustix::process::geteuid().as_raw() {
            return Err(LocalReceiptKeyFault::Owner);
        }
        if opened.mode() & 0o077 != 0 {
            return Err(LocalReceiptKeyFault::Permissions);
        }
        Ok(Some(file))
    }

    pub(super) fn sync_directory(directory: &Path) -> Result<(), LocalReceiptKeyFault> {
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| io_fault(&error))
    }
}

/// Elsewhere, including Windows: no owner-only protection runx can set and
/// check without platform FFI, so local signing is refused, never weakened.
/// A configured hosted or CI identity still signs.
#[cfg(not(unix))]
mod platform {
    use std::fs::File;
    use std::path::Path;

    use super::LocalReceiptKeyFault;

    pub(super) fn ensure_private_directory(_directory: &Path) -> Result<(), LocalReceiptKeyFault> {
        Err(LocalReceiptKeyFault::Unprotectable)
    }

    pub(super) fn create_private(_path: &Path) -> Result<File, LocalReceiptKeyFault> {
        Err(LocalReceiptKeyFault::Unprotectable)
    }

    /// No key is ever created here, so a home has none to trust; a file in
    /// its place is refused rather than read unprotected.
    pub(super) fn open_private(path: &Path) -> Result<Option<File>, LocalReceiptKeyFault> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => Err(LocalReceiptKeyFault::Unprotectable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(super::io_fault(&error)),
        }
    }

    pub(super) fn sync_directory(_directory: &Path) -> Result<(), LocalReceiptKeyFault> {
        Err(LocalReceiptKeyFault::Unprotectable)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Barrier};

    use runx_receipts::SignatureVerifier;

    use super::*;
    use crate::paths::RUNX_HOME_ENV;
    use crate::receipts::signing::{
        ReceiptTrust, RuntimeReceiptSigner, receipt_trust, receipt_verifier,
    };

    fn home() -> Result<(tempfile::TempDir, RunxPaths), io::Error> {
        let root = tempfile::tempdir()?;
        let paths = RunxPaths::resolve(
            &BTreeMap::from([(RUNX_HOME_ENV.to_owned(), root.path().display().to_string())]),
            root.path(),
        );
        Ok((root, paths))
    }

    fn fault(error: RuntimeReceiptSigningError) -> Option<LocalReceiptKeyFault> {
        match error {
            RuntimeReceiptSigningError::LocalKey { fault, .. } => Some(fault),
            _ => None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn local_key_is_created_once() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, paths) = home()?;

        let first = load_or_create_local_signer(&paths)?;
        let document = fs::read(paths.local_receipt_key())?;
        let second = load_or_create_local_signer(&paths)?;

        assert_eq!(first.issuer(), second.issuer());
        assert_eq!(fs::read(paths.local_receipt_key())?, document);
        assert_eq!(first.issuer().issuer_type, ReceiptIssuerType::Local);
        assert_eq!(
            first.issuer().kid.as_str(),
            local_receipt_kid(first.public_key())
        );
        let leftovers = fs::read_dir(paths.keys())?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0, "no staged key file is left behind");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_processes_publish_one_local_key() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, paths) = home()?;
        let start = Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let paths = paths.clone();
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    load_or_create_local_signer(&paths).map(|signer| signer.issuer())
                })
            })
            .collect::<Vec<_>>();
        let issuers = handles
            .into_iter()
            .map(|handle| handle.join().map_err(|_| "a creator panicked"))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;

        assert!(issuers.windows(2).all(|pair| pair[0] == pair[1]));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn corrupt_or_unreadable_local_key_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, paths) = home()?;
        load_or_create_local_signer(&paths)?;
        let path = paths.local_receipt_key();
        let original = fs::read(&path)?;

        let mut document: serde_json::Value = serde_json::from_slice(&original)?;
        document["public_key"] = serde_json::Value::String(STANDARD.encode([7_u8; 32]));
        fs::write(&path, serde_json::to_vec(&document)?)?;
        assert_eq!(
            load_or_create_local_signer(&paths).err().and_then(fault),
            Some(LocalReceiptKeyFault::PublicKeyMismatch)
        );

        document["public_key"] =
            serde_json::from_slice::<serde_json::Value>(&original)?["public_key"].clone();
        document["extra"] = serde_json::Value::Bool(true);
        fs::write(&path, serde_json::to_vec(&document)?)?;
        assert_eq!(
            load_local_trusted_key(&paths).err().and_then(fault),
            Some(LocalReceiptKeyFault::Malformed)
        );

        fs::write(&path, b"not json")?;
        assert_eq!(
            load_or_create_local_signer(&paths).err().and_then(fault),
            Some(LocalReceiptKeyFault::Malformed)
        );
        // A broken key is never replaced: it stays exactly as it was found.
        assert_eq!(fs::read(&path)?, b"not json");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn insecure_local_key_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let (root, paths) = home()?;
        load_or_create_local_signer(&paths)?;
        let path = paths.local_receipt_key();
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(paths.keys())?.permissions().mode() & 0o777,
            0o700
        );

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        assert_eq!(
            load_or_create_local_signer(&paths).err().and_then(fault),
            Some(LocalReceiptKeyFault::Permissions)
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

        let elsewhere = root.path().join("elsewhere.json");
        fs::rename(&path, &elsewhere)?;
        symlink(&elsewhere, &path)?;
        assert_eq!(
            load_or_create_local_signer(&paths).err().and_then(fault),
            Some(LocalReceiptKeyFault::Link)
        );
        fs::remove_file(&path)?;
        fs::rename(&elsewhere, &path)?;

        // A key in a directory others can write is refused, not just a new
        // one, and the refusal names the directory and how to close it.
        fs::set_permissions(paths.keys(), fs::Permissions::from_mode(0o777))?;
        let writable = Some(LocalReceiptKeyFault::Directory {
            directory: paths.keys().display().to_string(),
            problem: KeyDirectoryProblem::Writable,
        });
        assert_eq!(
            load_or_create_local_signer(&paths).err().and_then(fault),
            writable
        );
        assert_eq!(
            load_local_trusted_key(&paths).err().and_then(fault),
            writable
        );
        fs::remove_file(&path)?;
        let refused = load_or_create_local_signer(&paths).err();
        assert_eq!(refused.clone().and_then(fault), writable);
        assert!(
            refused
                .map(|error| error.to_string())
                .is_some_and(|message| message.contains("chmod 700")),
        );
        Ok(())
    }

    #[cfg(not(unix))]
    #[test]
    fn local_signing_is_refused_where_the_key_cannot_be_protected()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, paths) = home()?;

        assert_eq!(
            load_or_create_local_signer(&paths).err().and_then(fault),
            Some(LocalReceiptKeyFault::Unprotectable)
        );
        assert!(!paths.local_receipt_key().exists());
        Ok(())
    }

    #[test]
    fn verification_never_creates_a_key() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, paths) = home()?;

        let trust = receipt_trust(&BTreeMap::new(), &paths, ReceiptTrust::HomeAndConfigured)?;

        assert!(trust.is_empty());
        assert!(!paths.local_receipt_key().exists());
        assert!(!paths.keys().exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn typed_trust_rejects_another_home_and_issuer_type() -> Result<(), Box<dyn std::error::Error>>
    {
        let (_root, paths) = home()?;
        let (_other_root, other) = home()?;
        let signer = load_or_create_local_signer(&paths)?;
        load_or_create_local_signer(&other)?;
        let digest = "sha256:receipt";
        let signature = signer.sign_receipt_body(digest)?;

        let own = receipt_verifier(&BTreeMap::new(), &paths, ReceiptTrust::HomeAndConfigured)?;
        assert_eq!(own.verify(&signer.issuer(), &signature, digest), Ok(()));
        let another = receipt_verifier(&BTreeMap::new(), &other, ReceiptTrust::HomeAndConfigured)?;
        assert_eq!(
            another.verify(&signer.issuer(), &signature, digest),
            Err(runx_receipts::SignatureVerificationFailure::MissingKey)
        );
        let configured_only = receipt_verifier(&BTreeMap::new(), &paths, ReceiptTrust::Configured)?;
        assert_eq!(
            configured_only.verify(&signer.issuer(), &signature, digest),
            Err(runx_receipts::SignatureVerificationFailure::MissingKey)
        );
        let mut hosted = signer.issuer();
        hosted.issuer_type = ReceiptIssuerType::Hosted;
        assert_eq!(
            own.verify(&hosted, &signature, digest),
            Err(runx_receipts::SignatureVerificationFailure::MissingKey)
        );
        Ok(())
    }
}
