mod error;
#[cfg(feature = "catalog")]
mod external_receipt;
mod metadata;
mod provider_effect;
mod provider_permission;
mod registry;
mod state;
mod types;

pub use error::RuntimeEffectError;
#[cfg(feature = "catalog")]
pub use external_receipt::{
    EXTERNAL_RECEIPT_EFFECT_FAMILY, EXTERNAL_RECEIPT_VERIFY_TOOL, ExternalReceiptEffect,
};
pub(crate) use metadata::effect_verification_refs;
pub use metadata::{EFFECT_VERIFICATION_REFS_METADATA, insert_effect_verification_ref};
pub use provider_effect::{
    ProviderAcknowledgementEvidence, ProviderApprovalEvidence, ProviderEffectAcknowledged,
    ProviderEffectAmount, ProviderEffectAttempt, ProviderEffectAuthority, ProviderEffectClass,
    ProviderEffectError, ProviderEffectFinality, ProviderEffectIntent, ProviderEffectIntentInput,
    ProviderEffectReadback, ProviderEffectReadbackEvidence, ProviderEffectResolved,
    ProviderEffectUnknown,
};
pub use provider_permission::{
    ASSISTANT_CONFIDENTIAL_TERMS_ENV, ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV,
    NOTIFICATION_AUTHORITY_ID_ENV, NOTIFICATION_SOURCE_SET_DIGEST_ENV, NotificationAuthorityError,
    NotificationAuthorityGrant, NotificationAuthorityGrantSpec, NotificationAuthorityStatus,
    PROVIDER_MUTATE_TOOL, PROVIDER_PERMISSION_EFFECT_FAMILY, PROVIDER_PERMISSION_GRANT_ID_ENV,
    PROVIDER_PERMISSION_GRANTED_SCOPES_ENV, PROVIDER_PERMISSION_PAID_EXTERNAL_JOB_AUTHORITY_ENV,
    PROVIDER_PERMISSION_PRINCIPAL_REF_ENV, PROVIDER_PERMISSION_TRANSPORT_ENV, PROVIDER_READ_TOOL,
    ProviderPermissionAdmission, ProviderPermissionEffect, ProviderScopeTransportError,
    assistant_text_is_safe, decode_provider_scopes_env, encode_provider_scopes_env,
    install_notification_authority, notification_authority_status,
    notification_intent_has_reservation, revoke_notification_authority,
};
#[cfg(feature = "catalog")]
pub use provider_permission::{
    LocalProviderTransportReadiness, ProviderTransportPreference,
    preflight_local_provider_transport, resolve_provider_transport_preference,
};
pub use registry::RuntimeEffectRegistry;
pub use state::{EffectAdmission, EffectReplay};
pub use types::{
    EffectOutputRequest, EffectPreparationOutcome, EffectReceiptRequest, EffectReplayOutputRequest,
    EffectReplayReceiptRequest, EffectStepRequest, EffectToolOutput, EffectToolRequest,
    ResolvedEffectTarget, RuntimeEffect,
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use runx_contracts::{AuthorityVerb, JsonObject, JsonValue, Reference};
    use runx_core::state_machine::AuthorityAdmissionWitness;
    use runx_parser::GraphStep;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::adapter::InvocationOutput;

    #[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
    #[serde(deny_unknown_fields)]
    struct DeployInput {
        resource: String,
    }

    impl crate::CapabilityInput for DeployInput {}

    static DEPLOY_CAPABILITY: crate::TypedCapability<DeployInput> =
        crate::TypedCapability::new(crate::CapabilityDefinition {
            id: "deploy.inspect",
            owner: "mock-deploy",
            summary: "Inspect a deployment through the mock effect boundary.",
            scopes: &["deploy.read"],
            effect: crate::CapabilityEffect::Read,
            approval: crate::CapabilityApproval::None,
            artifacts: crate::CapabilityArtifacts::Named {
                output: "deployment",
                packet: "mock.deployment.v1",
            },
            fields: &[crate::CapabilityField {
                name: "resource",
                description: "Resource to inspect.",
            }],
        });
    static EFFECT_CAPABILITIES: &[&dyn crate::CapabilityContract] = &[&DEPLOY_CAPABILITY];

    #[cfg(feature = "catalog")]
    #[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
    #[serde(deny_unknown_fields)]
    struct EmptyInput {}

    #[cfg(feature = "catalog")]
    impl crate::CapabilityInput for EmptyInput {}

    #[cfg(feature = "catalog")]
    static COLLIDING_CAPABILITY: crate::TypedCapability<EmptyInput> =
        crate::TypedCapability::new(crate::CapabilityDefinition {
            id: "fs.read",
            owner: "mock-deploy",
            summary: "Invalid collision fixture.",
            scopes: &[],
            effect: crate::CapabilityEffect::Read,
            approval: crate::CapabilityApproval::None,
            artifacts: crate::CapabilityArtifacts::None,
            fields: &[],
        });
    #[cfg(feature = "catalog")]
    static COLLIDING_CAPABILITIES: &[&dyn crate::CapabilityContract] = &[&COLLIDING_CAPABILITY];

    struct MockEffect;

    impl RuntimeEffect for MockEffect {
        fn family(&self) -> &'static str {
            "deploy"
        }

        fn capabilities(&self) -> &'static [&'static dyn crate::CapabilityContract] {
            EFFECT_CAPABILITIES
        }

        fn matches_target(&self, request: EffectStepRequest<'_>) -> bool {
            request.target.tool_ref == Some("deploy.inspect")
        }

        fn admit(
            &self,
            request: EffectStepRequest<'_>,
        ) -> Result<Option<EffectAdmission>, RuntimeEffectError> {
            let _ = request;
            Ok(Some(EffectAdmission::new(
                "deploy",
                AuthorityVerb::Write,
                AuthorityAdmissionWitness {
                    verb: AuthorityVerb::Write,
                    parent_term_id: "parent".to_owned(),
                    child_term_id: "child".to_owned(),
                    idempotency_key: Some("deploy-key".to_owned()),
                    capability_ref: None,
                },
                (),
            )))
        }

        #[cfg(feature = "catalog")]
        fn invoke_tool(
            &self,
            request: EffectToolRequest<'_>,
        ) -> Option<Result<runx_contracts::JsonValue, crate::RuntimeError>> {
            (request.tool_ref == "deploy.inspect")
                .then(|| Ok(runx_contracts::JsonValue::Object(JsonObject::new())))
        }
    }

    #[test]
    fn registry_dispatches_resolved_effect_target() {
        let registry = valid_registry(MockEffect);
        let mut step = test_step();
        step.tool = Some("deploy.inspect".to_owned());
        let inputs = JsonObject::new();
        let env = BTreeMap::new();
        let result = registry.admit(EffectStepRequest {
            step: &step,
            target: ResolvedEffectTarget {
                skill_name: None,
                tool_ref: step.tool.as_deref(),
            },
            inputs: &inputs,
            env: &env,
            graph_dir: Path::new("."),
        });
        assert!(
            matches!(
                &result,
                Ok(Some(admission))
                    if admission.family() == "deploy" && admission.verb() == AuthorityVerb::Write
            ),
            "unexpected admission result: {result:?}"
        );
    }

    #[test]
    fn registry_rejects_ambiguous_resolved_effect_ownership() {
        struct ConflictingTargetEffect;

        impl RuntimeEffect for ConflictingTargetEffect {
            fn family(&self) -> &'static str {
                "conflicting-target"
            }

            fn matches_target(&self, request: EffectStepRequest<'_>) -> bool {
                request.target.tool_ref == Some("deploy.inspect")
            }
        }

        let mut registry = valid_registry(MockEffect);
        assert!(registry.register_effect(ConflictingTargetEffect).is_ok());
        let mut step = test_step();
        step.tool = Some("deploy.inspect".to_owned());
        let inputs = JsonObject::new();
        let env = BTreeMap::new();

        let result = registry.admit(EffectStepRequest {
            step: &step,
            target: ResolvedEffectTarget {
                skill_name: None,
                tool_ref: step.tool.as_deref(),
            },
            inputs: &inputs,
            env: &env,
            graph_dir: Path::new("."),
        });

        assert!(
            matches!(
                result,
                Err(RuntimeEffectError::InvalidMetadata { ref message, .. })
                    if message.contains("deploy") && message.contains("conflicting-target")
            ),
            "unexpected ambiguous-owner result: {result:?}"
        );
    }

    #[test]
    fn registry_rejects_declared_effect_that_does_not_admit() {
        struct NonAdmittingEffect;

        impl RuntimeEffect for NonAdmittingEffect {
            fn family(&self) -> &'static str {
                "observe"
            }

            fn matches_target(&self, request: EffectStepRequest<'_>) -> bool {
                request.target.tool_ref == Some("observe.inspect")
            }
        }

        let registry = valid_registry(NonAdmittingEffect);
        let mut step = test_step();
        step.tool = Some("observe.inspect".to_owned());
        let inputs = JsonObject::new();
        let env = BTreeMap::new();

        let result = registry.admit(EffectStepRequest {
            step: &step,
            target: ResolvedEffectTarget {
                skill_name: None,
                tool_ref: step.tool.as_deref(),
            },
            inputs: &inputs,
            env: &env,
            graph_dir: Path::new("."),
        });

        assert!(
            matches!(
                result,
                Err(RuntimeEffectError::InvalidMetadata { ref family, ref message })
                    if family == "observe" && message.contains("did not provide an admissible effect contract")
            ),
            "unexpected non-admission result: {result:?}"
        );
    }

    #[test]
    fn registry_rejects_duplicate_effect_tool_ownership() {
        struct ConflictingEffect;

        impl RuntimeEffect for ConflictingEffect {
            fn family(&self) -> &'static str {
                "conflicting-deploy"
            }

            fn capabilities(&self) -> &'static [&'static dyn crate::CapabilityContract] {
                EFFECT_CAPABILITIES
            }
        }

        let mut registry = valid_registry(MockEffect);
        let result = registry.register_effect(ConflictingEffect);
        assert!(
            matches!(
                result,
                Err(RuntimeEffectError::InvalidMetadata { ref family, ref message })
                    if family == "conflicting-deploy"
                        && message.contains("deploy.inspect")
                        && message.contains("deploy")
            ),
            "unexpected duplicate-tool result: {result:?}"
        );
    }

    #[cfg(feature = "catalog")]
    #[test]
    fn registry_rejects_effect_tool_collision_with_runtime_catalog() {
        struct CollidingEffect;

        impl RuntimeEffect for CollidingEffect {
            fn family(&self) -> &'static str {
                "colliding-deploy"
            }

            fn capabilities(&self) -> &'static [&'static dyn crate::CapabilityContract] {
                COLLIDING_CAPABILITIES
            }
        }

        let result = RuntimeEffectRegistry::with_effect(CollidingEffect);
        assert!(
            matches!(
                result,
                Err(RuntimeEffectError::InvalidMetadata { ref family, ref message })
                    if family == "colliding-deploy"
                        && message.contains("fs.read")
                        && message.contains("runtime-owned")
            ),
            "unexpected core-collision result: {result:?}"
        );
    }

    #[cfg(feature = "catalog")]
    #[test]
    fn effect_tools_are_discoverable_only_through_their_declared_source() {
        let registry = valid_registry(MockEffect);
        let matching = crate::search_tools_with_effects(
            &crate::ToolSearchOptions {
                query: "deploy".to_owned(),
                source: Some("mock-deploy".to_owned()),
                limit: 20,
                fixture_catalog_enabled: false,
            },
            &registry,
        );
        assert_eq!(matching.results.len(), 1);
        assert_eq!(matching.results[0].name, "deploy.inspect");
        assert_eq!(matching.results[0].source, "mock-deploy");

        let runtime_only = crate::search_tools_with_effects(
            &crate::ToolSearchOptions {
                query: "deploy".to_owned(),
                source: Some("runx-runtime".to_owned()),
                limit: 20,
                fixture_catalog_enabled: false,
            },
            &registry,
        );
        assert!(runtime_only.results.is_empty());
    }

    #[test]
    fn registry_rejects_missing_effect_family_after_admission() {
        let registry = RuntimeEffectRegistry::empty();
        let step = test_step();
        let admission = EffectAdmission::new(
            "absent",
            AuthorityVerb::Write,
            AuthorityAdmissionWitness {
                verb: AuthorityVerb::Write,
                parent_term_id: "parent".to_owned(),
                child_term_id: "child".to_owned(),
                idempotency_key: None,
                capability_ref: None,
            },
            (),
        );
        let claim = JsonObject::new();
        let mut output = InvocationOutput::runtime_success(JsonValue::Null, 0, JsonObject::new());

        let result = registry.prepare_output(EffectOutputRequest {
            step: &step,
            admission: &admission,
            claim: &claim,
            output: &mut output,
        });

        assert!(
            matches!(result, Err(RuntimeEffectError::MissingFamily { ref family }) if family == "absent"),
            "unexpected missing-family result: {result:?}"
        );
    }

    #[test]
    fn verification_refs_round_trip_through_metadata() {
        let mut metadata = JsonObject::new();
        let reference = Reference::runx(runx_contracts::ReferenceType::Verification, "proof:1");
        let insert = insert_effect_verification_ref(&mut metadata, reference.clone());
        assert!(insert.is_ok(), "unexpected insert result: {insert:?}");
        assert_eq!(effect_verification_refs(&metadata), Ok(vec![reference]));
    }

    fn valid_registry<T>(effect: T) -> RuntimeEffectRegistry
    where
        T: RuntimeEffect + 'static,
    {
        let result = RuntimeEffectRegistry::with_effect(effect);
        assert!(
            result.is_ok(),
            "effect fixture metadata must be valid: {result:?}"
        );
        result.unwrap_or_default()
    }

    fn test_step() -> GraphStep {
        GraphStep {
            id: "ship".to_owned(),
            label: None,
            skill: None,
            tool: None,
            run: None,
            artifacts: None,
            outputs: None,
            runner: None,
            inputs: JsonObject::new(),
            context: BTreeMap::new(),
            context_edges: Vec::new(),
            context_skills: Vec::new(),
            scopes: Vec::new(),
            allowed_tools: None,
            retry: None,
            policy: None,
            fanout_group: None,
            when: None,
            idempotency_key: None,
            mint_authority: None,
            requested_scope_from: None,
        }
    }
}
