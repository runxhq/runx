use std::any::Any;
use std::fmt;
use std::sync::Arc;

use runx_contracts::AuthorityVerb;
use runx_core::state_machine::AuthorityAdmissionWitness;

#[derive(Clone)]
pub struct EffectAdmission {
    family: &'static str,
    verb: AuthorityVerb,
    witness: AuthorityAdmissionWitness,
    context: Arc<dyn Any + Send + Sync>,
}

impl EffectAdmission {
    #[must_use]
    pub fn new<T>(
        family: &'static str,
        verb: AuthorityVerb,
        witness: AuthorityAdmissionWitness,
        context: T,
    ) -> Self
    where
        T: Any + Send + Sync + 'static,
    {
        Self {
            family,
            verb,
            witness,
            context: Arc::new(context),
        }
    }

    #[must_use]
    pub fn family(&self) -> &'static str {
        self.family
    }

    #[must_use]
    pub fn verb(&self) -> AuthorityVerb {
        self.verb.clone()
    }

    #[must_use]
    pub fn witness(&self) -> &AuthorityAdmissionWitness {
        &self.witness
    }

    #[must_use]
    pub fn context<T: Any>(&self) -> Option<&T> {
        self.context.as_ref().downcast_ref::<T>()
    }

    #[must_use]
    pub(crate) fn with_context<T>(self, context: T) -> Self
    where
        T: Any + Send + Sync + 'static,
    {
        Self {
            family: self.family,
            verb: self.verb,
            witness: self.witness,
            context: Arc::new(context),
        }
    }
}

impl fmt::Debug for EffectAdmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EffectAdmission")
            .field("family", &self.family)
            .field("verb", &self.verb)
            .field("witness", &self.witness)
            .finish_non_exhaustive()
    }
}
