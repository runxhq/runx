//! Registry tier statement vectors: statements signed here with Rust's own
//! signing payload and a fixture key, and statements whose terms the
//! contract refuses. `@runxhq/contracts` builds the same payload bytes and
//! refuses the same statements, so what Cloud signs is what runx verifies.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{Ed25519KeyPair, KeyPair};
use runx_contracts::canonical_json::canonical_stable_json;
use runx_contracts::{
    REGISTRY_TIER_STATEMENT_SCHEMA, RegistryManifestSignature, RegistryManifestSigner,
    RegistrySubject, RegistrySubjectKind, RegistryTierStatement, TrustTier,
};
use serde_json::{Value, json};

const FIXTURE: &str = "registry-tier-statement-v1.vectors.json";
const SEED: [u8; 32] = [5; 32];
const KEY_ID: &str = "vectors-ed25519";

struct Options {
    out_dir: PathBuf,
    check: bool,
}

fn main() -> io::Result<()> {
    let options = parse_args()?;
    let document = fixture_document()?;
    reconcile_file(
        &options.out_dir.join(FIXTURE),
        canonical_bytes(&document)?,
        options.check,
    )?;
    reject_orphans(&options.out_dir, options.check)
}

fn parse_args() -> io::Result<Options> {
    let mut out_dir = None;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out_dir = args.next().map(PathBuf::from),
            "--check" => check = true,
            other => return Err(io::Error::other(format!("unsupported argument: {other}"))),
        }
    }
    Ok(Options {
        out_dir: out_dir.ok_or_else(|| io::Error::other("--out is required"))?,
        check,
    })
}

/// A statement of `tier` for `subject@version`, signed by the fixture key.
fn signed(
    key: &Ed25519KeyPair,
    subject: RegistrySubject,
    version: &str,
    content_digest: String,
    tier: TrustTier,
) -> io::Result<(RegistryTierStatement, String)> {
    let mut statement = RegistryTierStatement {
        schema: REGISTRY_TIER_STATEMENT_SCHEMA.to_owned(),
        subject,
        version: version.to_owned(),
        content_digest,
        tier,
        issued_at: "2026-11-08T00:00:00Z".to_owned(),
        expires_at: "2026-11-15T00:00:00Z".to_owned(),
        signer: RegistryManifestSigner {
            id: "runx-registry".to_owned(),
            key_id: KEY_ID.to_owned(),
        },
        signature: RegistryManifestSignature {
            alg: "ed25519".to_owned(),
            value: String::new(),
        },
    };
    let payload = statement.signing_payload().map_err(io::Error::other)?;
    statement.signature.value = format!(
        "base64:{}",
        URL_SAFE_NO_PAD.encode(key.sign(&payload).as_ref())
    );
    statement.validate().map_err(io::Error::other)?;
    let payload = String::from_utf8(payload).map_err(io::Error::other)?;
    Ok((statement, payload))
}

fn fixture_document() -> io::Result<Value> {
    let key = Ed25519KeyPair::from_seed_unchecked(&SEED)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let (skill, skill_payload) = signed(
        &key,
        RegistrySubject {
            kind: RegistrySubjectKind::Skill,
            name: "acme/echo".to_owned(),
        },
        "1.0.0",
        format!("sha256:{}", "a".repeat(64)),
        TrustTier::Verified,
    )?;
    let (connector, connector_payload) = signed(
        &key,
        RegistrySubject {
            kind: RegistrySubjectKind::Connector,
            name: "acme/records".to_owned(),
        },
        "2.1.0",
        format!("sha256:{}", "b".repeat(64)),
        TrustTier::Community,
    )?;
    let skill_value = serde_json::to_value(&skill).map_err(io::Error::other)?;
    let refused = |name: &str, change: &dyn Fn(&mut Value)| {
        let mut statement = skill_value.clone();
        change(&mut statement);
        json!({ "name": name, "statement": statement })
    };
    Ok(json!({
        "schema": "runx.registry_tier_statement.vectors.v1",
        "key_id": KEY_ID,
        "public_key_base64": STANDARD.encode(key.public_key().as_ref()),
        "statements": [
            {
                "name": "a verified skill version",
                "statement": skill_value,
                "signing_payload": skill_payload,
            },
            {
                "name": "a community connector version",
                "statement": serde_json::to_value(&connector).map_err(io::Error::other)?,
                "signing_payload": connector_payload,
            },
        ],
        "invalid_statements": [
            refused("a field the contract does not name", &|statement| {
                statement["withdrawn"] = json!(false);
            }),
            refused("an instant with fractions of a second", &|statement| {
                statement["issued_at"] = json!("2026-11-08T00:00:00.000Z");
            }),
            refused("an instant with an offset", &|statement| {
                statement["expires_at"] = json!("2026-11-15T00:00:00+00:00");
            }),
            refused("expires_at at issued_at", &|statement| {
                statement["expires_at"] = statement["issued_at"].clone();
            }),
            refused("expires_at before issued_at", &|statement| {
                statement["expires_at"] = json!("2026-11-01T00:00:00Z");
            }),
            refused("another schema", &|statement| {
                statement["schema"] = json!("runx.registry.signed_manifest.v1");
            }),
            refused("a tier runx has no name for", &|statement| {
                statement["tier"] = json!("trusted");
            }),
            refused("a skill named without its publisher", &|statement| {
                statement["subject"]["name"] = json!("echo");
            }),
            refused("a short content digest", &|statement| {
                statement["content_digest"] = json!("sha256:abc");
            }),
            refused("another signature algorithm", &|statement| {
                statement["signature"]["alg"] = json!("rsa");
            }),
        ],
    }))
}

fn canonical_bytes(value: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = canonical_stable_json(value)
        .map_err(io::Error::other)?
        .into_bytes();
    bytes.push(b'\n');
    Ok(bytes)
}

fn reconcile_file(path: &Path, expected: Vec<u8>, check: bool) -> io::Result<()> {
    if check {
        let actual = fs::read(path).map_err(|error| {
            io::Error::other(format!(
                "missing generated fixture {}: {error}",
                path.display()
            ))
        })?;
        if actual != expected {
            return Err(io::Error::other(format!(
                "generated fixture is stale: {}",
                path.display()
            )));
        }
    } else {
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| io::Error::other("fixture path has no parent"))?,
        )?;
        fs::write(path, expected)?;
    }
    Ok(())
}

fn reject_orphans(out_dir: &Path, check: bool) -> io::Result<()> {
    let expected = BTreeSet::from([FIXTURE]);
    if !out_dir.exists() {
        return if check {
            Err(io::Error::other(format!(
                "missing generated fixture directory: {}",
                out_dir.display()
            )))
        } else {
            Ok(())
        };
    }
    for entry in fs::read_dir(out_dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if path.extension().and_then(|value| value.to_str()) != Some("json")
            || expected.contains(name)
        {
            continue;
        }
        if check {
            return Err(io::Error::other(format!(
                "orphan tier statement fixture: {}",
                path.display()
            )));
        }
        fs::remove_file(path)?;
    }
    Ok(())
}
