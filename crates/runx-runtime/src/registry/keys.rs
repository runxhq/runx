//! The registry keys a runx trusts (D30): the official keys the CLI carries,
//! as a list so a rotation key can be added and committed packages keep
//! verifying, and an operator's own, each scoped to one publisher and one
//! registry, from `RUNX_REGISTRY_KEYS` and `<home>/registry-keys.json`. A
//! key signs a version's manifest and states its tier; what it may sign and
//! state is its scope's to say, never the document's.

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::Deserialize;

use super::source_authority::{
    RegistryManifestSourceAuthority, registry_manifest_source_authority_from_registry_url,
};
use super::types::{RegistrySignedManifest, RegistrySubjectKind, TrustTier};
use crate::RunxPaths;
use runx_contracts::{
    REGISTRY_MANIFEST_SIGNATURE_ALG, REGISTRY_MANIFEST_SIGNATURE_PREFIX,
    REGISTRY_TIER_STATEMENT_SCHEMA, RegistryManifestSignature, RegistryManifestSigner,
    RegistryTierStatement,
};

pub use runx_contracts::REGISTRY_SIGNED_MANIFEST_SCHEMA;

/// The operator's registry keys: a JSON array of
/// `{ key_id, public_key, publisher, source }`.
pub const RUNX_REGISTRY_KEYS_ENV: &str = "RUNX_REGISTRY_KEYS";

/// The official keys, `(key_id, public key as base64)`.
const OFFICIAL_REGISTRY_KEYS: &[(&str, &str)] = &[(
    "runx-registry-ed25519-v1",
    "u770bVZvqF7ULrnxWeTeLTSOsEwlbIkoOQJEAVy98No=",
)];

/// A namespace no third-party key covers: runx's own, and the one a
/// person's own packages bind under.
const RESERVED_NAMESPACES: [&str; 2] = ["runx", "project"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryKey {
    pub key_id: String,
    pub public_key: Vec<u8>,
    pub scope: RegistryKeyScope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryKeyScope {
    /// One of the CLI's official keys: it signs from the official registry,
    /// and states `first_party` for runx's own skills only.
    Official,
    /// An operator's key: it signs `publisher`'s namespace from `source`,
    /// and never states `first_party`.
    ThirdParty {
        publisher: String,
        source: RegistryManifestSourceAuthority,
    },
}

/// One operator key as `RUNX_REGISTRY_KEYS` and `registry-keys.json` hold it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorKey {
    key_id: String,
    public_key: String,
    publisher: String,
    /// The registry it signs from, by its URL.
    source: String,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryKeysError {
    #[error("{RUNX_REGISTRY_KEYS_ENV} is invalid: {0}")]
    Env(String),
    #[error("{path} is invalid: {message}")]
    File { path: String, message: String },
    #[error(
        "{path} is in a runx home that fell back to the workspace, so it counts for nothing; set HOME or RUNX_HOME"
    )]
    FellBack { path: String },
}

impl RegistryKey {
    /// A key of `scope` from its base64 public half.
    ///
    /// # Errors
    ///
    /// Returns why the key is malformed.
    pub fn from_base64(
        key_id: &str,
        public_key: &str,
        scope: RegistryKeyScope,
    ) -> Result<Self, String> {
        if !(1..=128).contains(&key_id.len()) || !key_id.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(format!(
                "key id {key_id:?} is not a short printable identifier"
            ));
        }
        let public_key = decode_base64(public_key)
            .ok()
            .filter(|bytes| bytes.len() == 32)
            .ok_or_else(|| format!("key {key_id}'s public_key is not a base64 ed25519 key"))?;
        Ok(Self {
            key_id: key_id.to_owned(),
            public_key,
            scope,
        })
    }

    /// An operator's key, scoped to `publisher` from the registry at `source`.
    ///
    /// # Errors
    ///
    /// Returns why the key is refused: a malformed key, a publisher that is
    /// not a namespace or is reserved, or a source that is not a registry URL
    /// or is the official registry.
    pub fn third_party(
        key_id: &str,
        public_key: &str,
        publisher: &str,
        source: &str,
    ) -> Result<Self, String> {
        if !runx_contracts::connector::is_publisher_namespace(publisher)
            || RESERVED_NAMESPACES.contains(&publisher)
        {
            return Err(format!(
                "key {key_id} names publisher {publisher:?}, which an operator key cannot cover"
            ));
        }
        if !(source.starts_with("https://") || source.starts_with("http://")) {
            return Err(format!(
                "key {key_id}'s source {source:?} is not a registry URL"
            ));
        }
        let source = registry_manifest_source_authority_from_registry_url(source);
        if source == RegistryManifestSourceAuthority::OfficialRunx {
            return Err(format!(
                "key {key_id} names the official registry, which only the official keys sign from"
            ));
        }
        Self::from_base64(
            key_id,
            public_key,
            RegistryKeyScope::ThirdParty {
                publisher: publisher.to_owned(),
                source,
            },
        )
    }
}

/// The official keys the CLI carries.
#[must_use]
pub fn official_registry_keys() -> Vec<RegistryKey> {
    OFFICIAL_REGISTRY_KEYS
        .iter()
        .filter_map(|(key_id, public_key)| {
            RegistryKey::from_base64(key_id, public_key, RegistryKeyScope::Official).ok()
        })
        .collect()
}

/// Every key this runx trusts: the official keys, then the operator's in
/// `RUNX_REGISTRY_KEYS`, then those in `<home>/registry-keys.json`. A key id
/// is one key: the same id twice is refused.
///
/// # Errors
///
/// Returns an error when the environment or the file holds a malformed key,
/// or the file is in a home that fell back to the workspace.
pub fn registry_keys(
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
) -> Result<Vec<RegistryKey>, RegistryKeysError> {
    let mut keys = official_registry_keys();
    if let Some(value) = env
        .get(RUNX_REGISTRY_KEYS_ENV)
        .filter(|value| !value.trim().is_empty())
    {
        keys.extend(operator_keys(value).map_err(RegistryKeysError::Env)?);
    }
    let file = paths.registry_keys_file();
    let path = file.display().to_string();
    match std::fs::read_to_string(&file) {
        Ok(_) if paths.home_fell_back() => return Err(RegistryKeysError::FellBack { path }),
        Ok(value) => {
            keys.extend(
                operator_keys(&value).map_err(|message| RegistryKeysError::File {
                    path: path.clone(),
                    message,
                })?,
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(RegistryKeysError::File {
                path,
                message: error.to_string(),
            });
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    if let Some(key) = keys.iter().find(|key| !seen.insert(key.key_id.clone())) {
        return Err(RegistryKeysError::Env(format!(
            "key id {} names more than one key",
            key.key_id
        )));
    }
    Ok(keys)
}

/// The keys a JSON array of operator keys holds.
fn operator_keys(value: &str) -> Result<Vec<RegistryKey>, String> {
    let keys: Vec<OperatorKey> = serde_json::from_str(value).map_err(|error| error.to_string())?;
    keys.iter()
        .map(|key| {
            RegistryKey::third_party(&key.key_id, &key.public_key, &key.publisher, &key.source)
        })
        .collect()
}

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
/// The caller then checks the key's scope covers what it names, and that the
/// files hash to `content_digest`.
pub fn verify_registry_signed_manifest<'a>(
    manifest: &RegistrySignedManifest,
    keys: &'a [RegistryKey],
) -> Result<&'a RegistryKey, RegistryManifestVerificationFailure> {
    if manifest.schema != REGISTRY_SIGNED_MANIFEST_SCHEMA {
        return Err(RegistryManifestVerificationFailure::UnsupportedSchema);
    }
    manifest
        .validate()
        .map_err(|_| RegistryManifestVerificationFailure::MalformedPayload)?;
    let payload = manifest
        .signing_payload()
        .map_err(|_| RegistryManifestVerificationFailure::MalformedPayload)?;
    verify_signature(&manifest.signer, &manifest.signature, &payload, keys)
}

/// Verifies a tier statement's terms and its signature by a trusted key, as
/// a manifest's are verified. The caller checks it states this version and
/// these bytes, that it holds now, and that the key may state its tier.
pub fn verify_registry_tier_statement<'a>(
    statement: &RegistryTierStatement,
    keys: &'a [RegistryKey],
) -> Result<&'a RegistryKey, RegistryManifestVerificationFailure> {
    if statement.schema != REGISTRY_TIER_STATEMENT_SCHEMA {
        return Err(RegistryManifestVerificationFailure::UnsupportedSchema);
    }
    statement
        .validate()
        .map_err(|_| RegistryManifestVerificationFailure::MalformedPayload)?;
    let payload = statement
        .signing_payload()
        .map_err(|_| RegistryManifestVerificationFailure::MalformedPayload)?;
    verify_signature(&statement.signer, &statement.signature, &payload, keys)
}

fn verify_signature<'a>(
    signer: &RegistryManifestSigner,
    signature: &RegistryManifestSignature,
    payload: &[u8],
    keys: &'a [RegistryKey],
) -> Result<&'a RegistryKey, RegistryManifestVerificationFailure> {
    if signature.alg != REGISTRY_MANIFEST_SIGNATURE_ALG {
        return Err(RegistryManifestVerificationFailure::UnsupportedAlgorithm);
    }
    let key = keys
        .iter()
        .find(|key| key.key_id == signer.key_id)
        .ok_or(RegistryManifestVerificationFailure::UnknownKey)?;
    if key.public_key.len() != 32 {
        return Err(RegistryManifestVerificationFailure::MalformedKey);
    }
    let signature = decode_signature(&signature.value)?;
    if signature.len() != 64 {
        return Err(RegistryManifestVerificationFailure::MalformedSignature);
    }
    UnparsedPublicKey::new(&ED25519, &key.public_key)
        .verify(payload, &signature)
        .map_err(|_| RegistryManifestVerificationFailure::SignatureMismatch)?;
    Ok(key)
}

impl RegistryManifestVerificationFailure {
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema => "unsupported schema",
            Self::UnsupportedAlgorithm => "unsupported algorithm",
            Self::MalformedPayload => "malformed terms",
            Self::UnknownKey => "signed by a key outside the registry key set",
            Self::MalformedKey => "malformed trusted key",
            Self::MalformedSignature => "malformed signature",
            Self::SignatureMismatch => "signature mismatch",
        }
    }
}

/// Whether `key` may sign `name` of `kind`. At install `source` is the
/// registry the version came from, and the key's scope is checked against
/// both its publisher and that source; at bind a package on disk has no
/// source, so `source` is none and only the publisher is checked. A skill's
/// namespace is its publisher; a connector's is its name's first segment.
///
/// # Errors
///
/// Returns why the key's scope does not cover it.
pub fn registry_key_signs(
    key: &RegistryKey,
    kind: RegistrySubjectKind,
    name: &str,
    source: Option<&RegistryManifestSourceAuthority>,
) -> Result<(), String> {
    let namespace = namespace(kind, name)?;
    match &key.scope {
        RegistryKeyScope::Official => match source {
            None | Some(RegistryManifestSourceAuthority::OfficialRunx) => Ok(()),
            Some(_) => Err(format!(
                "official key {} signs only from the official runx registry source",
                key.key_id
            )),
        },
        RegistryKeyScope::ThirdParty {
            publisher,
            source: allowed,
        } => {
            if RESERVED_NAMESPACES.contains(&namespace) {
                return Err(format!(
                    "operator key {} may not sign {namespace}/*",
                    key.key_id
                ));
            }
            if namespace != publisher {
                return Err(format!(
                    "operator key {} signs only in the {publisher} namespace",
                    key.key_id
                ));
            }
            match source {
                Some(source) if source != allowed => Err(format!(
                    "operator key {} signs only from registry {}",
                    key.key_id,
                    super::registry_manifest_source_key(allowed)
                )),
                _ => Ok(()),
            }
        }
    }
}

/// Whether `key` may make `statement` from `source`: it must be able to sign
/// its subject, an operator key states only for its own publisher and never
/// `first_party`, and the official key states `first_party` only for a skill
/// under `runx/` and for a connector whose statement names publisher `runx`.
/// A connector's name doesn't say who published it, so the statement's
/// signed publisher does.
///
/// # Errors
///
/// Returns why the key may not state it.
pub fn registry_key_states(
    key: &RegistryKey,
    statement: &RegistryTierStatement,
    source: Option<&RegistryManifestSourceAuthority>,
) -> Result<(), String> {
    let kind = statement.subject.kind;
    let name = statement.subject.name.as_str();
    registry_key_signs(key, kind, name, source)?;
    let first_party = statement.tier == TrustTier::FirstParty;
    match &key.scope {
        RegistryKeyScope::ThirdParty { publisher, .. } if statement.publisher != *publisher => {
            Err(format!(
                "operator key {} states only for publisher {publisher}, and this statement names {}",
                key.key_id, statement.publisher
            ))
        }
        RegistryKeyScope::ThirdParty { .. } if first_party => Err(format!(
            "operator key {} may not state first_party",
            key.key_id
        )),
        RegistryKeyScope::ThirdParty { .. } => Ok(()),
        RegistryKeyScope::Official if !first_party => Ok(()),
        RegistryKeyScope::Official => {
            let runx_own = match kind {
                RegistrySubjectKind::Skill => namespace(kind, name)? == "runx",
                RegistrySubjectKind::Connector => statement.publisher == "runx",
            };
            if runx_own {
                Ok(())
            } else {
                Err(format!(
                    "official key {} states first_party only for a runx/ skill or a connector runx publishes",
                    key.key_id
                ))
            }
        }
    }
}

fn namespace(kind: RegistrySubjectKind, name: &str) -> Result<&str, String> {
    match kind {
        RegistrySubjectKind::Skill => name
            .split_once('/')
            .map(|(publisher, _)| publisher)
            .ok_or_else(|| "a skill name must include its publisher".to_owned()),
        RegistrySubjectKind::Connector => Ok(name
            .split_once('/')
            .map_or(name, |(namespace, _)| namespace)),
    }
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
    use runx_contracts::RegistrySubject;

    const KEY: &str = "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=";

    fn operator(publisher: &str, source: &str) -> Result<RegistryKey, String> {
        RegistryKey::third_party("operator-key", KEY, publisher, source)
    }

    fn rooted(home: &std::path::Path) -> RunxPaths {
        RunxPaths::resolve(
            &BTreeMap::from([(
                crate::paths::RUNX_HOME_ENV.to_owned(),
                home.display().to_string(),
            )]),
            home,
        )
    }

    #[test]
    fn the_official_keys_are_a_list() {
        let keys = official_registry_keys();
        assert_eq!(keys.len(), OFFICIAL_REGISTRY_KEYS.len());
        assert!(
            keys.iter()
                .all(|key| key.scope == RegistryKeyScope::Official)
        );
    }

    #[test]
    fn an_operator_key_never_covers_runx_project_or_the_official_registry() {
        for publisher in ["runx", "project", "Acme", "acme/team", ""] {
            assert!(
                operator(publisher, "https://registry.acme.test").is_err(),
                "{publisher}"
            );
        }
        for source in [
            "https://api.runx.ai",
            "https://runx.ai/",
            "registry.acme.test",
        ] {
            assert!(operator("acme", source).is_err(), "{source}");
        }
        assert!(operator("acme", "https://registry.acme.test").is_ok());
    }

    #[test]
    fn at_install_a_key_is_scoped_by_publisher_and_source_and_at_bind_by_publisher_alone()
    -> Result<(), String> {
        let key = operator("acme", "https://registry.acme.test")?;
        let acme =
            registry_manifest_source_authority_from_registry_url("https://registry.acme.test/");
        let other = registry_manifest_source_authority_from_registry_url("https://other.test");
        let skill = RegistrySubjectKind::Skill;
        assert_eq!(
            registry_key_signs(&key, skill, "acme/echo", Some(&acme)),
            Ok(())
        );
        assert!(registry_key_signs(&key, skill, "acme/echo", Some(&other)).is_err());
        assert_eq!(registry_key_signs(&key, skill, "acme/echo", None), Ok(()));
        for name in ["other/echo", "runx/echo", "project/echo"] {
            assert!(
                registry_key_signs(&key, skill, name, None).is_err(),
                "{name}"
            );
        }
        let official = official_registry_keys().remove(0);
        assert_eq!(
            registry_key_signs(
                &official,
                skill,
                "acme/echo",
                Some(&RegistryManifestSourceAuthority::OfficialRunx)
            ),
            Ok(())
        );
        assert!(registry_key_signs(&official, skill, "acme/echo", Some(&acme)).is_err());
        assert_eq!(
            registry_key_signs(&official, skill, "acme/echo", None),
            Ok(())
        );
        Ok(())
    }

    /// A statement that `publisher` published `name` of `kind` at `tier`.
    fn statement(
        kind: RegistrySubjectKind,
        name: &str,
        publisher: &str,
        tier: TrustTier,
    ) -> RegistryTierStatement {
        RegistryTierStatement {
            schema: REGISTRY_TIER_STATEMENT_SCHEMA.to_owned(),
            subject: RegistrySubject {
                kind,
                name: name.to_owned(),
            },
            version: "1.0.0".to_owned(),
            content_digest: format!("sha256:{}", "a".repeat(64)),
            publisher: publisher.to_owned(),
            tier,
            issued_at: "2026-11-08T00:00:00Z".to_owned(),
            expires_at: "2026-11-15T00:00:00Z".to_owned(),
            signer: RegistryManifestSigner {
                id: "registry".to_owned(),
                key_id: "key".to_owned(),
            },
            signature: RegistryManifestSignature {
                alg: "ed25519".to_owned(),
                value: "base64:AAAA".to_owned(),
            },
        }
    }

    /// The official key states first_party for a runx/ skill and for a
    /// connector runx publishes, by its statement's signed publisher, since a
    /// connector's name doesn't say; no operator key ever does, and one
    /// states only for its own publisher.
    #[test]
    fn only_the_official_key_states_first_party_and_only_for_what_runx_publishes()
    -> Result<(), String> {
        let official = official_registry_keys().remove(0);
        let key = operator("acme", "https://registry.acme.test")?;
        let (skill, connector) = (RegistrySubjectKind::Skill, RegistrySubjectKind::Connector);
        let first_party = TrustTier::FirstParty;
        let states = |key: &RegistryKey, statement: RegistryTierStatement| {
            registry_key_states(key, &statement, None).is_ok()
        };
        assert!(states(
            &official,
            statement(skill, "runx/echo", "runx", first_party)
        ));
        assert!(states(
            &official,
            statement(connector, "github", "runx", first_party)
        ));
        assert!(!states(
            &official,
            statement(skill, "acme/echo", "acme", first_party)
        ));
        assert!(!states(
            &official,
            statement(connector, "stripe", "acme", first_party)
        ));
        assert!(!states(
            &official,
            statement(connector, "runx", "acme", first_party)
        ));
        assert!(states(
            &official,
            statement(connector, "stripe", "acme", TrustTier::Verified)
        ));
        assert!(!states(
            &key,
            statement(skill, "acme/echo", "acme", first_party)
        ));
        assert!(states(
            &key,
            statement(skill, "acme/echo", "acme", TrustTier::Verified)
        ));
        assert!(states(
            &key,
            statement(connector, "acme/records", "acme", TrustTier::Verified)
        ));
        assert!(!states(
            &key,
            statement(connector, "acme/records", "runx", TrustTier::Verified)
        ));
        Ok(())
    }

    /// The shared vectors' cases of what an official key may state, signed
    /// by the vectors' key, which stands here as an official key.
    #[test]
    fn the_shared_vectors_say_what_an_official_key_states() -> Result<(), String> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/registry-tier-statement/registry-tier-statement-v1.vectors.json"
        );
        let vectors: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let official = RegistryKey::from_base64(
            vectors["key_id"].as_str().ok_or("key id")?,
            vectors["public_key_base64"].as_str().ok_or("public key")?,
            RegistryKeyScope::Official,
        )?;
        let cases = vectors["official_key_states"].as_array().ok_or("cases")?;
        assert!(!cases.is_empty());
        for case in cases {
            let statement: RegistryTierStatement =
                serde_json::from_value(case["statement"].clone())
                    .map_err(|error| error.to_string())?;
            let key = verify_registry_tier_statement(&statement, std::slice::from_ref(&official))
                .map_err(|failure| failure.reason().to_owned())?;
            assert_eq!(
                registry_key_states(
                    key,
                    &statement,
                    Some(&RegistryManifestSourceAuthority::OfficialRunx)
                )
                .is_ok(),
                case["states"].as_bool().ok_or("states")?,
                "{}",
                case["name"]
            );
        }
        Ok(())
    }

    #[test]
    fn the_key_set_reads_the_environment_and_a_rooted_homes_file() -> Result<(), String> {
        let home = tempfile::tempdir().map_err(|error| error.to_string())?;
        let paths = rooted(home.path());
        let entry = |key_id: &str| {
            format!(
                r#"[{{"key_id":"{key_id}","public_key":"{KEY}","publisher":"acme","source":"https://registry.acme.test"}}]"#
            )
        };
        let env = BTreeMap::from([(RUNX_REGISTRY_KEYS_ENV.to_owned(), entry("from-env"))]);
        std::fs::write(paths.registry_keys_file(), entry("from-home"))
            .map_err(|error| error.to_string())?;
        let keys = registry_keys(&env, &paths).map_err(|error| error.to_string())?;
        let ids = keys
            .iter()
            .map(|key| key.key_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["runx-registry-ed25519-v1", "from-env", "from-home"]);
        let twice = BTreeMap::from([(RUNX_REGISTRY_KEYS_ENV.to_owned(), entry("from-home"))]);
        assert!(registry_keys(&twice, &paths).is_err());
        let malformed = BTreeMap::from([(RUNX_REGISTRY_KEYS_ENV.to_owned(), "[{}]".to_owned())]);
        assert!(matches!(
            registry_keys(&malformed, &paths),
            Err(RegistryKeysError::Env(_))
        ));
        Ok(())
    }

    /// A home that fell back is `<workspace>/.runx`, which a repository can
    /// ship: a `registry-keys.json` there is refused, never read.
    #[test]
    fn a_home_that_fell_back_refuses_its_registry_keys_file() -> Result<(), String> {
        let workspace = tempfile::tempdir().map_err(|error| error.to_string())?;
        let env = BTreeMap::from([(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.path().display().to_string(),
        )]);
        let paths = RunxPaths::resolve(&env, workspace.path());
        assert!(paths.home_fell_back());
        assert_eq!(
            registry_keys(&env, &paths).map(|keys| keys.len()),
            Ok(OFFICIAL_REGISTRY_KEYS.len())
        );
        std::fs::create_dir_all(paths.home()).map_err(|error| error.to_string())?;
        std::fs::write(
            paths.registry_keys_file(),
            format!(
                r#"[{{"key_id":"planted","public_key":"{KEY}","publisher":"acme","source":"https://registry.acme.test"}}]"#
            ),
        )
        .map_err(|error| error.to_string())?;
        assert!(matches!(
            registry_keys(&env, &paths),
            Err(RegistryKeysError::FellBack { .. })
        ));
        Ok(())
    }
}
