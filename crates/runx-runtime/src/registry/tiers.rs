//! A signed package's tier (D30): a statement the registry signs apart from
//! the manifest, bound to the exact version and content digest, holding only
//! until it expires. It is read only when a decision needs more than
//! `community`, kept in the runx home at `<home>/tiers/<content digest>.json`
//! (signed, so the location only keeps the workspace clean), and refreshed
//! from the registry its key is scoped to when absent or expired. When the
//! refresh fails or work is offline the tier is `community`, so offline work
//! waits on a person's trust and never assumes the best; a withdrawn version
//! gets no fresh statement, and reads `community` once its last one expires.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{RegistrySubject, RegistryTierStatement, TrustTier};

use super::keys::{
    RegistryKey, RegistryKeyScope, registry_key_states, registry_keys,
    verify_registry_tier_statement,
};
use super::signature::SignedPackage;
use super::source_authority::{
    RegistryManifestSourceAuthority, is_official_runx_registry_url,
    registry_manifest_source_authority_from_registry_url,
};
use crate::RunxPaths;

/// What a statement must state to hold for a package: its exact subject,
/// version and content digest.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StatedPackage<'a> {
    pub(crate) subject: &'a RegistrySubject,
    pub(crate) version: &'a str,
    pub(crate) content_digest: &'a str,
}

/// The tier `statement` states for `package`, when it holds at `now`: signed
/// by a key in `keys` that may state that tier for that subject, from
/// `source` when it came from a registry, about exactly these bytes.
#[must_use]
pub(crate) fn verified_tier(
    statement: &RegistryTierStatement,
    package: StatedPackage<'_>,
    keys: &[RegistryKey],
    source: Option<&RegistryManifestSourceAuthority>,
    now: &str,
) -> Option<TrustTier> {
    let key = verify_registry_tier_statement(statement, keys).ok()?;
    (statement.states(package.subject, package.version, package.content_digest)
        && statement.holds_at(now)
        && registry_key_states(key, statement, source).is_ok())
    .then_some(statement.tier)
}

/// The tier `statement`, served with `package` by the registry at `source`,
/// states for it, kept in `tiers` (the home's tier statements, none when the
/// home fell back) when it holds. A statement that does not hold is no
/// statement, so the tier is `community`.
pub(crate) fn accept_served(
    statement: Option<&RegistryTierStatement>,
    package: StatedPackage<'_>,
    keys: &[RegistryKey],
    source: &RegistryManifestSourceAuthority,
    tiers: Option<&Path>,
) -> TrustTier {
    let now = crate::time::now_iso8601();
    let Some((statement, tier)) = statement.and_then(|statement| {
        verified_tier(statement, package, keys, Some(source), &now).map(|tier| (statement, tier))
    }) else {
        return TrustTier::Community;
    };
    if let Some(tiers) = tiers {
        // Keeping it saves the next decision a refresh; failing to keep it
        // costs only that.
        let _kept = keep(tiers, package.content_digest, statement);
    }
    tier
}

/// The home's tier statements, unless the home fell back to the workspace,
/// where a cached statement counts for nothing.
#[must_use]
pub fn tier_cache(paths: &RunxPaths) -> Option<PathBuf> {
    (!paths.home_fell_back()).then(|| paths.tier_statements())
}

/// The tier a decision reads for `signed`, a package whose signature
/// verified: its statement kept in the home when it still holds, else a fresh
/// one read again from the registry its key is scoped to, else `community`.
#[must_use]
pub(crate) fn stated_tier(
    signed: &SignedPackage,
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
) -> TrustTier {
    match registry_keys(env, paths) {
        Ok(keys) => stated_tier_under(signed, &keys, env, paths),
        Err(_) => TrustTier::Community,
    }
}

/// [`stated_tier`] under the key set `keys`.
#[must_use]
pub(crate) fn stated_tier_under(
    signed: &SignedPackage,
    keys: &[RegistryKey],
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
) -> TrustTier {
    let subject = RegistrySubject {
        kind: signed.kind,
        name: signed.name.clone(),
    };
    let package = StatedPackage {
        subject: &subject,
        version: &signed.version,
        content_digest: &signed.content_digest,
    };
    let tiers = tier_cache(paths);
    let now = crate::time::now_iso8601();
    if let Some(tier) = tiers
        .as_deref()
        .and_then(|tiers| kept(tiers, signed.content_digest.as_str()))
        .and_then(|statement| verified_tier(&statement, package, keys, None, &now))
    {
        return tier;
    }
    let Some(registry_url) = keys
        .iter()
        .find(|key| key.key_id == signed.key_id)
        .and_then(|key| refresh_registry(key, env))
    else {
        return TrustTier::Community;
    };
    let source = registry_manifest_source_authority_from_registry_url(&registry_url);
    match served_statement(&registry_url, signed, env) {
        Some(statement) => {
            accept_served(Some(&statement), package, keys, &source, tiers.as_deref())
        }
        None => TrustTier::Community,
    }
}

/// The registry a statement for a version signed by `key` is read again
/// from: an operator key's own, and for an official key the official
/// registry, when this runx is pointed at it. A runx pointed elsewhere reads
/// no statement for an official version, and so reads it as `community`.
fn refresh_registry(key: &RegistryKey, env: &BTreeMap<String, String>) -> Option<String> {
    match &key.scope {
        RegistryKeyScope::ThirdParty {
            source: RegistryManifestSourceAuthority::RegistrySource(source),
            ..
        } => source.strip_prefix("remote:").map(str::to_owned),
        RegistryKeyScope::ThirdParty { .. } => None,
        RegistryKeyScope::Official => super::resolve::resolve_registry_target(None, env)
            .ok()
            .map(|target| target.registry_url)
            .filter(|url| is_official_runx_registry_url(url)),
    }
}

/// The statement the registry at `registry_url` serves with `signed`'s
/// version, read as the version is read again: a skill's record, a
/// connector's package. None when it serves none or cannot be reached.
fn served_statement(
    registry_url: &str,
    signed: &SignedPackage,
    env: &BTreeMap<String, String>,
) -> Option<RegistryTierStatement> {
    let client = super::resolve::registry_client(registry_url, env).ok()?;
    match signed.kind {
        runx_contracts::RegistrySubjectKind::Skill => {
            client
                .version_record(&signed.name, &signed.version)
                .ok()
                .flatten()?
                .tier_statement
        }
        runx_contracts::RegistrySubjectKind::Connector => {
            let name = runx_contracts::connector::ConnectorName::new(&signed.name)?;
            client
                .connector_package(&name, &signed.version)
                .ok()
                .flatten()?
                .tier_statement
        }
    }
}

fn statement_path(tiers: &Path, content_digest: &str) -> PathBuf {
    tiers.join(format!(
        "{}.json",
        content_digest.trim_start_matches("sha256:")
    ))
}

fn kept(tiers: &Path, content_digest: &str) -> Option<RegistryTierStatement> {
    let text = std::fs::read_to_string(statement_path(tiers, content_digest)).ok()?;
    serde_json::from_str(&text).ok()
}

fn keep(
    tiers: &Path,
    content_digest: &str,
    statement: &RegistryTierStatement,
) -> Result<(), String> {
    std::fs::create_dir_all(tiers).map_err(|error| error.to_string())?;
    let path = statement_path(tiers, content_digest);
    let staged = tiers.join(format!(".{}", super::install::unique_suffix()));
    let encoded = serde_json::to_vec_pretty(statement).map_err(|error| error.to_string())?;
    std::fs::write(&staged, encoded).map_err(|error| error.to_string())?;
    std::fs::rename(&staged, &path).map_err(|error| {
        let _ignored = std::fs::remove_file(&staged);
        error.to_string()
    })
}

/// Issuing tier statements in tests with the test registry key.
#[cfg(test)]
pub(crate) mod testing {
    use base64::Engine;
    use runx_contracts::{
        REGISTRY_TIER_STATEMENT_SCHEMA, RegistryManifestSignature, RegistryManifestSigner,
        RegistrySubject, RegistryTierStatement, TrustTier,
    };

    /// A statement that `publisher` published `subject@version`, over
    /// `content_digest`, at `tier`, held from `issued_at` until `expires_at`,
    /// signed by the test key.
    pub(crate) fn statement(
        subject: RegistrySubject,
        version: &str,
        content_digest: &str,
        publisher: &str,
        tier: TrustTier,
        (issued_at, expires_at): (&str, &str),
    ) -> Result<RegistryTierStatement, String> {
        let mut statement = RegistryTierStatement {
            schema: REGISTRY_TIER_STATEMENT_SCHEMA.to_owned(),
            subject,
            version: version.to_owned(),
            content_digest: content_digest.to_owned(),
            publisher: publisher.to_owned(),
            tier,
            issued_at: issued_at.to_owned(),
            expires_at: expires_at.to_owned(),
            signer: RegistryManifestSigner {
                id: "runx-test-registry".to_owned(),
                key_id: super::super::signature::testing::KEY_ID.to_owned(),
            },
            signature: RegistryManifestSignature {
                alg: "ed25519".to_owned(),
                value: String::new(),
            },
        };
        let signature = super::super::signature::testing::sign_bytes(
            &statement
                .signing_payload()
                .map_err(|error| error.to_string())?,
        )?;
        statement.signature.value = format!(
            "base64:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
        );
        Ok(statement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runx_contracts::RegistrySubjectKind;

    const HELD: (&str, &str) = ("2026-11-01T00:00:00Z", "2099-01-01T00:00:00Z");
    const EXPIRED: (&str, &str) = ("2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z");

    fn digest() -> String {
        format!("sha256:{}", "c".repeat(64))
    }

    fn subject(name: &str) -> RegistrySubject {
        RegistrySubject {
            kind: RegistrySubjectKind::Skill,
            name: name.to_owned(),
        }
    }

    fn keys(publisher: &str) -> Result<Vec<RegistryKey>, String> {
        let env = super::super::signature::testing::trusting(publisher)?;
        let home = tempfile::tempdir().map_err(|error| error.to_string())?;
        let paths = RunxPaths::resolve(
            &BTreeMap::from([(
                crate::paths::RUNX_HOME_ENV.to_owned(),
                home.path().display().to_string(),
            )]),
            home.path(),
        );
        registry_keys(&env, &paths).map_err(|error| error.to_string())
    }

    fn source() -> RegistryManifestSourceAuthority {
        registry_manifest_source_authority_from_registry_url(
            super::super::signature::testing::TEST_REGISTRY,
        )
    }

    #[test]
    fn a_fresh_statement_states_its_tier_for_exactly_these_bytes() -> Result<(), String> {
        let keys = keys("acme")?;
        let subject = subject("acme/echo");
        let digest = digest();
        let package = StatedPackage {
            subject: &subject,
            version: "1.0.0",
            content_digest: &digest,
        };
        let now = crate::time::now_iso8601();
        let held = testing::statement(
            subject.clone(),
            "1.0.0",
            &digest,
            "acme",
            TrustTier::Verified,
            HELD,
        )?;
        assert_eq!(
            verified_tier(&held, package, &keys, Some(&source()), &now),
            Some(TrustTier::Verified)
        );
        assert_eq!(
            verified_tier(&held, package, &keys, None, &now),
            Some(TrustTier::Verified)
        );
        let other = testing::statement(
            subject.clone(),
            "1.0.0",
            &format!("sha256:{}", "d".repeat(64)),
            "acme",
            TrustTier::Verified,
            HELD,
        )?;
        assert_eq!(verified_tier(&other, package, &keys, None, &now), None);
        let expired = testing::statement(
            subject.clone(),
            "1.0.0",
            &digest,
            "acme",
            TrustTier::Verified,
            EXPIRED,
        )?;
        assert_eq!(verified_tier(&expired, package, &keys, None, &now), None);
        let elsewhere = registry_manifest_source_authority_from_registry_url("https://other.test");
        assert_eq!(
            verified_tier(&held, package, &keys, Some(&elsewhere), &now),
            None
        );
        assert_eq!(verified_tier(&held, package, &[], None, &now), None);
        Ok(())
    }

    /// No operator key states first_party, whatever it signs: such a
    /// statement is no statement, and the tier reads community.
    #[test]
    fn an_operator_key_stating_first_party_states_nothing() -> Result<(), String> {
        let keys = keys("acme")?;
        let subject = subject("acme/echo");
        let digest = digest();
        let package = StatedPackage {
            subject: &subject,
            version: "1.0.0",
            content_digest: &digest,
        };
        let claimed = testing::statement(
            subject.clone(),
            "1.0.0",
            &digest,
            "acme",
            TrustTier::FirstParty,
            HELD,
        )?;
        let now = crate::time::now_iso8601();
        assert_eq!(verified_tier(&claimed, package, &keys, None, &now), None);
        assert_eq!(
            accept_served(Some(&claimed), package, &keys, &source(), None),
            TrustTier::Community
        );
        Ok(())
    }

    /// A statement served with a version is kept in a rooted home, and a
    /// home that fell back keeps none and reads none: a statement planted in
    /// `<workspace>/.runx/tiers` is never read.
    #[test]
    fn a_statement_is_kept_only_in_a_rooted_home() -> Result<(), String> {
        let keys = keys("acme")?;
        let subject = subject("acme/echo");
        let digest = digest();
        let package = StatedPackage {
            subject: &subject,
            version: "1.0.0",
            content_digest: &digest,
        };
        let held = testing::statement(
            subject.clone(),
            "1.0.0",
            &digest,
            "acme",
            TrustTier::Verified,
            HELD,
        )?;
        let home = tempfile::tempdir().map_err(|error| error.to_string())?;
        let rooted = RunxPaths::resolve(
            &BTreeMap::from([(
                crate::paths::RUNX_HOME_ENV.to_owned(),
                home.path().display().to_string(),
            )]),
            home.path(),
        );
        let tiers = tier_cache(&rooted).ok_or("a rooted home keeps statements")?;
        assert_eq!(
            accept_served(Some(&held), package, &keys, &source(), Some(&tiers)),
            TrustTier::Verified
        );
        assert_eq!(kept(&tiers, &digest), Some(held.clone()));
        let workspace = tempfile::tempdir().map_err(|error| error.to_string())?;
        let fell_back = RunxPaths::resolve(
            &BTreeMap::from([(
                crate::paths::RUNX_CWD_ENV.to_owned(),
                workspace.path().display().to_string(),
            )]),
            workspace.path(),
        );
        assert_eq!(tier_cache(&fell_back), None);
        Ok(())
    }

    /// A statement kept in a rooted home answers while it holds, with no
    /// registry asked; the same statement planted in a home that fell back
    /// to the workspace is never read, and the package reads community.
    #[test]
    fn a_kept_statement_answers_only_from_a_rooted_home() -> Result<(), String> {
        let signed = SignedPackage {
            kind: RegistrySubjectKind::Skill,
            name: "acme/echo".to_owned(),
            version: "1.0.0".to_owned(),
            content_digest: digest(),
            key_id: super::super::signature::testing::KEY_ID.to_owned(),
        };
        let statement = testing::statement(
            subject("acme/echo"),
            "1.0.0",
            &signed.content_digest,
            "acme",
            TrustTier::Verified,
            HELD,
        )?;
        let workspace = tempfile::tempdir().map_err(|error| error.to_string())?;
        let home = tempfile::tempdir().map_err(|error| error.to_string())?;
        let mut env = super::super::signature::testing::trusting("acme")?;
        env.insert(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.path().display().to_string(),
        );
        let fell_back = RunxPaths::resolve(&env, workspace.path());
        keep(
            &fell_back.tier_statements(),
            &signed.content_digest,
            &statement,
        )?;
        assert_eq!(stated_tier(&signed, &env, &fell_back), TrustTier::Community);
        env.insert("HOME".to_owned(), home.path().display().to_string());
        let rooted = RunxPaths::resolve(&env, workspace.path());
        keep(
            &rooted.tier_statements(),
            &signed.content_digest,
            &statement,
        )?;
        assert_eq!(stated_tier(&signed, &env, &rooted), TrustTier::Verified);
        Ok(())
    }

    /// With no statement kept and no registry to read one from, a signed
    /// package reads community: offline work waits on a person's trust.
    #[test]
    fn offline_a_signed_package_reads_community() -> Result<(), String> {
        let workspace = tempfile::tempdir().map_err(|error| error.to_string())?;
        let mut env = super::super::signature::testing::trusting("acme")?;
        env.insert(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.path().display().to_string(),
        );
        // The test key's registry is loopback, and this runx is not let reach
        // a private address: the refresh is refused on the machine.
        let paths = RunxPaths::resolve(&env, workspace.path());
        let signed = SignedPackage {
            kind: RegistrySubjectKind::Skill,
            name: "acme/echo".to_owned(),
            version: "1.0.0".to_owned(),
            content_digest: digest(),
            key_id: super::super::signature::testing::KEY_ID.to_owned(),
        };
        assert_eq!(stated_tier(&signed, &env, &paths), TrustTier::Community);
        Ok(())
    }
}
