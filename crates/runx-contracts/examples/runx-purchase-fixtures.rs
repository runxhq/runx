use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use runx_contracts::{
    CANCEL_PURCHASE, CancelPurchaseRequest, CancelPurchaseResult, EXECUTE_PURCHASE,
    ExecutePurchaseRequest, ExecutePurchaseResult, GetPurchaseRequest, GetPurchaseResult,
    JsonObject, JsonValue as RunxJsonValue, MAX_PORTABLE_INTEGER, PURCHASE_REQUEST_DIGEST_SCHEMA,
    PaidSkillListing, PurchaseRead, QUOTE_PURCHASE, QuotePurchaseRequest, QuotePurchaseResult,
    STABLE_JSON_CANONICALIZATION, canonical_stable_json, digest_cancel_purchase_request,
    digest_execute_purchase_request, digest_quote_purchase_request, sha256_prefixed,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const SCHEMAS: &[(&str, &str)] = &[
    (
        "runx.marketplace.paid_skill_listing.v1",
        "paid-skill-listing.schema.json",
    ),
    ("runx.payment.purchase.v1", "purchase.schema.json"),
    (
        "runx.payment.offer_version_binding.v1",
        "offer-version-binding.schema.json",
    ),
    (
        "runx.payment.quote_purchase.request.v1",
        "quote-purchase-request.schema.json",
    ),
    (
        "runx.payment.quote_purchase.result.v1",
        "quote-purchase-result.schema.json",
    ),
    (
        "runx.payment.execute_purchase.request.v1",
        "execute-purchase-request.schema.json",
    ),
    (
        "runx.payment.execute_purchase.result.v1",
        "execute-purchase-result.schema.json",
    ),
    (
        "runx.payment.get_purchase.request.v1",
        "get-purchase-request.schema.json",
    ),
    (
        "runx.payment.get_purchase.result.v1",
        "get-purchase-result.schema.json",
    ),
    ("runx.payment.purchase_read.v1", "purchase-read.schema.json"),
    (
        "runx.payment.cancel_purchase.request.v1",
        "cancel-purchase-request.schema.json",
    ),
    (
        "runx.payment.cancel_purchase.result.v1",
        "cancel-purchase-result.schema.json",
    ),
];

struct Options {
    out_dir: PathBuf,
    schema_dir: PathBuf,
    oracle_out: PathBuf,
    check: bool,
}

struct Vector {
    file: &'static str,
    description: &'static str,
    operation: &'static str,
    schema_id: &'static str,
    expectation: &'static str,
    payload: Value,
}

fn main() -> io::Result<()> {
    let options = parse_args()?;
    let vectors = vectors()?;
    reconcile_vectors(&options, &vectors)?;
    let manifest = manifest(&options, &vectors)?;
    reconcile_file(
        &options.out_dir.join("manifest.json"),
        canonical_bytes(&manifest)?,
        options.check,
    )?;
    reconcile_file(
        &options.oracle_out,
        canonical_bytes(&digest_oracle()?)?,
        options.check,
    )?;
    reject_orphan_vectors(&options, &vectors)?;
    Ok(())
}

fn parse_args() -> io::Result<Options> {
    let mut out_dir = None;
    let mut schema_dir = None;
    let mut oracle_out = None;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out_dir = args.next().map(PathBuf::from),
            "--schema-dir" => schema_dir = args.next().map(PathBuf::from),
            "--oracle-out" => oracle_out = args.next().map(PathBuf::from),
            "--check" => check = true,
            other => return Err(io::Error::other(format!("unsupported argument: {other}"))),
        }
    }
    Ok(Options {
        out_dir: out_dir.ok_or_else(|| io::Error::other("--out is required"))?,
        schema_dir: schema_dir.ok_or_else(|| io::Error::other("--schema-dir is required"))?,
        oracle_out: oracle_out.ok_or_else(|| io::Error::other("--oracle-out is required"))?,
        check,
    })
}

fn vectors() -> io::Result<Vec<Vector>> {
    let direct = purchase("paid_direct", "settled", "queued", "open", true);
    let replay = purchase("paid_direct", "settled", "queued", "open", true);
    let fulfilled = purchase(
        "paid_fulfilled",
        "settled",
        "succeeded",
        "fulfilment_won",
        true,
    );
    let challenge_payload = json!({
        "authorization": "opaque-provider-payload",
        "resource": "runx:offer:transcribe-v1"
    });
    let challenge_digest = digest_value(&challenge_payload)?;

    Ok(vec![
        valid::<PaidSkillListing>(
            "paid-skill-listing-multi-rail.json",
            "One ordinary skill runner advertises provider-neutral compatible settlement families.",
            "PaidSkillListing",
            "runx.marketplace.paid_skill_listing.v1",
            paid_skill_listing(),
        )?,
        valid::<QuotePurchaseResult>(
            "quote-direct-admission.json",
            "A direct quote admits one purchase and an opaque, digest-bound challenge.",
            "QuotePurchase",
            "runx.payment.quote_purchase.result.v1",
            json!({
                "status": "admitted",
                "value": {
                    "payment_options": [challenge(challenge_payload, challenge_digest)],
                    "purchase": direct.clone()
                }
            }),
        )?,
        valid::<QuotePurchaseResult>(
            "quote-same-term-replay.json",
            "The same idempotency binding and terms resolve to the same purchase.",
            "QuotePurchase",
            "runx.payment.quote_purchase.result.v1",
            json!({
                "status": "admitted",
                "value": {
                    "payment_options": [challenge(json!({"authorization": "opaque-provider-payload", "resource": "runx:offer:transcribe-v1"}), challenge_digest_for_default()?)],
                    "purchase": replay
                }
            }),
        )?,
        valid::<QuotePurchaseRequest>(
            "quote-independent-purchase.json",
            "The same input digest with another idempotency binding remains a distinct purchase intent.",
            "QuotePurchase",
            "runx.payment.quote_purchase.request.v1",
            quote_request("idem_independent"),
        )?,
        valid::<QuotePurchaseResult>(
            "quote-terms-changed.json",
            "A change of terms is a typed domain refusal.",
            "QuotePurchase",
            "runx.payment.quote_purchase.result.v1",
            refusal("terms_changed", "The offer version no longer matches."),
        )?,
        valid::<QuotePurchaseResult>(
            "quote-replay-conflict.json",
            "Reusing an idempotency key for different terms is a typed domain refusal.",
            "QuotePurchase",
            "runx.payment.quote_purchase.result.v1",
            refusal(
                "replay_conflict",
                "The idempotency binding commits to other terms.",
            ),
        )?,
        valid::<QuotePurchaseResult>(
            "quote-expired.json",
            "An expired quote remains a valid typed domain refusal.",
            "QuotePurchase",
            "runx.payment.quote_purchase.result.v1",
            refusal("quote_expired", "The quote has expired."),
        )?,
        valid::<ExecutePurchaseRequest>(
            "execute-request.json",
            "Execution names the purchase, its payment proof, its settlement family and the idempotency binding of the quote it executes.",
            "ExecutePurchase",
            "runx.payment.execute_purchase.request.v1",
            execute_request(),
        )?,
        valid::<ExecutePurchaseResult>(
            "execute-admitted.json",
            "Payment authorization admits execution without exposing a provider rail payload.",
            "ExecutePurchase",
            "runx.payment.execute_purchase.result.v1",
            admitted(direct),
        )?,
        valid::<GetPurchaseResult>(
            "get-fulfilment-won.json",
            "A fulfilled record carries the fulfilment winner.",
            "GetPurchase",
            "runx.payment.get_purchase.result.v1",
            admitted(fulfilled.clone()),
        )?,
        valid::<PurchaseRead>(
            "purchase-read-fulfilled.json",
            "A buyer reads its fulfilled purchase: the purchase, its receipt, the runner's public result and what runx holds for it, never a held value. Never the seller's run.",
            "PurchaseRead",
            "runx.payment.purchase_read.v1",
            json!({
                "purchase": fulfilled.clone(),
                "receipt": {
                    "id": "sha256:purchase-fulfilled",
                    "schema": "runx.hosted_purchase_terminal_receipt.v1",
                    "terminal": {"outcome": "fulfilment"},
                    "digest": format!("sha256:{}", "9".repeat(64)),
                    "signature": {"alg": "Ed25519", "value": "base64:fixture"}
                },
                "result": {"resource_lease": {"inbox_id": "inb_7Hq2x9", "address": "agent@inbox.example"}},
                "holds": [{
                    "connector": "ausca/inbox",
                    "credential": "lease",
                    "target": "ausca:agent-inbox/inb_7Hq2x9",
                    "expires_at": "2026-08-22T10:00:00Z",
                    "value_digest": format!("sha256:{}", "a".repeat(64))
                }]
            }),
        )?,
        valid::<GetPurchaseResult>(
            "get-refund-won.json",
            "The refund winner is orthogonal to terminal execution observation.",
            "GetPurchase",
            "runx.payment.get_purchase.result.v1",
            admitted(purchase(
                "paid_refund",
                "refunded",
                "failed",
                "refund_won",
                true,
            )),
        )?,
        valid::<GetPurchaseResult>(
            "get-run-reference.json",
            "A completed purchase exposes its hosted run without embedding run-domain state.",
            "GetPurchase",
            "runx.payment.get_purchase.result.v1",
            json!({
                "status": "admitted",
                "value": {
                    "purchase": purchase("paid_run", "settled", "succeeded", "fulfilment_won", true),
                    "receipt_ref": reference("receipt", "runx:receipt:paid-run-1"),
                    "run_ref": reference("act", "runx:run:hosted-1")
                }
            }),
        )?,
        valid::<GetPurchaseRequest>(
            "get-request.json",
            "Lookup names only the purchase.",
            "GetPurchase",
            "runx.payment.get_purchase.request.v1",
            json!({"purchase_id": "paid_direct"}),
        )?,
        valid::<CancelPurchaseRequest>(
            "cancel-request.json",
            "Cancellation is idempotently bound to the purchase.",
            "CancelPurchase",
            "runx.payment.cancel_purchase.request.v1",
            json!({"idempotency": idempotency("cancel_direct"), "purchase_id": "paid_direct"}),
        )?,
        valid::<CancelPurchaseResult>(
            "cancel-before-settlement.json",
            "Cancellation before settlement leaves the payment unpaid.",
            "CancelPurchase",
            "runx.payment.cancel_purchase.result.v1",
            admitted(purchase(
                "paid_cancel_early",
                "unpaid",
                "cancelled",
                "open",
                false,
            )),
        )?,
        valid::<CancelPurchaseResult>(
            "cancel-after-settlement.json",
            "Cancellation after settlement does not imply a refund transition.",
            "CancelPurchase",
            "runx.payment.cancel_purchase.result.v1",
            admitted(purchase(
                "paid_cancel_late",
                "settled",
                "cancelled",
                "open",
                true,
            )),
        )?,
        invalid(
            "invalid-paid-skill-listing-provider-field.json",
            "Provider-specific commercial fields cannot enter the marketplace listing contract.",
            "PaidSkillListing",
            "runx.marketplace.paid_skill_listing.v1",
            with_field(
                paid_skill_listing(),
                "stripe_price_id",
                json!("price_private"),
            )?,
        ),
        invalid(
            "invalid-unknown-field.json",
            "V1 request objects reject additive fields.",
            "QuotePurchase",
            "runx.payment.quote_purchase.request.v1",
            with_field(quote_request("idem_unknown"), "unexpected", json!(true))?,
        ),
        invalid(
            "invalid-malformed-digest.json",
            "Digest values are lowercase prefixed SHA-256 only.",
            "QuotePurchase",
            "runx.payment.quote_purchase.request.v1",
            with_field(
                quote_request("idem_digest"),
                "input_digest",
                json!("sha256:no"),
            )?,
        ),
        invalid(
            "invalid-amount-above-portable.json",
            "Minor-unit amounts above the shared portable integer bound are rejected.",
            "QuotePurchase",
            "runx.payment.quote_purchase.request.v1",
            with_field(
                quote_request("idem_amount"),
                "amount_minor",
                json!(MAX_PORTABLE_INTEGER + 1),
            )?,
        ),
        invalid(
            "invalid-non-principal.json",
            "The buyer reference cannot name another reference type than a principal.",
            "QuotePurchase",
            "runx.payment.quote_purchase.request.v1",
            with_field(
                quote_request("idem_principal"),
                "buyer_ref",
                reference("artifact", "runx:artifact:not-a-principal"),
            )?,
        ),
        invalid(
            "invalid-expiry.json",
            "Quote expiry must use the shared UTC ISO datetime shape.",
            "QuotePurchase",
            "runx.payment.quote_purchase.result.v1",
            with_pointer_value(
                admitted(purchase(
                    "paid_expiry",
                    "unpaid",
                    "unstarted",
                    "open",
                    false,
                )),
                "/value/purchase/expires_at",
                json!("tomorrow"),
            )?,
        ),
        invalid(
            "invalid-response-status-mismatch.json",
            "An admitted status cannot carry refusal fields.",
            "ExecutePurchase",
            "runx.payment.execute_purchase.result.v1",
            json!({"code": "payment_not_authorized", "reason": "No authority.", "status": "admitted"}),
        ),
    ])
}

fn valid<T: DeserializeOwned + Serialize>(
    file: &'static str,
    description: &'static str,
    operation: &'static str,
    schema_id: &'static str,
    payload: Value,
) -> io::Result<Vector> {
    let typed: T = serde_json::from_value(payload).map_err(io::Error::other)?;
    let payload = serde_json::to_value(typed).map_err(io::Error::other)?;
    Ok(Vector {
        file,
        description,
        operation,
        schema_id,
        expectation: "valid",
        payload,
    })
}

fn invalid(
    file: &'static str,
    description: &'static str,
    operation: &'static str,
    schema_id: &'static str,
    payload: Value,
) -> Vector {
    Vector {
        file,
        description,
        operation,
        schema_id,
        expectation: "invalid",
        payload,
    }
}

fn quote_request(idempotency_key: &str) -> Value {
    json!({
        "accepted_settlement_families": ["mock", "hosted"],
        "amount_minor": 1250,
        "buyer_ref": reference("principal", "runx:principal:buyer-1"),
        "canonicalizer_version": "runx.receipt.c14n.v1",
        "currency": "USD",
        "idempotency": idempotency(idempotency_key),
        "input_digest": digest('1'),
        "offer_version": offer_version(),
        "package_digest": digest('7'),
        "seller_ref": reference("principal", "runx:principal:seller-1"),
        "skill_ref": reference("skill_binding", "runx:skill:acme/transcribe@1.0.0#transcribe")
    })
}

fn paid_skill_listing() -> Value {
    json!({
        "skill_id": "acme/transcribe",
        "version": "1.0.0",
        "skill_digest": digest('8'),
        "profile_digest": digest('9'),
        "bundle_digest": digest('a'),
        "seller_ref": reference("principal", "runx:principal:seller-1"),
        "offers": {"transcribe": {
            "offer_version": {
                "offer_id": "acme/transcribe#transcribe",
                "version": "1.0.0",
                "profile_digest": digest('9'),
                "input_schema_digest": digest('2'),
                "output_schema_digest": digest('3')
            },
            "price": {"model": "fixed", "amount_minor": 1250},
            "currency": "USD",
            "accepted_settlement_families": ["x402", "stripe-spt"]
        }}
    })
}

fn execute_request() -> Value {
    json!({
        "idempotency": idempotency("execute_direct"),
        "purchase_id": "paid_direct",
        "payment_ref": reference("receipt", "runx:receipt:payment-proof-1"),
        "settlement_family": "hosted"
    })
}

fn purchase(
    purchase_id: &str,
    payment_state: &str,
    execution_state: &str,
    outcome_gate: &str,
    settled: bool,
) -> Value {
    let mut value = json!({
        "accepted_settlement_families": ["mock", "hosted"],
        "amount_minor": 1250,
        "buyer_ref": reference("principal", "runx:principal:buyer-1"),
        "canonicalizer_version": "runx.receipt.c14n.v1",
        "created_at": "2026-08-22T09:00:00Z",
        "currency": "USD",
        "execution_state": execution_state,
        "expires_at": "2026-08-22T09:05:00Z",
        "idempotency": idempotency("quote_direct"),
        "input_digest": digest('1'),
        "purchase_id": purchase_id,
        "offer_version": offer_version(),
        "outcome_gate": outcome_gate,
        "package_digest": digest('7'),
        "payment_state": payment_state,
        "seller_ref": reference("principal", "runx:principal:seller-1"),
        "skill_ref": reference("skill_binding", "runx:skill:acme/transcribe@1.0.0#transcribe"),
        "updated_at": "2026-08-22T09:01:00Z"
    });
    if settled && let Some(object) = value.as_object_mut() {
        object.insert("settlement_family".to_owned(), json!("hosted"));
        object.insert(
            "payment_ref".to_owned(),
            reference("receipt", "runx:receipt:payment-proof-1"),
        );
    }
    if execution_state != "unstarted"
        && let Some(object) = value.as_object_mut()
    {
        object.insert(
            "act_ref".to_owned(),
            reference("act", &format!("runx:act:{purchase_id}")),
        );
    }
    value
}

fn offer_version() -> Value {
    json!({
        "input_schema_digest": digest('2'),
        "offer_id": "transcribe-v1",
        "output_schema_digest": digest('3'),
        "profile_digest": digest('4'),
        "version": "2026-08-22.1"
    })
}

fn idempotency(key: &str) -> Value {
    json!({"binding_digest": digest('6'), "key": key})
}

fn digest_oracle() -> io::Result<Value> {
    let quote = quote_request("digest_quote");
    let quote_cases = [
        ("quote-base", quote.clone()),
        (
            "quote-buyer-ref",
            with_pointer_value(
                quote.clone(),
                "/buyer_ref/uri",
                json!("runx:principal:buyer-2"),
            )?,
        ),
        (
            "quote-seller-ref",
            with_pointer_value(
                quote.clone(),
                "/seller_ref/uri",
                json!("runx:principal:seller-2"),
            )?,
        ),
        (
            "quote-skill-ref",
            with_pointer_value(
                quote.clone(),
                "/skill_ref/uri",
                json!("runx:skill:acme/transcribe@1.0.1#transcribe"),
            )?,
        ),
        (
            "quote-offer-version",
            with_pointer_value(
                quote.clone(),
                "/offer_version/version",
                json!("2026-08-22.2"),
            )?,
        ),
        (
            "quote-package-digest",
            with_pointer_value(quote.clone(), "/package_digest", json!(digest('8')))?,
        ),
        (
            "quote-input-digest",
            with_pointer_value(quote.clone(), "/input_digest", json!(digest('9')))?,
        ),
        (
            "quote-maximum-portable-amount",
            with_pointer_value(quote.clone(), "/amount_minor", json!(MAX_PORTABLE_INTEGER))?,
        ),
        (
            "quote-currency",
            with_pointer_value(quote.clone(), "/currency", json!("EUR"))?,
        ),
        (
            "quote-settlement-families",
            with_pointer_value(
                quote.clone(),
                "/accepted_settlement_families",
                json!(["hosted"]),
            )?,
        ),
        (
            "quote-idempotency",
            with_pointer_value(quote, "/idempotency/binding_digest", json!(digest('a')))?,
        ),
    ];

    let execute = execute_request();
    let execute_cases = [
        ("execute-base", execute.clone()),
        (
            "execute-purchase",
            with_pointer_value(execute.clone(), "/purchase_id", json!("paid_other"))?,
        ),
        (
            "execute-settlement-family",
            with_pointer_value(execute.clone(), "/settlement_family", json!("mock"))?,
        ),
        (
            "execute-payment-ref",
            with_pointer_value(
                execute.clone(),
                "/payment_ref/uri",
                json!("runx:receipt:payment-proof-2"),
            )?,
        ),
        (
            "execute-idempotency",
            with_pointer_value(execute, "/idempotency/binding_digest", json!(digest('a')))?,
        ),
    ];

    let cancel = json!({
        "idempotency": idempotency("digest_cancel"),
        "purchase_id": "paid_direct"
    });
    let cancel_cases = [
        ("cancel-base", cancel.clone()),
        (
            "cancel-purchase",
            with_pointer_value(cancel.clone(), "/purchase_id", json!("paid_other"))?,
        ),
        (
            "cancel-idempotency",
            with_pointer_value(cancel, "/idempotency/binding_digest", json!(digest('a')))?,
        ),
    ];

    let mut cases = Vec::new();
    for (name, request) in quote_cases {
        cases.push(digest_case(name, QUOTE_PURCHASE, request)?);
    }
    for (name, request) in execute_cases {
        cases.push(digest_case(name, EXECUTE_PURCHASE, request)?);
    }
    for (name, request) in cancel_cases {
        cases.push(digest_case(name, CANCEL_PURCHASE, request)?);
    }

    Ok(json!({
        "canonicalization": STABLE_JSON_CANONICALIZATION,
        "cases": cases,
        "schema": "runx.canonical_json_oracle.v1"
    }))
}

fn digest_case(name: &str, operation: &str, request: Value) -> io::Result<Value> {
    let expected_digest = match operation {
        QUOTE_PURCHASE => digest_quote_purchase_request(
            &serde_json::from_value::<QuotePurchaseRequest>(request.clone())
                .map_err(io::Error::other)?,
        ),
        EXECUTE_PURCHASE => digest_execute_purchase_request(
            &serde_json::from_value::<ExecutePurchaseRequest>(request.clone())
                .map_err(io::Error::other)?,
        ),
        CANCEL_PURCHASE => digest_cancel_purchase_request(
            &serde_json::from_value::<CancelPurchaseRequest>(request.clone())
                .map_err(io::Error::other)?,
        ),
        _ => {
            return Err(io::Error::other(format!(
                "unsupported operation: {operation}"
            )));
        }
    }
    .map_err(io::Error::other)?;

    let request_value =
        serde_json::from_value::<RunxJsonValue>(request.clone()).map_err(io::Error::other)?;
    let preimage = RunxJsonValue::Object(JsonObject::from([
        (
            "canonicalization".to_owned(),
            RunxJsonValue::String(STABLE_JSON_CANONICALIZATION.to_owned()),
        ),
        (
            "operation".to_owned(),
            RunxJsonValue::String(operation.to_owned()),
        ),
        ("request".to_owned(), request_value),
        (
            "schema".to_owned(),
            RunxJsonValue::String(PURCHASE_REQUEST_DIGEST_SCHEMA.to_owned()),
        ),
    ]));
    let canonical_json = canonical_stable_json(&preimage).map_err(io::Error::other)?;
    if sha256_prefixed(canonical_json.as_bytes()) != expected_digest {
        return Err(io::Error::other(format!(
            "request digest helper and oracle preimage disagree for {name}"
        )));
    }

    Ok(json!({
        "canonical_json": canonical_json,
        "expected_digest": expected_digest,
        "name": name,
        "operation": operation,
        "preimage": serde_json::to_value(preimage).map_err(io::Error::other)?,
        "request": request
    }))
}

fn reference(reference_type: &str, uri: &str) -> Value {
    json!({"type": reference_type, "uri": uri})
}

fn challenge(payload: Value, payload_digest: String) -> Value {
    json!({
        "media_type": "application/vnd.example.payment+json",
        "payload": payload,
        "payload_digest": payload_digest,
        "protocol_version": "2026-08-01",
        "quote_expires_at": "2026-08-22T09:05:00Z",
        "quote_ref": reference("receipt", "runx:receipt:quote-1"),
        "settlement_family": "hosted"
    })
}

fn challenge_digest_for_default() -> io::Result<String> {
    digest_value(&json!({
        "authorization": "opaque-provider-payload",
        "resource": "runx:offer:transcribe-v1"
    }))
}

fn digest_value(value: &Value) -> io::Result<String> {
    Ok(sha256_prefixed(
        serde_json::to_string(value)
            .map_err(io::Error::other)?
            .as_bytes(),
    ))
}

fn digest(fill: char) -> String {
    format!("sha256:{}", fill.to_string().repeat(64))
}

fn admitted(purchase: Value) -> Value {
    json!({"status": "admitted", "value": {"purchase": purchase}})
}

fn refusal(code: &str, reason: &str) -> Value {
    json!({"code": code, "reason": reason, "status": "refused"})
}

fn with_field(mut value: Value, name: &str, field: Value) -> io::Result<Value> {
    value
        .as_object_mut()
        .ok_or_else(|| io::Error::other("fixture payload must be an object"))?
        .insert(name.to_owned(), field);
    Ok(value)
}

fn with_pointer_value(mut value: Value, pointer: &str, field: Value) -> io::Result<Value> {
    let target = value
        .pointer_mut(pointer)
        .ok_or_else(|| io::Error::other(format!("fixture pointer does not exist: {pointer}")))?;
    *target = field;
    Ok(value)
}

fn reconcile_vectors(options: &Options, vectors: &[Vector]) -> io::Result<()> {
    if !options.check {
        fs::create_dir_all(&options.out_dir)?;
    }
    for vector in vectors {
        let document = json!({
            "description": vector.description,
            "expectation": vector.expectation,
            "operation": vector.operation,
            "payload": vector.payload,
            "schema_id": vector.schema_id
        });
        reconcile_file(
            &options.out_dir.join(vector.file),
            canonical_bytes(&document)?,
            options.check,
        )?;
    }
    Ok(())
}

fn manifest(options: &Options, vectors: &[Vector]) -> io::Result<Value> {
    let schemas = SCHEMAS
        .iter()
        .map(|(schema_id, schema_file)| {
            let schema_path = options.schema_dir.join(schema_file);
            let schema_bytes = fs::read(&schema_path)?;
            Ok(json!({
                "schema_digest": sha256_prefixed(&schema_bytes),
                "schema_file": schema_file,
                "schema_id": schema_id
            }))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let vector_entries = vectors
        .iter()
        .map(|vector| {
            let bytes = fs::read(options.out_dir.join(vector.file))?;
            Ok(json!({
                "expectation": vector.expectation,
                "file": vector.file,
                "operation": vector.operation,
                "payload_pointer": "/payload",
                "schema_id": vector.schema_id,
                "vector_digest": sha256_prefixed(&bytes)
            }))
        })
        .collect::<io::Result<Vec<_>>>()?;
    Ok(json!({
        "deny_unknown_fields": true,
        "schema": "runx.payment.purchase.fixtures.v1",
        "schemas": schemas,
        "vectors": vector_entries
    }))
}

fn canonical_bytes(value: &Value) -> io::Result<Vec<u8>> {
    reject_noncanonical_value(value, "$")?;
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn reject_noncanonical_value(value: &Value, path: &str) -> io::Result<()> {
    match value {
        Value::Number(number) if !number.is_i64() && !number.is_u64() => {
            Err(io::Error::other(format!("floating-point value at {path}")))
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                reject_noncanonical_value(value, &format!("{path}/{index}"))?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for (key, value) in values {
                if !key.is_ascii() {
                    return Err(io::Error::other(format!(
                        "non-ASCII object key at {path}/{key}"
                    )));
                }
                reject_noncanonical_value(value, &format!("{path}/{key}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
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
        fs::write(path, expected)?;
    }
    Ok(())
}

fn reject_orphan_vectors(options: &Options, vectors: &[Vector]) -> io::Result<()> {
    let expected = vectors
        .iter()
        .map(|vector| vector.file)
        .chain(std::iter::once("manifest.json"))
        .collect::<BTreeSet<_>>();
    for entry in fs::read_dir(&options.out_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| io::Error::other("fixture filename is not UTF-8"))?;
        if expected.contains(file_name) {
            continue;
        }
        if options.check {
            return Err(io::Error::other(format!(
                "orphan purchase fixture: {}",
                path.display()
            )));
        }
        fs::remove_file(path)?;
    }
    Ok(())
}
