use runx_contracts::{JsonNumber, JsonObject, JsonValue, sha256_prefixed};
use url::Url;

use super::{is_sha256, object, text};

#[derive(Clone)]
pub(super) struct IndexedSource {
    source_digest: String,
    content_digest: String,
    source_ref: String,
    source_kind: &'static str,
    extracted: String,
    provenance: JsonObject,
}

impl IndexedSource {
    pub(super) fn from_packet(
        source: &JsonObject,
        observed_at: &str,
        max_characters: u64,
    ) -> Result<Self, Vec<String>> {
        if source.contains_key("contents") {
            return Self::from_local_file(source, observed_at, max_characters);
        }
        Self::from_fetch(source, max_characters)
    }

    fn from_fetch(source: &JsonObject, max_characters: u64) -> Result<Self, Vec<String>> {
        let extracted = text(source.get("extracted"));
        let final_url = text(source.get("final_url"));
        let content_digest = text(source.get("content_digest"));
        let provenance = object(source.get("provenance"));
        let status = coerced_nonnegative_integer(source.get("status"));
        let bytes = coerced_nonnegative_integer(provenance.get("bytes"));
        let observed_at = text(provenance.get("fetched_at"));
        let blockers = fetch_blockers(FetchFields {
            extracted: &extracted,
            final_url: &final_url,
            content_digest: &content_digest,
            observed_at: &observed_at,
            status,
            bytes,
            max_characters,
        });
        if !blockers.is_empty() {
            return Err(blockers);
        }

        Ok(Self {
            source_digest: sha256_prefixed(extracted.as_bytes()),
            content_digest,
            source_ref: final_url,
            source_kind: "fetch",
            extracted,
            provenance: JsonObject::from([
                ("observed_at".to_owned(), JsonValue::String(observed_at)),
                (
                    "bytes".to_owned(),
                    JsonValue::Number(JsonNumber::U64(bytes.unwrap_or_default())),
                ),
                (
                    "status".to_owned(),
                    JsonValue::Number(JsonNumber::U64(status.unwrap_or_default())),
                ),
                (
                    "redirects".to_owned(),
                    JsonValue::Array(
                        provenance
                            .get("redirects")
                            .and_then(JsonValue::as_array)
                            .cloned()
                            .unwrap_or_default(),
                    ),
                ),
            ]),
        })
    }

    fn from_local_file(
        source: &JsonObject,
        observed_at: &str,
        max_characters: u64,
    ) -> Result<Self, Vec<String>> {
        let extracted = source
            .get("contents")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_owned();
        let path = text(source.get("path"));
        let content_digest = text(source.get("content_digest"));
        let bytes = coerced_nonnegative_integer(source.get("bytes"));
        let truncated = source.get("truncated").and_then(JsonValue::as_bool);
        let mut blockers = Vec::new();
        check(!path.is_empty(), "path is missing", &mut blockers);
        check(
            is_sha256(&content_digest),
            "content_digest is not sha256",
            &mut blockers,
        );
        check(
            !extracted.is_empty(),
            "contents text is missing",
            &mut blockers,
        );
        check(bytes.is_some(), "bytes is invalid", &mut blockers);
        check(
            truncated == Some(false),
            "local file read is truncated",
            &mut blockers,
        );
        check(
            !observed_at.trim().is_empty(),
            "runtime observation time is missing",
            &mut blockers,
        );
        if extracted.encode_utf16().count() as u64 > max_characters {
            blockers.push(format!("contents text exceeds {max_characters} characters"));
        }
        check(
            sha256_prefixed(extracted.as_bytes()) == content_digest,
            "content_digest does not match contents",
            &mut blockers,
        );
        if !blockers.is_empty() {
            return Err(blockers);
        }

        Ok(Self {
            source_digest: sha256_prefixed(extracted.as_bytes()),
            content_digest,
            source_ref: format!("file:{path}"),
            source_kind: "local_file",
            extracted,
            provenance: JsonObject::from([
                (
                    "observed_at".to_owned(),
                    JsonValue::String(observed_at.to_owned()),
                ),
                (
                    "bytes".to_owned(),
                    JsonValue::Number(JsonNumber::U64(bytes.unwrap_or_default())),
                ),
                ("path".to_owned(), JsonValue::String(path)),
            ]),
        })
    }

    pub(super) fn character_count(&self) -> u64 {
        self.extracted.encode_utf16().count() as u64
    }

    pub(super) fn digest(&self) -> &str {
        &self.source_digest
    }

    pub(super) fn digest_json(&self) -> JsonValue {
        JsonValue::String(self.source_digest.clone())
    }

    pub(super) fn as_json(&self) -> JsonValue {
        JsonValue::Object(JsonObject::from([
            ("source_digest".to_owned(), self.digest_json()),
            (
                "content_digest".to_owned(),
                JsonValue::String(self.content_digest.clone()),
            ),
            (
                "source_ref".to_owned(),
                JsonValue::String(self.source_ref.clone()),
            ),
            (
                "source_kind".to_owned(),
                JsonValue::String(self.source_kind.to_owned()),
            ),
            (
                "extracted".to_owned(),
                JsonValue::String(self.extracted.clone()),
            ),
            (
                "provenance".to_owned(),
                JsonValue::Object(self.provenance.clone()),
            ),
        ]))
    }

    pub(super) fn index_material(&self) -> JsonValue {
        let mut value = self.as_json().as_object().cloned().unwrap_or_default();
        value.remove("extracted");
        value.insert(
            "extracted_digest".to_owned(),
            JsonValue::String(sha256_prefixed(self.extracted.as_bytes())),
        );
        JsonValue::Object(value)
    }

    pub(super) fn evidence_json(&self) -> JsonValue {
        JsonValue::Object(JsonObject::from([
            ("evidence_digest".to_owned(), self.digest_json()),
            (
                "content_digest".to_owned(),
                JsonValue::String(self.content_digest.clone()),
            ),
            (
                "source_ref".to_owned(),
                JsonValue::String(self.source_ref.clone()),
            ),
            (
                "source_kind".to_owned(),
                JsonValue::String(self.source_kind.to_owned()),
            ),
            (
                "provenance".to_owned(),
                JsonValue::Object(self.provenance.clone()),
            ),
        ]))
    }
}

struct FetchFields<'a> {
    extracted: &'a str,
    final_url: &'a str,
    content_digest: &'a str,
    observed_at: &'a str,
    status: Option<u64>,
    bytes: Option<u64>,
    max_characters: u64,
}

fn fetch_blockers(fields: FetchFields<'_>) -> Vec<String> {
    let mut blockers = Vec::new();
    check(
        matches!(fields.status, Some(200..=299)),
        "status is not 2xx",
        &mut blockers,
    );
    check(
        is_http_url(fields.final_url),
        "final_url is not http(s)",
        &mut blockers,
    );
    check(
        is_sha256(fields.content_digest),
        "content_digest is not sha256",
        &mut blockers,
    );
    check(
        !fields.extracted.is_empty(),
        "extracted text is missing",
        &mut blockers,
    );
    if fields.extracted.encode_utf16().count() as u64 > fields.max_characters {
        blockers.push(format!(
            "extracted text exceeds {} characters",
            fields.max_characters
        ));
    }
    check(
        !fields.observed_at.is_empty(),
        "provenance.fetched_at is missing",
        &mut blockers,
    );
    check(
        fields.bytes.is_some(),
        "provenance.bytes is invalid",
        &mut blockers,
    );
    blockers
}

fn check(condition: bool, message: &str, blockers: &mut Vec<String>) {
    if !condition {
        blockers.push(message.to_owned());
    }
}

/// The source fields of a packet: a `web/page.read` operation's result, a
/// file read's data, or the fields themselves.
pub(super) fn unwrap_source_packet(value: &JsonValue) -> &JsonObject {
    let packet = object(Some(value));
    let packet = ["provider_operation", "file_read"]
        .into_iter()
        .find_map(|output| packet.get(output).and_then(JsonValue::as_object))
        .unwrap_or(packet);
    let data = packet
        .get("data")
        .and_then(JsonValue::as_object)
        .unwrap_or(packet);
    data.get("result")
        .and_then(JsonValue::as_object)
        .unwrap_or(data)
}

fn coerced_nonnegative_integer(value: Option<&JsonValue>) -> Option<u64> {
    match value {
        Some(JsonValue::Number(JsonNumber::U64(value))) => Some(*value),
        Some(JsonValue::Number(JsonNumber::I64(value))) if *value >= 0 => Some(*value as u64),
        Some(JsonValue::Number(JsonNumber::F64(value)))
            if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 =>
        {
            Some(*value as u64)
        }
        Some(JsonValue::String(value)) => value.trim().parse().ok(),
        _ => None,
    }
}

fn is_http_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}
