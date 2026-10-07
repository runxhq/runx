use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use runx_parser::GraphStep;

use super::{
    EffectAdmission, EffectOutputRequest, EffectPreparationOutcome, EffectReceiptRequest,
    EffectReplay, EffectReplayOutputRequest, EffectReplayReceiptRequest, EffectStepRequest,
    RuntimeEffect, RuntimeEffectError,
};

mod catalog;

#[derive(Clone)]
pub struct RuntimeEffectRegistry {
    families: BTreeMap<&'static str, Arc<dyn RuntimeEffect>>,
    /// Exact response bytes admitted by the harness front. This state has no
    /// environment or skill-input loader and is absent from every live registry.
    harness_http_responses: Option<Arc<BTreeMap<String, crate::http::RuntimeHttpResponse>>>,
}

impl RuntimeEffectRegistry {
    /// Standard portable local effect owners used by every operator surface.
    pub fn local() -> Result<Self, RuntimeEffectError> {
        #[cfg(feature = "async-http")]
        #[allow(unused_mut)]
        let mut registry = Self::with_effect(crate::kernel::operation::OperationEffect::default())?;
        #[cfg(not(feature = "async-http"))]
        #[allow(unused_mut)]
        let mut registry = Self::empty();
        #[cfg(feature = "catalog")]
        registry.register_effect(super::ExternalReceiptEffect)?;
        Ok(registry)
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            families: BTreeMap::new(),
            harness_http_responses: None,
        }
    }

    pub(crate) fn with_harness_http(
        mut self,
        responses: Option<Arc<BTreeMap<String, crate::http::RuntimeHttpResponse>>>,
    ) -> Self {
        self.harness_http_responses = responses;
        self
    }

    #[cfg(feature = "catalog")]
    pub(crate) fn harness_http_responses(
        &self,
    ) -> Option<&BTreeMap<String, crate::http::RuntimeHttpResponse>> {
        self.harness_http_responses.as_deref()
    }

    pub fn with_effect<T>(effect: T) -> Result<Self, RuntimeEffectError>
    where
        T: RuntimeEffect + 'static,
    {
        let mut registry = Self::empty();
        registry.register_effect(effect)?;
        Ok(registry)
    }

    pub fn register_effect<T>(&mut self, effect: T) -> Result<(), RuntimeEffectError>
    where
        T: RuntimeEffect + 'static,
    {
        let family = effect.family();
        if self.families.contains_key(family) {
            return Err(RuntimeEffectError::DuplicateFamily {
                family: family.to_owned(),
            });
        }
        catalog::validate_capabilities(family, effect.capabilities())?;
        for capability in effect.capabilities() {
            let definition = capability.definition();
            #[cfg(feature = "catalog")]
            if crate::tool_catalogs::native::is_core_tool(definition.id) {
                return Err(RuntimeEffectError::InvalidMetadata {
                    family: family.to_owned(),
                    message: format!(
                        "tool {} conflicts with a runtime-owned catalog tool",
                        definition.id
                    ),
                });
            }
            if let Some(owner) = self.capability_owner(definition.id) {
                return Err(RuntimeEffectError::InvalidMetadata {
                    family: family.to_owned(),
                    message: format!(
                        "tool {} is already owned by effect family {owner}",
                        definition.id
                    ),
                });
            }
        }
        self.families.insert(family, Arc::new(effect));
        Ok(())
    }

    /// Replace one effect family while preserving every unrelated runtime
    /// effect. This is crate-private because only isolated runtime assembly
    /// (currently the deterministic harness) may alter an already-wired
    /// registry.
    #[cfg(feature = "async-http")]
    pub(crate) fn replace_effect<T>(&mut self, effect: T) -> Result<(), RuntimeEffectError>
    where
        T: RuntimeEffect + 'static,
    {
        self.families.remove(effect.family());
        self.register_effect(effect)
    }

    pub(crate) fn find_replay(
        &self,
        request: EffectStepRequest<'_>,
    ) -> Result<Option<EffectReplay>, RuntimeEffectError> {
        if let Some(effect) = self.resolved_effect(request)? {
            return effect.find_replay(request);
        }
        Ok(None)
    }

    pub(crate) fn recover_pending(
        &self,
        request: EffectStepRequest<'_>,
    ) -> Result<(), RuntimeEffectError> {
        if let Some(effect) = self.resolved_effect(request)? {
            return effect.recover_pending(request);
        }
        Ok(())
    }

    pub(crate) fn admit(
        &self,
        request: EffectStepRequest<'_>,
    ) -> Result<Option<EffectAdmission>, RuntimeEffectError> {
        if let Some(effect) = self.resolved_effect(request)? {
            return effect.admit(request)?.map(Some).ok_or_else(|| {
                RuntimeEffectError::InvalidMetadata {
                    family: effect.family().to_owned(),
                    message: format!(
                        "resolved target for step {} belongs to this effect family but did not provide an admissible effect contract",
                        request.step.id
                    ),
                }
            });
        }
        Ok(None)
    }

    pub(crate) fn prepare_execution(
        &self,
        request: EffectStepRequest<'_>,
        admission: EffectAdmission,
        host: &mut dyn crate::Host,
    ) -> Result<EffectPreparationOutcome, RuntimeEffectError> {
        let effect = self.require_effect(admission.family())?;
        effect.prepare_execution(request.step, admission, host)
    }

    pub(crate) fn prepare_output(
        &self,
        request: EffectOutputRequest<'_>,
    ) -> Result<(), RuntimeEffectError> {
        let family = request.admission.family();
        self.require_effect(family)?.prepare_output(request)
    }

    pub(crate) fn finalize_output(
        &self,
        request: EffectReceiptRequest<'_>,
    ) -> Result<(), RuntimeEffectError> {
        let family = request.admission.family();
        self.require_effect(family)?.finalize_output(request)
    }

    pub(crate) fn persist(
        &self,
        request: EffectReceiptRequest<'_>,
    ) -> Result<(), RuntimeEffectError> {
        let family = request.admission.family();
        self.require_effect(family)?.persist(request)
    }

    pub(crate) fn prepare_replay_output(
        &self,
        request: EffectReplayOutputRequest<'_>,
    ) -> Result<(), RuntimeEffectError> {
        let family = request.replay.family();
        self.require_effect(family)?.prepare_replay_output(request)
    }

    pub(crate) fn validate_replay(
        &self,
        request: EffectReplayReceiptRequest<'_>,
    ) -> Result<(), RuntimeEffectError> {
        let family = request.replay.family();
        self.require_effect(family)?.validate_replay(request)
    }

    pub(crate) fn authority(
        &self,
        admission: &EffectAdmission,
    ) -> Result<crate::receipts::SealAuthority, RuntimeEffectError> {
        self.require_effect(admission.family())?
            .authority(admission)
    }

    pub(crate) fn allows_parallel_step(&self, step: &GraphStep) -> bool {
        self.families
            .values()
            .all(|effect| effect.can_run_parallel(step))
    }

    fn resolved_effect(
        &self,
        request: EffectStepRequest<'_>,
    ) -> Result<Option<&dyn RuntimeEffect>, RuntimeEffectError> {
        let mut matched = self
            .families
            .values()
            .filter(|effect| effect.matches_target(request));
        let first = matched.next().map(Arc::as_ref);
        let Some(second) = matched.next() else {
            return Ok(first);
        };
        let first_family = first.map_or("unknown", RuntimeEffect::family);
        Err(RuntimeEffectError::InvalidMetadata {
            family: second.family().to_owned(),
            message: format!(
                "resolved target is claimed by both effect families {first_family} and {}",
                second.family()
            ),
        })
    }

    fn require_effect(
        &self,
        family: &'static str,
    ) -> Result<&dyn RuntimeEffect, RuntimeEffectError> {
        self.families.get(family).map(Arc::as_ref).ok_or_else(|| {
            RuntimeEffectError::MissingFamily {
                family: family.to_owned(),
            }
        })
    }
}

impl Default for RuntimeEffectRegistry {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Debug for RuntimeEffectRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let families = self.families.keys().copied().collect::<Vec<_>>();
        formatter
            .debug_struct("RuntimeEffectRegistry")
            .field("families", &families)
            .field(
                "harness_http_response_count",
                &self
                    .harness_http_responses
                    .as_ref()
                    .map_or(0, |responses| responses.len()),
            )
            .finish()
    }
}
