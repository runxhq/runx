#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::JsonObject;

use crate::RuntimeEffectRegistry;
use crate::credentials::CredentialDelivery;
use crate::paths::RUNX_CWD_ENV;
use crate::services::WorkspaceBounds;
use crate::tool_catalogs::native::{NativeInvocation, fixture_input};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Run one file tool's handler in `workspace` with `inputs` as a call gives them, defaults filled.
pub(super) fn invoke<I, O>(
    workspace: &Path,
    inputs: serde_json::Value,
    bounds: WorkspaceBounds,
    handler: fn(&NativeInvocation<'_, I>) -> Result<O, crate::RuntimeError>,
) -> Result<O, String>
where
    I: crate::CapabilityInput,
{
    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.to_string_lossy().into_owned(),
    )]);
    let inputs: JsonObject = serde_json::from_value(inputs).map_err(|error| error.to_string())?;
    let inputs = fixture_input::<I>(inputs).map_err(|error| error.to_string())?;
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    handler(&NativeInvocation {
        inputs: &inputs,
        observed_at: "2026-01-01T00:00:00Z",
        env: &env,
        skill_directory: workspace,
        credential_delivery: &delivery,
        local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
        effects: &effects,
        bounds,
        call: None,
    })
    .map_err(|error| error.to_string())
}

/// The digest a read returns and a write names.
pub(super) fn digest(text: &str) -> runx_contracts::Digest {
    runx_contracts::Digest::of(text.as_bytes())
}
