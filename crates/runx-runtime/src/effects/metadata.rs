use runx_contracts::{JsonObject, Reference};
use serde::{Deserialize, Serialize};

use super::RuntimeEffectError;

pub(crate) const EFFECT_VERIFICATION_REFS_METADATA: &str = "runx_effect_verification_refs";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectVerificationRefsMetadata {
    refs: Vec<Reference>,
}

pub(crate) fn insert_effect_verification_ref(
    metadata: &mut JsonObject,
    reference: Reference,
) -> Result<(), RuntimeEffectError> {
    let mut refs = effect_verification_refs(metadata)?;
    refs.push(reference);
    let value = serde_json::to_value(EffectVerificationRefsMetadata { refs })
        .and_then(serde_json::from_value)
        .map_err(|source| RuntimeEffectError::InvalidMetadata {
            family: "runtime".to_owned(),
            message: source.to_string(),
        })?;
    metadata.insert(EFFECT_VERIFICATION_REFS_METADATA.to_owned(), value);
    Ok(())
}

pub(crate) fn effect_verification_refs(
    metadata: &JsonObject,
) -> Result<Vec<Reference>, RuntimeEffectError> {
    let Some(value) = metadata.get(EFFECT_VERIFICATION_REFS_METADATA) else {
        return Ok(Vec::new());
    };
    let refs = serde_json::to_value(value)
        .and_then(serde_json::from_value::<EffectVerificationRefsMetadata>)
        .map_err(|source| RuntimeEffectError::InvalidMetadata {
            family: "runtime".to_owned(),
            message: source.to_string(),
        })?;
    Ok(refs.refs)
}
