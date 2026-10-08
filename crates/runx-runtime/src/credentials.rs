// Module rationale: credential delivery is one secret-handling trust surface; secret
// string/env types, redaction, material resolution, and the delivery boundary stay colocated so the
// "secrets never leak" review happens against the whole module at once.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use base64::Engine as _;
use runx_contracts::policy_proof::CredentialEnvelope;
use runx_contracts::{
    CredentialDeliveryMode, CredentialDeliveryObservation, CredentialDeliveryObservationStatus,
    CredentialDeliveryPurpose, CredentialEnvelopeKind, JsonObject, JsonValue, ProofKind, Reference,
    ReferenceType, sha256_hex, sha256_prefixed,
};
use runx_core::policy::CredentialBindingDecision;
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::Zeroize;

const REDACTED_CREDENTIAL: &str = "[redacted-credential]";
const MAX_STRUCTURED_REDACTION_DEPTH: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialDeliveryProfile {
    provider: String,
    auth_mode: String,
    env_bindings: Vec<CredentialEnvBinding>,
}

impl CredentialDeliveryProfile {
    pub fn env_token(
        provider: impl Into<String>,
        auth_mode: impl Into<String>,
        env_var: impl Into<String>,
    ) -> Result<Self, CredentialDeliveryError> {
        let env_var = env_var.into();
        validate_env_name(&env_var)?;
        Ok(Self {
            provider: provider.into(),
            auth_mode: auth_mode.into(),
            env_bindings: vec![CredentialEnvBinding {
                role: CredentialMaterialRole::ApiKey,
                env_var,
                required: true,
            }],
        })
    }

    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    #[must_use]
    pub fn auth_mode(&self) -> &str {
        &self.auth_mode
    }

    pub fn from_contract_profile(
        profile: &runx_contracts::CredentialDeliveryProfile,
    ) -> Result<Self, CredentialDeliveryError> {
        if profile.delivery_mode != runx_contracts::CredentialDeliveryMode::ProcessEnv {
            return Err(CredentialDeliveryError::UnsupportedDeliveryMode {
                mode: format!("{:?}", profile.delivery_mode),
            });
        }
        let mut env_bindings = Vec::with_capacity(profile.env_bindings.len());
        for binding in &profile.env_bindings {
            let role = CredentialMaterialRole::from_contract_role(binding.role.clone());
            validate_env_name(&binding.env_var)?;
            env_bindings.push(CredentialEnvBinding {
                role,
                env_var: binding.env_var.clone(),
                required: binding.required,
            });
        }
        Ok(Self {
            provider: profile.provider.to_string(),
            auth_mode: profile.auth_mode.to_string(),
            env_bindings,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CredentialEnvBinding {
    role: CredentialMaterialRole,
    env_var: String,
    required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CredentialMaterialRole {
    PersonalToken,
    ApiKey,
    ClientSecret,
    SessionToken,
}

impl CredentialMaterialRole {
    const fn label(self) -> &'static str {
        match self {
            Self::PersonalToken => "personal_token",
            Self::ApiKey => "api_key",
            Self::ClientSecret => "client_secret",
            Self::SessionToken => "session_token",
        }
    }

    fn from_contract_role(role: runx_contracts::CredentialMaterialRole) -> Self {
        match role {
            runx_contracts::CredentialMaterialRole::PersonalToken => Self::PersonalToken,
            runx_contracts::CredentialMaterialRole::ApiKey => Self::ApiKey,
            runx_contracts::CredentialMaterialRole::ClientSecret => Self::ClientSecret,
            runx_contracts::CredentialMaterialRole::SessionToken => Self::SessionToken,
        }
    }
}

pub trait MaterialResolver {
    fn resolve_material(
        &self,
        material_ref: &str,
    ) -> Result<ResolvedCredentialMaterial, CredentialDeliveryError>;
}

pub struct CredentialResolutionRequest<'a> {
    pub decision: &'a CredentialBindingDecision,
    pub credential: &'a CredentialEnvelope,
    pub profile: &'a CredentialDeliveryProfile,
    /// The non-secret observation recording this delivery. Required so a resolved
    /// secret can never be delivered without its audit record on the receipt.
    pub observation: CredentialDeliveryObservation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialResolution {
    delivery: CredentialDelivery,
}

impl CredentialResolution {
    #[must_use]
    pub fn into_delivery(self) -> CredentialDelivery {
        self.delivery
    }
}

pub trait CredentialSupervisor {
    fn resolve(
        &self,
        request: CredentialResolutionRequest<'_>,
    ) -> Result<CredentialResolution, CredentialDeliveryError>;
}

pub struct MaterialCredentialSupervisor<'a, R> {
    resolver: &'a R,
}

impl<'a, R> MaterialCredentialSupervisor<'a, R>
where
    R: MaterialResolver,
{
    #[must_use]
    pub const fn new(resolver: &'a R) -> Self {
        Self { resolver }
    }
}

impl<R> CredentialSupervisor for MaterialCredentialSupervisor<'_, R>
where
    R: MaterialResolver,
{
    fn resolve(
        &self,
        request: CredentialResolutionRequest<'_>,
    ) -> Result<CredentialResolution, CredentialDeliveryError> {
        require_allowed_binding(request.decision)?;
        if request.credential.provider != request.profile.provider {
            return Err(CredentialDeliveryError::ProviderMismatch {
                credential_provider: request.credential.provider.to_string(),
                profile_provider: request.profile.provider.clone(),
            });
        }
        let material = self
            .resolver
            .resolve_material(&request.credential.material_ref)?;
        if material.material_ref != request.credential.material_ref {
            return Err(CredentialDeliveryError::MaterialRefMismatch {
                expected_hash: hash_material_ref(&request.credential.material_ref),
                actual_hash: hash_material_ref(&material.material_ref),
            });
        }
        Ok(CredentialResolution {
            delivery: CredentialDelivery::from_parts(
                apply_profile(request.profile, &material)?,
                Some(request.observation),
            ),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryMaterialResolver {
    materials: BTreeMap<String, ResolvedCredentialMaterial>,
}

impl InMemoryMaterialResolver {
    #[must_use]
    pub fn with_material(
        material_ref: impl Into<String>,
        material: ResolvedCredentialMaterial,
    ) -> Self {
        let mut materials = BTreeMap::new();
        materials.insert(material_ref.into(), material);
        Self { materials }
    }
}

impl MaterialResolver for InMemoryMaterialResolver {
    fn resolve_material(
        &self,
        material_ref: &str,
    ) -> Result<ResolvedCredentialMaterial, CredentialDeliveryError> {
        self.materials.get(material_ref).cloned().ok_or_else(|| {
            CredentialDeliveryError::MaterialNotFound {
                material_ref_hash: hash_material_ref(material_ref),
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCredentialMaterial {
    material_ref: String,
    values: BTreeMap<CredentialMaterialRole, SecretString>,
}

impl ResolvedCredentialMaterial {
    #[must_use]
    pub fn api_key(material_ref: impl Into<String>, value: impl Into<String>) -> Self {
        Self::with_role(material_ref, CredentialMaterialRole::ApiKey, value)
    }

    #[must_use]
    pub fn with_role(
        material_ref: impl Into<String>,
        role: CredentialMaterialRole,
        value: impl Into<String>,
    ) -> Self {
        let mut values = BTreeMap::new();
        values.insert(role, SecretString::new(value));
        Self {
            material_ref: material_ref.into(),
            values,
        }
    }
}

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

    /// Build a delivery from a resolved, per-run local credential descriptor.
    ///
    /// This is the OSS local-delivery path: no network and no brokerage. The
    /// resolver may have loaded encrypted profile material or a declared
    /// workspace value. This derives a delivery profile, a credential envelope, and an
    /// allowed binding decision purely from the supplied descriptor, resolves
    /// the secret in-memory, and routes it through the same
    /// [`Self::from_allowed_binding`] seam so policy checks and redaction stay
    /// centralized. The secret value is held only for the lifetime of this run.
    pub fn from_local_descriptor(
        provider: impl Into<String>,
        auth_mode: impl Into<String>,
        env_var: impl Into<String>,
        material_ref: impl Into<String>,
        scopes: Vec<String>,
        secret: impl Into<String>,
    ) -> Result<Self, CredentialDeliveryError> {
        let provider = provider.into();
        let auth_mode = auth_mode.into();
        let material_ref = material_ref.into();

        // Captured before the values move into the envelope/resolver below, so
        // the run records a non-secret observation of the local provision.
        let observation = build_local_provision_observation(&provider, &auth_mode, &material_ref);

        let profile =
            CredentialDeliveryProfile::env_token(provider.clone(), auth_mode.clone(), env_var)?;
        let envelope = CredentialEnvelope {
            kind: CredentialEnvelopeKind::V1,
            grant_id: material_ref.clone().into(),
            provider: provider.into(),
            auth_mode: auth_mode.into(),
            material_kind: "api_key".into(),
            provider_reference: "local_per_run".into(),
            scopes: scopes.into_iter().map(Into::into).collect(),
            grant_reference: None,
            material_ref: material_ref.clone().into(),
        };
        let decision = CredentialBindingDecision::Allow {
            reasons: vec!["local per-run credential provision".to_owned()],
        };
        let resolver = InMemoryMaterialResolver::with_material(
            material_ref.clone(),
            ResolvedCredentialMaterial::api_key(material_ref, secret),
        );

        Self::from_allowed_binding(&decision, &envelope, &profile, &resolver, observation)
    }

    pub fn from_allowed_binding<R: MaterialResolver>(
        decision: &CredentialBindingDecision,
        credential: &CredentialEnvelope,
        profile: &CredentialDeliveryProfile,
        resolver: &R,
        observation: CredentialDeliveryObservation,
    ) -> Result<Self, CredentialDeliveryError> {
        MaterialCredentialSupervisor::new(resolver)
            .resolve(CredentialResolutionRequest {
                decision,
                credential,
                profile,
                observation,
            })
            .map(CredentialResolution::into_delivery)
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
    #[error("credential binding denied: {}", reasons.join("; "))]
    BindingDenied { reasons: Vec<String> },
    #[error(
        "credential provider '{credential_provider}' does not match delivery profile provider '{profile_provider}'"
    )]
    ProviderMismatch {
        credential_provider: String,
        profile_provider: String,
    },
    #[error("credential material with hash '{material_ref_hash}' was not found")]
    MaterialNotFound { material_ref_hash: String },
    #[error(
        "credential material ref hash mismatch: expected '{expected_hash}', got '{actual_hash}'"
    )]
    MaterialRefMismatch {
        expected_hash: String,
        actual_hash: String,
    },
    #[error("credential material is missing role '{role}'")]
    MissingRole { role: String },
    #[error("credential material for role '{role}' is empty")]
    EmptyMaterial { role: String },
    #[error("invalid credential delivery env var '{name}'")]
    InvalidEnvName { name: String },
    #[error("unsupported credential delivery mode '{mode}'")]
    UnsupportedDeliveryMode { mode: String },
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

fn require_allowed_binding(
    decision: &CredentialBindingDecision,
) -> Result<(), CredentialDeliveryError> {
    match decision {
        CredentialBindingDecision::Allow { .. } => Ok(()),
        CredentialBindingDecision::Deny { reasons } => {
            Err(CredentialDeliveryError::BindingDenied {
                reasons: reasons.clone(),
            })
        }
    }
}

fn apply_profile(
    profile: &CredentialDeliveryProfile,
    material: &ResolvedCredentialMaterial,
) -> Result<SecretEnv, CredentialDeliveryError> {
    let mut values = BTreeMap::new();
    for binding in &profile.env_bindings {
        let Some(secret) = material.values.get(&binding.role) else {
            if !binding.required {
                continue;
            }
            return Err(CredentialDeliveryError::MissingRole {
                role: binding.role.label().to_owned(),
            });
        };
        if secret.expose().trim().is_empty() {
            return Err(CredentialDeliveryError::EmptyMaterial {
                role: binding.role.label().to_owned(),
            });
        }
        values.insert(binding.env_var.clone(), secret.clone());
    }
    Ok(SecretEnv { values })
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
        let delivery = CredentialDelivery::from_local_descriptor(
            "example",
            "api_key",
            "EXAMPLE_TOKEN",
            "local:example:test",
            vec!["example:read".to_owned()],
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
    fn optional_env_binding_is_skipped_when_material_role_is_missing()
    -> Result<(), CredentialDeliveryError> {
        let profile = CredentialDeliveryProfile {
            provider: "github".to_owned(),
            auth_mode: "api_key".to_owned(),
            env_bindings: vec![CredentialEnvBinding {
                role: CredentialMaterialRole::ApiKey,
                env_var: "GITHUB_TOKEN".to_owned(),
                required: false,
            }],
        };
        let material = ResolvedCredentialMaterial {
            material_ref: "secret://github/main".to_owned(),
            values: BTreeMap::new(),
        };

        let env = apply_profile(&profile, &material)?;

        assert!(env.is_empty());
        Ok(())
    }

    #[test]
    fn required_env_binding_fails_when_material_role_is_missing() {
        let profile = CredentialDeliveryProfile {
            provider: "github".to_owned(),
            auth_mode: "api_key".to_owned(),
            env_bindings: vec![CredentialEnvBinding {
                role: CredentialMaterialRole::ApiKey,
                env_var: "GITHUB_TOKEN".to_owned(),
                required: true,
            }],
        };
        let material = ResolvedCredentialMaterial {
            material_ref: "secret://github/main".to_owned(),
            values: BTreeMap::new(),
        };

        let result = apply_profile(&profile, &material);

        assert!(matches!(
            result,
            Err(CredentialDeliveryError::MissingRole { role }) if role == "api_key"
        ));
    }

    #[test]
    fn delivery_profile_resolves_non_api_contract_role() -> Result<(), CredentialDeliveryError> {
        let contract_profile = runx_contracts::CredentialDeliveryProfile {
            schema: runx_contracts::CredentialDeliveryProfileSchema::V1,
            profile_id: "github-app".into(),
            provider: "github".into(),
            auth_mode: "app".into(),
            purpose: CredentialDeliveryPurpose::ProviderApi,
            delivery_mode: CredentialDeliveryMode::ProcessEnv,
            material_roles: vec![runx_contracts::CredentialMaterialRole::ClientSecret],
            env_bindings: vec![runx_contracts::CredentialDeliveryEnvBinding {
                role: runx_contracts::CredentialMaterialRole::ClientSecret,
                env_var: "GITHUB_CLIENT_SECRET".to_owned(),
                required: true,
            }],
            redaction_policy_ref: Reference::with_uri(
                ReferenceType::RedactionPolicy,
                "runx:redaction:credential",
            ),
        };
        let profile = CredentialDeliveryProfile::from_contract_profile(&contract_profile)?;
        let material = ResolvedCredentialMaterial::with_role(
            "secret://github/app",
            CredentialMaterialRole::ClientSecret,
            "client-secret-value",
        );

        let env = apply_profile(&profile, &material)?;

        assert_eq!(env.get("GITHUB_CLIENT_SECRET"), Some("client-secret-value"));
        Ok(())
    }

    #[test]
    fn credential_supervisor_resolves_allowed_binding_without_secret_debug_leak()
    -> Result<(), CredentialDeliveryError> {
        let material_ref = "secret://github/main";
        let resolver = InMemoryMaterialResolver::with_material(
            material_ref,
            ResolvedCredentialMaterial::api_key(material_ref, "ghp_secret_value"),
        );
        let profile = CredentialDeliveryProfile::env_token("github", "api_key", "GITHUB_TOKEN")?;
        let credential = CredentialEnvelope {
            kind: CredentialEnvelopeKind::V1,
            grant_id: "grant_1".into(),
            provider: "github".into(),
            auth_mode: "api_key".into(),
            material_kind: "api_key".into(),
            provider_reference: "local_per_run".into(),
            scopes: vec!["repo:read".into()],
            grant_reference: None,
            material_ref: material_ref.into(),
        };
        let decision = CredentialBindingDecision::Allow {
            reasons: vec!["unit-test".to_owned()],
        };

        let delivery = MaterialCredentialSupervisor::new(&resolver)
            .resolve(CredentialResolutionRequest {
                decision: &decision,
                credential: &credential,
                profile: &profile,
                observation: build_local_provision_observation("github", "api_key", material_ref),
            })?
            .into_delivery();

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
        let delivery = CredentialDelivery::from_local_descriptor(
            "github",
            "api_key",
            "GITHUB_TOKEN",
            "local:github:shared",
            vec!["repo:read".to_owned()],
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
        let delivery = CredentialDelivery::from_local_descriptor(
            "example",
            "oauth1_user",
            "EXAMPLE_USER_AUTH",
            "local:example:user",
            Vec::new(),
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
        let delivery = CredentialDelivery::from_local_descriptor(
            "github",
            "api_key",
            "GITHUB_TOKEN",
            "local:github:grant_1",
            vec!["repo:read".to_owned()],
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

    #[test]
    fn material_ref_errors_report_hashes_not_raw_refs() {
        let result = InMemoryMaterialResolver::default().resolve_material("secret://github/main");
        assert!(result.is_err(), "missing material must fail");
        let missing = match result {
            Err(error) => error,
            Ok(_) => return,
        };
        let message = missing.to_string();
        assert!(message.contains("sha256:"));
        assert!(!message.contains("secret://github/main"));

        let mismatch = CredentialDeliveryError::MaterialRefMismatch {
            expected_hash: hash_material_ref("secret://github/main"),
            actual_hash: hash_material_ref("secret://github/other"),
        };
        let message = mismatch.to_string();
        assert!(message.contains("sha256:"));
        assert!(!message.contains("secret://github/main"));
        assert!(!message.contains("secret://github/other"));
    }
}
