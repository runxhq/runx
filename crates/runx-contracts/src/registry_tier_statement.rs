//! The registry's tier statement: a version's tier, signed apart from its
//! manifest because a tier and a withdrawal change after publication (D30).
//!
//! It binds the exact version and content digest the signed manifest names,
//! names the account that published the version, and holds only while it is
//! fresh. The registry signs the canonical JSON (`runx.stable-json.v1`) of the
//! statement without its `signature`, as it signs a manifest, with a key that
//! may state that tier for that subject and publisher.

use serde::{Deserialize, Serialize};

use crate::connector::is_publisher_namespace;
use crate::registry_signed_manifest::{
    REGISTRY_MANIFEST_SIGNATURE_ALG, REGISTRY_MANIFEST_SIGNATURE_PREFIX, RegistryManifestSignature,
    RegistryManifestSigner, RegistrySubject, is_registry_subject_name,
};
use crate::schema::RunxSchema;
use crate::{CanonicalJsonError, is_registry_version, to_canonical_bytes};

pub const REGISTRY_TIER_STATEMENT_SCHEMA: &str = "runx.registry.tier_statement.v1";

/// How far the registry vouches for a publisher: runx's own packages, a
/// publisher it verified, or anyone's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    FirstParty,
    Verified,
    Community,
}

impl TrustTier {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FirstParty => "first_party",
            Self::Verified => "verified",
            Self::Community => "community",
        }
    }

    /// The tier `name` names, as `as_str` writes it.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::FirstParty, Self::Verified, Self::Community]
            .into_iter()
            .find(|tier| tier.as_str() == name)
    }

    /// Whether a package of this tier meets `minimum`: first party meets
    /// every tier, verified meets verified and community, community only
    /// itself.
    #[must_use]
    pub const fn meets(self, minimum: Self) -> bool {
        self.rank() >= minimum.rank()
    }

    const fn rank(self) -> u8 {
        match self {
            Self::FirstParty => 2,
            Self::Verified => 1,
            Self::Community => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryTierStatement {
    pub schema: String,
    pub subject: RegistrySubject,
    pub version: String,
    pub content_digest: String,
    /// The namespace of the registry account that published the version. A
    /// skill's is its name's; a connector's name doesn't say who published
    /// it, since a provider namespace can belong to any verified publisher
    /// and can move, so this is the one signed place that does.
    pub publisher: String,
    pub tier: TrustTier,
    /// When the registry stated it, as `YYYY-MM-DDTHH:MM:SSZ`.
    pub issued_at: String,
    /// When it stops holding, as `YYYY-MM-DDTHH:MM:SSZ`.
    pub expires_at: String,
    pub signer: RegistryManifestSigner,
    pub signature: RegistryManifestSignature,
}

/// The signed part of the statement: everything but the signature.
#[derive(Serialize)]
struct SigningPayload<'a> {
    schema: &'a str,
    subject: &'a RegistrySubject,
    version: &'a str,
    content_digest: &'a str,
    publisher: &'a str,
    tier: TrustTier,
    issued_at: &'a str,
    expires_at: &'a str,
    signer: &'a RegistryManifestSigner,
}

impl RegistryTierStatement {
    /// The bytes the registry signs: the canonical JSON of the statement
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
            publisher: &self.publisher,
            tier: self.tier,
            issued_at: &self.issued_at,
            expires_at: &self.expires_at,
            signer: &self.signer,
        })
    }

    /// Checks every term: the schema, a subject of its kind's grammar, a
    /// registry version label, a `sha256:` content digest, two UTC instants
    /// to the second with `issued_at` before `expires_at`, a signer, and an
    /// `ed25519` signature in `base64:` form. The signature itself is checked
    /// against a trusted key by the verifier.
    ///
    /// # Errors
    ///
    /// Returns a message naming the first term that fails.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != REGISTRY_TIER_STATEMENT_SCHEMA {
            return Err(format!("schema must be {REGISTRY_TIER_STATEMENT_SCHEMA}"));
        }
        if !is_registry_subject_name(self.subject.kind, &self.subject.name) {
            return Err(format!(
                "subject.name {:?} is not a {} name",
                self.subject.name,
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
        if !is_publisher_namespace(&self.publisher) {
            return Err(format!(
                "publisher {:?} is not a publisher namespace",
                self.publisher
            ));
        }
        if self.subject.kind == crate::RegistrySubjectKind::Skill
            && self
                .subject
                .name
                .split_once('/')
                .map(|(namespace, _)| namespace)
                != Some(self.publisher.as_str())
        {
            return Err(format!(
                "a skill's publisher is its name's namespace, and {} names {}",
                self.subject.name, self.publisher
            ));
        }
        if !is_utc_second(&self.issued_at) || !is_utc_second(&self.expires_at) {
            return Err("issued_at and expires_at must be YYYY-MM-DDTHH:MM:SSZ".to_owned());
        }
        if self.issued_at >= self.expires_at {
            return Err("expires_at must follow issued_at".to_owned());
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

    /// Whether this statement is about exactly this version of this subject,
    /// with these bytes.
    #[must_use]
    pub fn states(&self, subject: &RegistrySubject, version: &str, content_digest: &str) -> bool {
        self.subject == *subject && self.version == version && self.content_digest == content_digest
    }

    /// Whether it holds at `now`, an RFC 3339 UTC instant: issued by then and
    /// not yet expired. Compared to the second.
    #[must_use]
    pub fn holds_at(&self, now: &str) -> bool {
        let Some(now) = now.get(..19).filter(|_| now.ends_with('Z')) else {
            return false;
        };
        self.issued_at[..19] <= *now && *now < self.expires_at[..19]
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ`, which orders as the instants it names.
fn is_utc_second(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 20
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 => *byte == b':',
            19 => *byte == b'Z',
            _ => byte.is_ascii_digit(),
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::RegistrySubjectKind;

    fn statement() -> RegistryTierStatement {
        RegistryTierStatement {
            schema: REGISTRY_TIER_STATEMENT_SCHEMA.to_owned(),
            subject: RegistrySubject {
                kind: RegistrySubjectKind::Skill,
                name: "acme/echo".to_owned(),
            },
            version: "1.0.0".to_owned(),
            content_digest: format!("sha256:{}", "a".repeat(64)),
            publisher: "acme".to_owned(),
            tier: TrustTier::Verified,
            issued_at: "2026-11-08T00:00:00Z".to_owned(),
            expires_at: "2026-11-15T00:00:00Z".to_owned(),
            signer: RegistryManifestSigner {
                id: "runx-registry".to_owned(),
                key_id: "runx-registry-ed25519-v1".to_owned(),
            },
            signature: RegistryManifestSignature {
                alg: "ed25519".to_owned(),
                value: "base64:AAAA".to_owned(),
            },
        }
    }

    #[test]
    fn the_signature_covers_every_term_but_itself() {
        let statement = statement();
        let payload = String::from_utf8(statement.signing_payload().unwrap()).unwrap();
        assert_eq!(
            payload,
            format!(
                r#"{{"content_digest":"sha256:{}","expires_at":"2026-11-15T00:00:00Z","issued_at":"2026-11-08T00:00:00Z","publisher":"acme","schema":"runx.registry.tier_statement.v1","signer":{{"id":"runx-registry","key_id":"runx-registry-ed25519-v1"}},"subject":{{"kind":"skill","name":"acme/echo"}},"tier":"verified","version":"1.0.0"}}"#,
                "a".repeat(64)
            )
        );
        let mut other = statement.clone();
        other.signature.value = "base64:BBBB".to_owned();
        assert_eq!(
            other.signing_payload().unwrap(),
            statement.signing_payload().unwrap()
        );
    }

    fn vectors() -> serde_json::Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/registry-tier-statement/registry-tier-statement-v1.vectors.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).expect("vectors"))
            .expect("vectors json")
    }

    /// The shared vectors `@runxhq/contracts` runs too: each statement's
    /// signing payload is these exact bytes, signed by the vectors' key, and
    /// each refused statement is refused here as there.
    #[test]
    fn the_shared_vectors_sign_these_bytes_and_refuse_these_terms() {
        use base64::Engine;
        let vectors = vectors();
        let public_key = base64::engine::general_purpose::STANDARD
            .decode(vectors["public_key_base64"].as_str().expect("key"))
            .expect("key bytes");
        let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key);
        for case in vectors["statements"].as_array().expect("statements") {
            let statement: RegistryTierStatement =
                serde_json::from_value(case["statement"].clone()).expect("statement");
            statement.validate().expect("valid");
            let payload = statement.signing_payload().expect("payload");
            assert_eq!(
                std::str::from_utf8(&payload).expect("utf8"),
                case["signing_payload"].as_str().expect("payload"),
                "{}",
                case["name"]
            );
            let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(
                    statement
                        .signature
                        .value
                        .strip_prefix("base64:")
                        .expect("prefix"),
                )
                .expect("signature bytes");
            key.verify(&payload, &signature)
                .expect("signature verifies");
        }
        for case in vectors["invalid_statements"].as_array().expect("cases") {
            let refused =
                serde_json::from_value::<RegistryTierStatement>(case["statement"].clone())
                    .map_or(true, |statement| statement.validate().is_err());
            assert!(refused, "{}", case["name"]);
        }
    }

    #[test]
    fn a_statement_holds_from_its_issue_until_it_expires() {
        let statement = statement();
        statement.validate().unwrap();
        assert!(!statement.holds_at("2026-11-07T23:59:59.999Z"));
        assert!(statement.holds_at("2026-11-08T00:00:00.000Z"));
        assert!(statement.holds_at("2026-11-14T23:59:59Z"));
        assert!(!statement.holds_at("2026-11-15T00:00:00.000Z"));
        assert!(!statement.holds_at("not a time"));
    }

    /// A connector's name doesn't say who published it, so a connector's
    /// statement names any publisher, `runx` among them.
    #[test]
    fn a_connector_statement_names_its_publisher_whatever_its_name() {
        let mut statement = statement();
        statement.subject = RegistrySubject {
            kind: RegistrySubjectKind::Connector,
            name: "stripe".to_owned(),
        };
        statement.publisher = "runx".to_owned();
        assert_eq!(statement.validate(), Ok(()));
        let without = serde_json::to_value(&statement)
            .map(|mut value| {
                value
                    .as_object_mut()
                    .map(|object| object.remove("publisher"));
                value
            })
            .unwrap();
        assert!(serde_json::from_value::<RegistryTierStatement>(without).is_err());
    }

    #[test]
    fn a_statement_binds_the_exact_version_and_bytes() {
        let statement = statement();
        let subject = statement.subject.clone();
        let digest = statement.content_digest.clone();
        assert!(statement.states(&subject, "1.0.0", &digest));
        assert!(!statement.states(&subject, "1.0.1", &digest));
        assert!(!statement.states(&subject, "1.0.0", &format!("sha256:{}", "b".repeat(64))));
        let connector = RegistrySubject {
            kind: RegistrySubjectKind::Connector,
            name: "acme/echo".to_owned(),
        };
        assert!(!statement.states(&connector, "1.0.0", &digest));
    }

    #[test]
    fn malformed_terms_are_refused() {
        type Mutation = fn(&mut RegistryTierStatement);
        let cases: [(&str, Mutation); 8] = [
            ("schema", |s| s.schema = "runx.registry.tier.v0".to_owned()),
            ("instant", |s| {
                s.issued_at = "2026-11-08T00:00:00.000Z".to_owned();
            }),
            ("order", |s| s.expires_at = s.issued_at.clone()),
            ("digest", |s| s.content_digest = "sha256:abc".to_owned()),
            ("subject", |s| s.subject.name = "echo".to_owned()),
            ("publisher", |s| s.publisher = "Acme".to_owned()),
            // A skill is published by its name's namespace.
            ("skill publisher", |s| s.publisher = "runx".to_owned()),
            ("missing publisher", |s| s.publisher = String::new()),
        ];
        for (name, mutate) in cases {
            let mut statement = statement();
            mutate(&mut statement);
            assert!(statement.validate().is_err(), "{name}");
        }
        let parsed = serde_json::from_value::<RegistryTierStatement>(serde_json::json!({
            "schema": REGISTRY_TIER_STATEMENT_SCHEMA,
            "subject": { "kind": "skill", "name": "acme/echo" },
            "version": "1.0.0",
            "content_digest": format!("sha256:{}", "a".repeat(64)),
            "publisher": "acme",
            "tier": "verified",
            "issued_at": "2026-11-08T00:00:00Z",
            "expires_at": "2026-11-15T00:00:00Z",
            "signer": { "id": "r", "key_id": "k" },
            "signature": { "alg": "ed25519", "value": "base64:AAAA" },
            "withdrawn": false,
        }));
        assert!(parsed.is_err());
    }
}
