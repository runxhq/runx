use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{ED25519, UnparsedPublicKey};
use std::collections::BTreeMap;

use super::source_authority::{
    RegistryManifestSourceAuthority, registry_manifest_source_authority_from_env,
    registry_manifest_source_key,
};
use super::types::{RegistrySignedManifest, RegistrySubjectKind, TrustTier};
use runx_contracts::{REGISTRY_MANIFEST_SIGNATURE_ALG, REGISTRY_MANIFEST_SIGNATURE_PREFIX};

pub use runx_contracts::REGISTRY_SIGNED_MANIFEST_SCHEMA;
pub const RUNX_REGISTRY_MANIFEST_TRUST_KEY_ENV: &str = "RUNX_REGISTRY_MANIFEST_TRUST_KEY_BASE64";
pub const RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID_ENV: &str = "RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID";
pub const RUNX_REGISTRY_MANIFEST_TRUST_OWNER_ENV: &str = "RUNX_REGISTRY_MANIFEST_TRUST_OWNER";

const RUNX_REGISTRY_MANIFEST_KEY_ID: &str = "runx-registry-ed25519-v1";
const RUNX_REGISTRY_MANIFEST_PUBLIC_KEY_BASE64: &str =
    "u770bVZvqF7ULrnxWeTeLTSOsEwlbIkoOQJEAVy98No=";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedRegistryManifestKey {
    pub key_id: String,
    pub public_key: Vec<u8>,
    pub scope: RegistryManifestTrustScope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryManifestTrustScope {
    OfficialRunx,
    ThirdParty {
        allowed_owner: String,
        allowed_source: String,
    },
}

impl TrustedRegistryManifestKey {
    pub fn from_base64(
        key_id: String,
        public_key: &str,
        allowed_owner: String,
        allowed_source: String,
    ) -> Result<Self, RegistryManifestKeyError> {
        let allowed_owner = validate_owner_namespace(allowed_owner)?;
        let allowed_source = validate_registry_source(allowed_source)?;
        Self::from_base64_with_scope(
            key_id,
            public_key,
            RegistryManifestTrustScope::ThirdParty {
                allowed_owner,
                allowed_source,
            },
        )
    }

    pub fn official_from_base64(
        key_id: String,
        public_key: &str,
    ) -> Result<Self, RegistryManifestKeyError> {
        Self::from_base64_with_scope(key_id, public_key, RegistryManifestTrustScope::OfficialRunx)
    }

    fn from_base64_with_scope(
        key_id: String,
        public_key: &str,
        scope: RegistryManifestTrustScope,
    ) -> Result<Self, RegistryManifestKeyError> {
        let public_key = decode_base64(public_key).map_err(|_| RegistryManifestKeyError)?;
        if public_key.len() != 32 {
            return Err(RegistryManifestKeyError);
        }
        Ok(Self {
            key_id,
            public_key,
            scope,
        })
    }

    #[must_use]
    pub fn public_key_base64(&self) -> String {
        STANDARD.encode(&self.public_key)
    }
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("registry manifest key is invalid")]
pub struct RegistryManifestKeyError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryManifestVerificationFailure {
    UnsupportedSchema,
    UnsupportedAlgorithm,
    MalformedPayload,
    UnknownKey,
    MalformedKey,
    MalformedSignature,
    SignatureMismatch,
}

/// Verifies a registry envelope's terms and its signature by a trusted key:
/// the key it names must sign the canonical envelope without its signature.
/// The caller then checks the envelope names what it holds, and that the
/// files hash to `content_digest`.
pub fn verify_registry_signed_manifest<'a>(
    manifest: &RegistrySignedManifest,
    trusted_keys: &'a [TrustedRegistryManifestKey],
) -> Result<&'a TrustedRegistryManifestKey, RegistryManifestVerificationFailure> {
    if manifest.schema != REGISTRY_SIGNED_MANIFEST_SCHEMA {
        return Err(RegistryManifestVerificationFailure::UnsupportedSchema);
    }
    if manifest.signature.alg != REGISTRY_MANIFEST_SIGNATURE_ALG {
        return Err(RegistryManifestVerificationFailure::UnsupportedAlgorithm);
    }
    manifest
        .validate()
        .map_err(|_| RegistryManifestVerificationFailure::MalformedPayload)?;
    let key = trusted_keys
        .iter()
        .find(|key| key.key_id == manifest.signer.key_id)
        .ok_or(RegistryManifestVerificationFailure::UnknownKey)?;
    if key.public_key.len() != 32 {
        return Err(RegistryManifestVerificationFailure::MalformedKey);
    }
    let signature = decode_signature(&manifest.signature.value)?;
    if signature.len() != 64 {
        return Err(RegistryManifestVerificationFailure::MalformedSignature);
    }
    let payload = manifest
        .signing_payload()
        .map_err(|_| RegistryManifestVerificationFailure::MalformedPayload)?;
    UnparsedPublicKey::new(&ED25519, &key.public_key)
        .verify(&payload, &signature)
        .map_err(|_| RegistryManifestVerificationFailure::SignatureMismatch)?;
    Ok(key)
}

pub fn default_trusted_registry_manifest_keys()
-> Result<Vec<TrustedRegistryManifestKey>, RegistryManifestKeyError> {
    Ok(vec![TrustedRegistryManifestKey::official_from_base64(
        RUNX_REGISTRY_MANIFEST_KEY_ID.to_owned(),
        RUNX_REGISTRY_MANIFEST_PUBLIC_KEY_BASE64,
    )?])
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryManifestTrustEnvError {
    #[error("registry manifest trust key is invalid")]
    InvalidKey,
    #[error("registry manifest trust key id is required")]
    MissingKeyId,
    #[error("registry manifest trust owner is required")]
    MissingOwner,
    #[error("registry manifest trust source is required")]
    MissingSource,
}

pub fn trusted_registry_manifest_keys_from_env(
    env: &BTreeMap<String, String>,
) -> Result<Vec<TrustedRegistryManifestKey>, RegistryManifestTrustEnvError> {
    trusted_registry_manifest_keys_from_env_with_source(
        env,
        registry_manifest_source_authority_from_env(env),
    )
}

pub fn trusted_registry_manifest_keys_from_env_with_source(
    env: &BTreeMap<String, String>,
    source_authority: Option<RegistryManifestSourceAuthority>,
) -> Result<Vec<TrustedRegistryManifestKey>, RegistryManifestTrustEnvError> {
    let mut trusted_keys = default_trusted_registry_manifest_keys()
        .map_err(|_| RegistryManifestTrustEnvError::InvalidKey)?;
    let Some(public_key) = env.get(RUNX_REGISTRY_MANIFEST_TRUST_KEY_ENV) else {
        return Ok(trusted_keys);
    };
    let key_id = env
        .get(RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID_ENV)
        .cloned()
        .ok_or(RegistryManifestTrustEnvError::MissingKeyId)?;
    if matches!(
        source_authority,
        Some(RegistryManifestSourceAuthority::OfficialRunx)
    ) {
        return Err(RegistryManifestTrustEnvError::InvalidKey);
    }
    let allowed_owner = env
        .get(RUNX_REGISTRY_MANIFEST_TRUST_OWNER_ENV)
        .cloned()
        .ok_or(RegistryManifestTrustEnvError::MissingOwner)?;
    let allowed_source = source_authority
        .as_ref()
        .map(registry_manifest_source_key)
        .ok_or(RegistryManifestTrustEnvError::MissingSource)?;
    let key =
        TrustedRegistryManifestKey::from_base64(key_id, public_key, allowed_owner, allowed_source)
            .map_err(|_| RegistryManifestTrustEnvError::InvalidKey)?;
    trusted_keys.push(key);
    Ok(trusted_keys)
}

/// Whether `key` may sign `subject_ref` of `kind` (a skill's
/// `publisher/name`, or a connector's name) at `trust_tier` from
/// `source_authority`. A skill's namespace is its publisher; a connector's is
/// its name's first segment. Which publisher holds a provider namespace is
/// the registry's record, not something a name shows, so the official key
/// grants `first_party` to runx's own skills only.
pub fn registry_manifest_key_allows(
    key: &TrustedRegistryManifestKey,
    kind: RegistrySubjectKind,
    subject_ref: &str,
    trust_tier: &TrustTier,
    source_authority: Option<&RegistryManifestSourceAuthority>,
) -> Result<(), String> {
    let namespace = match kind {
        RegistrySubjectKind::Skill => subject_ref
            .split_once('/')
            .map(|(owner, _)| owner)
            .ok_or_else(|| "a skill ref must include its publisher".to_owned())?,
        RegistrySubjectKind::Connector => subject_ref
            .split_once('/')
            .map_or(subject_ref, |(namespace, _)| namespace),
    };
    match &key.scope {
        RegistryManifestTrustScope::OfficialRunx => {
            if !matches!(
                source_authority,
                Some(RegistryManifestSourceAuthority::OfficialRunx)
            ) {
                return Err(
                    "official key may only grant trust for the official runx registry source"
                        .to_owned(),
                );
            }
            if matches!(trust_tier, TrustTier::FirstParty)
                && (kind != RegistrySubjectKind::Skill || namespace != "runx")
            {
                return Err("official key may only grant first_party trust to runx/* skills".to_owned());
            }
            Ok(())
        }
        RegistryManifestTrustScope::ThirdParty {
            allowed_owner,
            allowed_source,
        } => {
            if matches!(trust_tier, TrustTier::FirstParty) {
                return Err("third-party keys may not grant first_party trust".to_owned());
            }
            let actual_source = source_authority
                .map(registry_manifest_source_key)
                .ok_or_else(|| "third-party key requires a registry source".to_owned())?;
            if actual_source != *allowed_source {
                return Err(format!(
                    "third-party key may only sign from registry source {allowed_source}"
                ));
            }
            if namespace == "runx" {
                return Err("third-party keys may not sign runx/*".to_owned());
            }
            if namespace != allowed_owner {
                return Err(format!(
                    "third-party key may only sign in the {allowed_owner} namespace"
                ));
            }
            Ok(())
        }
    }
}

fn validate_owner_namespace(value: String) -> Result<String, RegistryManifestKeyError> {
    let owner = value.trim();
    if owner.is_empty()
        || owner == "runx"
        || owner.contains('/')
        || owner
            .bytes()
            .any(|byte| matches!(byte, b'\n' | b'\r' | b'=' | 0))
    {
        return Err(RegistryManifestKeyError);
    }
    Ok(owner.to_owned())
}

fn validate_registry_source(value: String) -> Result<String, RegistryManifestKeyError> {
    let source = value.trim();
    if source.is_empty()
        || source
            .bytes()
            .any(|byte| matches!(byte, b'\n' | b'\r' | b'=' | 0))
    {
        return Err(RegistryManifestKeyError);
    }
    Ok(source.to_owned())
}

fn decode_signature(value: &str) -> Result<Vec<u8>, RegistryManifestVerificationFailure> {
    let Some(encoded) = value.strip_prefix(REGISTRY_MANIFEST_SIGNATURE_PREFIX) else {
        return Err(RegistryManifestVerificationFailure::MalformedSignature);
    };
    decode_base64(encoded).map_err(|_| RegistryManifestVerificationFailure::MalformedSignature)
}

fn decode_base64(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| STANDARD.decode(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trust_env() -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID_ENV.to_owned(),
                "test-key".to_owned(),
            ),
            (
                RUNX_REGISTRY_MANIFEST_TRUST_KEY_ENV.to_owned(),
                "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=".to_owned(),
            ),
        ])
    }

    #[test]
    fn env_trust_key_cannot_promote_itself_to_official_source() {
        let result = trusted_registry_manifest_keys_from_env_with_source(
            &trust_env(),
            Some(RegistryManifestSourceAuthority::OfficialRunx),
        );
        assert_eq!(result, Err(RegistryManifestTrustEnvError::InvalidKey));
    }

    #[test]
    fn third_party_source_trust_key_still_requires_owner() {
        let result = trusted_registry_manifest_keys_from_env_with_source(
            &trust_env(),
            Some(RegistryManifestSourceAuthority::RegistrySource(
                "local:/tmp/runx-registry".to_owned(),
            )),
        );

        assert_eq!(result, Err(RegistryManifestTrustEnvError::MissingOwner));
    }
}
