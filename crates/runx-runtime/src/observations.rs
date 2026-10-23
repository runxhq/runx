//! What the runtime observed about one invocation, for its receipt.
//!
//! An invocation's result is its status, value and diagnostics. Beside it the
//! runtime records what it observed itself: where the invocation ran, the
//! credentials it delivered, the contract checks it ran and the verification
//! of its effect. Only the runtime writes these.
//! An adapter answers with a value; it cannot state an observation, so a
//! receipt seals only what the runtime earned.

use std::collections::BTreeMap;

use runx_contracts::{CredentialDeliveryObservation, ExecutionBoundaryKind, Reference};

use crate::RuntimeError;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Observations {
    pub(crate) boundary: Option<ExecutionBoundaryKind>,
    pub(crate) credentials: Vec<CredentialDeliveryObservation>,
    pub(crate) contract: ContractChecks,
    pub(crate) effect_verification_refs: Vec<Reference>,
}

impl Observations {
    /// Where the invocation ran.
    #[must_use]
    pub fn boundary(&self) -> Option<ExecutionBoundaryKind> {
        self.boundary
    }

    /// The credentials the runtime delivered to the invocation.
    #[must_use]
    pub fn credentials(&self) -> &[CredentialDeliveryObservation] {
        &self.credentials
    }

    /// Record one credential delivery. Recording the same observation again
    /// is a no-op; the same id with different evidence is refused.
    pub(crate) fn record_credential(
        &mut self,
        observation: &CredentialDeliveryObservation,
    ) -> Result<(), RuntimeError> {
        match self
            .credentials
            .iter()
            .find(|existing| existing.observation_id == observation.observation_id)
        {
            Some(existing) if existing == observation => Ok(()),
            Some(_) => Err(RuntimeError::ReceiptInvalid {
                message: format!(
                    "credential delivery observation {:?} was recorded with conflicting evidence",
                    observation.observation_id.as_str()
                ),
            }),
            None => {
                self.credentials.push(observation.clone());
                Ok(())
            }
        }
    }
}

/// The contract checks the runtime ran on an invocation's result, each with
/// the digest of what it checked against. Empty when the step declares no
/// output or packet contract.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ContractChecks {
    pub(crate) output_contract_digest: Option<String>,
    pub(crate) voice_profile_digest: Option<String>,
    /// Each packet output, by name, and the packet schema it satisfied.
    pub(crate) packet_schemas: BTreeMap<String, PacketCheck>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PacketCheck {
    pub(crate) packet: runx_contracts::PacketId,
    pub(crate) schema_digest: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::CredentialDelivery;

    #[test]
    fn a_credential_is_recorded_once_and_conflicting_evidence_is_refused()
    -> Result<(), Box<dyn std::error::Error>> {
        let delivery = CredentialDelivery::process_env(
            "example",
            "api_key",
            "EXAMPLE_TOKEN",
            "local:example:test",
            "secret",
        )?;
        let observation = delivery
            .public_observation()
            .cloned()
            .ok_or("local delivery omitted its observation")?;
        let mut observations = Observations::default();

        observations.record_credential(&observation)?;
        observations.record_credential(&observation)?;
        assert_eq!(observations.credentials().len(), 1);

        let mut conflicting = observation;
        conflicting.provider = "different".into();
        assert!(observations.record_credential(&conflicting).is_err());
        Ok(())
    }
}
