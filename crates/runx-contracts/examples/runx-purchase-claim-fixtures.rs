//! Purchaser signature vectors: claims and reads a purchaser key signed,
//! made here with Rust's signing and message construction. runx's verifier
//! admits each signed request and refuses each one that was changed after
//! signing or signed by another key.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{Ed25519KeyPair, KeyPair};
use runx_contracts::canonical_json::canonical_stable_json;
use runx_contracts::{
    PURCHASE_CLAIM_SCHEMA, PURCHASE_READ_SCHEMA, PurchaseClaim, purchase_claim_message,
    purchase_read_message,
};
use serde_json::{Value, json};

const FIXTURE: &str = "purchase-claim-v1.vectors.json";
const PURCHASER_SEED: [u8; 32] = [9; 32];
const STRANGER_SEED: [u8; 32] = [11; 32];
const CLAIM_URL: &str = "https://api.runx.ai/v1/purchases/paid_7Hq2x9/claim";
const READ_URL: &str = "https://api.runx.ai/v1/purchases/paid_7Hq2x9";
const ISSUED_AT: &str = "2026-10-15T00:01:30Z";

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

fn key_pair(seed: &[u8; 32]) -> io::Result<Ed25519KeyPair> {
    Ed25519KeyPair::from_seed_unchecked(seed).map_err(|error| io::Error::other(error.to_string()))
}

fn ed25519(bytes: &[u8]) -> String {
    format!("ed25519:{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn fixture_document() -> io::Result<Value> {
    let purchaser = key_pair(&PURCHASER_SEED)?;
    let stranger = key_pair(&STRANGER_SEED)?;
    let receipt_digest = format!("sha256:{}", "7".repeat(64));
    let claim =
        |claim_url: &str, receipt_digest: &str, key: &Ed25519KeyPair| -> io::Result<Value> {
            let message = purchase_claim_message(claim_url, ISSUED_AT, receipt_digest)
                .map_err(io::Error::other)?;
            let value = json!({
                "receipt_digest": receipt_digest,
                "claim_url": claim_url,
                "issued_at": ISSUED_AT,
                "signature": ed25519(key.sign(&message).as_ref()),
            });
            // Every claim is well formed; only its signature differs.
            serde_json::from_value::<PurchaseClaim>(value.clone()).map_err(io::Error::other)?;
            Ok(value)
        };
    let read = |method: &str, url: &str, key: &Ed25519KeyPair| -> io::Result<Value> {
        let message = purchase_read_message(method, url, ISSUED_AT).map_err(io::Error::other)?;
        Ok(json!({
            "method": method,
            "url": url,
            "issued_at": ISSUED_AT,
            "signature": ed25519(key.sign(&message).as_ref()),
        }))
    };
    let with = |value: Value, field: &str, replacement: &str| {
        let mut changed = value.as_object().cloned().unwrap_or_default();
        changed.insert(field.to_owned(), json!(replacement));
        Value::Object(changed)
    };

    let signed_claim = claim(CLAIM_URL, &receipt_digest, &purchaser)?;
    let signed_read = read("GET", READ_URL, &purchaser)?;
    let cases = vec![
        case("claim", "claim", signed_claim.clone(), true),
        case(
            "claim-of-another-receipt",
            "claim",
            with(
                signed_claim.clone(),
                "receipt_digest",
                &format!("sha256:{}", "8".repeat(64)),
            ),
            false,
        ),
        case(
            "claim-for-another-endpoint",
            "claim",
            with(
                signed_claim.clone(),
                "claim_url",
                "https://api.runx.ai/v1/purchases/paid_other/claim",
            ),
            false,
        ),
        case(
            "claim-signed-at-another-time",
            "claim",
            with(signed_claim, "issued_at", "2026-10-15T00:01:31Z"),
            false,
        ),
        case(
            "claim-signed-by-another-key",
            "claim",
            claim(CLAIM_URL, &receipt_digest, &stranger)?,
            false,
        ),
        case("read", "read", signed_read.clone(), true),
        case(
            "read-of-another-purchase",
            "read",
            with(
                signed_read.clone(),
                "url",
                "https://api.runx.ai/v1/purchases/paid_other",
            ),
            false,
        ),
        case(
            "read-by-another-method",
            "read",
            with(signed_read, "method", "POST"),
            false,
        ),
        case(
            "read-signed-by-another-key",
            "read",
            read("GET", READ_URL, &stranger)?,
            false,
        ),
    ];
    Ok(json!({
        "cases": cases,
        "claim_schema": PURCHASE_CLAIM_SCHEMA,
        "purchaser_key": ed25519(purchaser.public_key().as_ref()),
        "read_schema": PURCHASE_READ_SCHEMA,
        "schema": "runx.payment.purchaser_signature.fixtures.v1",
    }))
}

fn case(name: &str, kind: &str, request: Value, verifies: bool) -> Value {
    json!({"kind": kind, "name": name, "request": request, "verifies": verifies})
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
                "orphan purchaser signature fixture: {}",
                path.display()
            )));
        }
        fs::remove_file(path)?;
    }
    Ok(())
}
