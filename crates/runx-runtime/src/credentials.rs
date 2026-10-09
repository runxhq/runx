// Module rationale: credential delivery is one secret-handling trust surface; secret
// string/env types, redaction, and the delivery boundary stay colocated so the
// "secrets never leak" review happens against the whole module at once.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use base64::Engine as _;
use runx_contracts::{
    CredentialDeliveryMode, CredentialDeliveryObservation, CredentialDeliveryObservationStatus,
    CredentialDeliveryPurpose, JsonObject, JsonValue, ProofKind, Reference, ReferenceType,
    sha256_hex, sha256_prefixed,
};
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::Zeroize;

const REDACTED_CREDENTIAL: &str = "[redacted-credential]";
const MAX_STRUCTURED_REDACTION_DEPTH: usize = 128;

#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED_CREDENTIAL)
    }
}

impl PartialEq for SecretString {
    fn eq(&self, other: &Self) -> bool {
        bool::from(self.0.as_bytes().ct_eq(other.0.as_bytes()))
    }
}

impl Eq for SecretString {}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretEnv {
    values: BTreeMap<String, SecretString>,
}

impl SecretEnv {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(SecretString::expose)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values
            .iter()
            .map(|(key, value)| (key.as_str(), value.expose()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CredentialDelivery {
    inner: Arc<CredentialDeliveryState>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CredentialDeliveryState {
    secret_env: SecretEnv,
    secret_taint: SecretTaint,
    public_observation: Option<runx_contracts::CredentialDeliveryObservation>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SecretTaint {
    derived: Vec<SecretString>,
}

impl SecretTaint {
    fn from_secret_env(secret_env: &SecretEnv) -> Self {
        let mut variants = BTreeSet::new();
        for (_, delivered) in secret_env.iter() {
            // A credential of several fields is delivered as one JSON object;
            // each field is secret on its own, wherever a process prints it.
            let fields = serde_json::from_str::<BTreeMap<String, String>>(delivered)
                .map(BTreeMap::into_values)
                .unwrap_or_default()
                .collect::<Vec<_>>();
            for secret in std::iter::once(delivered).chain(fields.iter().map(String::as_str)) {
                // Derived encodings of tiny values are too collision-prone to
                // be a useful redaction boundary. The exact raw value is still
                // scrubbed.
                if secret.len() < 6 {
                    continue;
                }
                if secret != delivered {
                    variants.insert(secret.to_owned());
                }
                Self::encodings(secret, &mut variants);
            }
        }
        let mut derived = variants
            .into_iter()
            .map(SecretString::new)
            .collect::<Vec<_>>();
        derived.sort_by_key(|value| std::cmp::Reverse(value.expose().len()));
        Self { derived }
    }

    fn encodings(secret: &str, variants: &mut BTreeSet<String>) {
        let encoded = [
            url::form_urlencoded::byte_serialize(secret.as_bytes()).collect::<String>(),
            form_urlencoded_value(secret),
            base64::engine::general_purpose::STANDARD.encode(secret),
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(secret),
            base64::engine::general_purpose::URL_SAFE.encode(secret),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret),
        ];
        for value in encoded {
            if value != secret && value.len() >= 8 {
                variants.insert(value);
            }
        }
    }
}

fn form_urlencoded_value(value: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("", value)
        .finish()
        .strip_prefix('=')
        .unwrap_or_default()
        .to_owned()
}

impl CredentialDelivery {
    #[must_use]
    pub fn none() -> Self {
        Self::from_parts(SecretEnv::default(), None)
    }

    fn from_parts(
        secret_env: SecretEnv,
        public_observation: Option<runx_contracts::CredentialDeliveryObservation>,
    ) -> Self {
        let secret_taint = SecretTaint::from_secret_env(&secret_env);
        Self {
            inner: Arc::new(CredentialDeliveryState {
                secret_env,
                secret_taint,
                public_observation,
            }),
        }
    }

    /// Deliver one credential to a process as an environment variable, with
    /// the non-secret observation the receipt records. The secret is held only
    /// for the lifetime of the run.
    pub fn process_env(
        connector: &str,
        kind: &str,
        env_var: &str,
        material_ref: &str,
        secret: impl Into<String>,
    ) -> Result<Self, CredentialDeliveryError> {
        validate_env_name(env_var)?;
        let secret = SecretString::new(secret);
        if secret.expose().trim().is_empty() {
            return Err(CredentialDeliveryError::EmptySecret);
        }
        Ok(Self::from_parts(
            SecretEnv {
                values: BTreeMap::from([(env_var.to_owned(), secret)]),
            },
            Some(build_local_provision_observation(
                connector,
                kind,
                material_ref,
            )),
        ))
    }

    #[must_use]
    pub fn secret_env(&self) -> &SecretEnv {
        &self.inner.secret_env
    }

    pub fn reject_process_env_boundary(
        &self,
        boundary: &'static str,
    ) -> Result<(), CredentialDeliveryError> {
        if self.inner.secret_env.is_empty() {
            return Ok(());
        }
        Err(CredentialDeliveryError::ProcessEnvBoundaryUnsupported {
            boundary: boundary.to_owned(),
        })
    }

    pub fn ensure_environment_disjoint(
        &self,
        environment: &BTreeMap<String, String>,
    ) -> Result<(), CredentialDeliveryError> {
        if let Some(name) = self
            .inner
            .secret_env
            .values
            .keys()
            .find(|name| environment.contains_key(*name))
        {
            return Err(CredentialDeliveryError::EnvironmentCollision { name: name.clone() });
        }
        Ok(())
    }

    #[must_use]
    pub fn with_public_observation(
        mut self,
        observation: runx_contracts::CredentialDeliveryObservation,
    ) -> Self {
        Arc::make_mut(&mut self.inner).public_observation = Some(observation);
        self
    }

    #[must_use]
    pub fn public_observation(&self) -> Option<&runx_contracts::CredentialDeliveryObservation> {
        self.inner.public_observation.as_ref()
    }

    #[must_use]
    pub fn credential_refs(&self) -> Option<Vec<runx_contracts::Reference>> {
        self.inner
            .public_observation
            .as_ref()
            .and_then(|observation| {
                (!observation.credential_refs.is_empty())
                    .then(|| observation.credential_refs.clone())
            })
    }

    #[must_use]
    pub fn redact_text(&self, text: impl Into<String>) -> String {
        let mut redacted = text.into();
        for value in self.inner.secret_env.values.values() {
            let secret = value.expose();
            if !secret.is_empty() {
                redacted = redacted.replace(secret, REDACTED_CREDENTIAL);
            }
        }
        for value in &self.inner.secret_taint.derived {
            redacted = redacted.replace(value.expose(), REDACTED_CREDENTIAL);
        }
        redacted
    }

    /// Redact delivered credential material from every string-bearing JSON
    /// position after decoding. Callers must not redact a serialized
    /// representation and then parse it, because JSON escapes can otherwise
    /// reconstruct the secret after the redaction pass.
    fn redact_json_value_at_depth(&self, value: &mut JsonValue, depth: usize) {
        if depth >= MAX_STRUCTURED_REDACTION_DEPTH {
            *value = JsonValue::String(REDACTED_CREDENTIAL.to_owned());
            return;
        }
        match value {
            JsonValue::String(text) => {
                *text = self.redact_output_text_at_depth(std::mem::take(text), depth + 1);
            }
            JsonValue::Array(values) => {
                for child in values {
                    self.redact_json_value_at_depth(child, depth + 1);
                }
            }
            JsonValue::Object(object) => self.redact_json_object_at_depth(object, depth),
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) => {}
        }
    }

    fn redact_json_object_at_depth(&self, object: &mut JsonObject, depth: usize) {
        let mut redacted = JsonObject::new();
        for (key, mut value) in std::mem::take(object) {
            self.redact_json_value_at_depth(&mut value, depth + 1);
            let base = self.redact_output_text_at_depth(key, depth + 1);
            let mut candidate = base.clone();
            let mut suffix = 2_u64;
            while redacted.contains_key(&candidate) {
                candidate = format!("{base}#{suffix}");
                suffix = suffix.saturating_add(1);
            }
            redacted.insert(candidate, value);
        }
        *object = redacted;
    }

    /// Redact a final output string that may itself be a JSON document. JSON
    /// strings nested inside that document are handled recursively because
    /// process output can legitimately carry structured output inside a string
    /// field.
    #[must_use]
    pub(crate) fn redact_output_text(&self, text: impl Into<String>) -> String {
        self.redact_output_text_at_depth(text.into(), 0)
    }

    fn redact_output_text_at_depth(&self, text: String, depth: usize) -> String {
        if self.inner.secret_env.is_empty() {
            return text;
        }
        if depth >= MAX_STRUCTURED_REDACTION_DEPTH {
            return REDACTED_CREDENTIAL.to_owned();
        }
        if !matches!(
            text.trim_start().as_bytes().first(),
            Some(b'{') | Some(b'[') | Some(b'"')
        ) {
            return self.redact_text(text);
        }
        match serde_json::from_str::<JsonValue>(&text) {
            Ok(mut value) => {
                self.redact_json_value_at_depth(&mut value, depth);
                // `JsonValue` serialization is infallible in practice. Keep this
                // boundary fail-closed nevertheless: falling back to the encoded
                // source text would recreate the escape bypass this branch exists
                // to prevent.
                match serde_json::to_string(&value) {
                    Ok(serialized) => serialized,
                    Err(_) => REDACTED_CREDENTIAL.to_owned(),
                }
            }
            Err(_) => self.redact_text(text),
        }
    }

    #[must_use]
    /// Redact captured output without trusting its wire representation. Valid
    /// JSON is decoded and redacted structurally before it is serialized again;
    /// all other output is treated as text. This keeps every process-backed
    /// adapter from having to reimplement the same escape-safe boundary.
    pub fn redact_bytes_to_string(&self, bytes: Vec<u8>, limit_bytes: usize) -> String {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let redacted = self.redact_output_text(text);
        crate::bytes::truncate_utf8_bytes(&redacted, limit_bytes)
    }
}

#[derive(Debug, Error)]
pub enum CredentialDeliveryError {
    #[error("the credential is empty")]
    EmptySecret,
    #[error("invalid credential delivery env var '{name}'")]
    InvalidEnvName { name: String },
    #[error("credential process-env delivery is not supported across the '{boundary}' boundary")]
    ProcessEnvBoundaryUnsupported { boundary: String },
    #[error(
        "credential delivery environment variable '{name}' collides with non-secret process environment"
    )]
    EnvironmentCollision { name: String },
}

/// Build the non-secret observation that records a local per-run credential
/// provision on the sealed receipt. It carries no secret material: only the
/// provider, profile, scoped credential reference, and a hash of the opaque
/// material ref. The timestamp is captured at observation time because local
/// credential provision is a live trust boundary, not a fixture surface.
fn build_local_provision_observation(
    provider: &str,
    auth_mode: &str,
    material_ref: &str,
) -> CredentialDeliveryObservation {
    let material_ref_hash = hash_material_ref(material_ref);
    let material_ref_id = sha256_hex(material_ref.as_bytes());
    CredentialDeliveryObservation {
        schema: runx_contracts::CredentialDeliveryObservationSchema::V1,
        observation_id: format!("local-credential-delivery/{material_ref_id}").into(),
        request_id: format!("local-credential-provision/{material_ref_id}").into(),
        response_id: None,
        status: CredentialDeliveryObservationStatus::Delivered,
        harness_ref: Reference::with_uri(
            ReferenceType::Harness,
            "runx:harness:local-credential-provision",
        ),
        host_ref: Some(Reference::with_uri(
            ReferenceType::Host,
            "runx:host:local-cli",
        )),
        profile_id: format!("{provider}-{auth_mode}").into(),
        provider: provider.into(),
        purpose: CredentialDeliveryPurpose::ProviderApi,
        delivery_mode: Some(CredentialDeliveryMode::ProcessEnv),
        credential_refs: vec![Reference {
            reference_type: ReferenceType::Credential,
            uri: format!("runx:credential:local:{material_ref_id}").into(),
            provider: Some(provider.to_owned().into()),
            locator: None,
            label: None,
            observed_at: None,
            proof_kind: Some(ProofKind::CredentialResolution),
        }],
        material_ref_hash: Some(material_ref_hash.into()),
        delivered_roles: vec![runx_contracts::CredentialMaterialRole::ApiKey],
        redaction_refs: None,
        observed_at: crate::time::now_iso8601().into(),
    }
}

fn hash_material_ref(material_ref: &str) -> String {
    sha256_prefixed(material_ref.as_bytes())
}

fn validate_env_name(name: &str) -> Result<(), CredentialDeliveryError> {
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_uppercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(CredentialDeliveryError::InvalidEnvName {
            name: name.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_free_output_preserves_exact_bytes() {
        let output = b"{\"message\":\"hello\"}\n".to_vec();

        assert_eq!(
            CredentialDelivery::none().redact_bytes_to_string(output, 64 * 1024),
            "{\"message\":\"hello\"}\n"
        );
    }

    #[test]
    fn captured_json_uses_structured_credential_redaction() -> Result<(), Box<dyn std::error::Error>>
    {
        const SECRET: &str = "credential-redaction-sentinel-\"quoted\\slash\ncontrol";
        const MARKER: &str = "credential-redaction-sentinel";
        let delivery = CredentialDelivery::process_env(
            "example",
            "api_key",
            "EXAMPLE_TOKEN",
            "local:example:test",
            SECRET,
        )?;
        let document = JsonValue::Object(JsonObject::from([
            (SECRET.to_owned(), JsonValue::String(SECRET.to_owned())),
            (
                "nested".to_owned(),
                JsonValue::Array(vec![JsonValue::String(SECRET.to_owned())]),
            ),
        ]));
        let encoded = serde_json::to_vec(&JsonValue::Object(JsonObject::from([
            ("document".to_owned(), document.clone()),
            (
                "embedded".to_owned(),
                JsonValue::String(serde_json::to_string(&document)?),
            ),
        ])))?;

        let output = delivery.redact_bytes_to_string(encoded, 64 * 1024);
        let decoded = serde_json::from_str::<JsonValue>(&output)?;

        assert!(output.contains(REDACTED_CREDENTIAL));
        assert!(!output.contains(MARKER));
        assert!(!format!("{decoded:?}").contains(MARKER));
        assert_eq!(delivery.redact_output_text("1e3"), "1e3");
        Ok(())
    }

    #[test]
    fn a_delivery_refuses_an_empty_secret_or_a_bad_variable_name() {
        assert!(matches!(
            CredentialDelivery::process_env(
                "github",
                "api_key",
                "GITHUB_TOKEN",
                "local:github",
                " "
            ),
            Err(CredentialDeliveryError::EmptySecret)
        ));
        assert!(matches!(
            CredentialDelivery::process_env(
                "github",
                "api_key",
                "github-token",
                "local:github",
                "x"
            ),
            Err(CredentialDeliveryError::InvalidEnvName { .. })
        ));
    }

    #[test]
    fn a_delivery_never_prints_its_secret() -> Result<(), CredentialDeliveryError> {
        let delivery = CredentialDelivery::process_env(
            "github",
            "api_key",
            "GITHUB_TOKEN",
            "local:github:main",
            "ghp_secret_value",
        )?;
        assert_eq!(
            delivery.secret_env().get("GITHUB_TOKEN"),
            Some("ghp_secret_value")
        );
        assert!(!format!("{delivery:?}").contains("ghp_secret_value"));
        Ok(())
    }

    #[test]
    fn credential_delivery_clones_share_one_zeroizing_secret_owner()
    -> Result<(), CredentialDeliveryError> {
        let delivery = CredentialDelivery::process_env(
            "github",
            "api_key",
            "GITHUB_TOKEN",
            "local:github:shared",
            "ghp_secret_value",
        )?;
        let clone = delivery.clone();

        assert!(Arc::ptr_eq(&delivery.inner, &clone.inner));
        assert_eq!(
            clone.secret_env().get("GITHUB_TOKEN"),
            Some("ghp_secret_value")
        );
        Ok(())
    }

    #[test]
    fn each_field_of_a_multi_field_credential_is_redacted_on_its_own() -> Result<(), String> {
        let delivery = CredentialDelivery::process_env(
            "example",
            "oauth1_user",
            "EXAMPLE_USER_AUTH",
            "local:example:user",
            r#"{"consumer_key":"ck-field-sentinel","consumer_secret":"cs-field-sentinel"}"#,
        )
        .map_err(|error| error.to_string())?;
        let printed =
            delivery.redact_text(r#"{"value":"cs-field-sentinel"} and ck-field-sentinel"#);
        assert!(!printed.contains("cs-field-sentinel"), "{printed}");
        assert!(!printed.contains("ck-field-sentinel"), "{printed}");
        Ok(())
    }

    #[test]
    fn local_credential_observation_marks_credential_resolution_proof() -> Result<(), String> {
        let delivery = CredentialDelivery::process_env(
            "github",
            "api_key",
            "GITHUB_TOKEN",
            "local:github:grant_1",
            "ghp_secret_value",
        )
        .map_err(|error| error.to_string())?;
        let refs = delivery
            .credential_refs()
            .ok_or_else(|| "expected credential refs".to_owned())?;

        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].reference_type.as_str(), "credential");
        assert_eq!(refs[0].provider.as_deref(), Some("github"));
        assert_eq!(refs[0].proof_kind, Some(ProofKind::CredentialResolution));
        let observation = delivery
            .public_observation()
            .ok_or_else(|| "expected a public observation".to_owned())?;
        let serialized = serde_json::to_string(observation).map_err(|error| error.to_string())?;
        assert!(!serialized.contains("ghp_secret_value"));
        Ok(())
    }
}
