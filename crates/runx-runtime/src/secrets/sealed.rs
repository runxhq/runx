//! What the runx home holds sealed: values encrypted with AES-256-GCM under
//! the home's own key, one private file each, and the proofs made with that
//! key. The key is a private file in the same home, so this guards against
//! accidental disclosure (config output, repository files, logs, receipts),
//! not against a program running as the operator.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, Generate, KeyInit, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::paths::RunxPaths;
use crate::private_file::{self, PrivateFileFault};

/// A sealed value or the home's key is a few hundred bytes.
const MAX_SEALED_BYTES: u64 = 64 * 1024;

#[derive(Debug, Error)]
pub enum SealedError {
    #[error("the runx key file {path} is corrupted or unreadable{suffix}")]
    Corrupt { path: PathBuf, suffix: String },
    #[error("runx key crypto failed: {0}")]
    Crypto(String),
    #[error("{path} {fault}")]
    PrivateFile {
        path: PathBuf,
        fault: PrivateFileFault,
    },
    #[error(transparent)]
    Io(#[from] io::Error),
}

fn private(path: &Path) -> impl FnOnce(PrivateFileFault) -> SealedError + '_ {
    move |fault| SealedError::PrivateFile {
        path: path.to_path_buf(),
        fault,
    }
}

#[derive(Deserialize)]
struct SealedPayload {
    alg: String,
    iv: String,
    ciphertext: String,
    auth_tag: String,
}

/// What a sealed file holds. `id` labels the file; reading never needs it,
/// so a file sealed by runx 0.10 under its `ref` label reads the same.
#[derive(Serialize)]
struct StoredSealedPayload<'a> {
    id: &'a str,
    alg: &'static str,
    iv: String,
    ciphertext: String,
    auth_tag: String,
}

type SealedNonce = Nonce<Aes256Gcm>;

/// Seal `value` in a new file of this home, and return its id,
/// `<kind>_<digest>`: a new id for every value sealed.
pub(crate) fn store(paths: &RunxPaths, value: &str, kind: &str) -> Result<String, SealedError> {
    let key_dir = paths.keys();
    let cipher = cipher(&key_dir)?;
    let nonce =
        SealedNonce::try_generate().map_err(|error| SealedError::Crypto(error.to_string()))?;
    let mut sealed = cipher
        .encrypt(&nonce, value.as_bytes())
        .map_err(|error| SealedError::Crypto(error.to_string()))?;
    let auth_tag = sealed.split_off(sealed.len().saturating_sub(16));
    let material_id = format!(
        "{kind}_{}",
        hex(&Sha256::digest(
            [nonce.as_slice(), sealed.as_slice()].concat()
        ))
        .chars()
        .take(24)
        .collect::<String>()
    );
    let payload = StoredSealedPayload {
        id: &material_id,
        alg: "aes-256-gcm",
        iv: URL_SAFE_NO_PAD.encode(nonce),
        ciphertext: URL_SAFE_NO_PAD.encode(sealed),
        auth_tag: URL_SAFE_NO_PAD.encode(auth_tag),
    };
    let contents = serde_json::to_string_pretty(&payload)
        .map_err(|error| SealedError::Crypto(error.to_string()))?;
    let path = key_dir.join(format!("{material_id}.json"));
    private_file::replace_private(&path, format!("{contents}\n").as_bytes())
        .map_err(private(&path))?;
    Ok(material_id)
}

/// The value sealed under `material_id`.
pub(crate) fn load(paths: &RunxPaths, material_id: &str) -> Result<String, SealedError> {
    let key_dir = paths.keys();
    let path = key_dir.join(format!("{material_id}.json"));
    let contents = private_file::read_private(&path, MAX_SEALED_BYTES)
        .map_err(private(&path))?
        .ok_or_else(|| corrupt(&path, Some("it does not exist".to_owned())))?;
    let payload: SealedPayload = serde_json::from_slice(&contents)
        .map_err(|error| corrupt(&path, Some(error.to_string())))?;
    if payload.alg != "aes-256-gcm" {
        return Err(corrupt(&path, None));
    }
    let part = |value: &str| {
        URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|error| corrupt(&path, Some(error.to_string())))
    };
    let nonce_bytes = part(&payload.iv)?;
    let nonce = SealedNonce::try_from(nonce_bytes.as_slice()).map_err(|_| {
        corrupt(
            &path,
            Some(format!(
                "expected 12-byte aes-256-gcm nonce, found {} bytes",
                nonce_bytes.len()
            )),
        )
    })?;
    let mut sealed = part(&payload.ciphertext)?;
    sealed.extend(part(&payload.auth_tag)?);
    let plaintext = cipher(&key_dir)?
        .decrypt(&nonce, sealed.as_ref())
        .map_err(|error| corrupt(&path, Some(error.to_string())))?;
    String::from_utf8(plaintext).map_err(|error| corrupt(&path, Some(error.to_string())))
}

/// Remove the file sealed under `material_id`, if it is there.
pub(crate) fn remove(paths: &RunxPaths, material_id: &str) -> Result<(), SealedError> {
    match fs::remove_file(paths.keys().join(format!("{material_id}.json"))) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SealedError::Io(error)),
    }
}

/// An HMAC, keyed by this home's key, over `message`: proof that this home
/// made a binding, which a project store cannot forge.
pub(crate) fn prove(paths: &RunxPaths, message: &[u8]) -> Result<String, SealedError> {
    use hmac::{Hmac, KeyInit, Mac};
    let key = home_key(&paths.keys())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())
        .map_err(|error| SealedError::Crypto(error.to_string()))?;
    mac.update(message);
    Ok(format!("hmac-sha256:{}", hex(&mac.finalize().into_bytes())))
}

fn cipher(key_dir: &Path) -> Result<Aes256Gcm, SealedError> {
    let key = Sha256::digest(home_key(key_dir)?.as_bytes());
    Aes256Gcm::new_from_slice(&key).map_err(|error| SealedError::Crypto(error.to_string()))
}

/// The home's key, created once in its private key directory. Two processes
/// creating it at once keep the first one written.
fn home_key(key_dir: &Path) -> Result<String, SealedError> {
    private_file::ensure_private_directory(key_dir).map_err(private(key_dir))?;
    let path = key_dir.join("local-config-secret");
    if let Some(key) = read_home_key(&path)? {
        return Ok(key);
    }
    let bytes =
        <[u8; 32]>::try_generate().map_err(|error| SealedError::Crypto(error.to_string()))?;
    let key = URL_SAFE_NO_PAD.encode(bytes);
    if private_file::create_private_new(&path, key.as_bytes()).map_err(private(&path))? {
        return Ok(key);
    }
    read_home_key(&path)?.ok_or_else(|| corrupt(&path, None))
}

fn read_home_key(path: &Path) -> Result<Option<String>, SealedError> {
    private_file::read_private(path, MAX_SEALED_BYTES)
        .map_err(private(path))?
        .map(|bytes| {
            String::from_utf8(bytes).map_err(|error| corrupt(path, Some(error.to_string())))
        })
        .transpose()
}

fn corrupt(path: &Path, cause: Option<String>) -> SealedError {
    SealedError::Corrupt {
        path: path.to_path_buf(),
        suffix: cause.map_or_else(String::new, |message| format!(": {message}")),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
