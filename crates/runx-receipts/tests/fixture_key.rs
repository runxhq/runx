//! A public fixture identity for this crate's tests and generated fixtures.
//! Its seed is published, so a receipt it seals proves only that a test or
//! generator ran. Included by path wherever a fixture is sealed.
#![allow(dead_code)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use runx_contracts::{
    Receipt, ReceiptIssuer, ReceiptIssuerType, ReceiptSignature, SignatureAlgorithm,
    sha256_prefixed,
};
use runx_receipts::{
    ReceiptError, SignatureVerificationFailure, SignatureVerifier, canonical_receipt_body_digest,
    content_addressed_receipt_id,
};

/// The seed the runtime's harness and the demo sign with.
pub const FIXTURE_SEED: [u8; 32] = [0x42; 32];

pub struct FixtureKey {
    key_pair: Ed25519KeyPair,
    issuer_type: ReceiptIssuerType,
    kid: String,
}

impl FixtureKey {
    #[must_use]
    pub fn new(seed: [u8; 32], issuer_type: ReceiptIssuerType, kid: &str) -> Self {
        Self {
            key_pair: Ed25519KeyPair::from_seed_unchecked(&seed)
                .unwrap_or_else(|_| unreachable!("a 32-byte seed is a key")),
            issuer_type,
            kid: kid.to_owned(),
        }
    }

    /// A local identity: its key id derived from its public key, as a runx
    /// home's own key is.
    #[must_use]
    pub fn local(seed: [u8; 32]) -> Self {
        let public_key = Ed25519KeyPair::from_seed_unchecked(&seed)
            .unwrap_or_else(|_| unreachable!("a 32-byte seed is a key"))
            .public_key()
            .as_ref()
            .to_vec();
        let kid = format!(
            "local-{}",
            sha256_prefixed(&public_key).trim_start_matches("sha256:")
        );
        Self::new(seed, ReceiptIssuerType::Local, &kid)
    }

    #[must_use]
    pub fn issuer(&self) -> ReceiptIssuer {
        ReceiptIssuer {
            issuer_type: self.issuer_type.clone(),
            kid: self.kid.clone().into(),
            public_key_sha256: sha256_prefixed(self.public_key()).into(),
        }
    }

    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.key_pair.public_key().as_ref()
    }

    /// Seal `receipt` as it now reads: its issuer, content address, digest
    /// and signature.
    pub fn seal(&self, receipt: &mut Receipt) -> Result<(), ReceiptError> {
        receipt.issuer = self.issuer();
        receipt.id = content_addressed_receipt_id(receipt)?.into();
        self.sign_keeping_id(receipt)
    }

    /// Re-sign the receipt's body under the id it already carries, as a
    /// forger reusing another receipt's id would.
    pub fn sign_keeping_id(&self, receipt: &mut Receipt) -> Result<(), ReceiptError> {
        let digest = canonical_receipt_body_digest(receipt)?;
        receipt.digest = digest.clone().into();
        receipt.signature = ReceiptSignature {
            alg: SignatureAlgorithm::Ed25519,
            value: format!(
                "base64:{}",
                URL_SAFE_NO_PAD.encode(self.key_pair.sign(digest.as_bytes()).as_ref())
            )
            .into(),
        };
        Ok(())
    }
}

impl SignatureVerifier for FixtureKey {
    fn verify(
        &self,
        issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        FixtureKeys(vec![self]).verify(issuer, signature, body_digest)
    }
}

/// The fixture keys a verifier trusts, matched by issuer type and key id.
pub struct FixtureKeys<'a>(pub Vec<&'a FixtureKey>);

impl SignatureVerifier for FixtureKeys<'_> {
    fn verify(
        &self,
        issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        let key = self
            .0
            .iter()
            .find(|key| key.issuer_type == issuer.issuer_type && key.kid == issuer.kid.as_str())
            .ok_or(SignatureVerificationFailure::MissingKey)?;
        if issuer.public_key_sha256.as_str() != sha256_prefixed(key.public_key()) {
            return Err(SignatureVerificationFailure::KeyHashMismatch);
        }
        let encoded = signature
            .value
            .strip_prefix("base64:")
            .ok_or(SignatureVerificationFailure::MalformedSignature)?;
        let signature = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| SignatureVerificationFailure::MalformedSignature)?;
        UnparsedPublicKey::new(&ED25519, key.public_key())
            .verify(body_digest.as_bytes(), &signature)
            .map_err(|_| SignatureVerificationFailure::SignatureMismatch)
    }
}
