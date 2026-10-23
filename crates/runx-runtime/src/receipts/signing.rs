// Module rationale: signer material, trusted keys and the trust a verifier
// selects are audited as one receipt boundary.
use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use runx_contracts::{
    ReceiptIssuer, ReceiptIssuerType, ReceiptSignature, SignatureAlgorithm, sha256_prefixed,
};
use runx_receipts::{SignatureVerificationFailure, SignatureVerifier};
use thiserror::Error;

use super::local_key::{LocalReceiptKeyFault, load_local_trusted_key, load_or_create_local_signer};
use super::seal::RuntimeReceiptSignaturePolicy;
use crate::paths::RunxPaths;

pub const RECEIPT_SIGNATURE_BASE64_PREFIX: &str = "base64:";
pub const RUNX_RECEIPT_SIGN_KID_ENV: &str = "RUNX_RECEIPT_SIGN_KID";
pub const RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV: &str = "RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64";
pub const RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV: &str = "RUNX_RECEIPT_SIGN_ISSUER_TYPE";
pub const RUNX_RECEIPT_VERIFY_KID_ENV: &str = "RUNX_RECEIPT_VERIFY_KID";
pub const RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV: &str =
    "RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64";
pub const RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV: &str = "RUNX_RECEIPT_VERIFY_ISSUER_TYPE";

/// The public seed of the fixture signing key: every harness case and test
/// fixture signs with it, so their receipts are deterministic. Its receipts
/// prove only that a test ran; nothing trusts it but a verifier told to.
pub const FIXTURE_RECEIPT_SEED: [u8; 32] = [0x42; 32];

pub(crate) fn is_receipt_signing_env_name(name: &str) -> bool {
    name.starts_with("RUNX_RECEIPT_SIGN_")
}

pub(crate) fn strip_receipt_signing_env(env: &mut BTreeMap<String, String>) {
    env.retain(|name, _| !is_receipt_signing_env_name(name));
}

pub trait RuntimeReceiptSigner {
    fn issuer(&self) -> ReceiptIssuer;
    fn sign_receipt_body(
        &self,
        body_digest: &str,
    ) -> Result<ReceiptSignature, RuntimeReceiptSigningError>;
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RuntimeReceiptSigningError {
    #[error("sealing a receipt requires a signer")]
    MissingSigner,
    #[error("a receipt is sealed under an issuer other than its signer's")]
    IssuerMismatch,
    #[error("receipt signer key id is missing")]
    MissingKeyId,
    #[error("receipt signer issuer type is unsupported")]
    UnsupportedIssuerType,
    #[error("receipt signer returned an unsupported signature algorithm")]
    UnsupportedAlgorithm,
    #[error("receipt signer key material is malformed")]
    MalformedSignerKey,
    #[error("receipt verifier key material is malformed")]
    MalformedVerifierKey,
    #[error(
        "configured receipt signing requires {RUNX_RECEIPT_SIGN_KID_ENV}, {RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV}, and {RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV} to be set together"
    )]
    IncompleteSigningEnv,
    #[error(
        "receipt verification requires {RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV}, {RUNX_RECEIPT_VERIFY_KID_ENV}, and {RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV} to be set together"
    )]
    IncompleteVerificationEnv,
    #[error("the local receipt key at {path} {fault}")]
    LocalKey {
        path: String,
        fault: LocalReceiptKeyFault,
    },
    #[error("receipt signature did not verify: {0:?}")]
    SignatureVerification(SignatureVerificationFailure),
}

/// A key a verifier trusts: the issuer type and key id a receipt names, and
/// the public key whose hash it must carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedReceiptKey {
    issuer_type: ReceiptIssuerType,
    kid: String,
    public_key: Vec<u8>,
}

impl TrustedReceiptKey {
    #[must_use]
    pub fn new(
        issuer_type: ReceiptIssuerType,
        kid: impl Into<String>,
        public_key: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            issuer_type,
            kid: kid.into(),
            public_key: public_key.into(),
        }
    }

    pub fn from_public_key_base64(
        issuer_type: ReceiptIssuerType,
        kid: impl Into<String>,
        public_key: &str,
    ) -> Result<Self, RuntimeReceiptSigningError> {
        let public_key = decode_key_material(public_key)
            .map_err(|_| RuntimeReceiptSigningError::MalformedVerifierKey)?;
        Ok(Self::new(issuer_type, kid, public_key))
    }

    #[must_use]
    pub fn issuer_type(&self) -> ReceiptIssuerType {
        self.issuer_type.clone()
    }

    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    #[must_use]
    pub fn public_key_digest(&self) -> String {
        sha256_prefixed(&self.public_key)
    }
}

/// The key id a local identity derives from its public key.
#[must_use]
pub fn local_receipt_kid(public_key: &[u8]) -> String {
    let digest = sha256_prefixed(public_key);
    format!("local-{}", digest.trim_start_matches("sha256:"))
}

pub struct Ed25519ReceiptSigner {
    issuer: ReceiptIssuer,
    key_pair: Ed25519KeyPair,
}

impl Ed25519ReceiptSigner {
    pub fn from_seed(
        kid: impl Into<String>,
        issuer_type: ReceiptIssuerType,
        seed: &[u8],
    ) -> Result<Self, RuntimeReceiptSigningError> {
        let key_pair = Ed25519KeyPair::from_seed_unchecked(seed)
            .map_err(|_| RuntimeReceiptSigningError::MalformedSignerKey)?;
        let kid = kid.into();
        if kid.trim().is_empty() {
            return Err(RuntimeReceiptSigningError::MissingKeyId);
        }
        let issuer = ReceiptIssuer {
            issuer_type,
            kid: kid.into(),
            public_key_digest: sha256_prefixed(key_pair.public_key().as_ref()).into(),
            runx_version: crate::RUNX_VERSION.into(),
        };
        Ok(Self { issuer, key_pair })
    }

    pub fn from_seed_base64(
        kid: impl Into<String>,
        issuer_type: ReceiptIssuerType,
        seed: &str,
    ) -> Result<Self, RuntimeReceiptSigningError> {
        let seed = decode_key_material(seed)
            .map_err(|_| RuntimeReceiptSigningError::MalformedSignerKey)?;
        Self::from_seed(kid, issuer_type, &seed)
    }

    /// A local identity: issuer type `local`, its key id derived from its
    /// public key.
    pub fn local(seed: &[u8]) -> Result<Self, RuntimeReceiptSigningError> {
        let key_pair = Ed25519KeyPair::from_seed_unchecked(seed)
            .map_err(|_| RuntimeReceiptSigningError::MalformedSignerKey)?;
        let kid = local_receipt_kid(key_pair.public_key().as_ref());
        Self::from_seed(kid, ReceiptIssuerType::Local, seed)
    }

    #[must_use]
    pub fn trusted_key(&self) -> TrustedReceiptKey {
        TrustedReceiptKey::new(
            self.issuer.issuer_type.clone(),
            self.issuer.kid.to_string(),
            self.key_pair.public_key().as_ref().to_vec(),
        )
    }

    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.key_pair.public_key().as_ref()
    }
}

impl RuntimeReceiptSigner for Ed25519ReceiptSigner {
    fn issuer(&self) -> ReceiptIssuer {
        self.issuer.clone()
    }

    fn sign_receipt_body(
        &self,
        body_digest: &str,
    ) -> Result<ReceiptSignature, RuntimeReceiptSigningError> {
        let signature = self.key_pair.sign(body_digest.as_bytes());
        Ok(ReceiptSignature {
            alg: SignatureAlgorithm::Ed25519,
            value: format!(
                "{RECEIPT_SIGNATURE_BASE64_PREFIX}{}",
                URL_SAFE_NO_PAD.encode(signature.as_ref())
            )
            .into(),
        })
    }
}

/// Verifies a receipt's Ed25519 signature against the keys it trusts: the
/// receipt's issuer type and key id select a key, its public key digest must be
/// that key's, and the signature must be that key's over the digest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ed25519ReceiptVerifier {
    keys: Vec<TrustedReceiptKey>,
}

impl Ed25519ReceiptVerifier {
    #[must_use]
    pub fn new(keys: impl IntoIterator<Item = TrustedReceiptKey>) -> Self {
        Self {
            keys: keys.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn keys(&self) -> &[TrustedReceiptKey] {
        &self.keys
    }

    fn resolve_key(
        &self,
        issuer: &ReceiptIssuer,
    ) -> Result<&TrustedReceiptKey, SignatureVerificationFailure> {
        self.keys
            .iter()
            .find(|key| key.issuer_type == issuer.issuer_type && key.kid == issuer.kid.as_str())
            .ok_or(SignatureVerificationFailure::MissingKey)
    }
}

impl SignatureVerifier for Ed25519ReceiptVerifier {
    fn verify(
        &self,
        issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        let key = self.resolve_key(issuer)?;
        if key.public_key.len() != 32 {
            return Err(SignatureVerificationFailure::MalformedKey);
        }
        if issuer.public_key_digest != key.public_key_digest() {
            return Err(SignatureVerificationFailure::KeyDigestMismatch);
        }
        let signature_bytes = decode_signature_value(&signature.value)?;
        if signature_bytes.len() != 64 {
            return Err(SignatureVerificationFailure::MalformedSignature);
        }
        UnparsedPublicKey::new(&ED25519, &key.public_key)
            .verify(body_digest.as_bytes(), &signature_bytes)
            .map_err(|_| SignatureVerificationFailure::SignatureMismatch)
    }
}

/// The Ed25519 primitive a hosted notary's counter-seal is checked with.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ed25519CounterSealVerifier;

impl runx_receipts::CounterSealVerifier for Ed25519CounterSealVerifier {
    fn verifies(&self, public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
        UnparsedPublicKey::new(&ED25519, public_key)
            .verify(message, signature)
            .is_ok()
    }
}

/// The identity a runtime seals receipts under, and the verifier that checks
/// each seal before it is kept.
#[derive(Clone)]
pub struct RuntimeReceiptSignatureConfig {
    material: Arc<SignatureMaterial>,
}

struct SignatureMaterial {
    signer: Ed25519ReceiptSigner,
    verifier: Ed25519ReceiptVerifier,
}

impl std::fmt::Debug for RuntimeReceiptSignatureConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeReceiptSignatureConfig")
            .field("issuer", &self.material.signer.issuer)
            .finish()
    }
}

impl RuntimeReceiptSignatureConfig {
    /// Seal under `signer`, checking each seal against `signer`'s own key.
    #[must_use]
    pub fn signing(signer: Ed25519ReceiptSigner) -> Self {
        let verifier = Ed25519ReceiptVerifier::new([signer.trusted_key()]);
        Self {
            material: Arc::new(SignatureMaterial { signer, verifier }),
        }
    }

    /// The fixture identity, under the public fixture seed.
    pub fn fixture() -> Result<Self, RuntimeReceiptSigningError> {
        Ok(Self::signing(Ed25519ReceiptSigner::local(
            &FIXTURE_RECEIPT_SEED,
        )?))
    }

    /// The identity this process seals under: the signer the environment
    /// configures, else the runx home's own local key, created on first use.
    pub fn resolve(
        env: &BTreeMap<String, String>,
        paths: &RunxPaths,
    ) -> Result<Self, RuntimeReceiptSigningError> {
        match configured_signer(env)? {
            Some(signer) => Ok(Self::signing(signer)),
            None => Ok(Self::signing(load_or_create_local_signer(paths)?)),
        }
    }

    #[must_use]
    pub fn issuer(&self) -> ReceiptIssuer {
        self.material.signer.issuer()
    }

    #[must_use]
    pub fn signature_policy(&self) -> RuntimeReceiptSignaturePolicy<'_> {
        RuntimeReceiptSignaturePolicy::signing(&self.material.signer, &self.material.verifier)
    }

    #[must_use]
    pub fn verifier(&self) -> &Ed25519ReceiptVerifier {
        &self.material.verifier
    }
}

/// Which keys a verifier trusts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptTrust {
    /// The selected runx home's own local key, and every configured key.
    HomeAndConfigured,
    /// Only configured keys: a host verifying receipts it did not issue.
    Configured,
}

/// One trusted key and where its trust came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptTrustSource {
    pub key: TrustedReceiptKey,
    pub origin: ReceiptTrustOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptTrustOrigin {
    /// `RUNX_RECEIPT_VERIFY_*` names the key.
    ConfiguredVerifier,
    /// `RUNX_RECEIPT_SIGN_*` configures a signer whose public key is trusted.
    ConfiguredSigner,
    /// The runx home's own local key.
    Home,
}

impl ReceiptTrustOrigin {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ConfiguredVerifier => "configured_verifier",
            Self::ConfiguredSigner => "configured_signer",
            Self::Home => "home",
        }
    }
}

/// The keys a verifier trusts under `trust`, each with where it came from.
/// Reading trust never creates a key.
pub fn receipt_trust(
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
    trust: ReceiptTrust,
) -> Result<Vec<ReceiptTrustSource>, RuntimeReceiptSigningError> {
    let mut sources = Vec::new();
    let kid = non_empty_env(env, RUNX_RECEIPT_VERIFY_KID_ENV);
    let public_key = non_empty_env(env, RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV);
    let issuer_type = non_empty_env(env, RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV);
    match (issuer_type, kid, public_key) {
        (None, None, None) => {}
        (Some(issuer_type), Some(kid), Some(public_key)) => sources.push(ReceiptTrustSource {
            key: TrustedReceiptKey::from_public_key_base64(
                parse_issuer_type(issuer_type)?,
                kid,
                public_key,
            )?,
            origin: ReceiptTrustOrigin::ConfiguredVerifier,
        }),
        _ => return Err(RuntimeReceiptSigningError::IncompleteVerificationEnv),
    }
    if let Some(signer) = configured_signer(env)? {
        sources.push(ReceiptTrustSource {
            key: signer.trusted_key(),
            origin: ReceiptTrustOrigin::ConfiguredSigner,
        });
    }
    if trust == ReceiptTrust::HomeAndConfigured
        && let Some(key) = load_local_trusted_key(paths)?
    {
        sources.push(ReceiptTrustSource {
            key,
            origin: ReceiptTrustOrigin::Home,
        });
    }
    Ok(sources)
}

/// The identity a run would seal under, read without creating a key: the
/// configured signer, else the home's own key when it has one.
pub fn receipt_signer(
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
) -> Result<Option<ReceiptTrustSource>, RuntimeReceiptSigningError> {
    if let Some(signer) = configured_signer(env)? {
        return Ok(Some(ReceiptTrustSource {
            key: signer.trusted_key(),
            origin: ReceiptTrustOrigin::ConfiguredSigner,
        }));
    }
    Ok(
        load_local_trusted_key(paths)?.map(|key| ReceiptTrustSource {
            key,
            origin: ReceiptTrustOrigin::Home,
        }),
    )
}

/// The verifier for the keys `receipt_trust` names.
pub fn receipt_verifier(
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
    trust: ReceiptTrust,
) -> Result<Ed25519ReceiptVerifier, RuntimeReceiptSigningError> {
    Ok(Ed25519ReceiptVerifier::new(
        receipt_trust(env, paths, trust)?
            .into_iter()
            .map(|source| source.key),
    ))
}

/// The signer `RUNX_RECEIPT_SIGN_*` configures, when it does: a `hosted` or
/// `ci` identity. A local identity is the home's own key, never configured.
fn configured_signer(
    env: &BTreeMap<String, String>,
) -> Result<Option<Ed25519ReceiptSigner>, RuntimeReceiptSigningError> {
    let kid = non_empty_env(env, RUNX_RECEIPT_SIGN_KID_ENV);
    let seed = non_empty_env(env, RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV);
    let issuer_type = non_empty_env(env, RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV);
    match (kid, seed, issuer_type) {
        (None, None, None) => Ok(None),
        (Some(kid), Some(seed), Some(issuer_type)) => {
            let issuer_type = match parse_issuer_type(issuer_type)? {
                ReceiptIssuerType::Local => {
                    return Err(RuntimeReceiptSigningError::UnsupportedIssuerType);
                }
                issuer_type => issuer_type,
            };
            Ed25519ReceiptSigner::from_seed_base64(kid, issuer_type, seed).map(Some)
        }
        _ => Err(RuntimeReceiptSigningError::IncompleteSigningEnv),
    }
}

fn parse_issuer_type(value: &str) -> Result<ReceiptIssuerType, RuntimeReceiptSigningError> {
    ReceiptIssuerType::from_label(value).ok_or(RuntimeReceiptSigningError::UnsupportedIssuerType)
}

/// A signature value is `base64:` and the unpadded URL-safe encoding the
/// signer writes; no other spelling of the same bytes is read.
fn decode_signature_value(value: &str) -> Result<Vec<u8>, SignatureVerificationFailure> {
    let Some(encoded) = value.strip_prefix(RECEIPT_SIGNATURE_BASE64_PREFIX) else {
        return Err(SignatureVerificationFailure::MalformedSignature);
    };
    URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| SignatureVerificationFailure::MalformedSignature)
}

/// Key material in the environment is standard Base64, as the variables'
/// names say.
fn decode_key_material(value: &str) -> Result<Vec<u8>, ()> {
    STANDARD.decode(value).map_err(|_| ())
}

fn non_empty_env<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    env.get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KID: &str = "operator-signing-key";
    const SEED: [u8; 32] = [0x41; 32];

    fn env(pairs: &[(&str, String)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect()
    }

    fn signing_env(issuer_type: &str) -> Vec<(&'static str, String)> {
        vec![
            (RUNX_RECEIPT_SIGN_KID_ENV, KID.to_owned()),
            (
                RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV,
                STANDARD.encode(SEED),
            ),
            (RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV, issuer_type.to_owned()),
        ]
    }

    #[test]
    fn a_configured_signer_is_trusted_and_verifies_its_seal()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let paths = RunxPaths::resolve(
            &BTreeMap::from([(
                crate::paths::RUNX_HOME_ENV.to_owned(),
                home.path().display().to_string(),
            )]),
            home.path(),
        );
        let env = env(&signing_env("ci"));
        let signer = Ed25519ReceiptSigner::from_seed(KID, ReceiptIssuerType::Ci, &SEED)?;
        let trust = receipt_trust(&env, &paths, ReceiptTrust::Configured)?;
        assert_eq!(trust.len(), 1);
        assert_eq!(trust[0].origin, ReceiptTrustOrigin::ConfiguredSigner);
        let digest = "sha256:receipt";
        let signature = signer.sign_receipt_body(digest)?;
        receipt_verifier(&env, &paths, ReceiptTrust::Configured)?
            .verify(&signer.issuer(), &signature, digest)
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        Ok(())
    }

    #[test]
    fn configured_signing_refuses_a_local_issuer_and_partial_trust_is_refused() {
        let home = std::env::temp_dir();
        let paths = RunxPaths::resolve(&BTreeMap::new(), &home);
        assert_eq!(
            receipt_trust(
                &env(&signing_env("local")),
                &paths,
                ReceiptTrust::Configured
            ),
            Err(RuntimeReceiptSigningError::UnsupportedIssuerType)
        );
        assert_eq!(
            receipt_trust(
                &env(&[(RUNX_RECEIPT_VERIFY_KID_ENV, KID.to_owned())]),
                &paths,
                ReceiptTrust::Configured
            ),
            Err(RuntimeReceiptSigningError::IncompleteVerificationEnv)
        );
    }

    #[test]
    fn a_signature_has_one_spelling() -> Result<(), Box<dyn std::error::Error>> {
        let signer = Ed25519ReceiptSigner::from_seed(KID, ReceiptIssuerType::Ci, &SEED)?;
        let digest = "sha256:receipt";
        let mut signature = signer.sign_receipt_body(digest)?;
        let verifier = Ed25519ReceiptVerifier::new([signer.trusted_key()]);
        assert_eq!(
            verifier.verify(&signer.issuer(), &signature, digest),
            Ok(())
        );
        // The last character of an unpadded 64-byte signature carries unused
        // bits; another spelling of the same bytes is refused.
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut value = signature.value.to_string().into_bytes();
        let last = value.len() - 1;
        let index = ALPHABET
            .iter()
            .position(|byte| *byte == value[last])
            .ok_or("signature is not URL-safe Base64")?;
        value[last] = ALPHABET[index ^ 1];
        signature.value = String::from_utf8(value)?.into();
        assert_eq!(
            verifier.verify(&signer.issuer(), &signature, digest),
            Err(SignatureVerificationFailure::MalformedSignature)
        );
        Ok(())
    }

    #[test]
    fn a_trusted_key_matches_issuer_type_and_kid() -> Result<(), Box<dyn std::error::Error>> {
        let signer = Ed25519ReceiptSigner::from_seed(KID, ReceiptIssuerType::Ci, &SEED)?;
        let digest = "sha256:receipt";
        let signature = signer.sign_receipt_body(digest)?;
        let hosted = Ed25519ReceiptVerifier::new([TrustedReceiptKey::new(
            ReceiptIssuerType::Hosted,
            KID,
            signer.public_key().to_vec(),
        )]);
        assert_eq!(
            hosted.verify(&signer.issuer(), &signature, digest),
            Err(SignatureVerificationFailure::MissingKey)
        );
        Ok(())
    }
}
