pub use runx_contracts::{
    ActAssignment, ActAssignmentActor, ActAssignmentHost, ActAssignmentHostKind,
    ActAssignmentIdempotency, BuildActAssignment, IntentKeyInput, derive_content_digest,
    derive_intent_key, derive_trigger_key,
};

#[must_use]
pub fn build_act_assignment(input: BuildActAssignment) -> ActAssignment {
    input.build()
}
