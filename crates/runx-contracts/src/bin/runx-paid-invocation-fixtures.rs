use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use runx_contracts::{
    CANCEL_PAID_INVOCATION, CancelPaidInvocationRequest, CancelPaidInvocationResult,
    EXECUTE_PAID_INVOCATION, ExecutePaidInvocationRequest, ExecutePaidInvocationResult,
    GetPaidInvocationRequest, GetPaidInvocationResult, JsonObject, JsonValue as RunxJsonValue,
    MAX_PORTABLE_INTEGER, PAID_INVOCATION_REQUEST_FINGERPRINT_SCHEMA, PaidSkillListing,
    QUOTE_PAID_INVOCATION, QuotePaidInvocationRequest, QuotePaidInvocationResult,
    STABLE_JSON_CANONICALIZATION, canonical_stable_json,
    fingerprint_cancel_paid_invocation_request, fingerprint_execute_paid_invocation_request,
    fingerprint_quote_paid_invocation_request, quote_paid_invocation_binding, sha256_prefixed,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const SCHEMAS: &[(&str, &str)] = &[
    (
        "runx.marketplace.paid_skill_listing.v1",
        "paid-skill-listing.schema.json",
    ),
    (
        "runx.payment.paid_invocation.v1",
        "paid-invocation.schema.json",
    ),
    (
        "runx.payment.offer_revision_ref.v1",
        "offer-revision-ref.schema.json",
    ),
    (
        "runx.payment.parent_invocation_binding.v1",
        "parent-invocation-binding.schema.json",
    ),
    (
        "runx.payment.quote_paid_invocation.request.v1",
        "quote-paid-invocation-request.schema.json",
    ),
    (
        "runx.payment.quote_paid_invocation.result.v1",
        "quote-paid-invocation-result.schema.json",
    ),
    (
        "runx.payment.execute_paid_invocation.request.v1",
        "execute-paid-invocation-request.schema.json",
    ),
    (
        "runx.payment.execute_paid_invocation.result.v1",
        "execute-paid-invocation-result.schema.json",
    ),
    (
        "runx.payment.get_paid_invocation.request.v1",
        "get-paid-invocation-request.schema.json",
    ),
    (
        "runx.payment.get_paid_invocation.result.v1",
        "get-paid-invocation-result.schema.json",
    ),
    (
        "runx.payment.cancel_paid_invocation.request.v1",
        "cancel-paid-invocation-request.schema.json",
    ),
    (
        "runx.payment.cancel_paid_invocation.result.v1",
        "cancel-paid-invocation-result.schema.json",
    ),
];

struct Options {
    out_dir: PathBuf,
    schema_dir: PathBuf,
    packet_dir: PathBuf,
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
    authority_mapping: Option<Value>,
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
        canonical_bytes(&fingerprint_oracle()?)?,
        options.check,
    )?;
    reject_orphan_vectors(&options, &vectors)?;
    Ok(())
}

fn parse_args() -> io::Result<Options> {
    let mut out_dir = None;
    let mut schema_dir = None;
    let mut packet_dir = None;
    let mut oracle_out = None;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out_dir = args.next().map(PathBuf::from),
            "--schema-dir" => schema_dir = args.next().map(PathBuf::from),
            "--packet-dir" => packet_dir = args.next().map(PathBuf::from),
            "--oracle-out" => oracle_out = args.next().map(PathBuf::from),
            "--check" => check = true,
            other => return Err(io::Error::other(format!("unsupported argument: {other}"))),
        }
    }
    Ok(Options {
        out_dir: out_dir.ok_or_else(|| io::Error::other("--out is required"))?,
        schema_dir: schema_dir.ok_or_else(|| io::Error::other("--schema-dir is required"))?,
        packet_dir: packet_dir.ok_or_else(|| io::Error::other("--packet-dir is required"))?,
        oracle_out: oracle_out.ok_or_else(|| io::Error::other("--oracle-out is required"))?,
        check,
    })
}

fn vectors() -> io::Result<Vec<Vector>> {
    let parent = parent_binding();
    let direct = invocation("paid_direct", "settled", "queued", "open", None, true);
    let replay = invocation("paid_direct", "settled", "queued", "open", None, true);
    let outer_parent = quote_request("idem_parent", Some(parent.clone()));
    let inner_parent = invocation(
        "paid_child",
        "settled",
        "succeeded",
        "fulfilment_won",
        Some(parent),
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
        valid::<QuotePaidInvocationResult>(
            "quote-direct-admission.json",
            "A direct quote admits one commercial invocation and an opaque, digest-bound challenge.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.result.v1",
            json!({
                "status": "admitted",
                "value": {
                    "challenge": challenge(challenge_payload, challenge_digest),
                    "invocation": direct.clone()
                }
            }),
        )?,
        valid::<QuotePaidInvocationResult>(
            "quote-same-term-replay.json",
            "The same idempotency binding and terms resolve to the same commercial invocation.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.result.v1",
            json!({
                "status": "admitted",
                "value": {
                    "challenge": challenge(json!({"authorization": "opaque-provider-payload", "resource": "runx:offer:transcribe-v1"}), challenge_digest_for_default()?),
                    "invocation": replay
                }
            }),
        )?,
        valid::<QuotePaidInvocationRequest>(
            "quote-independent-purchase.json",
            "The same input digest with another idempotency binding remains a distinct purchase intent.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.request.v1",
            quote_request("idem_independent", None),
        )?,
        valid::<QuotePaidInvocationRequest>(
            "quote-outer-parent-binding.json",
            "A child quote binds its parent commercial invocation and execution digest.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.request.v1",
            outer_parent,
        )?,
        valid::<QuotePaidInvocationResult>(
            "quote-terms-changed.json",
            "A term revision is a typed domain refusal.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.result.v1",
            refusal("terms_changed", "The offer revision no longer matches."),
        )?,
        valid::<QuotePaidInvocationResult>(
            "quote-replay-conflict.json",
            "Reusing an idempotency key for different terms is a typed domain refusal.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.result.v1",
            refusal(
                "replay_conflict",
                "The idempotency binding commits to other terms.",
            ),
        )?,
        valid::<QuotePaidInvocationResult>(
            "quote-expired.json",
            "An expired quote remains a valid typed domain refusal.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.result.v1",
            refusal("quote_expired", "The quote has expired."),
        )?,
        valid_with_authority::<ExecutePaidInvocationRequest>(
            "execute-authority-values.json",
            "Execution carries values directly comparable with the generic payment AuthorityEffectLimit.",
            "ExecutePaidInvocation",
            "runx.payment.execute_paid_invocation.request.v1",
            execute_request(),
            json!({
                "effect_limit": {
                    "channels": ["hosted"],
                    "family": "payment",
                    "idempotency_required": true,
                    "max_per_call_units": 1250,
                    "operation": "ExecutePaidInvocation",
                    "peer": "https://vendor.example/offers/transcribe-v1",
                    "unit": "USD"
                },
                "idempotency_binding": idempotency("execute_direct")
            }),
        )?,
        valid::<ExecutePaidInvocationResult>(
            "execute-admitted.json",
            "Payment authorization admits execution without exposing a provider rail payload.",
            "ExecutePaidInvocation",
            "runx.payment.execute_paid_invocation.result.v1",
            admitted(direct),
        )?,
        valid::<GetPaidInvocationResult>(
            "get-inner-parent-fulfilment-won.json",
            "The child record carries its parent binding and the fulfilment winner.",
            "GetPaidInvocation",
            "runx.payment.get_paid_invocation.result.v1",
            admitted(inner_parent),
        )?,
        valid::<GetPaidInvocationResult>(
            "get-refund-won.json",
            "The refund winner is orthogonal to terminal execution observation.",
            "GetPaidInvocation",
            "runx.payment.get_paid_invocation.result.v1",
            admitted(invocation(
                "paid_refund",
                "refunded",
                "failed",
                "refund_won",
                None,
                true,
            )),
        )?,
        valid::<GetPaidInvocationResult>(
            "get-run-reference.json",
            "A completed invocation exposes its hosted run without embedding run-domain state.",
            "GetPaidInvocation",
            "runx.payment.get_paid_invocation.result.v1",
            json!({
                "status": "admitted",
                "value": {
                    "invocation": invocation(
                        "paid_run",
                        "settled",
                        "succeeded",
                        "fulfilment_won",
                        None,
                        true,
                    ),
                    "receipt_ref": reference("receipt", "runx:receipt:paid-run-1"),
                    "run_ref": reference("act", "runx:run:hosted-1")
                }
            }),
        )?,
        valid::<GetPaidInvocationRequest>(
            "get-request.json",
            "Lookup names only the commercial invocation.",
            "GetPaidInvocation",
            "runx.payment.get_paid_invocation.request.v1",
            json!({"invocation_id": "paid_direct"}),
        )?,
        valid::<CancelPaidInvocationRequest>(
            "cancel-request.json",
            "Cancellation is idempotently bound to the commercial invocation.",
            "CancelPaidInvocation",
            "runx.payment.cancel_paid_invocation.request.v1",
            json!({"idempotency": idempotency("cancel_direct"), "invocation_id": "paid_direct"}),
        )?,
        valid::<CancelPaidInvocationResult>(
            "cancel-before-settlement.json",
            "Cancellation before settlement leaves the payment unpaid.",
            "CancelPaidInvocation",
            "runx.payment.cancel_paid_invocation.result.v1",
            admitted(invocation(
                "paid_cancel_early",
                "unpaid",
                "cancelled",
                "open",
                None,
                false,
            )),
        )?,
        valid::<CancelPaidInvocationResult>(
            "cancel-after-settlement.json",
            "Cancellation after settlement does not imply a refund transition.",
            "CancelPaidInvocation",
            "runx.payment.cancel_paid_invocation.result.v1",
            admitted(invocation(
                "paid_cancel_late",
                "settled",
                "cancelled",
                "open",
                None,
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
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.request.v1",
            with_field(
                quote_request("idem_unknown", None),
                "unexpected",
                json!(true),
            )?,
        ),
        invalid(
            "invalid-malformed-digest.json",
            "Digest values are lowercase prefixed SHA-256 only.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.request.v1",
            with_field(
                quote_request("idem_digest", None),
                "input_digest",
                json!("sha256:no"),
            )?,
        ),
        invalid(
            "invalid-amount-above-portable.json",
            "Minor-unit amounts above the shared portable integer bound are rejected.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.request.v1",
            with_field(
                quote_request("idem_amount", None),
                "amount_minor",
                json!(MAX_PORTABLE_INTEGER + 1),
            )?,
        ),
        invalid(
            "invalid-non-principal.json",
            "The principal reference cannot name another reference type.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.request.v1",
            with_field(
                quote_request("idem_principal", None),
                "principal",
                reference("artifact", "runx:artifact:not-a-principal"),
            )?,
        ),
        invalid(
            "invalid-expiry.json",
            "Quote expiry must use the shared UTC ISO datetime shape.",
            "QuotePaidInvocation",
            "runx.payment.quote_paid_invocation.result.v1",
            with_pointer_value(
                admitted(invocation(
                    "paid_expiry",
                    "unpaid",
                    "unstarted",
                    "open",
                    None,
                    false,
                )),
                "/value/invocation/expires_at",
                json!("tomorrow"),
            )?,
        ),
        invalid(
            "invalid-response-status-mismatch.json",
            "An admitted status cannot carry refusal fields.",
            "ExecutePaidInvocation",
            "runx.payment.execute_paid_invocation.result.v1",
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
        authority_mapping: None,
    })
}

fn valid_with_authority<T: DeserializeOwned + Serialize>(
    file: &'static str,
    description: &'static str,
    operation: &'static str,
    schema_id: &'static str,
    payload: Value,
    authority_mapping: Value,
) -> io::Result<Vector> {
    let mut vector = valid::<T>(file, description, operation, schema_id, payload)?;
    vector.authority_mapping = Some(authority_mapping);
    Ok(vector)
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
        authority_mapping: None,
    }
}

fn quote_request(idempotency_key: &str, parent: Option<Value>) -> Value {
    let mut value = json!({
        "accepted_settlement_families": ["mock", "hosted"],
        "amount_minor": 1250,
        "canonicalizer_version": "runx.receipt.c14n.v1",
        "counterparty": reference("external_url", "https://vendor.example/offers/transcribe-v1"),
        "currency": "USD",
        "idempotency": idempotency(idempotency_key),
        "input_digest": digest('1'),
        "offer_revision": offer_revision(),
        "package_digest": digest('7'),
        "principal": reference("principal", "runx:principal:buyer-1"),
        "vendor_ref": reference("principal", "runx:principal:vendor-1")
    });
    if let Some(parent) = parent
        && let Some(object) = value.as_object_mut()
    {
        object.insert("parent".to_owned(), parent);
    }
    value
}

fn paid_skill_listing() -> Value {
    json!({
        "skill_id": "acme/transcribe",
        "version": "1.0.0",
        "skill_digest": digest('8'),
        "profile_digest": digest('9'),
        "package_digest": digest('a'),
        "vendor_ref": reference("principal", "runx:principal:vendor-1"),
        "offers": {"transcribe": {
            "offer_revision": {
                "offer_id": "acme/transcribe#transcribe",
                "revision": "1.0.0",
                "revision_digest": digest('9'),
                "input_schema_digest": digest('2'),
                "output_schema_digest": digest('3')
            },
            "amount_minor": 1250,
            "currency": "USD",
            "accepted_settlement_families": ["x402", "stripe-spt"]
        }}
    })
}

fn execute_request() -> Value {
    json!({
        "idempotency": idempotency("execute_direct"),
        "invocation_id": "paid_direct",
        "payment_ref": reference("receipt", "runx:receipt:payment-proof-1"),
        "settlement_family": "hosted"
    })
}

fn invocation(
    invocation_id: &str,
    payment_state: &str,
    execution_state: &str,
    outcome_gate: &str,
    parent: Option<Value>,
    settled: bool,
) -> Value {
    let mut value = json!({
        "accepted_settlement_families": ["mock", "hosted"],
        "amount_minor": 1250,
        "canonicalizer_version": "runx.receipt.c14n.v1",
        "counterparty": reference("external_url", "https://vendor.example/offers/transcribe-v1"),
        "created_at": "2026-08-22T09:00:00Z",
        "currency": "USD",
        "execution_state": execution_state,
        "expires_at": "2026-08-22T09:05:00Z",
        "idempotency": idempotency("quote_direct"),
        "input_digest": digest('1'),
        "invocation_id": invocation_id,
        "offer_revision": offer_revision(),
        "outcome_gate": outcome_gate,
        "package_digest": digest('7'),
        "payment_state": payment_state,
        "principal": reference("principal", "runx:principal:buyer-1"),
        "vendor_ref": reference("principal", "runx:principal:vendor-1"),
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
            "execution_ref".to_owned(),
            reference("act", &format!("runx:act:{invocation_id}")),
        );
    }
    if let Some(parent) = parent
        && let Some(object) = value.as_object_mut()
    {
        object.insert("parent".to_owned(), parent);
    }
    value
}

fn offer_revision() -> Value {
    json!({
        "input_schema_digest": digest('2'),
        "offer_id": "transcribe-v1",
        "output_schema_digest": digest('3'),
        "revision": "2026-08-22.1",
        "revision_digest": digest('4')
    })
}

fn parent_binding() -> Value {
    json!({
        "execution_digest": digest('5'),
        "invocation_id": "paid_parent"
    })
}

fn idempotency(key: &str) -> Value {
    json!({"binding_digest": digest('6'), "key": key})
}

fn presentation() -> Value {
    json!({
        "service_name": "Vendor Transcribe",
        "description": "Transcribe one committed audio artifact.",
        "media_type": "application/json",
        "tags": ["audio", "transcription"],
        "input_example": "{\"artifact\":\"runx:artifact:sha256:1111111111111111111111111111111111111111111111111111111111111111\"}",
        "output_example": "{\"text\":\"hello\"}",
        "input_schema": "{\"properties\":{\"artifact\":{\"type\":\"string\"}},\"type\":\"object\"}",
        "output_schema": "{\"properties\":{\"text\":{\"type\":\"string\"}},\"type\":\"object\"}"
    })
}

fn attribution() -> Value {
    json!({
        "source": "frantic",
        "campaign": "bounty-135"
    })
}

fn fingerprint_oracle() -> io::Result<Value> {
    let quote = quote_request("fingerprint_quote", None);
    let quote_cases = [
        ("quote-base", quote.clone()),
        (
            "quote-principal",
            with_pointer_value(
                quote.clone(),
                "/principal/uri",
                json!("runx:principal:buyer-2"),
            )?,
        ),
        (
            "quote-vendor-ref",
            with_pointer_value(
                quote.clone(),
                "/vendor_ref/uri",
                json!("runx:principal:vendor-2"),
            )?,
        ),
        (
            "quote-counterparty",
            with_pointer_value(
                quote.clone(),
                "/counterparty/uri",
                json!("https://vendor.example/offers/transcribe-v2"),
            )?,
        ),
        (
            "quote-offer-revision",
            with_pointer_value(
                quote.clone(),
                "/offer_revision/revision",
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
            with_pointer_value(
                quote.clone(),
                "/idempotency/binding_digest",
                json!(digest('a')),
            )?,
        ),
        (
            "quote-parent",
            with_field(quote.clone(), "parent", parent_binding())?,
        ),
        // Presentation and acquisition attribution never enter quote identity:
        // these cases must digest exactly as quote-base does.
        (
            "quote-attribution",
            with_field(quote.clone(), "attribution", attribution())?,
        ),
        (
            "quote-presentation",
            with_field(quote, "presentation", presentation())?,
        ),
    ];

    let execute = execute_request();
    let execute_cases = [
        ("execute-base", execute.clone()),
        (
            "execute-invocation",
            with_pointer_value(execute.clone(), "/invocation_id", json!("paid_other"))?,
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
        "idempotency": idempotency("fingerprint_cancel"),
        "invocation_id": "paid_direct"
    });
    let cancel_cases = [
        ("cancel-base", cancel.clone()),
        (
            "cancel-invocation",
            with_pointer_value(cancel.clone(), "/invocation_id", json!("paid_other"))?,
        ),
        (
            "cancel-idempotency",
            with_pointer_value(cancel, "/idempotency/binding_digest", json!(digest('a')))?,
        ),
    ];

    let mut cases = Vec::new();
    for (name, request) in quote_cases {
        cases.push(fingerprint_case(name, QUOTE_PAID_INVOCATION, request)?);
    }
    for (name, request) in execute_cases {
        cases.push(fingerprint_case(name, EXECUTE_PAID_INVOCATION, request)?);
    }
    for (name, request) in cancel_cases {
        cases.push(fingerprint_case(name, CANCEL_PAID_INVOCATION, request)?);
    }

    Ok(json!({
        "canonicalization": STABLE_JSON_CANONICALIZATION,
        "cases": cases,
        "schema": "runx.canonical_json_oracle.v1"
    }))
}

fn fingerprint_case(name: &str, operation: &str, request: Value) -> io::Result<Value> {
    let expected_sha256 = match operation {
        QUOTE_PAID_INVOCATION => fingerprint_quote_paid_invocation_request(
            &serde_json::from_value::<QuotePaidInvocationRequest>(request.clone())
                .map_err(io::Error::other)?,
        ),
        EXECUTE_PAID_INVOCATION => fingerprint_execute_paid_invocation_request(
            &serde_json::from_value::<ExecutePaidInvocationRequest>(request.clone())
                .map_err(io::Error::other)?,
        ),
        CANCEL_PAID_INVOCATION => fingerprint_cancel_paid_invocation_request(
            &serde_json::from_value::<CancelPaidInvocationRequest>(request.clone())
                .map_err(io::Error::other)?,
        ),
        _ => {
            return Err(io::Error::other(format!(
                "unsupported operation: {operation}"
            )));
        }
    }
    .map_err(io::Error::other)?;

    // The oracle preimage carries what the operation binds; a quote binds its
    // request without the vendor presentation.
    let bound_request = match operation {
        QUOTE_PAID_INVOCATION => serde_json::to_value(quote_paid_invocation_binding(
            &serde_json::from_value::<QuotePaidInvocationRequest>(request.clone())
                .map_err(io::Error::other)?,
        ))
        .map_err(io::Error::other)?,
        _ => request.clone(),
    };
    let request_value =
        serde_json::from_value::<RunxJsonValue>(bound_request).map_err(io::Error::other)?;
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
            RunxJsonValue::String(PAID_INVOCATION_REQUEST_FINGERPRINT_SCHEMA.to_owned()),
        ),
    ]));
    let canonical_json = canonical_stable_json(&preimage).map_err(io::Error::other)?;
    if sha256_prefixed(canonical_json.as_bytes()) != expected_sha256 {
        return Err(io::Error::other(format!(
            "fingerprint helper and oracle preimage disagree for {name}"
        )));
    }

    Ok(json!({
        "canonical_json": canonical_json,
        "expected_sha256": expected_sha256,
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
        "media_type": "application/vnd.vendor.payment+json",
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

fn admitted(invocation: Value) -> Value {
    json!({"status": "admitted", "value": {"invocation": invocation}})
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
        let mut document = json!({
            "description": vector.description,
            "expectation": vector.expectation,
            "operation": vector.operation,
            "payload": vector.payload,
            "schema_id": vector.schema_id
        });
        if let Some(authority_mapping) = &vector.authority_mapping {
            document
                .as_object_mut()
                .ok_or_else(|| io::Error::other("fixture envelope must be an object"))?
                .insert("authority_mapping".to_owned(), authority_mapping.clone());
        }
        reconcile_file(
            &options.out_dir.join(vector.file),
            canonical_bytes(&document)?,
            options.check,
        )?;
    }
    Ok(())
}

fn manifest(options: &Options, vectors: &[Vector]) -> io::Result<Value> {
    let packets = packets_by_id(&options.packet_dir)?;
    let schemas = SCHEMAS
        .iter()
        .map(|(schema_id, schema_file)| {
            let schema_path = options.schema_dir.join(schema_file);
            let schema_bytes = fs::read(&schema_path)?;
            let packet_path = packets.get(*schema_id).ok_or_else(|| {
                io::Error::other(format!("missing packet projection for {schema_id}"))
            })?;
            let packet_bytes = fs::read(packet_path)?;
            Ok(json!({
                "packet_digest": sha256_prefixed(&packet_bytes),
                "packet_file": packet_path.file_name().and_then(|name| name.to_str()).ok_or_else(|| io::Error::other("packet filename is not UTF-8"))?,
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
        "schema": "runx.payment.paid_invocation.fixtures.v1",
        "schemas": schemas,
        "vectors": vector_entries
    }))
}

fn packets_by_id(packet_dir: &Path) -> io::Result<BTreeMap<String, PathBuf>> {
    let mut packets = BTreeMap::new();
    for entry in fs::read_dir(packet_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let document: Value =
            serde_json::from_slice(&fs::read(&path)?).map_err(io::Error::other)?;
        let Some(packet_id) = document.get("x-runx-packet-id").and_then(Value::as_str) else {
            continue;
        };
        if packets.insert(packet_id.to_owned(), path).is_some() {
            return Err(io::Error::other(format!(
                "multiple packet projections declare {packet_id}"
            )));
        }
    }
    Ok(packets)
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
                "orphan paid-invocation fixture: {}",
                path.display()
            )));
        }
        fs::remove_file(path)?;
    }
    Ok(())
}
