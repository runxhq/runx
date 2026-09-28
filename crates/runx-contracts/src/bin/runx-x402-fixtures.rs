use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use runx_contracts::{
    RunxX402InvocationExtensionInfo, X402_PAYMENT_PAYLOAD_SCHEMA_ID,
    X402_PAYMENT_REQUIRED_SCHEMA_ID, X402_SETTLE_RESPONSE_SCHEMA_ID, X402_UPSTREAM_COMMIT,
    X402_UPSTREAM_PACKAGE, X402_UPSTREAM_PACKAGE_VERSION, X402PaymentPayload, X402PaymentRequired,
    X402SettleResponse, sha256_prefixed,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const OFFICIAL_REQUIRED_HEADER: &str = "eyJ4NDAyVmVyc2lvbiI6MiwiZXJyb3IiOiJQQVlNRU5ULVNJR05BVFVSRSBoZWFkZXIgaXMgcmVxdWlyZWQiLCJyZXNvdXJjZSI6eyJ1cmwiOiJodHRwczovL2FwaS5leGFtcGxlLmNvbS9wcmVtaXVtLWRhdGEiLCJkZXNjcmlwdGlvbiI6IkFjY2VzcyB0byBwcmVtaXVtIG1hcmtldCBkYXRhIiwibWltZVR5cGUiOiJhcHBsaWNhdGlvbi9qc29uIn0sImFjY2VwdHMiOlt7InNjaGVtZSI6ImV4YWN0IiwibmV0d29yayI6ImVpcDE1NTo4NDUzMiIsImFtb3VudCI6IjEwMDAwIiwiYXNzZXQiOiIweDAzNkNiRDUzODQyYzU0MjY2MzRlNzkyOTU0MWVDMjMxOGYzZENGN2UiLCJwYXlUbyI6IjB4MjA5NjkzQmM2YWZjMEM1MzI4YkEzNkZhRjAzQzUxNEVGMzEyMjg3QyIsIm1heFRpbWVvdXRTZWNvbmRzIjo2MCwiZXh0cmEiOnsibmFtZSI6IlVTREMiLCJ2ZXJzaW9uIjoiMiJ9fV19";
const OFFICIAL_SIGNATURE_HEADER: &str = "eyJ4NDAyVmVyc2lvbiI6MiwicmVzb3VyY2UiOnsidXJsIjoiaHR0cHM6Ly9hcGkuZXhhbXBsZS5jb20vcHJlbWl1bS1kYXRhIiwiZGVzY3JpcHRpb24iOiJBY2Nlc3MgdG8gcHJlbWl1bSBtYXJrZXQgZGF0YSIsIm1pbWVUeXBlIjoiYXBwbGljYXRpb24vanNvbiJ9LCJhY2NlcHRlZCI6eyJzY2hlbWUiOiJleGFjdCIsIm5ldHdvcmsiOiJlaXAxNTU6ODQ1MzIiLCJhbW91bnQiOiIxMDAwMCIsImFzc2V0IjoiMHgwMzZDYkQ1Mzg0MmM1NDI2NjM0ZTc5Mjk1NDFlQzIzMThmM2RDRjdlIiwicGF5VG8iOiIweDIwOTY5M0JjNmFmYzBDNTMyOGJBMzZGYUYwM0M1MTRFRjMxMjI4N0MiLCJtYXhUaW1lb3V0U2Vjb25kcyI6NjAsImV4dHJhIjp7Im5hbWUiOiJVU0RDIiwidmVyc2lvbiI6IjIifX0sInBheWxvYWQiOnsic2lnbmF0dXJlIjoiMHgyZDZhNzU4OGQ2YWNjYTUwNWNiZjBkOWE0YTIyN2UwYzUyYzZjMzQwMDhjOGU4OTg2YTEyODMyNTk3NjQxNzM2MDhhMmNlNjQ5NjY0MmUzNzdkNmRhOGRiYmY1ODM2ZTliZDE1MDkyZjllY2FiMDVkZWQzZDYyOTNhZjE0OGI1NzFjIiwiYXV0aG9yaXphdGlvbiI6eyJmcm9tIjoiMHg4NTdiMDY1MTlFOTFlM0E1NDUzODc5MWJEYmIwRTIyMzczZTM2YjY2IiwidG8iOiIweDIwOTY5M0JjNmFmYzBDNTMyOGJBMzZGYUYwM0M1MTRFRjMxMjI4N0MiLCJ2YWx1ZSI6IjEwMDAwIiwidmFsaWRBZnRlciI6IjE3NDA2NzIwODkiLCJ2YWxpZEJlZm9yZSI6IjE3NDA2NzIxNTQiLCJub25jZSI6IjB4ZjM3NDY2MTNjMmQ5MjBiNWZkYWJjMDg1NmYyYWViMmQ0Zjg4ZWU2MDM3YjhjYzVkMDRhNzFhNDQ2MmYxMzQ4MCJ9fX0=";
const OFFICIAL_RESPONSE_HEADER: &str = "eyJzdWNjZXNzIjp0cnVlLCJ0cmFuc2FjdGlvbiI6IjB4MTIzNDU2Nzg5MGFiY2RlZjEyMzQ1Njc4OTBhYmNkZWYxMjM0NTY3ODkwYWJjZGVmMTIzNDU2Nzg5MGFiY2RlZiIsIm5ldHdvcmsiOiJlaXAxNTU6ODQ1MzIiLCJwYXllciI6IjB4ODU3YjA2NTE5RTkxZTNBNTQ1Mzg3OTFiRGJiMEUyMjM3M2UzNmI2NiJ9";

const SCHEMAS: &[(&str, &str)] = &[
    (
        runx_contracts::X402_RESOURCE_INFO_SCHEMA_ID,
        "x402-v2-resource-info.schema.json",
    ),
    (
        runx_contracts::X402_PAYMENT_REQUIREMENTS_SCHEMA_ID,
        "x402-v2-payment-requirements.schema.json",
    ),
    (
        X402_PAYMENT_REQUIRED_SCHEMA_ID,
        "x402-v2-payment-required.schema.json",
    ),
    (
        X402_PAYMENT_PAYLOAD_SCHEMA_ID,
        "x402-v2-payment-payload.schema.json",
    ),
    (
        X402_SETTLE_RESPONSE_SCHEMA_ID,
        "x402-v2-settle-response.schema.json",
    ),
    (
        "runx.x402.invocation_extension.v1",
        "runx-x402-invocation-extension-v1.schema.json",
    ),
];

const SOURCES: &[(&str, &str)] = &[
    (
        "typescript/packages/core/src/schemas/index.ts",
        "sha256:6a772c8c33307dc81ebc1e6d0af4697a518f9910e1fe128c480b304a187f7564",
    ),
    (
        "typescript/packages/core/src/types/payments.ts",
        "sha256:bb5f8d9dce4910656991c36610ef07a8df592e417f051116e73d55af28c66ff2",
    ),
    (
        "typescript/packages/core/src/types/facilitator.ts",
        "sha256:2aff6bbf3e9e8a1b354b210b0635ee56a49184667e5391bdc002d4d58150387f",
    ),
    (
        "typescript/packages/core/src/http/index.ts",
        "sha256:b111b21ae7cf1e7f34f088a580434480188c70a47b93e3f87f45d03747c326ed",
    ),
    (
        "specs/x402-specification-v2.md",
        "sha256:cb66bdef710298233316b02d48fce4a9417ec5449ed9cab7f5d9fe6a454920cd",
    ),
    (
        "specs/transports-v2/http.md",
        "sha256:4f0298aaa23ac75de0eb49b1e96e6e67a5b910d7527b3a71c63e426b3bf5bdfb",
    ),
    (
        "specs/transports-v2/mcp.md",
        "sha256:4f1e0bb50c60e3fe384142f589cd6323b8a9ec4432335ef29053d1220f9e2146",
    ),
];

struct Options {
    out_dir: PathBuf,
    schema_dir: PathBuf,
    check: bool,
}

struct Vector {
    file: &'static str,
    kind: &'static str,
    provenance: &'static str,
    schema_id: &'static str,
    expectation: &'static str,
    payload: Value,
    header: Option<&'static str>,
}

fn main() -> io::Result<()> {
    let options = parse_args()?;
    let vectors = vectors()?;
    for vector in &vectors {
        let value = json!({
            "expectation": vector.expectation,
            "header": vector.header,
            "kind": vector.kind,
            "payload": vector.payload,
            "provenance": vector.provenance,
            "schema_id": vector.schema_id,
        });
        reconcile_file(
            &options.out_dir.join(vector.file),
            canonical_bytes(&value)?,
            options.check,
        )?;
    }

    let pin = pin();
    reconcile_file(
        &options.out_dir.join("upstream-pin.json"),
        canonical_bytes(&pin)?,
        options.check,
    )?;
    let manifest = manifest(&options, &vectors, &pin)?;
    reconcile_file(
        &options.out_dir.join("manifest.json"),
        canonical_bytes(&manifest)?,
        options.check,
    )?;
    reject_orphans(&options, &vectors)?;
    Ok(())
}

fn parse_args() -> io::Result<Options> {
    let mut out_dir = None;
    let mut schema_dir = None;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out_dir = args.next().map(PathBuf::from),
            "--schema-dir" => schema_dir = args.next().map(PathBuf::from),
            "--check" => check = true,
            other => return Err(io::Error::other(format!("unsupported argument: {other}"))),
        }
    }
    Ok(Options {
        out_dir: out_dir.ok_or_else(|| io::Error::other("--out is required"))?,
        schema_dir: schema_dir.ok_or_else(|| io::Error::other("--schema-dir is required"))?,
        check,
    })
}

fn vectors() -> io::Result<Vec<Vector>> {
    Ok(vec![
        valid::<X402PaymentRequired>(
            "official-payment-required.json",
            "payment-required",
            "x402 specification v2 section 5.1 and HTTP transport",
            X402_PAYMENT_REQUIRED_SCHEMA_ID,
            official_payment_required(),
            Some(OFFICIAL_REQUIRED_HEADER),
        )?,
        valid::<X402PaymentPayload>(
            "official-payment-payload.json",
            "payment-payload",
            "x402 specification v2 section 5.2 and HTTP transport",
            X402_PAYMENT_PAYLOAD_SCHEMA_ID,
            official_payment_payload(),
            Some(OFFICIAL_SIGNATURE_HEADER),
        )?,
        valid::<X402SettleResponse>(
            "official-settle-success.json",
            "settle-response",
            "x402 specification v2 section 5.3 and HTTP transport",
            X402_SETTLE_RESPONSE_SCHEMA_ID,
            official_settle_success(),
            Some(OFFICIAL_RESPONSE_HEADER),
        )?,
        valid::<X402SettleResponse>(
            "official-settle-failure.json",
            "settle-response",
            "x402 specification v2 HTTP transport failure example",
            X402_SETTLE_RESPONSE_SCHEMA_ID,
            json!({
                "success": false,
                "errorReason": "insufficient_funds",
                "payer": "0x857b06519E91e3A54538791bDbb0E22373e36b66",
                "transaction": "",
                "network": "eip155:84532"
            }),
            None,
        )?,
        valid::<RunxX402InvocationExtensionInfo>(
            "runx-discovery-extension.json",
            "runx_invocation_extension",
            "Runx RX-01-C projection",
            "runx.x402.invocation_extension.v1",
            discovery_extension(),
            None,
        )?,
        valid::<RunxX402InvocationExtensionInfo>(
            "runx-invocation-extension.json",
            "runx_invocation_extension",
            "Runx RX-01-C projection",
            "runx.x402.invocation_extension.v1",
            invocation_extension(),
            None,
        )?,
        invalid(
            "invalid-external-v1.json",
            "payment-required",
            X402_PAYMENT_REQUIRED_SCHEMA_ID,
            json!({ "x402Version": 1, "resource": { "url": "https://example.com" }, "accepts": [] }),
        ),
        invalid(
            "invalid-empty-accepts.json",
            "payment-required",
            X402_PAYMENT_REQUIRED_SCHEMA_ID,
            json!({ "x402Version": 2, "resource": { "url": "https://example.com" }, "accepts": [] }),
        ),
        invalid(
            "invalid-runx-extension-field.json",
            "runx_invocation_extension",
            "runx.x402.invocation_extension.v1",
            json!({
                "purpose": "discovery",
                "offer_revision": offer_revision(),
                "package_digest": digest('7'),
                "terms_digest": digest('8')
            }),
        ),
    ])
}

fn valid<T: DeserializeOwned>(
    file: &'static str,
    kind: &'static str,
    provenance: &'static str,
    schema_id: &'static str,
    payload: Value,
    header: Option<&'static str>,
) -> io::Result<Vector> {
    serde_json::from_value::<T>(payload.clone())
        .map_err(|error| io::Error::other(format!("invalid fixture {file}: {error}")))?;
    Ok(Vector {
        file,
        kind,
        provenance,
        schema_id,
        expectation: "valid",
        payload,
        header,
    })
}

fn invalid(
    file: &'static str,
    kind: &'static str,
    schema_id: &'static str,
    payload: Value,
) -> Vector {
    Vector {
        file,
        kind,
        provenance: "Runx negative conformance vector",
        schema_id,
        expectation: "invalid",
        payload,
        header: None,
    }
}

fn pin() -> Value {
    json!({
        "package": { "name": X402_UPSTREAM_PACKAGE, "version": X402_UPSTREAM_PACKAGE_VERSION },
        "repository": "https://github.com/x402-foundation/x402",
        "revision": X402_UPSTREAM_COMMIT,
        "schema": "runx.x402.upstream_pin.v1",
        "source_verification": {
            "command": "pnpm x402:conformance -- --upstream-dir <pinned-checkout>",
            "mode": "pinned_checkout_sha256"
        },
        "sources": SOURCES.iter().map(|(path, digest)| json!({ "digest": digest, "path": path })).collect::<Vec<_>>()
    })
}

fn manifest(options: &Options, vectors: &[Vector], pin: &Value) -> io::Result<Value> {
    let schemas = SCHEMAS
        .iter()
        .map(|(schema_id, file)| {
            let bytes = fs::read(options.schema_dir.join(file))?;
            Ok(json!({
                "file": file,
                "schema_id": schema_id,
                "sha256": sha256_prefixed(&bytes)
            }))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let vector_entries = vectors
        .iter()
        .map(|vector| {
            let bytes = canonical_bytes(&json!({
                "expectation": vector.expectation,
                "header": vector.header,
                "kind": vector.kind,
                "payload": vector.payload,
                "provenance": vector.provenance,
                "schema_id": vector.schema_id,
            }))?;
            Ok(json!({
                "expectation": vector.expectation,
                "file": vector.file,
                "kind": vector.kind,
                "schema_id": vector.schema_id,
                "sha256": sha256_prefixed(&bytes)
            }))
        })
        .collect::<io::Result<Vec<_>>>()?;
    Ok(json!({
        "external_protocol": { "name": "x402", "version": 2 },
        "pin_sha256": sha256_prefixed(&canonical_bytes(pin)?),
        "schema": "runx.x402.contract_fixtures.v1",
        "schemas": schemas,
        "vectors": vector_entries
    }))
}

fn official_payment_required() -> Value {
    json!({
        "x402Version": 2,
        "error": "PAYMENT-SIGNATURE header is required",
        "resource": {
            "url": "https://api.example.com/premium-data",
            "description": "Access to premium market data",
            "mimeType": "application/json"
        },
        "accepts": [requirements()]
    })
}

fn official_payment_payload() -> Value {
    json!({
        "x402Version": 2,
        "resource": {
            "url": "https://api.example.com/premium-data",
            "description": "Access to premium market data",
            "mimeType": "application/json"
        },
        "accepted": requirements(),
        "payload": {
            "signature": "0x2d6a7588d6acca505cbf0d9a4a227e0c52c6c34008c8e8986a1283259764173608a2ce6496642e377d6da8dbbf5836e9bd15092f9ecab05ded3d6293af148b571c",
            "authorization": {
                "from": "0x857b06519E91e3A54538791bDbb0E22373e36b66",
                "to": "0x209693Bc6afc0C5328bA36FaF03C514EF312287C",
                "value": "10000",
                "validAfter": "1740672089",
                "validBefore": "1740672154",
                "nonce": "0xf3746613c2d920b5fdabc0856f2aeb2d4f88ee6037b8cc5d04a71a4462f13480"
            }
        }
    })
}

fn official_settle_success() -> Value {
    json!({
        "success": true,
        "transaction": "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        "network": "eip155:84532",
        "payer": "0x857b06519E91e3A54538791bDbb0E22373e36b66"
    })
}

fn requirements() -> Value {
    json!({
        "scheme": "exact",
        "network": "eip155:84532",
        "amount": "10000",
        "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
        "payTo": "0x209693Bc6afc0C5328bA36FaF03C514EF312287C",
        "maxTimeoutSeconds": 60,
        "extra": { "name": "USDC", "version": "2" }
    })
}

fn discovery_extension() -> Value {
    json!({
        "purpose": "discovery",
        "offer_revision": offer_revision(),
        "package_digest": digest('7')
    })
}

fn invocation_extension() -> Value {
    json!({
        "purpose": "invocation",
        "invocation_id": "paid_ocr_1",
        "quote_ref": { "type": "receipt", "uri": "runx:receipt:quote-1" },
        "offer_revision": offer_revision(),
        "package_digest": digest('7'),
        "input_digest": digest('1'),
        "canonicalizer_version": "runx.receipt.c14n.v1",
        "idempotency": { "key": "ocr-1", "binding_digest": digest('6') }
    })
}

fn offer_revision() -> Value {
    json!({
        "offer_id": "ocr-v1",
        "revision": "2026-08-22.1",
        "revision_digest": digest('4'),
        "input_schema_digest": digest('2'),
        "output_schema_digest": digest('3')
    })
}

fn digest(character: char) -> String {
    format!("sha256:{}", character.to_string().repeat(64))
}

fn canonical_bytes(value: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn reconcile_file(path: &Path, expected: Vec<u8>, check: bool) -> io::Result<()> {
    if check {
        let actual = fs::read(path)?;
        if actual != expected {
            return Err(io::Error::other(format!(
                "fixture is stale: {}",
                path.display()
            )));
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, expected)
}

fn reject_orphans(options: &Options, vectors: &[Vector]) -> io::Result<()> {
    let mut expected = vectors
        .iter()
        .map(|vector| vector.file.to_owned())
        .collect::<BTreeSet<_>>();
    expected.insert("manifest.json".to_owned());
    expected.insert("upstream-pin.json".to_owned());
    if !options.out_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&options.out_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && name.ends_with(".json") && !expected.contains(&name) {
            return Err(io::Error::other(format!("orphan x402 fixture: {name}")));
        }
    }
    Ok(())
}
