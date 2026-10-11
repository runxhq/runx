use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use super::{
    EffectAdmission, EffectOutputRequest, EffectPreparationOutcome, EffectReceiptRequest,
    EffectStepRequest, RuntimeEffect, RuntimeEffectError,
};

mod catalog;

#[derive(Clone)]
pub struct RuntimeEffectRegistry {
    families: BTreeMap<&'static str, Arc<dyn RuntimeEffect>>,
    /// The exact HTTP exchanges admitted by the harness front. This state has
    /// no environment or skill-input loader and is absent from every live
    /// registry.
    harness_http: Option<Arc<crate::http::HarnessExchanges>>,
}

impl RuntimeEffectRegistry {
    /// Standard portable local effect owners used by every operator surface.
    pub fn local() -> Result<Self, RuntimeEffectError> {
        let mut registry = Self::with_effect(crate::kernel::operation::OperationEffect::default())?;
        registry.register_effect(super::ExternalReceiptEffect)?;
        Ok(registry)
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            families: BTreeMap::new(),
            harness_http: None,
        }
    }

    pub(crate) fn with_harness_http(
        mut self,
        exchanges: Option<Arc<crate::http::HarnessExchanges>>,
    ) -> Self {
        self.harness_http = exchanges;
        self
    }

    pub(crate) fn harness_http_exchanges(&self) -> Option<&crate::http::HarnessExchanges> {
        self.harness_http.as_deref()
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
    pub(crate) fn replace_effect<T>(&mut self, effect: T) -> Result<(), RuntimeEffectError>
    where
        T: RuntimeEffect + 'static,
    {
        self.families.remove(effect.family());
        self.register_effect(effect)
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

    pub(crate) fn authority(
        &self,
        admission: &EffectAdmission,
    ) -> Result<crate::receipts::SealAuthority, RuntimeEffectError> {
        self.require_effect(admission.family())?
            .authority(admission)
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
                "harness_http_exchange_count",
                &self
                    .harness_http
                    .as_ref()
                    .map_or(0, |exchanges| exchanges.len()),
            )
            .finish()
    }
}
