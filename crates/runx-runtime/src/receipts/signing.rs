// Module rationale: signer material parsing, production
// validation, and verifier behavior are audited as one receipt boundary.
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

use super::seal::RuntimeReceiptSignaturePolicy;

pub const RECEIPT_SIGNATURE_BASE64_PREFIX: &str = "base64:";
pub const RUNX_RECEIPT_SIGN_KID_ENV: &str = "RUNX_RECEIPT_SIGN_KID";
pub const RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV: &str = "RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64";
pub const RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV: &str = "RUNX_RECEIPT_SIGN_ISSUER_TYPE";
pub const RUNX_RECEIPT_VERIFY_KID_ENV: &str = "RUNX_RECEIPT_VERIFY_KID";
pub const RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV: &str =
    "RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64";

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
    #[error("production receipt signing requires a signer")]
    MissingSigner,
    #[error("production receipt signing requires a verifier")]
    MissingVerifier,
    #[error("production receipt signer key id is missing")]
    MissingKeyId,
    #[error("production receipt signer public key hash is missing")]
    MissingPublicKeySha256,
    #[error("production receipt signer public key hash is malformed")]
    MalformedPublicKeySha256,
    #[error("production receipt signer issuer type is missing")]
    MissingIssuerType,
    #[error("production receipt signer issuer type is unsupported")]
    UnsupportedIssuerType,
    #[error("production receipt signer returned an unsupported signature algorithm")]
    UnsupportedAlgorithm,
    #[error("production receipt signer returned a local pseudo signature")]
    PseudoSignature,
    #[error("production receipt signer key material is malformed")]
    MalformedSignerKey,
    #[error("production receipt verifier key material is malformed")]
    MalformedVerifierKey,
    #[error(
        "production receipt signing requires {RUNX_RECEIPT_SIGN_KID_ENV}, {RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV}, and {RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV} to be set together"
    )]
    IncompleteSigningEnv,
    #[error(
        "production receipt verification requires {RUNX_RECEIPT_VERIFY_KID_ENV} and {RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV} to be set together"
    )]
    IncompleteVerificationEnv,
    #[error(
        "governed runtime receipt signing requires {RUNX_RECEIPT_SIGN_KID_ENV}, {RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV}, and {RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV}"
    )]
    MissingSigningEnv,
    #[error("production receipt signature did not verify: {0:?}")]
    SignatureVerification(SignatureVerificationFailure),
}

#[derive(Clone, Default)]
pub struct RuntimeReceiptSignatureConfig {
    production: Option<Arc<ProductionReceiptSignatureMaterial>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeReceiptVerifierSource {
    ExplicitVerifier,
    SigningIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedReceiptVerifier {
    verifier: Ed25519ReceiptVerifier,
    source: RuntimeReceiptVerifierSource,
}

impl ResolvedReceiptVerifier {
    #[must_use]
    pub fn verifier(&self) -> &Ed25519ReceiptVerifier {
        &self.verifier
    }

    #[must_use]
    pub fn into_verifier(self) -> Ed25519ReceiptVerifier {
        self.verifier
    }

    #[must_use]
    pub fn source(&self) -> RuntimeReceiptVerifierSource {
        self.source
    }
}

struct ProductionReceiptSignatureMaterial {
    signer: Ed25519ReceiptSigner,
    verifier: Ed25519ReceiptVerifier,
}

impl std::fmt::Debug for RuntimeReceiptSignatureConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeReceiptSignatureConfig")
            .field("production_configured", &self.production.is_some())
            .finish()
    }
}

impl RuntimeReceiptSignatureConfig {
    #[must_use]
    pub fn local_development() -> Self {
        Self { production: None }
    }

    pub fn production_signing(
        signer: Ed25519ReceiptSigner,
        verifier: Ed25519ReceiptVerifier,
    ) -> Self {
        Self {
            production: Some(Arc::new(ProductionReceiptSignatureMaterial {
                signer,
                verifier,
            })),
        }
    }

    pub fn from_env(env: &BTreeMap<String, String>) -> Result<Self, RuntimeReceiptSigningError> {
        let kid = non_empty_env(env, RUNX_RECEIPT_SIGN_KID_ENV);
        let seed = non_empty_env(env, RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV);
        let issuer_type = non_empty_env(env, RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV);
        match (kid, seed, issuer_type) {
            (None, None, None) => Err(RuntimeReceiptSigningError::MissingSigningEnv),
            (Some(kid), Some(seed), Some(issuer_type)) => {
                let issuer_type = parse_production_issuer_type(issuer_type)?;
                let signer = Ed25519ReceiptSigner::from_seed_base64(kid, issuer_type, seed)?;
                let verifier = Ed25519ReceiptVerifier::new([signer.production_key()]);
                Ok(Self::production_signing(signer, verifier))
            }
            (Some(_), Some(_), None) => Err(RuntimeReceiptSigningError::MissingIssuerType),
            _ => Err(RuntimeReceiptSigningError::IncompleteSigningEnv),
        }
    }

    #[must_use]
    pub fn signature_policy(&self) -> RuntimeReceiptSignaturePolicy<'_> {
        match self.production.as_ref() {
            Some(production) => RuntimeReceiptSignaturePolicy::production_signing(
                &production.signer,
                &production.verifier,
            ),
            None => RuntimeReceiptSignaturePolicy::local_development(),
        }
    }

    #[must_use]
    pub fn production_key_for_kid(&self, kid: &str) -> Option<ProductionReceiptKey> {
        self.production
            .as_ref()
            .map(|production| production.signer.production_key())
            .filter(|key| key.kid() == kid)
    }

    #[must_use]
    pub fn production_verifier(&self) -> Option<Ed25519ReceiptVerifier> {
        self.production
            .as_ref()
            .map(|production| production.verifier.clone())
    }
}

/// Resolve the one production receipt verifier selected by the operator's
/// environment. An explicit verifier is authoritative. When it is absent, a
/// complete signing identity supplies its own public verifier so the process
/// that signs local receipts can inspect them without duplicating key
/// configuration.
pub fn receipt_verifier_from_env(
    env: &BTreeMap<String, String>,
) -> Result<Option<ResolvedReceiptVerifier>, RuntimeReceiptSigningError> {
    let kid = non_empty_env(env, RUNX_RECEIPT_VERIFY_KID_ENV);
    let public_key = non_empty_env(env, RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV);
    match (kid, public_key) {
        (Some(kid), Some(public_key)) => {
            Ed25519ReceiptVerifier::from_public_key_base64(kid.to_owned(), public_key).map(
                |verifier| {
                    Some(ResolvedReceiptVerifier {
                        verifier,
                        source: RuntimeReceiptVerifierSource::ExplicitVerifier,
                    })
                },
            )
        }
        (Some(_), None) | (None, Some(_)) => {
            Err(RuntimeReceiptSigningError::IncompleteVerificationEnv)
        }
        (None, None) => match RuntimeReceiptSignatureConfig::from_env(env) {
            Ok(config) => {
                Ok(config
                    .production_verifier()
                    .map(|verifier| ResolvedReceiptVerifier {
                        verifier,
                        source: RuntimeReceiptVerifierSource::SigningIdentity,
                    }))
            }
            Err(RuntimeReceiptSigningError::MissingSigningEnv) => Ok(None),
            Err(error) => Err(error),
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionReceiptKey {
    kid: String,
    public_key: Vec<u8>,
}

impl ProductionReceiptKey {
    #[must_use]
    pub fn new(kid: impl Into<String>, public_key: impl Into<Vec<u8>>) -> Self {
        Self {
            kid: kid.into(),
            public_key: public_key.into(),
        }
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
    pub fn public_key_sha256(&self) -> String {
        sha256_prefixed(&self.public_key)
    }
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
        let issuer = ReceiptIssuer {
            issuer_type,
            kid: kid.into(),
            public_key_sha256: sha256_prefixed(key_pair.public_key().as_ref()).into(),
        };
        validate_production_issuer(&issuer)?;
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

    #[must_use]
    pub fn production_key(&self) -> ProductionReceiptKey {
        ProductionReceiptKey::new(
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ed25519ReceiptVerifier {
    keys: Vec<ProductionReceiptKey>,
}

impl Ed25519ReceiptVerifier {
    #[must_use]
    pub fn new(keys: impl IntoIterator<Item = ProductionReceiptKey>) -> Self {
        Self {
            keys: keys.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn from_public_key(kid: impl Into<String>, public_key: impl Into<Vec<u8>>) -> Self {
        Self::new([ProductionReceiptKey::new(kid, public_key)])
    }

    pub fn from_public_key_base64(
        kid: impl Into<String>,
        public_key: &str,
    ) -> Result<Self, RuntimeReceiptSigningError> {
        let public_key = decode_key_material(public_key)
            .map_err(|_| RuntimeReceiptSigningError::MalformedVerifierKey)?;
        Ok(Self::from_public_key(kid, public_key))
    }

    #[must_use]
    pub fn keys(&self) -> &[ProductionReceiptKey] {
        &self.keys
    }

    fn resolve_key(
        &self,
        issuer: &ReceiptIssuer,
    ) -> Result<&ProductionReceiptKey, SignatureVerificationFailure> {
        if issuer.issuer_type == ReceiptIssuerType::Local {
            return Err(SignatureVerificationFailure::UnsupportedIssuer);
        }
        if issuer.kid.trim().is_empty() {
            return Err(SignatureVerificationFailure::MissingKey);
        }
        self.keys
            .iter()
            .find(|key| key.kid == issuer.kid)
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
        if issuer.public_key_sha256 != key.public_key_sha256() {
            return Err(SignatureVerificationFailure::KeyHashMismatch);
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

pub(crate) fn validate_production_issuer(
    issuer: &ReceiptIssuer,
) -> Result<(), RuntimeReceiptSigningError> {
    if issuer.issuer_type == ReceiptIssuerType::Local {
        return Err(RuntimeReceiptSigningError::UnsupportedIssuerType);
    }
    if issuer.kid.trim().is_empty() {
        return Err(RuntimeReceiptSigningError::MissingKeyId);
    }
    if issuer.public_key_sha256.trim().is_empty() {
        return Err(RuntimeReceiptSigningError::MissingPublicKeySha256);
    }
    if !is_well_formed_sha256(&issuer.public_key_sha256) {
        return Err(RuntimeReceiptSigningError::MalformedPublicKeySha256);
    }
    Ok(())
}

fn parse_production_issuer_type(
    value: &str,
) -> Result<ReceiptIssuerType, RuntimeReceiptSigningError> {
    match value {
        "hosted" => Ok(ReceiptIssuerType::Hosted),
        "ci" => Ok(ReceiptIssuerType::Ci),
        _ => Err(RuntimeReceiptSigningError::UnsupportedIssuerType),
    }
}

pub(crate) fn is_local_pseudo_signature(value: &str) -> bool {
    value.starts_with("sig:")
}

fn decode_signature_value(value: &str) -> Result<Vec<u8>, SignatureVerificationFailure> {
    let Some(encoded) = value.strip_prefix(RECEIPT_SIGNATURE_BASE64_PREFIX) else {
        return Err(SignatureVerificationFailure::MalformedSignature);
    };
    URL_SAFE_NO_PAD
        .decode(encoded)
        .or_else(|_| STANDARD.decode(encoded))
        .map_err(|_| SignatureVerificationFailure::MalformedSignature)
}

fn decode_key_material(value: &str) -> Result<Vec<u8>, ()> {
    URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| STANDARD.decode(value))
        .map_err(|_| ())
}

fn non_empty_env<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    env.get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn is_well_formed_sha256(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KID: &str = "operator-signing-key";
    const SEED: [u8; 32] = [0x42; 32];

    #[test]
    fn verifier_falls_back_to_complete_signing_identity() -> Result<(), Box<dyn std::error::Error>>
    {
        let env = BTreeMap::from([
            (RUNX_RECEIPT_SIGN_KID_ENV.to_owned(), KID.to_owned()),
            (
                RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV.to_owned(),
                STANDARD.encode(SEED),
            ),
            (
                RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV.to_owned(),
                "ci".to_owned(),
            ),
        ]);
        let signer = Ed25519ReceiptSigner::from_seed(KID, ReceiptIssuerType::Ci, &SEED)?;
        let resolved = receipt_verifier_from_env(&env)?.ok_or("missing derived verifier")?;
        assert_eq!(
            resolved.source(),
            RuntimeReceiptVerifierSource::SigningIdentity
        );
        let digest = "sha256:receipt";
        let signature = signer.sign_receipt_body(digest)?;

        resolved
            .verifier()
            .verify(&signer.issuer(), &signature, digest)
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        Ok(())
    }

    #[test]
    fn partial_explicit_verifier_does_not_fall_back_to_signer() {
        let env = BTreeMap::from([
            (RUNX_RECEIPT_VERIFY_KID_ENV.to_owned(), KID.to_owned()),
            (RUNX_RECEIPT_SIGN_KID_ENV.to_owned(), KID.to_owned()),
            (
                RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV.to_owned(),
                STANDARD.encode(SEED),
            ),
            (
                RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV.to_owned(),
                "ci".to_owned(),
            ),
        ]);

        assert_eq!(
            receipt_verifier_from_env(&env),
            Err(RuntimeReceiptSigningError::IncompleteVerificationEnv)
        );
    }
}
