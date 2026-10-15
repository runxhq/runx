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

/// A process that drives a run its host named ends with that host, so a
/// host that dies leaves no driver behind and its successor can see that.
/// The runtime holds the run's driver lock while it drives.
pub(crate) fn end_with_host() {
    runx_runtime::interrupt::end_with_parent();
}

/// A process that drives a run to where it next waits stops gracefully on
/// `SIGTERM`: it admits no further step or call, a command it supervises is
/// terminated and recorded stopped, and the run stays where it stopped for
/// its next driver.
pub(crate) fn stop_on_terminate() -> Result<(), String> {
    runx_runtime::interrupt::stop_on_terminate()
        .map_err(|error| format!("failed to install the termination handler: {error}"))
}
