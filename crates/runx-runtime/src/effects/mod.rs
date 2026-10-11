mod error;
mod external_receipt;
mod registry;
mod state;
mod types;

pub use crate::kernel::custody::Settlement;
pub use error::RuntimeEffectError;
pub use external_receipt::{
    EXTERNAL_RECEIPT_EFFECT_FAMILY, EXTERNAL_RECEIPT_VERIFY_TOOL, ExternalReceiptEffect,
};
pub use registry::RuntimeEffectRegistry;
pub use state::EffectAdmission;
pub use types::{
    EffectOutputRequest, EffectPreparationOutcome, EffectReceiptRequest, EffectStepRequest,
    EffectToolOutput, EffectToolRequest, ResolvedEffectTarget, RuntimeEffect,
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use runx_contracts::{AuthorityVerb, JsonObject, JsonValue};
    use runx_core::state_machine::AuthorityAdmissionWitness;
    use runx_parser::{GraphStep, StepKind, ToolTarget};
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

    #[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
    #[serde(deny_unknown_fields)]
    struct EmptyInput {}

    impl crate::CapabilityInput for EmptyInput {}

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
        let step = test_step("deploy.inspect");
        let inputs = JsonObject::new();
        let env = BTreeMap::new();
        let result = registry.admit(EffectStepRequest {
            step: &step,
            target: ResolvedEffectTarget {
                tool_ref: step.catalog_tool(),
            },
            inputs: &inputs,
            env: &env,
            graph_dir: Path::new("."),
            run_id: "run-1",
            run_authority: None,
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
        let step = test_step("deploy.inspect");
        let inputs = JsonObject::new();
        let env = BTreeMap::new();

        let result = registry.admit(EffectStepRequest {
            step: &step,
            target: ResolvedEffectTarget {
                tool_ref: step.catalog_tool(),
            },
            inputs: &inputs,
            env: &env,
            graph_dir: Path::new("."),
            run_id: "run-1",
            run_authority: None,
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
        let step = test_step("observe.inspect");
        let inputs = JsonObject::new();
        let env = BTreeMap::new();

        let result = registry.admit(EffectStepRequest {
            step: &step,
            target: ResolvedEffectTarget {
                tool_ref: step.catalog_tool(),
            },
            inputs: &inputs,
            env: &env,
            graph_dir: Path::new("."),
            run_id: "run-1",
            run_authority: None,
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

    #[test]
    fn effect_tools_are_discoverable_only_through_their_declared_source() {
        let registry = valid_registry(MockEffect);
        let matching = crate::search_tools_with_effects(
            &crate::ToolSearchOptions {
                query: "deploy".to_owned(),
                source: Some("mock-deploy".to_owned()),
                limit: 20,
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
            },
            &registry,
        );
        assert!(runtime_only.results.is_empty());
    }

    #[test]
    fn registry_rejects_missing_effect_family_after_admission() {
        let registry = RuntimeEffectRegistry::empty();
        let step = test_step("absent.inspect");
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
        let mut output = InvocationOutput::runtime_success(JsonValue::Null, 0);

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

    fn test_step(tool: &str) -> GraphStep {
        GraphStep {
            id: "ship".to_owned(),
            label: None,
            kind: StepKind::Tool(ToolTarget::Catalog(tool.to_owned())),
            artifacts: None,
            outputs: None,
            inputs: JsonObject::new(),
            bindings: Vec::new(),
            act: None,
            context_skills: Vec::new(),
            scopes: Vec::new(),
            when: None,
            duplicates: None,
        }
    }
}
