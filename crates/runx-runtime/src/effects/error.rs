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
    /// The call would spend more than is left of the run's money limit.
    #[error("effect family {family} refused a call over the run's money limit: {message}")]
    CostLimitExceeded { family: String, message: String },
    #[error("effect family {family} failed during {operation}: {message}")]
    Failed {
        family: String,
        operation: &'static str,
        message: String,
    },
}
