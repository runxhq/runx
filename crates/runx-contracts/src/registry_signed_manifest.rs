//! The registry's signed manifest: one envelope for skills and connectors.
//!
//! The registry signs the canonical JSON (`runx.stable-json.v1`) of the
//! envelope without its `signature`. `content_digest` covers every file of the
//! version, so each digest a kind derives (a skill's manual and profile, a
//! connector's manifest, tools and operations) comes from bytes the signature
//! already binds; none is carried separately. `@runxhq/contracts` mirrors this
//! against the same vectors (`fixtures/contracts/registry-signed-manifest`).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::connector::{is_connector_name, is_publisher_namespace};
use crate::{CanonicalJsonError, canonical_digest, is_registry_version, to_canonical_bytes};

pub const REGISTRY_SIGNED_MANIFEST_SCHEMA: &str = "runx.registry.signed_manifest.v1";
/// The only signature algorithm the registry uses.
pub const REGISTRY_MANIFEST_SIGNATURE_ALG: &str = "ed25519";
/// Prefix of an encoded signature: unpadded base64url follows.
pub const REGISTRY_MANIFEST_SIGNATURE_PREFIX: &str = "base64:";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrySubjectKind {
    Skill,
    Connector,
}

impl RegistrySubjectKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Connector => "connector",
        }
    }
}

/// What a version belongs to: `publisher/name` for a skill, the connector's
/// name for a connector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrySubject {
    pub kind: RegistrySubjectKind,
    #[serde(rename = "ref")]
    pub subject_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryManifestSigner {
    pub id: String,
    pub key_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryManifestSignature {
    pub alg: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrySignedManifest {
    pub schema: String,
    pub subject: RegistrySubject,
    pub version: String,
    pub content_digest: String,
    pub signer: RegistryManifestSigner,
    pub signature: RegistryManifestSignature,
}

/// The signed part of the envelope: everything but the signature.
#[derive(Serialize)]
struct SigningPayload<'a> {
    schema: &'a str,
    subject: &'a RegistrySubject,
    version: &'a str,
    content_digest: &'a str,
    signer: &'a RegistryManifestSigner,
}

impl RegistrySignedManifest {
    /// The bytes the registry signs: the canonical JSON of the envelope
    /// without its signature.
    ///
    /// # Errors
    ///
    /// Returns an error only when canonical serialization fails.
    pub fn signing_payload(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        to_canonical_bytes(&SigningPayload {
            schema: &self.schema,
            subject: &self.subject,
            version: &self.version,
            content_digest: &self.content_digest,
            signer: &self.signer,
        })
    }

    /// Checks every term: the schema, a subject ref of its kind's grammar, a
    /// registry version label, a `sha256:` content digest, a signer, and an
    /// `ed25519` signature in `base64:` form. The signature itself is checked
    /// against a trusted key by the verifier.
    ///
    /// # Errors
    ///
    /// Returns a message naming the first term that fails.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != REGISTRY_SIGNED_MANIFEST_SCHEMA {
            return Err(format!("schema must be {REGISTRY_SIGNED_MANIFEST_SCHEMA}"));
        }
        if !is_registry_subject_ref(self.subject.kind, &self.subject.subject_ref) {
            return Err(format!(
                "subject.ref {:?} is not a {} reference",
                self.subject.subject_ref,
                self.subject.kind.as_str()
            ));
        }
        if !is_registry_version(&self.version) {
            return Err(format!(
                "version {:?} is not a registry version label",
                self.version
            ));
        }
        if !is_sha256_digest(&self.content_digest) {
            return Err("content_digest must be sha256: and 64 lowercase hex digits".to_owned());
        }
        if !is_signer_term(&self.signer.id) || !is_signer_term(&self.signer.key_id) {
            return Err(
                "signer.id and signer.key_id must be short printable identifiers".to_owned(),
            );
        }
        if self.signature.alg != REGISTRY_MANIFEST_SIGNATURE_ALG {
            return Err(format!(
                "signature.alg must be {REGISTRY_MANIFEST_SIGNATURE_ALG}"
            ));
        }
        if !self
            .signature
            .value
            .starts_with(REGISTRY_MANIFEST_SIGNATURE_PREFIX)
        {
            return Err(format!(
                "signature.value must start with {REGISTRY_MANIFEST_SIGNATURE_PREFIX}"
            ));
        }
        Ok(())
    }

    /// Whether this envelope signs the given version of the given subject.
    /// The kind is part of the identity: a connector's envelope never stands
    /// for a skill, nor a skill's for a connector, even where the ref fits
    /// both grammars.
    #[must_use]
    pub fn signs(&self, subject: &RegistrySubject, version: &str) -> bool {
        self.subject == *subject && self.version == version
    }
}

/// A subject of the given kind: a skill's `publisher/name`, or a connector's
/// name.
#[must_use]
pub fn is_registry_subject_ref(kind: RegistrySubjectKind, value: &str) -> bool {
    match kind {
        RegistrySubjectKind::Skill => value.split_once('/').is_some_and(|(publisher, name)| {
            is_publisher_namespace(publisher) && is_skill_name(name)
        }),
        RegistrySubjectKind::Connector => is_connector_name(value),
    }
}

/// A frontmatter skill name: at most 64 lowercase letters, digits and
/// hyphens, without a leading, trailing or doubled hyphen.
fn is_skill_name(value: &str) -> bool {
    value.len() <= 64
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn is_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn is_signer_term(value: &str) -> bool {
    (1..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

/// A file a registry version consists of.
#[derive(Clone, Copy, Debug)]
pub struct RegistryContentFile<'a> {
    pub path: &'a str,
    pub content: &'a str,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryContentDigestError {
    #[error("a registry version has no files")]
    Empty,
    #[error("registry version lists {0} more than once")]
    DuplicatePath(String),
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
}

#[derive(Serialize)]
struct ContentDocument<'a> {
    files: Vec<ContentEntry<'a>>,
}

#[derive(Serialize)]
struct ContentEntry<'a> {
    path: &'a str,
    content: &'a str,
}

/// `sha256:` over the canonical JSON of every file of a version, in path
/// byte order: `{"files":[{"content":…,"path":…},…]}`. A skill's files
/// include its `SKILL.md` and `run.yaml`; a connector's its `connector.yaml`,
/// `plugin.json`, tools, README and mark.
///
/// # Errors
///
/// Returns an error for no files or a path listed twice.
pub fn registry_content_digest<'a>(
    files: impl IntoIterator<Item = RegistryContentFile<'a>>,
) -> Result<String, RegistryContentDigestError> {
    let mut entries = files
        .into_iter()
        .map(|file| ContentEntry {
            path: file.path,
            content: file.content,
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Err(RegistryContentDigestError::Empty);
    }
    entries.sort_by(|left, right| left.path.cmp(right.path));
    let mut seen = BTreeSet::new();
    for entry in &entries {
        if !seen.insert(entry.path) {
            return Err(RegistryContentDigestError::DuplicatePath(
                entry.path.to_owned(),
            ));
        }
    }
    Ok(canonical_digest(&ContentDocument { files: entries })?)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests;
