// Module rationale: the CLI assembles only provider-neutral runtime effects.
// Hosted products add private capabilities at the hosted execution boundary;
// the OSS binary never embeds live payment rails or settlement state.
use std::collections::BTreeMap;

use runx_runtime::{LocalOrchestrator, RuntimeEffectRegistry};

pub fn local_orchestrator(
    env: &BTreeMap<String, String>,
) -> Result<LocalOrchestrator, runx_runtime::RuntimeEffectError> {
    runtime_effect_registry()
        .map(|effects| LocalOrchestrator::with_effects_and_environment(effects, env.clone()))
}

pub fn runtime_effect_registry() -> Result<RuntimeEffectRegistry, runx_runtime::RuntimeEffectError>
{
    RuntimeEffectRegistry::local()
}
