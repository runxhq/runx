use runx_contracts::AuthorityVerb;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RuntimeEffectError {
    #[error("effect family {family} is already configured")]
    DuplicateFamily { family: String },
    #[error("effect family {family} is not configured")]
    MissingFamily { family: String },
    #[error("effect family {family} is invalid: {message}")]
    InvalidMetadata { family: String, message: String },
    #[error("effect family {family} rejected {verb:?}: {message}")]
    Denied {
        family: String,
        verb: AuthorityVerb,
        message: String,
    },
    /// A person declined the approval the call waited on.
    #[error("effect family {family}: a person declined: {message}")]
    Declined { family: String, message: String },
    /// An unsettled attempt of the act, of another intent, holds the call:
    /// it waits on its retry while that attempt, `plan_digest`, settles.
    #[error("effect family {family}: the call is held: {message}")]
    Held {
        family: String,
        plan_digest: String,
        idempotency_key: String,
        message: String,
    },
    /// The run's money limit doesn't cover the call: it would spend more
    /// than is left, its cost can't be counted, or it quotes a charge in a
    /// run that states no limit.
    #[error("effect family {family} refused a call for its cost: {message}")]
    CostLimitExceeded { family: String, message: String },
    #[error("effect family {family} failed during {operation}: {message}")]
    Failed {
        family: String,
        operation: &'static str,
        message: String,
    },
}
