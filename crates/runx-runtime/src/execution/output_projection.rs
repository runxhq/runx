use std::collections::BTreeSet;

use runx_contracts::JsonObject;
use runx_parser::SkillArtifactContract;

/// A step's declared result: its claim, and which outputs of it are
/// artifacts.
pub(crate) struct StepOutputProjection {
    pub(crate) outputs: JsonObject,
    /// The outputs that are artifacts, each held in its packet envelope.
    pub(crate) artifact_outputs: BTreeSet<String>,
}

/// Build the durable graph/receipt projection from one already-verified claim.
///
/// Raw adapter values and diagnostics never enter this boundary, so graph
/// state and receipt identity share one exact surface. `artifacts` is the
/// artifact contract the claim was projected under. A claim is never read for
/// references: what a receipt references is declared or admitted.
#[must_use]
pub(crate) fn project_step_claim(
    outputs: JsonObject,
    artifacts: Option<&SkillArtifactContract>,
) -> StepOutputProjection {
    let artifact_outputs = crate::output_contract::artifact_outputs(&outputs, artifacts);
    StepOutputProjection {
        outputs,
        artifact_outputs,
    }
}
