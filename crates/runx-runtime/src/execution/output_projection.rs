use runx_contracts::{JsonObject, JsonValue, Reference, ReferenceType};

pub(crate) struct StepOutputProjection {
    pub(crate) outputs: JsonObject,
    pub(crate) refs: StepOutputRefs,
}

#[derive(Debug, Default)]
pub(crate) struct StepOutputRefs {
    pub(crate) signal_refs: Vec<Reference>,
    pub(crate) source_refs: Vec<Reference>,
    pub(crate) evidence_refs: Vec<Reference>,
    pub(crate) surface_refs: Vec<Reference>,
    pub(crate) artifact_refs: Vec<Reference>,
    pub(crate) verification_refs: Vec<Reference>,
}

/// Build the durable graph/receipt projection from one already-verified claim.
///
/// Raw adapter values and diagnostics never enter this boundary. References
/// are derived only from fields the producer declared and the runtime admitted
/// into `claim`, keeping graph state and receipt identity on one exact surface.
#[must_use]
pub(crate) fn project_step_claim(outputs: JsonObject) -> StepOutputProjection {
    let refs = claim_refs(&outputs);
    StepOutputProjection { outputs, refs }
}

#[must_use]
pub(crate) fn claim_refs(outputs: &JsonObject) -> StepOutputRefs {
    let mut refs = StepOutputRefs::default();
    collect_output_artifact_refs(outputs, &mut refs);
    collect_output_signal_refs(outputs, &mut refs);
    collect_output_change_set_refs(outputs, &mut refs);
    refs
}

fn collect_output_artifact_refs(object: &JsonObject, refs: &mut StepOutputRefs) {
    if let Some(artifact) = object.get("artifact") {
        collect_artifact_reference(artifact, refs);
    }
    if let Some(artifacts) = object.get("artifacts") {
        collect_artifact_reference(artifacts, refs);
    }
}

fn collect_artifact_reference(value: &JsonValue, refs: &mut StepOutputRefs) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                collect_artifact_reference(item, refs);
            }
        }
        JsonValue::Object(object) => {
            let Some(artifact_id) = object
                .get("artifact_id")
                .or_else(|| object.get("id"))
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
            else {
                return;
            };
            let artifact_type = object
                .get("artifact_type")
                .or_else(|| object.get("type"))
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty());
            let mut reference = Reference::runx(ReferenceType::Artifact, artifact_id);
            reference.locator = Some(artifact_id.to_owned().into());
            reference.label = artifact_type.map(Into::into);
            refs.artifact_refs.push(reference);
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {}
    }
}

fn collect_output_signal_refs(object: &JsonObject, refs: &mut StepOutputRefs) {
    if let Some(signal) = object.get("signal") {
        collect_signal_reference(signal, refs);
    }
    if let Some(signals) = object.get("signals") {
        collect_signal_reference(signals, refs);
    }
}

fn collect_output_change_set_refs(object: &JsonObject, refs: &mut StepOutputRefs) {
    if let Some(change_set) = object.get("change_set") {
        collect_change_set_reference(change_set, refs);
    }
}

fn collect_change_set_reference(value: &JsonValue, refs: &mut StepOutputRefs) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                collect_change_set_reference(item, refs);
            }
        }
        JsonValue::Object(object) => {
            if let Some(target_surfaces) = object.get("target_surfaces") {
                collect_target_surface_reference(target_surfaces, refs);
            }
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {}
    }
}

fn collect_target_surface_reference(value: &JsonValue, refs: &mut StepOutputRefs) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                collect_target_surface_reference(item, refs);
            }
        }
        JsonValue::Object(object) => {
            let Some(surface) = object
                .get("surface")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
            else {
                return;
            };
            let mut reference = Reference::runx(ReferenceType::Surface, surface);
            reference.locator = Some(surface.to_owned().into());
            reference.label = object
                .get("kind")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(|value| value.to_owned().into());
            refs.surface_refs.push(reference);
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {}
    }
}

fn collect_signal_reference(value: &JsonValue, refs: &mut StepOutputRefs) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                collect_signal_reference(item, refs);
            }
        }
        JsonValue::Object(object) => {
            if let Some(signal_id) = object
                .get("signal_id")
                .or_else(|| object.get("id"))
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
            {
                refs.signal_refs
                    .push(Reference::runx(ReferenceType::Signal, signal_id));
            }
            if let Some(source_events) = object.get("source_events") {
                collect_source_event_reference(source_events, refs);
            }
            if let Some(artifact) = object.get("artifact") {
                collect_artifact_reference(artifact, refs);
            }
            if let Some(artifacts) = object.get("artifacts") {
                collect_artifact_reference(artifacts, refs);
            }
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {}
    }
}

fn collect_source_event_reference(value: &JsonValue, refs: &mut StepOutputRefs) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                collect_source_event_reference(item, refs);
            }
        }
        JsonValue::Object(object) => {
            let Some(locator) = object
                .get("source_locator")
                .or_else(|| object.get("locator"))
                .or_else(|| object.get("thread_locator"))
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
            else {
                return;
            };
            let provider = object
                .get("provider")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty());
            let label = object
                .get("title")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty());
            refs.source_refs.push(Reference {
                uri: locator.to_owned().into(),
                reference_type: reference_type_for_source(provider, locator),
                provider: provider.map(|value| value.to_owned().into()),
                locator: Some(locator.to_owned().into()),
                label: label.map(|value| value.to_owned().into()),
                observed_at: None,
                proof_kind: None,
            });
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {}
    }
}

fn reference_type_for_source(provider: Option<&str>, locator: &str) -> ReferenceType {
    crate::kernel::connectors::references::reference_kind(provider, locator)
}

#[cfg(test)]
mod tests {
    use super::project_step_claim;
    use runx_contracts::{JsonObject, JsonValue};

    #[test]
    fn references_are_derived_from_the_admitted_claim() {
        let claim = JsonObject::from([(
            "artifact".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "artifact_id".to_owned(),
                JsonValue::String("artifact_1".to_owned()),
            )])),
        )]);

        let projection = project_step_claim(claim);

        assert_eq!(projection.refs.artifact_refs.len(), 1);
        assert_eq!(
            projection.refs.artifact_refs[0].uri.as_str(),
            "runx:artifact:artifact_1"
        );
    }
}
