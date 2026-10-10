//! Native governed HTTP batches for provider skills.
//!
//! Domain adapters prepare typed request records and interpret provider results;
//! Runx owns transport, credential use, SSRF protection, host admission, response
//! bounds, retries, and secret redaction. This keeps provider algorithms portable
//! without letting every package grow its own HTTP client.

#[cfg(test)]
use runx_contracts::JsonValue;

use super::NativeInvocation;
use crate::RuntimeError;
use crate::http::{HttpMethod, STANDARD_HTTP_RESPONSE_BYTES};

mod auth;
mod batch;
mod capability;
mod output;
mod request;
mod resolution;

use super::capability::decode_typed_output;
pub(super) use capability::CAPABILITIES;
use capability::HttpBatchInput;
use output::HttpBatchOutput;

use batch::execute_batch;
#[cfg(test)]
use resolution::{percent_encode, response_reference};
#[cfg(test)]
use runx_contracts::JsonObject;
#[cfg(test)]
use std::collections::BTreeMap;

const TOOL: &str = "http";
const MAX_REQUESTS: usize = 50;
const MAX_HTTP_OUTPUT_BYTES: usize = STANDARD_HTTP_RESPONSE_BYTES;
const DEFAULT_RESPONSE_BYTES: usize = STANDARD_HTTP_RESPONSE_BYTES;
const MAX_RESPONSE_BYTES: usize = MAX_HTTP_OUTPUT_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BatchMode {
    Read,
    Query,
    Execute,
}

impl BatchMode {
    fn admits(self, method: HttpMethod) -> bool {
        match self {
            Self::Read => method == HttpMethod::Get,
            Self::Query => method == HttpMethod::Post,
            Self::Execute => true,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Read => "http.read",
            Self::Query => "http.query",
            Self::Execute => "http.execute",
        }
    }

    fn retries_as_idempotent(self) -> bool {
        self == Self::Query
    }
}

fn read(
    invocation: &NativeInvocation<'_, HttpBatchInput>,
) -> Result<HttpBatchOutput, RuntimeError> {
    decode_typed_output("http.read", execute_batch(invocation, BatchMode::Read)?)
}

fn query(
    invocation: &NativeInvocation<'_, HttpBatchInput>,
) -> Result<HttpBatchOutput, RuntimeError> {
    decode_typed_output("http.query", execute_batch(invocation, BatchMode::Query)?)
}

fn execute(
    invocation: &NativeInvocation<'_, HttpBatchInput>,
) -> Result<HttpBatchOutput, RuntimeError> {
    // The local assistant's mandatory approval and confidential-egress policy
    // is enforced by the provider effect owner. A raw HTTP mutation cannot
    // produce that effect's approval, attempt, or recovery evidence.
    if invocation
        .env
        .contains_key(crate::effects::ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV)
    {
        return Err(invalid(
            "assistant HTTP mutations require provider.mutate so approval and recovery own the effect",
        ));
    }
    decode_typed_output(
        "http.execute",
        execute_batch(invocation, BatchMode::Execute)?,
    )
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: TOOL.to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "catalog")]
    #[test]
    fn assistant_http_mutation_cannot_bypass_provider_effect() {
        let inputs = HttpBatchInput {
            requests: Vec::new(),
            allowed_hosts: Vec::new(),
            auth: None,
            stop_on_error: true,
        };
        let env = BTreeMap::from([(
            crate::effects::ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV.to_owned(),
            "required".to_owned(),
        )]);
        let credentials = crate::CredentialDelivery::none();
        let effects = crate::RuntimeEffectRegistry::default();
        let invocation = NativeInvocation {
            inputs: &inputs,
            observed_at: "2026-01-01T00:00:00Z",
            data_source_binding: None,
            env: &env,
            skill_directory: std::path::Path::new("."),
            credential_delivery: &credentials,
            local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
            effects: &effects,
        };
        assert!(matches!(
            execute(&invocation),
            Err(error) if error.to_string().contains("require provider.mutate")
        ));
    }

    #[test]
    fn oauth_percent_encoding_matches_rfc_5849() {
        assert_eq!(
            percent_encode("Ladies + Gentlemen"),
            "Ladies%20%2B%20Gentlemen"
        );
        assert_eq!(percent_encode("~-._"), "~-._");
    }

    #[test]
    fn response_references_are_exact_and_bounded() {
        let prior = BTreeMap::from([(
            "first".to_owned(),
            JsonObject::from([(
                "json".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "data".to_owned(),
                    JsonValue::Object(JsonObject::from([(
                        "id".to_owned(),
                        JsonValue::String("123".to_owned()),
                    )])),
                )])),
            )]),
        )]);
        assert_eq!(
            response_reference(&prior, "first.json.data.id").and_then(JsonValue::as_str),
            Some("123")
        );
        assert!(response_reference(&prior, "missing.json.data.id").is_none());
    }

    #[test]
    fn url_templates_resolve_typed_path_parameters() -> Result<(), Box<dyn std::error::Error>> {
        let path = JsonObject::from([
            (
                "office".to_owned(),
                JsonValue::String("LWX/primary".to_owned()),
            ),
            ("grid".to_owned(), JsonValue::String("97,71".to_owned())),
        ]);

        let resolved = resolution::resolve_url_template(
            "https://api.weather.gov/gridpoints/{office}/{grid}/forecast",
            &BTreeMap::new(),
            &path,
        )?;

        assert_eq!(
            resolved,
            "https://api.weather.gov/gridpoints/LWX%2Fprimary/97%2C71/forecast"
        );
        Ok(())
    }

    #[test]
    fn url_templates_reject_missing_and_unused_path_parameters()
    -> Result<(), Box<dyn std::error::Error>> {
        let missing = match resolution::resolve_url_template(
            "https://api.example.test/accounts/{account_id}",
            &BTreeMap::new(),
            &JsonObject::new(),
        ) {
            Ok(_) => return Err("missing path parameter unexpectedly resolved".into()),
            Err(error) => error,
        };
        assert!(missing.to_string().contains("account_id"));

        let unused = match resolution::resolve_url_template(
            "https://api.example.test/accounts",
            &BTreeMap::new(),
            &JsonObject::from([(
                "account_id".to_owned(),
                JsonValue::String("acct-1".to_owned()),
            )]),
        ) {
            Ok(_) => return Err("unused path parameter unexpectedly resolved".into()),
            Err(error) => error,
        };
        assert!(
            unused
                .to_string()
                .contains("not present in the request URL")
        );
        Ok(())
    }

    #[test]
    fn batch_modes_keep_queries_read_only_by_contract() {
        assert!(BatchMode::Read.admits(HttpMethod::Get));
        assert!(!BatchMode::Read.admits(HttpMethod::Post));
        assert!(BatchMode::Query.admits(HttpMethod::Post));
        assert!(!BatchMode::Query.admits(HttpMethod::Get));
        assert!(!BatchMode::Query.admits(HttpMethod::Delete));
        assert!(BatchMode::Execute.admits(HttpMethod::Delete));
        assert!(!BatchMode::Read.retries_as_idempotent());
        assert!(BatchMode::Query.retries_as_idempotent());
        assert!(!BatchMode::Execute.retries_as_idempotent());
    }
}
