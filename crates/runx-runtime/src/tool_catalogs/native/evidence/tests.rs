use runx_contracts::{JsonNumber, JsonObject, JsonValue};

use super::{EvidenceIndexInput, EvidenceVerifyInput, index, verify};
use crate::tool_catalogs::native::fixture_input;

const SOURCE_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A `web/page.read` step output carrying these source fields.
fn web_read(result: JsonObject) -> JsonValue {
    JsonValue::Object(JsonObject::from([(
        "provider_operation".to_owned(),
        JsonValue::Object(JsonObject::from([(
            "data".to_owned(),
            JsonValue::Object(JsonObject::from([
                ("status".to_owned(), JsonValue::String("success".to_owned())),
                ("provider".to_owned(), JsonValue::String("web".to_owned())),
                (
                    "operation".to_owned(),
                    JsonValue::String("page.read".to_owned()),
                ),
                ("result".to_owned(), JsonValue::Object(result)),
            ])),
        )])),
    )]))
}

#[test]
fn indexes_a_governed_fetch_packet() {
    let inputs = fixture_input::<EvidenceIndexInput>(JsonObject::from([
        (
            "objective".to_owned(),
            JsonValue::String("Compare the supplied evidence".to_owned()),
        ),
        (
            "source_packets".to_owned(),
            JsonValue::Array(vec![web_read(JsonObject::from([
                ("status".to_owned(), JsonValue::Number(JsonNumber::U64(200))),
                (
                    "final_url".to_owned(),
                    JsonValue::String("https://example.com/report".to_owned()),
                ),
                (
                    "content_digest".to_owned(),
                    JsonValue::String(SOURCE_DIGEST.to_owned()),
                ),
                (
                    "extracted".to_owned(),
                    JsonValue::String("Verified report text".to_owned()),
                ),
                (
                    "provenance".to_owned(),
                    JsonValue::Object(JsonObject::from([
                        (
                            "fetched_at".to_owned(),
                            JsonValue::String("2026-07-18T00:00:00Z".to_owned()),
                        ),
                        ("bytes".to_owned(), JsonValue::Number(JsonNumber::U64(20))),
                        ("truncated".to_owned(), JsonValue::Bool(false)),
                        ("redirects".to_owned(), JsonValue::Array(Vec::new())),
                    ])),
                ),
            ]))]),
        ),
    ]))
    .expect("typed index input");

    let output = index::build(&inputs, "2026-07-18T00:00:00Z").expect("valid source should index");
    let index = output
        .as_object()
        .and_then(|value| value.get("source_index"))
        .and_then(JsonValue::as_object)
        .expect("source_index output");
    assert_eq!(
        index.get("decision").and_then(JsonValue::as_str),
        Some("ready")
    );
    assert_eq!(
        index
            .get("source_digests")
            .and_then(JsonValue::as_array)
            .and_then(|values| values.first())
            .and_then(JsonValue::as_str),
        Some("sha256:8a30448b82b07b00065f79fc5ec7926b675c209c46209c97945c5200933dd353")
    );
}

#[test]
fn invalid_fetch_evidence_is_blocked_without_runtime_failure() {
    let inputs = fixture_input::<EvidenceIndexInput>(JsonObject::from([
        (
            "objective".to_owned(),
            JsonValue::String("Compare the supplied evidence".to_owned()),
        ),
        (
            "source_packets".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(JsonObject::from([
                ("status".to_owned(), JsonValue::Number(JsonNumber::U64(200))),
                (
                    "final_url".to_owned(),
                    JsonValue::String("file:///tmp/report".to_owned()),
                ),
                (
                    "content_digest".to_owned(),
                    JsonValue::String("not-a-digest".to_owned()),
                ),
            ]))]),
        ),
    ]))
    .expect("typed index input");

    let output = index::build(&inputs, "2026-07-18T00:00:00Z")
        .expect("invalid source is a governed outcome");
    assert_eq!(
        output
            .as_object()
            .and_then(|value| value.get("source_index"))
            .and_then(JsonValue::as_object)
            .and_then(|value| value.get("decision"))
            .and_then(JsonValue::as_str),
        Some("needs_more_evidence")
    );
}

#[test]
fn indexes_a_native_local_file_read_without_remote_transport() {
    let contents = "Local architecture evidence\n";
    let content_digest = runx_contracts::sha256_prefixed(contents.as_bytes());
    let inputs = fixture_input::<EvidenceIndexInput>(JsonObject::from([
        (
            "objective".to_owned(),
            JsonValue::String("Inspect local architecture evidence".to_owned()),
        ),
        (
            "source_packets".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(JsonObject::from([
                (
                    "path".to_owned(),
                    JsonValue::String("docs/architecture.md".to_owned()),
                ),
                (
                    "repo_root".to_owned(),
                    JsonValue::String("/workspace".to_owned()),
                ),
                (
                    "contents".to_owned(),
                    JsonValue::String(contents.to_owned()),
                ),
                ("bytes".to_owned(), JsonValue::Number(JsonNumber::U64(28))),
                ("truncated".to_owned(), JsonValue::Bool(false)),
                (
                    "content_digest".to_owned(),
                    JsonValue::String(content_digest.clone()),
                ),
            ]))]),
        ),
    ]))
    .expect("typed index input");

    let output =
        index::build(&inputs, "2026-07-18T00:00:00Z").expect("native file evidence should index");
    let source = output
        .as_object()
        .and_then(|value| value.get("source_index"))
        .and_then(JsonValue::as_object)
        .and_then(|value| value.get("sources"))
        .and_then(JsonValue::as_array)
        .and_then(|values| values.first())
        .and_then(JsonValue::as_object)
        .expect("indexed local source");
    assert_eq!(
        source.get("source_kind").and_then(JsonValue::as_str),
        Some("local_file")
    );
    assert_eq!(
        source.get("content_digest").and_then(JsonValue::as_str),
        Some(content_digest.as_str())
    );
}

#[test]
fn verifies_bound_artifact_and_applies_authoritative_fields() {
    let inputs = fixture_input::<EvidenceVerifyInput>(JsonObject::from([
        (
            "candidate".to_owned(),
            JsonValue::Object(JsonObject::from([
                ("decision".to_owned(), JsonValue::String("ready".to_owned())),
                (
                    "delivery_status".to_owned(),
                    JsonValue::String("not_sent".to_owned()),
                ),
            ])),
        ),
        (
            "source_digests".to_owned(),
            JsonValue::Array(vec![JsonValue::String(SOURCE_DIGEST.to_owned())]),
        ),
        (
            "claim_bindings".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(JsonObject::from([
                (
                    "claim".to_owned(),
                    JsonValue::String("Supported".to_owned()),
                ),
                (
                    "source_digests".to_owned(),
                    JsonValue::Array(vec![JsonValue::String(SOURCE_DIGEST.to_owned())]),
                ),
            ]))]),
        ),
    ]))
    .expect("typed verify input");

    let output = verify::build(&inputs).expect("bound artifact should verify");
    assert_eq!(
        output
            .as_object()
            .and_then(|value| value.get("verification"))
            .and_then(JsonValue::as_object)
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str),
        Some("pass")
    );
}

#[test]
fn rejects_unbound_effect_claim_and_returns_sanitized_fallback() {
    let inputs = fixture_input::<EvidenceVerifyInput>(JsonObject::from([
        (
            "candidate".to_owned(),
            JsonValue::Object(JsonObject::from([
                ("decision".to_owned(), JsonValue::String("ready".to_owned())),
                (
                    "provider_status".to_owned(),
                    JsonValue::String("delivered".to_owned()),
                ),
                (
                    "transaction_id".to_owned(),
                    JsonValue::String("txn_unverified".to_owned()),
                ),
            ])),
        ),
        (
            "fallback_artifact".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "decision".to_owned(),
                JsonValue::String("needs_more_evidence".to_owned()),
            )])),
        ),
        (
            "source_digests".to_owned(),
            JsonValue::Array(vec![JsonValue::String(SOURCE_DIGEST.to_owned())]),
        ),
        (
            "claim_bindings".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(JsonObject::from([
                (
                    "claim".to_owned(),
                    JsonValue::String("Unsupported".to_owned()),
                ),
                (
                    "source_digests".to_owned(),
                    JsonValue::Array(vec![JsonValue::String(
                        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                            .to_owned(),
                    )]),
                ),
            ]))]),
        ),
    ]))
    .expect("typed verify input");

    let output = verify::build(&inputs).expect("rejection is a governed outcome");
    assert_eq!(
        output
            .as_object()
            .and_then(|value| value.get("verification"))
            .and_then(JsonValue::as_object)
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str),
        Some("fail")
    );
    assert_eq!(
        output
            .as_object()
            .and_then(|value| value.get("verified_artifact"))
            .and_then(JsonValue::as_object)
            .and_then(|value| value.get("decision"))
            .and_then(JsonValue::as_str),
        Some("needs_more_evidence")
    );
}
