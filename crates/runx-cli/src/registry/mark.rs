//! `runx registry mark check <file>`: a standalone mark checked by the same
//! rule a package's `mark.svg` passes, so a registry can admit an image (a
//! publisher's profile mark) on the parser's word and key it as connector
//! marks are keyed.

use std::collections::BTreeMap;
use std::path::Path;

use super::output::{self, RegistryEnvelope, RegistryPayload};
use super::{RegistryCliError, RegistryCliOutput, RegistryPlan, internal_error};

/// A mark the parser accepted: the SHA-256 (lowercase hex) of exactly the
/// bytes checked, the key `/v1/marks/<sha256>.svg` serves it under.
#[derive(serde::Serialize)]
pub(super) struct CheckedMark {
    pub(super) mark_sha256: String,
    pub(super) bytes: usize,
}

pub(super) fn run_mark_check(
    plan: RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let path = runx_runtime::RunxPaths::resolve(env, cwd).existing_input(&plan.subject);
    let bytes = std::fs::read(&path)
        .map_err(|error| internal_error(format!("{} cannot be read: {error}", path.display())))?;
    runx_parser::mark::validate_mark_svg(&bytes).map_err(internal_error)?;
    let mark = CheckedMark {
        mark_sha256: runx_contracts::sha256_hex(&bytes),
        bytes: bytes.len(),
    };
    let human = format!(
        "\n  registry mark check  ok\n  sha256  {}\n  bytes   {}\n\n",
        mark.mark_sha256, mark.bytes
    );
    output::write_output(
        plan.json,
        &RegistryEnvelope {
            status: "success",
            registry: RegistryPayload::MarkCheck { mark },
        },
        || human,
    )
}
