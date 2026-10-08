//! Canonical JSON primitives shared by Runx contract boundaries.

use std::io::{self, Write};

use serde::Serialize;
use thiserror::Error;

use crate::sha256_prefixed;

pub const STABLE_JSON_CANONICALIZATION: &str = "runx.stable-json.v1";

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CanonicalJsonError {
    #[error("canonical JSON serialization failed: {message}")]
    Serialization { message: String },
}

mod serializer;

pub use serializer::to_canonical_bytes;

/// Render any serializable value under `runx.stable-json.v1`: recursively
/// sorted object keys with JavaScript `JSON.stringify`-compatible number and
/// string encoding. Written directly from the value; no tree is built.
pub fn canonical_stable_json<T: Serialize + ?Sized>(
    value: &T,
) -> Result<String, CanonicalJsonError> {
    String::from_utf8(to_canonical_bytes(value)?).map_err(|source| {
        CanonicalJsonError::Serialization {
            message: source.to_string(),
        }
    })
}

/// `sha256:<hex>` over the canonical encoding of `value`.
pub fn canonical_digest<T: Serialize + ?Sized>(value: &T) -> Result<String, CanonicalJsonError> {
    to_canonical_bytes(value).map(|bytes| sha256_prefixed(&bytes))
}

/// Append one serde value using the JSON leaf encoding shared by Runx's
/// canonical writers.
///
/// This is intentionally a leaf primitive: object ordering and field omission
/// remain the responsibility of the canonicalization contract calling it.
pub fn write_canonical_json_fragment<T: Serialize + ?Sized>(
    value: &T,
    output: &mut String,
) -> Result<(), CanonicalJsonError> {
    let mut serializer = serde_json::Serializer::new(JsonStringWriter { output });
    value
        .serialize(&mut serializer)
        .map_err(|source| CanonicalJsonError::Serialization {
            message: source.to_string(),
        })
}

struct JsonStringWriter<'a> {
    output: &'a mut String,
}

impl Write for JsonStringWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let text = std::str::from_utf8(bytes)
            .map_err(|source| io::Error::new(io::ErrorKind::InvalidData, source))?;
        self.output.push_str(text);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde::Deserialize;

    use super::{CanonicalJsonError, STABLE_JSON_CANONICALIZATION, canonical_stable_json};
    use crate::{JsonNumber, JsonValue, sha256_prefixed};

    const STABLE_JSON_ORACLE: &str =
        include_str!("../../../fixtures/contracts/canonical-json/runx-stable-json-v1.cases.json");
    const STABLE_JSON_NUMBERS_ORACLE: &str = include_str!(
        "../../../fixtures/contracts/canonical-json/runx-stable-json-v1.numbers.cases.json"
    );

    #[derive(Debug, Deserialize)]
    struct StableJsonFixture {
        canonicalization: String,
        cases: Vec<StableJsonCase>,
    }

    #[derive(Debug, Deserialize)]
    struct StableJsonCase {
        name: String,
        value: JsonValue,
        expected_canonical_json: String,
        expected_sha256: String,
    }

    #[test]
    fn sha256_prefixes_digest() {
        assert_eq!(
            sha256_prefixed(b"runx"),
            "sha256:8186b7035bea2f66ebe27c1f5cf7de4e94ef935e259a2f3160352adffc752f28"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]
        #[test]
        fn canonical_writer_is_internally_consistent(value in arbitrary_json_value(4)) {
            let first = canonical_stable_json(&value)
                .map_err(|error| proptest::test_runner::TestCaseError::fail(error.to_string()))?;
            let reparsed: JsonValue = serde_json::from_str(&first)
                .map_err(|error| proptest::test_runner::TestCaseError::fail(error.to_string()))?;
            let second = canonical_stable_json(&reparsed)
                .map_err(|error| proptest::test_runner::TestCaseError::fail(error.to_string()))?;
            prop_assert_eq!(first, second);
        }
    }

    #[test]
    fn stable_json_oracle_matches_rust_canonical_json() -> Result<(), CanonicalJsonError> {
        stable_json_oracle_matches(STABLE_JSON_ORACLE)
    }

    #[test]
    fn stable_json_numbers_oracle_matches_rust_canonical_json() -> Result<(), CanonicalJsonError> {
        stable_json_oracle_matches(STABLE_JSON_NUMBERS_ORACLE)
    }

    fn stable_json_oracle_matches(oracle_json: &str) -> Result<(), CanonicalJsonError> {
        let oracle: StableJsonFixture = serde_json::from_str(oracle_json).map_err(|source| {
            CanonicalJsonError::Serialization {
                message: source.to_string(),
            }
        })?;
        assert_eq!(oracle.canonicalization, STABLE_JSON_CANONICALIZATION);

        for case in oracle.cases {
            let actual = canonical_stable_json(&case.value)?;
            assert_eq!(
                actual, case.expected_canonical_json,
                "{} canonical JSON drifted",
                case.name
            );
            assert_eq!(
                sha256_prefixed(actual.as_bytes()),
                case.expected_sha256,
                "{} sha256 drifted",
                case.name
            );
        }
        Ok(())
    }

    fn arbitrary_json_value(depth: u32) -> BoxedStrategy<JsonValue> {
        // Floating-point encoding is covered by the byte-pinned oracle; this
        // round-trip property deliberately uses integers and ASCII strings.
        let leaf = prop_oneof![
            Just(JsonValue::Null),
            any::<bool>().prop_map(JsonValue::Bool),
            any::<i64>().prop_map(|value| JsonValue::Number(JsonNumber::I64(value))),
            "[ -~]{0,32}".prop_map(JsonValue::String),
        ];
        leaf.prop_recursive(depth, 32, 6, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..6).prop_map(JsonValue::Array),
                proptest::collection::btree_map("[a-zA-Z0-9_-]{1,8}", inner, 0..6)
                    .prop_map(JsonValue::Object),
            ]
        })
        .boxed()
    }
}
