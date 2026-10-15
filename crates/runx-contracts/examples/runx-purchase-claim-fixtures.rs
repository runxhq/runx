//! Purchase claim vectors: a purchase receipt signed by a runx receipt key,
//! and claims signed by the purchaser key it binds, made here with Rust's
//! signing and message construction. Every seller-side verifier accepts the
//! valid claim and refuses each tampered one for its stated reason.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{Ed25519KeyPair, KeyPair};
use runx_contracts::canonical_json::canonical_stable_json;
use runx_contracts::{
    PURCHASE_CLAIM_SCHEMA, PURCHASE_RECEIPT_SCHEMA, PurchaseClaim, purchase_claim_message,
    sha256_prefixed,
};
use serde_json::{Map, Value, json};

const FIXTURE: &str = "purchase-claim-v1.vectors.json";
const RUNX_SEED: [u8; 32] = [7; 32];
const PURCHASER_SEED: [u8; 32] = [9; 32];
const STRANGER_SEED: [u8; 32] = [11; 32];
const SELLER: &str = "runx:principal:ausca";
const CLAIM_URL: &str = "https://inbox.ausca.com/v1/agent-inboxes/inb_7Hq2x9/claim";
const NOW: &str = "2026-10-15T00:02:00Z";

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
    let runx = key_pair(&RUNX_SEED)?;
    let purchaser = key_pair(&PURCHASER_SEED)?;
    let stranger = key_pair(&STRANGER_SEED)?;
    let runx_public = runx.public_key().as_ref();
    let trust_root = json!({
        "schema": "runx.trust_root.v1",
        "keys": [{
            "kty": "OKP",
            "crv": "Ed25519",
            "alg": "EdDSA",
            "use": "sig",
            "kid": "fixture-runx-receipts",
            "purpose": "receipt_signature",
            "issuer_type": "hosted",
            "x": URL_SAFE_NO_PAD.encode(runx_public),
            "public_key_sha256": sha256_prefixed(runx_public),
        }],
    });
    let purchaser_key = ed25519(purchaser.public_key().as_ref());
    let receipt = |vendor: &str, outcome: &str, purchaser_key: Option<&str>| {
        let mut binding = json!({
            "invocation_id": "paid_7Hq2x9",
            "vendor_ref": {"type": "principal", "uri": vendor},
            "offer_revision": {
                "offer_id": "ausca/agent-inbox#create",
                "revision": "1.0.0",
                "revision_digest": format!("sha256:{}", "1".repeat(64)),
                "input_schema_digest": format!("sha256:{}", "2".repeat(64)),
                "output_schema_digest": format!("sha256:{}", "3".repeat(64)),
            },
            "package_digest": format!("sha256:{}", "4".repeat(64)),
            "input_digest": format!("sha256:{}", "5".repeat(64)),
        });
        if let (Some(key), Some(object)) = (purchaser_key, binding.as_object_mut()) {
            object.insert("purchaser_key".to_owned(), json!(key));
        }
        json!({
            "id": "sha256:receipt-fixture",
            "schema": PURCHASE_RECEIPT_SCHEMA,
            "principal_id": "buyer-1",
            "created_at": "2026-10-15T00:01:00Z",
            "paid_invocation": binding,
            "terminal": {"outcome": outcome},
        })
    };
    let sign_receipt = |body: Value, key: &Ed25519KeyPair| -> io::Result<Value> {
        let digest = receipt_digest(&body)?;
        let signature = key.sign(digest.as_bytes());
        let mut signed = body.as_object().cloned().unwrap_or_default();
        signed.insert("digest".to_owned(), json!(digest));
        signed.insert(
            "signature".to_owned(),
            json!({"alg": "Ed25519", "value": format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()))}),
        );
        Ok(Value::Object(signed))
    };
    let claim = |receipt: &Value,
                 claim_url: &str,
                 issued_at: &str,
                 key: &Ed25519KeyPair|
     -> io::Result<Value> {
        let digest = receipt
            .get("digest")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let message =
            purchase_claim_message(claim_url, issued_at, digest).map_err(io::Error::other)?;
        let value = json!({
            "receipt": receipt,
            "claim_url": claim_url,
            "issued_at": issued_at,
            "signature": ed25519(key.sign(&message).as_ref()),
        });
        // Every vector is a well-formed claim; only its proofs differ.
        serde_json::from_value::<PurchaseClaim>(value.clone()).map_err(io::Error::other)?;
        Ok(value)
    };

    let fulfilled = sign_receipt(receipt(SELLER, "fulfilment", Some(&purchaser_key)), &runx)?;
    let mut tampered = fulfilled.clone();
    if let Some(object) = tampered.as_object_mut() {
        object.insert("principal_id".to_owned(), json!("someone-else"));
    }
    let cases = vec![
        case(
            "valid",
            claim(&fulfilled, CLAIM_URL, "2026-10-15T00:01:30Z", &purchaser)?,
            json!({
                "invocation_id": "paid_7Hq2x9",
                "purchaser_key": purchaser_key,
            }),
        ),
        case(
            "receipt-changed-after-signing",
            claim(&tampered, CLAIM_URL, NOW, &purchaser)?,
            json!({"error": "receipt_digest"}),
        ),
        case(
            "receipt-signed-by-another-key",
            claim(
                &sign_receipt(
                    receipt(SELLER, "fulfilment", Some(&purchaser_key)),
                    &stranger,
                )?,
                CLAIM_URL,
                NOW,
                &purchaser,
            )?,
            json!({"error": "receipt_signature"}),
        ),
        case(
            "another-sellers-purchase",
            claim(
                &sign_receipt(
                    receipt(
                        "runx:principal:other-seller",
                        "fulfilment",
                        Some(&purchaser_key),
                    ),
                    &runx,
                )?,
                CLAIM_URL,
                NOW,
                &purchaser,
            )?,
            json!({"error": "seller"}),
        ),
        case(
            "refunded-purchase",
            claim(
                &sign_receipt(receipt(SELLER, "refund", Some(&purchaser_key)), &runx)?,
                CLAIM_URL,
                NOW,
                &purchaser,
            )?,
            json!({"error": "outcome"}),
        ),
        case(
            "purchase-without-a-purchaser-key",
            claim(
                &sign_receipt(receipt(SELLER, "fulfilment", None), &runx)?,
                CLAIM_URL,
                NOW,
                &purchaser,
            )?,
            json!({"error": "purchaser_key"}),
        ),
        case(
            "claim-for-another-endpoint",
            claim(
                &fulfilled,
                "https://inbox.ausca.com/v1/agent-inboxes/inb_other/claim",
                NOW,
                &purchaser,
            )?,
            json!({"error": "claim_url"}),
        ),
        case(
            "stale-claim",
            claim(&fulfilled, CLAIM_URL, "2026-10-14T23:50:00Z", &purchaser)?,
            json!({"error": "stale"}),
        ),
        case(
            "claim-signed-by-another-key",
            claim(&fulfilled, CLAIM_URL, NOW, &stranger)?,
            json!({"error": "purchaser_signature"}),
        ),
    ];
    Ok(json!({
        "cases": cases,
        "claim_url": CLAIM_URL,
        "max_skew_seconds": 300,
        "now": NOW,
        "schema": "runx.payment.purchase_claim.fixtures.v1",
        "claim_schema": PURCHASE_CLAIM_SCHEMA,
        "seller_principal_uri": SELLER,
        "trust_root": trust_root,
    }))
}

fn case(name: &str, claim: Value, expected: Value) -> Value {
    json!({"claim": claim, "expected": expected, "name": name})
}

/// A hosted receipt's digest: its body without `signature`, `digest` and
/// `metadata`, as stable JSON.
fn receipt_digest(receipt: &Value) -> io::Result<String> {
    let mut body: Map<String, Value> = receipt.as_object().cloned().unwrap_or_default();
    for field in ["signature", "digest", "metadata"] {
        body.remove(field);
    }
    let text = canonical_stable_json(&Value::Object(body)).map_err(io::Error::other)?;
    Ok(sha256_prefixed(text.as_bytes()))
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
                "orphan purchase-claim fixture: {}",
                path.display()
            )));
        }
        fs::remove_file(path)?;
    }
    Ok(())
}
