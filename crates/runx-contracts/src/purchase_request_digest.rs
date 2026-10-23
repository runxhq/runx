//! Canonical request digests for idempotent purchase operations.

use serde::Serialize;

use crate::{
    CANCEL_PURCHASE, CancelPurchaseRequest, CanonicalJsonError, EXECUTE_PURCHASE,
    ExecutePurchaseRequest, JsonObject, JsonValue, QUOTE_PURCHASE, QuotePurchaseRequest,
    STABLE_JSON_CANONICALIZATION, canonical_stable_json, sha256_prefixed,
};

pub const PURCHASE_REQUEST_DIGEST_SCHEMA: &str = "runx.payment.request_digest.v1";

pub fn digest_quote_purchase_request(
    request: &QuotePurchaseRequest,
) -> Result<String, CanonicalJsonError> {
    digest_request(QUOTE_PURCHASE, request)
}

pub fn digest_execute_purchase_request(
    request: &ExecutePurchaseRequest,
) -> Result<String, CanonicalJsonError> {
    digest_request(EXECUTE_PURCHASE, request)
}

pub fn digest_cancel_purchase_request(
    request: &CancelPurchaseRequest,
) -> Result<String, CanonicalJsonError> {
    digest_request(CANCEL_PURCHASE, request)
}

fn digest_request<T: Serialize>(
    operation: &'static str,
    request: &T,
) -> Result<String, CanonicalJsonError> {
    let request = serde_json::to_value(request)
        .and_then(serde_json::from_value::<JsonValue>)
        .map_err(serialization_error)?;
    let preimage = JsonValue::Object(JsonObject::from([
        (
            "canonicalization".to_owned(),
            JsonValue::String(STABLE_JSON_CANONICALIZATION.to_owned()),
        ),
        (
            "operation".to_owned(),
            JsonValue::String(operation.to_owned()),
        ),
        ("request".to_owned(), request),
        (
            "schema".to_owned(),
            JsonValue::String(PURCHASE_REQUEST_DIGEST_SCHEMA.to_owned()),
        ),
    ]));
    canonical_stable_json(&preimage).map(|bytes| sha256_prefixed(bytes.as_bytes()))
}

fn serialization_error(source: serde_json::Error) -> CanonicalJsonError {
    CanonicalJsonError::Serialization {
        message: source.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde::Deserialize;
    use serde_json::Value;

    use super::*;
    use crate::{MAX_PORTABLE_INTEGER, PortableAmountMinor};

    const ORACLE: &str = include_str!(
        "../../../fixtures/contracts/canonical-json/runx-purchase-request-digest-v1.oracles.json"
    );

    #[derive(Debug, Deserialize)]
    struct Oracle {
        canonicalization: String,
        cases: Vec<OracleCase>,
        schema: String,
    }

    #[derive(Debug, Deserialize)]
    struct OracleCase {
        canonical_json: String,
        expected_digest: String,
        name: String,
        operation: String,
        preimage: JsonValue,
        request: Value,
    }

    #[test]
    fn purchase_request_digests_match_the_generated_oracle() -> Result<(), CanonicalJsonError> {
        let oracle: Oracle = serde_json::from_str(ORACLE).map_err(serialization_error)?;
        assert_eq!(oracle.schema, "runx.canonical_json_oracle.v1");
        assert_eq!(oracle.canonicalization, STABLE_JSON_CANONICALIZATION);

        for case in oracle.cases {
            let actual = match case.operation.as_str() {
                QUOTE_PURCHASE => digest_quote_purchase_request(
                    &serde_json::from_value(case.request.clone()).map_err(serialization_error)?,
                )?,
                EXECUTE_PURCHASE => digest_execute_purchase_request(
                    &serde_json::from_value(case.request.clone()).map_err(serialization_error)?,
                )?,
                CANCEL_PURCHASE => digest_cancel_purchase_request(
                    &serde_json::from_value(case.request.clone()).map_err(serialization_error)?,
                )?,
                operation => {
                    return Err(test_error(format!(
                        "unexpected oracle operation: {operation}"
                    )));
                }
            };
            assert_eq!(actual, case.expected_digest, "{} digest drifted", case.name);
            assert_eq!(
                canonical_stable_json(&case.preimage)?,
                case.canonical_json,
                "{} canonical preimage drifted",
                case.name
            );
            assert_eq!(
                sha256_prefixed(case.canonical_json.as_bytes()),
                case.expected_digest,
                "{} oracle digest is not bound to its bytes",
                case.name
            );
        }
        Ok(())
    }

    #[test]
    fn purchase_request_digest_cases_cover_each_request_member() -> Result<(), CanonicalJsonError> {
        let oracle: Oracle = serde_json::from_str(ORACLE).map_err(serialization_error)?;

        let distinct_digests = oracle
            .cases
            .iter()
            .map(|case| case.expected_digest.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(distinct_digests.len(), oracle.cases.len());

        // Every request member has a case, and each moves the digest.
        assert_changed_members(
            &oracle.cases,
            QUOTE_PURCHASE,
            &[
                "accepted_settlement_families",
                "amount_minor",
                "buyer_ref",
                "currency",
                "idempotency",
                "input_digest",
                "offer_version",
                "package_digest",
                "seller_ref",
                "skill_ref",
            ],
        )?;
        assert_changed_members(
            &oracle.cases,
            EXECUTE_PURCHASE,
            &[
                "idempotency",
                "purchase_id",
                "payment_ref",
                "settlement_family",
            ],
        )?;
        assert_changed_members(
            &oracle.cases,
            CANCEL_PURCHASE,
            &["idempotency", "purchase_id"],
        )?;

        let quote_base = oracle
            .cases
            .iter()
            .find(|case| case.name == "quote-base")
            .ok_or_else(|| test_error("oracle is missing quote-base"))?;
        assert!(quote_base.canonical_json.starts_with(
            "{\"canonicalization\":\"runx.stable-json.v1\",\"operation\":\"QuotePurchase\",\"request\":{"
        ));
        assert!(
            quote_base
                .canonical_json
                .ends_with(",\"schema\":\"runx.payment.request_digest.v1\"}")
        );
        // V1 has exactly one valid canonicalizer_version, so it is committed
        // in the hand-checked base preimage rather than represented by an
        // impossible changed-term case.
        assert_eq!(
            quote_base
                .request
                .get("canonicalizer_version")
                .and_then(Value::as_str),
            Some("runx.receipt.c14n.v1")
        );
        Ok(())
    }

    #[test]
    fn purchase_rust_types_reject_identity_discriminants() -> Result<(), CanonicalJsonError> {
        let oracle: Oracle = serde_json::from_str(ORACLE).map_err(serialization_error)?;
        let request = oracle
            .cases
            .iter()
            .find(|case| case.name == "quote-base")
            .ok_or_else(|| test_error("oracle is missing quote-base"))?
            .request
            .clone();

        let mut top_level = request.clone();
        top_level["schema"] = Value::String("runx.payment.quote_purchase.request.v1".to_owned());
        assert!(serde_json::from_value::<QuotePurchaseRequest>(top_level).is_err());

        let mut nested = request;
        nested["buyer_ref"]["schema"] = Value::String("runx.reference.v1".to_owned());
        assert!(serde_json::from_value::<QuotePurchaseRequest>(nested).is_err());
        Ok(())
    }

    #[test]
    fn purchase_amount_is_portable_at_the_wire_boundary() -> Result<(), CanonicalJsonError> {
        assert_eq!(
            PortableAmountMinor::new(MAX_PORTABLE_INTEGER).map(PortableAmountMinor::get),
            Some(MAX_PORTABLE_INTEGER)
        );
        assert!(PortableAmountMinor::new(0).is_none());
        assert!(PortableAmountMinor::new(MAX_PORTABLE_INTEGER + 1).is_none());

        let oracle: Oracle = serde_json::from_str(ORACLE).map_err(serialization_error)?;
        let mut request = oracle
            .cases
            .iter()
            .find(|case| case.name == "quote-base")
            .ok_or_else(|| test_error("oracle is missing quote-base"))?
            .request
            .clone();
        request["amount_minor"] = Value::from(MAX_PORTABLE_INTEGER + 1);
        assert!(serde_json::from_value::<QuotePurchaseRequest>(request).is_err());
        Ok(())
    }

    fn assert_changed_members(
        cases: &[OracleCase],
        operation: &str,
        expected: &[&str],
    ) -> Result<(), CanonicalJsonError> {
        let operation_cases = cases
            .iter()
            .filter(|case| case.operation == operation)
            .collect::<Vec<_>>();
        let base = &operation_cases
            .first()
            .ok_or_else(|| test_error(format!("oracle is missing {operation} base case")))?
            .request;
        let changed = operation_cases
            .iter()
            .skip(1)
            .map(|case| changed_top_level_members(base, &case.request))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<BTreeSet<_>>();
        assert_eq!(changed, expected.iter().copied().collect());
        Ok(())
    }

    fn changed_top_level_members<'a>(
        base: &'a Value,
        changed: &'a Value,
    ) -> Result<Vec<&'a str>, CanonicalJsonError> {
        let base = base
            .as_object()
            .ok_or_else(|| test_error("base request is not an object"))?;
        let changed = changed
            .as_object()
            .ok_or_else(|| test_error("changed request is not an object"))?;
        Ok(base
            .keys()
            .chain(changed.keys())
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|key| base.get(*key) != changed.get(*key))
            .collect())
    }

    fn test_error(message: impl Into<String>) -> CanonicalJsonError {
        CanonicalJsonError::Serialization {
            message: message.into(),
        }
    }
}
