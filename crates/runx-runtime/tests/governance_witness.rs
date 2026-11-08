#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Conformance for the uniform-governance seal invariant.
//!
//! Every registered graph step is admitted centrally and its admission witness is
//! sealed in one central place (`run_registered_step`). So an admitted step records
//! which authority admitted the act, and an unadmitted step falls back to a
//! local-runtime witness rather than fabricating one. Because the seal is central
//! and step-type-agnostic, exercising one real graph proves the invariant for every
//! step type, so this is two focused end-to-end checks rather than one per type.

use std::path::Path;

use runx_contracts::AuthorityVerb;
use runx_core::state_machine::AuthorityAdmissionWitness;
use runx_runtime::adapters::process_tool::ProcessToolAdapter;
use runx_runtime::{
    EffectAdmission, EffectStepRequest, NoopHost, Runtime, RuntimeEffect, RuntimeEffectError,
    RuntimeEffectRegistry, RuntimeOptions,
};

fn hello_workflow() -> (std::path::PathBuf, runx_parser::Workflow) {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/hello-workflow")
        .canonicalize()
        .expect("hello-workflow example exists");
    let workflow =
        crate::support::method_workflow(&package, "default").expect("hello-workflow's workflow");
    (package, workflow)
}

/// An effect that admits every step, emitting a known authority witness so the test
/// can assert the runtime records exactly that authority.
struct AdmitEveryStep;

impl RuntimeEffect for AdmitEveryStep {
    fn family(&self) -> &'static str {
        "test-admit"
    }

    fn matches_target(&self, _request: EffectStepRequest<'_>) -> bool {
        true
    }

    fn admit(
        &self,
        _request: EffectStepRequest<'_>,
    ) -> Result<Option<EffectAdmission>, RuntimeEffectError> {
        Ok(Some(EffectAdmission::new(
            "test-admit",
            AuthorityVerb::Execute,
            AuthorityAdmissionWitness {
                verb: AuthorityVerb::Execute,
                parent_term_id: "parent-term".to_owned(),
                child_term_id: "child-term".to_owned(),
                idempotency_key: None,
                capability_ref: None,
            },
            (),
        )))
    }
}

fn options_with_effects(effects: RuntimeEffectRegistry) -> (tempfile::TempDir, RuntimeOptions) {
    let (root, run) = crate::support::scratch_run("run_governance_witness").expect("scratch run");
    let mut options = crate::support::signed_runtime_options(run).expect("signed runtime options");
    options.effects = effects;
    (root, options)
}

#[test]
fn admitted_step_records_authority_in_sealed_witness() {
    let effects = RuntimeEffectRegistry::with_effect(AdmitEveryStep);
    assert!(
        effects.is_ok(),
        "effect fixture metadata must be valid: {effects:?}"
    );
    let (_run, options) = options_with_effects(effects.unwrap_or_default());
    let runtime = Runtime::new(ProcessToolAdapter, options);
    let (package, workflow) = hello_workflow();
    let run = runtime
        .run_workflow_with_host(&package, workflow, &mut NoopHost)
        .expect("graph runs to completion");
    let authority = run.steps[0]
        .admission_witness
        .authority
        .as_ref()
        .expect("an admitted step must record its authority in the sealed witness");
    assert_eq!(authority.verb, AuthorityVerb::Execute);
    assert_eq!(authority.child_term_id, "child-term");
}

#[test]
fn unadmitted_step_records_local_runtime_witness() {
    let (_run, options) = options_with_effects(RuntimeEffectRegistry::empty());
    let runtime = Runtime::new(ProcessToolAdapter, options);
    let (package, workflow) = hello_workflow();
    let run = runtime
        .run_workflow_with_host(&package, workflow, &mut NoopHost)
        .expect("graph runs to completion");
    assert!(
        run.steps[0].admission_witness.authority.is_none(),
        "a step with no admitted authority must not fabricate an authority witness"
    );
}
