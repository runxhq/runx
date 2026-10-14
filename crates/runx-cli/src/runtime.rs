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

/// A process that drives a named run holds the run's driver lock and ends
/// with the host that started it, so a host that dies leaves no driver
/// behind and its successor can see that.
pub(crate) fn drive(
    workspace: &runx_runtime::WorkspaceEnv,
    run_id: &str,
) -> Result<runx_runtime::run_record::DriverLock, String> {
    let lock = runx_runtime::run_record::DriverLock::acquire(workspace.paths(), run_id)
        .map_err(|error| error.to_string())?;
    runx_runtime::interrupt::end_with_parent();
    Ok(lock)
}
