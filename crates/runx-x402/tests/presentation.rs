use std::error::Error;
use std::io;

use runx_contracts::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use runx_contracts::{
    JsonObject, OfferRevisionRef, PaidInvocationCanonicalizerVersion, PaymentIdempotencyBinding,
    RUNX_INVOCATION_EXTENSION_KEY, Reference, ReferenceType, RunxX402InvocationExtensionInfo,
    Sha256Digest, X402AcceptedRequirements, X402Network, X402PaymentPayload,
    X402PaymentRequirements, X402PositiveNumber, X402ResourceInfo, X402Version2,
};
use runx_x402::{
    X402PresentationError, assemble_payment_required, bind_payment_required_challenge,
    decode_payment_required_header, decode_payment_response_header,
    decode_payment_signature_header, encode_payment_required_header,
    encode_payment_response_header, encode_payment_signature_header,
    payment_required_from_challenge, runx_invocation_schema_reference, validate_payment_retry,
};
use serde_json::json;

const OFFICIAL_PAYMENT_REQUIRED_HEADER: &str = "eyJ4NDAyVmVyc2lvbiI6MiwiZXJyb3IiOiJQQVlNRU5ULVNJR05BVFVSRSBoZWFkZXIgaXMgcmVxdWlyZWQiLCJyZXNvdXJjZSI6eyJ1cmwiOiJodHRwczovL2FwaS5leGFtcGxlLmNvbS9wcmVtaXVtLWRhdGEiLCJkZXNjcmlwdGlvbiI6IkFjY2VzcyB0byBwcmVtaXVtIG1hcmtldCBkYXRhIiwibWltZVR5cGUiOiJhcHBsaWNhdGlvbi9qc29uIn0sImFjY2VwdHMiOlt7InNjaGVtZSI6ImV4YWN0IiwibmV0d29yayI6ImVpcDE1NTo4NDUzMiIsImFtb3VudCI6IjEwMDAwIiwiYXNzZXQiOiIweDAzNkNiRDUzODQyYzU0MjY2MzRlNzkyOTU0MWVDMjMxOGYzZENGN2UiLCJwYXlUbyI6IjB4MjA5NjkzQmM2YWZjMEM1MzI4YkEzNkZhRjAzQzUxNEVGMzEyMjg3QyIsIm1heFRpbWVvdXRTZWNvbmRzIjo2MCwiZXh0cmEiOnsibmFtZSI6IlVTREMiLCJ2ZXJzaW9uIjoiMiJ9fV19";
const OFFICIAL_PAYMENT_SIGNATURE_HEADER: &str = "eyJ4NDAyVmVyc2lvbiI6MiwicmVzb3VyY2UiOnsidXJsIjoiaHR0cHM6Ly9hcGkuZXhhbXBsZS5jb20vcHJlbWl1bS1kYXRhIiwiZGVzY3JpcHRpb24iOiJBY2Nlc3MgdG8gcHJlbWl1bSBtYXJrZXQgZGF0YSIsIm1pbWVUeXBlIjoiYXBwbGljYXRpb24vanNvbiJ9LCJhY2NlcHRlZCI6eyJzY2hlbWUiOiJleGFjdCIsIm5ldHdvcmsiOiJlaXAxNTU6ODQ1MzIiLCJhbW91bnQiOiIxMDAwMCIsImFzc2V0IjoiMHgwMzZDYkQ1Mzg0MmM1NDI2NjM0ZTc5Mjk1NDFlQzIzMThmM2RDRjdlIiwicGF5VG8iOiIweDIwOTY5M0JjNmFmYzBDNTMyOGJBMzZGYUYwM0M1MTRFRjMxMjI4N0MiLCJtYXhUaW1lb3V0U2Vjb25kcyI6NjAsImV4dHJhIjp7Im5hbWUiOiJVU0RDIiwidmVyc2lvbiI6IjIifX0sInBheWxvYWQiOnsic2lnbmF0dXJlIjoiMHgyZDZhNzU4OGQ2YWNjYTUwNWNiZjBkOWE0YTIyN2UwYzUyYzZjMzQwMDhjOGU4OTg2YTEyODMyNTk3NjQxNzM2MDhhMmNlNjQ5NjY0MmUzNzdkNmRhOGRiYmY1ODM2ZTliZDE1MDkyZjllY2FiMDVkZWQzZDYyOTNhZjE0OGI1NzFjIiwiYXV0aG9yaXphdGlvbiI6eyJmcm9tIjoiMHg4NTdiMDY1MTlFOTFlM0E1NDUzODc5MWJEYmIwRTIyMzczZTM2YjY2IiwidG8iOiIweDIwOTY5M0JjNmFmYzBDNTMyOGJBMzZGYUYwM0M1MTRFRjMxMjI4N0MiLCJ2YWx1ZSI6IjEwMDAwIiwidmFsaWRBZnRlciI6IjE3NDA2NzIwODkiLCJ2YWxpZEJlZm9yZSI6IjE3NDA2NzIxNTQiLCJub25jZSI6IjB4ZjM3NDY2MTNjMmQ5MjBiNWZkYWJjMDg1NmYyYWViMmQ0Zjg4ZWU2MDM3YjhjYzVkMDRhNzFhNDQ2MmYxMzQ4MCJ9fX0=";
const OFFICIAL_PAYMENT_RESPONSE_HEADER: &str = "eyJzdWNjZXNzIjp0cnVlLCJ0cmFuc2FjdGlvbiI6IjB4MTIzNDU2Nzg5MGFiY2RlZjEyMzQ1Njc4OTBhYmNkZWYxMjM0NTY3ODkwYWJjZGVmMTIzNDU2Nzg5MGFiY2RlZiIsIm5ldHdvcmsiOiJlaXAxNTU6ODQ1MzIiLCJwYXllciI6IjB4ODU3YjA2NTE5RTkxZTNBNTQ1Mzg3OTFiRGJiMEUyMjM3M2UzNmI2NiJ9";

#[test]
fn official_http_examples_round_trip_byte_for_byte() -> Result<(), Box<dyn Error>> {
    let required = decode_payment_required_header(OFFICIAL_PAYMENT_REQUIRED_HEADER)?;
    assert_eq!(
        encode_payment_required_header(&required)?,
        OFFICIAL_PAYMENT_REQUIRED_HEADER
    );

    let signature = decode_payment_signature_header(OFFICIAL_PAYMENT_SIGNATURE_HEADER)?;
    let canonical_signature = encode_payment_signature_header(&signature)?;
    assert_eq!(
        decode_payment_signature_header(&canonical_signature)?,
        signature
    );

    let response = decode_payment_response_header(OFFICIAL_PAYMENT_RESPONSE_HEADER)?;
    assert_eq!(
        encode_payment_response_header(&response)?,
        OFFICIAL_PAYMENT_RESPONSE_HEADER
    );
    Ok(())
}

#[test]
fn assembler_owns_runx_extension_and_retry_requires_exact_commitments() -> Result<(), Box<dyn Error>>
{
    let expected_resource = resource();
    let requirement = requirement()?;
    let invocation = invocation('4', '7', '1')?;
    let challenge = assemble_payment_required(
        expected_resource.clone(),
        accepts(requirement.clone())?,
        invocation.clone(),
        None,
        JsonObject::new(),
    )?;
    let retry = X402PaymentPayload {
        x402_version: X402Version2,
        resource: Some(expected_resource),
        accepted: requirement,
        payload: json_object(json!({ "signature": "opaque" }))?,
        extensions: challenge.extensions.clone(),
        additional: JsonObject::new(),
    };

    let rail_challenge = bind_payment_required_challenge(
        &challenge,
        Reference::runx(ReferenceType::Receipt, "quote-1"),
        IsoDateTime::from("2026-08-22T10:00:00Z"),
    )?;
    let recovered = payment_required_from_challenge(&rail_challenge)?;
    let validated = validate_payment_retry(&recovered, &retry)?;
    assert_eq!(validated.requirement_index, 0);
    assert_eq!(validated.invocation, invocation);

    let mut changed_requirement = retry.clone();
    changed_requirement.accepted.amount = NonEmptyString::from("10001");
    assert_eq!(
        validate_payment_retry(&challenge, &changed_requirement),
        Err(X402PresentationError::RequirementMismatch)
    );

    let mut changed_resource = retry.clone();
    changed_resource.resource = Some(X402ResourceInfo {
        url: NonEmptyString::from("https://api.example.com/v1/other"),
        ..resource()
    });
    assert_eq!(
        validate_payment_retry(&challenge, &changed_resource),
        Err(X402PresentationError::ResourceMismatch)
    );

    let mut missing_resource = retry.clone();
    missing_resource.resource = None;
    assert_eq!(
        validate_payment_retry(&challenge, &missing_resource),
        Err(X402PresentationError::ResourceMismatch)
    );

    let mut missing_extension = retry.clone();
    missing_extension.extensions = None;
    assert_eq!(
        validate_payment_retry(&challenge, &missing_extension),
        Err(X402PresentationError::MissingRunxInvocation)
    );

    let mut swapped_schema = retry.clone();
    let mut swapped_extensions = swapped_schema
        .extensions
        .take()
        .ok_or_else(|| io::Error::other("challenge extensions"))?;
    swapped_extensions.insert(
        RUNX_INVOCATION_EXTENSION_KEY.to_owned(),
        json_value(json!({ "info": invocation, "schema": {} }))?,
    );
    swapped_schema.extensions = Some(swapped_extensions);
    assert_eq!(
        validate_payment_retry(&challenge, &swapped_schema),
        Err(X402PresentationError::RunxInvocationSchemaMismatch)
    );

    let changed_challenge = assemble_payment_required(
        retry
            .resource
            .clone()
            .ok_or_else(|| io::Error::other("resource"))?,
        accepts(retry.accepted.clone())?,
        discovery('5', '7')?,
        None,
        JsonObject::new(),
    )?;
    let mut changed_extension = retry;
    changed_extension.extensions = changed_challenge.extensions;
    assert_eq!(
        validate_payment_retry(&challenge, &changed_extension),
        Err(X402PresentationError::RunxInvocationMismatch)
    );
    Ok(())
}

#[test]
fn declaration_advertises_the_published_schema_by_reference() -> Result<(), Box<dyn Error>> {
    let invocation = invocation('4', '7', '1')?;
    let challenge = assemble_payment_required(
        resource(),
        accepts(requirement()?)?,
        invocation.clone(),
        None,
        JsonObject::new(),
    )?;
    let declared = challenge
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get(RUNX_INVOCATION_EXTENSION_KEY))
        .cloned()
        .ok_or_else(|| io::Error::other("runx.invocation"))?;
    let published_id = RunxX402InvocationExtensionInfo::json_schema()["$id"].clone();
    assert_eq!(
        declared,
        json_value(json!({ "info": invocation, "schema": { "$ref": published_id } }))?
    );
    assert_eq!(
        runx_invocation_schema_reference()?,
        json_object(
            json!({ "$ref": "https://schemas.runx.ai/runx/x402/invocation-extension/v1.json" })
        )?
    );

    // A challenge assembled before the reference form carried the whole
    // document; an echo of it names the same schema and still validates.
    let mut inline_extensions = JsonObject::new();
    inline_extensions.insert(
        RUNX_INVOCATION_EXTENSION_KEY.to_owned(),
        json_value(json!({
            "info": invocation,
            "schema": RunxX402InvocationExtensionInfo::json_schema(),
        }))?,
    );
    let inline_retry = X402PaymentPayload {
        x402_version: X402Version2,
        resource: Some(resource()),
        accepted: requirement()?,
        payload: json_object(json!({ "signature": "opaque" }))?,
        extensions: Some(inline_extensions),
        additional: JsonObject::new(),
    };
    let validated = validate_payment_retry(&challenge, &inline_retry)?;
    assert_eq!(validated.invocation, invocation);

    let mut foreign_reference = JsonObject::new();
    foreign_reference.insert(
        RUNX_INVOCATION_EXTENSION_KEY.to_owned(),
        json_value(json!({
            "info": invocation,
            "schema": { "$ref": "https://schemas.runx.ai/runx/x402/invocation-extension/v2.json" },
        }))?,
    );
    let foreign_retry = X402PaymentPayload {
        extensions: Some(foreign_reference),
        ..inline_retry
    };
    assert_eq!(
        validate_payment_retry(&challenge, &foreign_retry),
        Err(X402PresentationError::RunxInvocationSchemaMismatch)
    );
    Ok(())
}

#[test]
fn assembler_preserves_opaque_vendor_extensions_and_reserves_runx_key() -> Result<(), Box<dyn Error>>
{
    let opaque = json_value(json!(["vendor", { "future": true }]))?;
    let mut vendor_extensions = JsonObject::new();
    vendor_extensions.insert("vendor.example".to_owned(), opaque.clone());
    let challenge = assemble_payment_required(
        resource(),
        accepts(requirement()?)?,
        discovery('4', '7')?,
        None,
        vendor_extensions,
    )?;
    assert_eq!(
        challenge
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("vendor.example")),
        Some(&opaque)
    );

    let mut reserved = JsonObject::new();
    reserved.insert(
        RUNX_INVOCATION_EXTENSION_KEY.to_owned(),
        json_value(json!({ "attacker": true }))?,
    );
    assert_eq!(
        assemble_payment_required(
            resource(),
            accepts(requirement()?)?,
            discovery('4', '7')?,
            None,
            reserved,
        ),
        Err(X402PresentationError::ReservedExtension)
    );
    Ok(())
}

#[test]
fn rail_neutral_challenge_detects_payload_tampering() -> Result<(), Box<dyn Error>> {
    let payment_required = assemble_payment_required(
        resource(),
        accepts(requirement()?)?,
        discovery('4', '7')?,
        None,
        JsonObject::new(),
    )?;
    let challenge = bind_payment_required_challenge(
        &payment_required,
        Reference::runx(ReferenceType::Receipt, "quote-1"),
        IsoDateTime::from("2026-08-22T10:00:00Z"),
    )?;
    assert_eq!(
        payment_required_from_challenge(&challenge)?,
        payment_required
    );

    let mut wrong_kind = challenge.clone();
    wrong_kind.protocol_version = NonEmptyString::from("3");
    assert_eq!(
        payment_required_from_challenge(&wrong_kind),
        Err(X402PresentationError::ChallengeKindMismatch)
    );

    let mut tampered = challenge;
    tampered.payload = json_value(json!({ "x402Version": 2 }))?;
    assert_eq!(
        payment_required_from_challenge(&tampered),
        Err(X402PresentationError::ChallengeDigestMismatch)
    );
    Ok(())
}

#[test]
fn external_reader_preserves_unknown_fields_and_errors_are_redacted() -> Result<(), Box<dyn Error>>
{
    let raw = json!({
        "x402Version": 2,
        "resource": { "url": "https://example.com", "futureResource": { "a": 1 } },
        "accepted": {
            "scheme": "exact",
            "network": "eip155:84532",
            "amount": "1",
            "asset": "USDC",
            "payTo": "merchant",
            "maxTimeoutSeconds": 0.5,
            "futureRequirement": true
        },
        "payload": { "secret": "do-not-echo" },
        "futurePayload": [1, 2, 3]
    });
    let value: X402PaymentPayload = serde_json::from_value(raw.clone())?;
    assert_eq!(serde_json::to_value(value)?, raw);

    let error = decode_payment_signature_header("not a signature!")
        .err()
        .ok_or_else(|| io::Error::other("expected decode failure"))?;
    assert_eq!(error.to_string(), "x402 header is not standard base64");
    assert!(!format!("{error:?}").contains("signature"));
    let unpadded = OFFICIAL_PAYMENT_REQUIRED_HEADER.trim_end_matches('=');
    assert_eq!(
        decode_payment_required_header(unpadded)?,
        decode_payment_required_header(OFFICIAL_PAYMENT_REQUIRED_HEADER)?
    );
    Ok(())
}

fn resource() -> X402ResourceInfo {
    X402ResourceInfo {
        url: NonEmptyString::from("https://api.example.com/v1/ocr"),
        description: Some("OCR one document".to_owned()),
        mime_type: Some("application/json".to_owned()),
        service_name: None,
        tags: None,
        icon_url: None,
        additional: JsonObject::new(),
    }
}

fn requirement() -> Result<X402PaymentRequirements, Box<dyn Error>> {
    Ok(X402PaymentRequirements {
        scheme: NonEmptyString::from("exact"),
        network: X402Network::new("eip155:84532")
            .ok_or_else(|| io::Error::other("valid network fixture"))?,
        amount: NonEmptyString::from("10000"),
        asset: NonEmptyString::from("USDC"),
        pay_to: NonEmptyString::from("merchant"),
        max_timeout_seconds: X402PositiveNumber::new(60.0)
            .ok_or_else(|| io::Error::other("valid timeout fixture"))?,
        extra: None,
        additional: JsonObject::new(),
    })
}

fn accepts(
    requirement: X402PaymentRequirements,
) -> Result<X402AcceptedRequirements, Box<dyn Error>> {
    X402AcceptedRequirements::new(vec![requirement])
        .ok_or_else(|| io::Error::other("non-empty requirements").into())
}

fn discovery(
    revision: char,
    package: char,
) -> Result<RunxX402InvocationExtensionInfo, Box<dyn Error>> {
    Ok(RunxX402InvocationExtensionInfo::Discovery {
        offer_revision: OfferRevisionRef {
            offer_id: NonEmptyString::from("ocr-v1"),
            revision: NonEmptyString::from("2026-08-22.1"),
            revision_digest: digest(revision)?,
            input_schema_digest: digest('2')?,
            output_schema_digest: digest('3')?,
        },
        package_digest: digest(package)?,
    })
}

fn invocation(
    revision: char,
    package: char,
    input: char,
) -> Result<RunxX402InvocationExtensionInfo, Box<dyn Error>> {
    Ok(RunxX402InvocationExtensionInfo::Invocation {
        invocation_id: NonEmptyString::from("paid_ocr_1"),
        quote_ref: Box::new(Reference::runx(ReferenceType::Receipt, "quote-1")),
        offer_revision: OfferRevisionRef {
            offer_id: NonEmptyString::from("ocr-v1"),
            revision: NonEmptyString::from("2026-08-22.1"),
            revision_digest: digest(revision)?,
            input_schema_digest: digest('2')?,
            output_schema_digest: digest('3')?,
        },
        package_digest: digest(package)?,
        input_digest: digest(input)?,
        canonicalizer_version: PaidInvocationCanonicalizerVersion::ReceiptC14nV1,
        idempotency: PaymentIdempotencyBinding {
            key: NonEmptyString::from("ocr-1"),
            binding_digest: digest('6')?,
        },
        parent: None,
    })
}

fn digest(character: char) -> Result<Sha256Digest, Box<dyn Error>> {
    Sha256Digest::new(format!("sha256:{}", character.to_string().repeat(64)))
        .ok_or_else(|| io::Error::other("valid digest fixture").into())
}

fn json_object(value: serde_json::Value) -> Result<JsonObject, Box<dyn Error>> {
    Ok(serde_json::from_value(value)?)
}

fn json_value(value: serde_json::Value) -> Result<runx_contracts::JsonValue, Box<dyn Error>> {
    Ok(serde_json::from_value(value)?)
}
