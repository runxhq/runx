//! Which connector operations a package needs, derived from its steps.
//!
//! A step names `<connector>/<operation>`, and an agent step's `calls` name
//! the operations its turn may call, so a package's requirements are what
//! its steps say. The root `connectors:` block says only what steps can't: a
//! pin, `{ manifest_digest }`, and a connector chosen per run that several
//! steps share, `{ choose: $input.<path>, from: [<connector>, ...] }`, which
//! steps use by its name. Scopes, verbs, approval floors and credentials all
//! come from the connector. Whether a requirement must pin a
//! `manifest_digest` depends on where it resolves (outside the skill's
//! plugin, and not a runtime capability), so resolution enforces it, not
//! this grammar.

use std::collections::BTreeMap;

use runx_contracts::JsonValue;
use runx_contracts::connector::{Digest, OperationName, is_connector_name};
use serde::{Deserialize, Serialize};

use crate::ValidationError;
use crate::workflow::Workflow;

use super::SkillMethodDefinition;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorRequirement {
    /// Every operation the package's steps name on it.
    pub operations: Vec<OperationName>,
    /// The exact manifest a connector outside the skill's own plugin must
    /// match; runtime capabilities and plugin connectors take none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_digest: Option<Digest>,
    /// `$input.<path>`: where a connector chosen per run is read from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choose: Option<String>,
    /// The connectors a per-run choice may name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<String>,
}

impl ConnectorRequirement {
    /// Whether the requirement lists this operation.
    #[must_use]
    pub fn lists(&self, operation: &str) -> bool {
        self.operations
            .iter()
            .any(|listed| listed.as_str() == operation)
    }
}

/// One entry of the root `connectors:` block, as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredConnector {
    #[serde(default)]
    manifest_digest: Option<Digest>,
    #[serde(default)]
    choose: Option<String>,
    #[serde(default)]
    from: Vec<String>,
}

/// The package's requirements: each connector its steps name, with every
/// operation they name on it, and what the root `connectors:` block says of
/// it.
pub(crate) fn connector_requirements(
    value: Option<&JsonValue>,
    methods: &BTreeMap<String, SkillMethodDefinition>,
) -> Result<BTreeMap<String, ConnectorRequirement>, ValidationError> {
    let declared: BTreeMap<String, DeclaredConnector> = match value {
        None => BTreeMap::new(),
        Some(value) => value
            .clone()
            .deserialize_into()
            .map_err(|error| invalid("connectors", format!("connectors is invalid: {error}")))?,
    };
    let mut requirements = BTreeMap::<String, ConnectorRequirement>::new();
    for method in methods.values() {
        for (requirement, operation) in named_operations(&method.workflow) {
            let entry = requirements
                .entry(requirement)
                .or_insert_with(|| ConnectorRequirement {
                    operations: Vec::new(),
                    manifest_digest: None,
                    choose: None,
                    from: Vec::new(),
                });
            if !entry.operations.contains(&operation) {
                entry.operations.push(operation);
            }
        }
    }
    for (key, requirement) in &mut requirements {
        let field = format!("connectors.{key}");
        if !is_connector_name(key) {
            return Err(invalid(
                &field,
                format!("{key} is not a connector name or a per-run choice's name."),
            ));
        }
        requirement.operations.sort();
        let Some(entry) = declared.get(key) else {
            continue;
        };
        requirement
            .manifest_digest
            .clone_from(&entry.manifest_digest);
        requirement.choose.clone_from(&entry.choose);
        requirement.from.clone_from(&entry.from);
    }
    for (key, entry) in &declared {
        let field = format!("connectors.{key}");
        if !requirements.contains_key(key) {
            return Err(invalid(
                &field,
                format!("{field} names a connector no step calls; a step names what it calls."),
            ));
        }
        match &entry.choose {
            None if !entry.from.is_empty() => {
                return Err(invalid(
                    &field,
                    format!("{field}.from needs choose, naming the per-run choice."),
                ));
            }
            None if entry.manifest_digest.is_none() => {
                return Err(invalid(
                    &field,
                    format!("{field} pins a manifest_digest or chooses a connector per run."),
                ));
            }
            None => {}
            Some(choose) => {
                if entry.manifest_digest.is_some() {
                    return Err(invalid(
                        &field,
                        format!(
                            "{field} chooses a connector per run, so it can't pin one manifest."
                        ),
                    ));
                }
                let path_valid = choose.strip_prefix("$input.").is_some_and(|path| {
                    !path.is_empty() && path.split('.').all(|segment| !segment.is_empty())
                });
                if !path_valid {
                    return Err(invalid(
                        &field,
                        format!("{field}.choose must be $input.<path>."),
                    ));
                }
                if entry.from.is_empty() {
                    return Err(invalid(
                        &field,
                        format!("{field}.from lists the connectors the run may choose."),
                    ));
                }
                for (index, reference) in entry.from.iter().enumerate() {
                    if !is_connector_name(reference) || entry.from[..index].contains(reference) {
                        return Err(invalid(
                            &field,
                            format!("{field}.from must contain unique connector names."),
                        ));
                    }
                }
            }
        }
    }
    Ok(requirements)
}

/// Every operation a workflow's steps name: an operation step's, each a
/// per-run choice may take, and each an agent step's turn or a script step
/// may call.
fn named_operations(workflow: &Workflow) -> Vec<(String, OperationName)> {
    let mut named = Vec::new();
    for step in &workflow.steps {
        match &step.kind {
            crate::StepKind::Operation(target) => {
                named.extend(
                    target
                        .operation
                        .iter()
                        .chain(&target.from)
                        .map(|operation| (target.requirement.clone(), operation.clone())),
                );
            }
            // The step's parser admitted each as <connector>/<operation>.
            crate::StepKind::Agent(crate::AgentTask { allowance, .. })
            | crate::StepKind::Script(crate::ScriptStep { allowance, .. }) => {
                named.extend(allowance.operations.iter().filter_map(|allowed| {
                    OperationName::new(allowed.operation.as_str())
                        .map(|operation| (allowed.requirement.as_str().to_owned(), operation))
                }));
            }
            _ => {}
        }
    }
    named
}

/// The requirements of the connectors an allowance's operations name.
fn allowance_connectors(
    allowance: &runx_contracts::TurnAllowance,
    requirements: &BTreeMap<String, ConnectorRequirement>,
) -> BTreeMap<String, ConnectorRequirement> {
    allowance
        .operations
        .iter()
        .filter_map(|allowed| {
            requirements
                .get_key_value(allowed.requirement.as_str())
                .map(|(key, requirement)| (key.clone(), requirement.clone()))
        })
        .collect()
}

/// Bind each operation step to its requirement, so the kernel admits it
/// without the manifest at hand, and each agent step to the requirements its
/// turn's operations name. A step on a connector chosen per run reads the
/// choice as its call's `connector`.
pub(crate) fn bind_operation_steps(
    methods: &mut BTreeMap<String, SkillMethodDefinition>,
    requirements: &BTreeMap<String, ConnectorRequirement>,
) -> Result<(), ValidationError> {
    for (name, method) in methods {
        for step in &mut method.workflow.steps {
            let field = format!("methods.{name}.steps.{}.operation", step.id);
            match &mut step.kind {
                crate::StepKind::Agent(agent) => {
                    agent.connectors = allowance_connectors(&agent.allowance, requirements);
                }
                crate::StepKind::Script(script) => {
                    script.connectors = allowance_connectors(&script.allowance, requirements);
                }
                crate::StepKind::Operation(target) => {
                    let requirement = requirements.get(&target.requirement).ok_or_else(|| {
                        invalid(
                            &field,
                            format!(
                                "{field} names '{}', which no requirement holds.",
                                target.requirement
                            ),
                        )
                    })?;
                    if let Some(choose) = &requirement.choose {
                        if step.inputs.contains_key("connector") {
                            return Err(invalid(
                                &field,
                                format!(
                                    "{field} binds a connector; connectors.{}.choose chooses it.",
                                    target.requirement
                                ),
                            ));
                        }
                        // Validated above as `$input.<path>`.
                        let path = choose
                            .strip_prefix("$input.")
                            .unwrap_or_default()
                            .split('.')
                            .map(str::to_owned)
                            .collect();
                        step.inputs
                            .insert("connector".to_owned(), JsonValue::String(choose.clone()));
                        step.bindings.push(crate::workflow::Binding {
                            at: vec!["connector".to_owned()],
                            source: crate::workflow::Source::Input { path },
                        });
                    }
                    target.declared = Some(requirement.clone());
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn invalid(field: &str, message: String) -> ValidationError {
    ValidationError::InvalidField {
        field: field.to_owned(),
        message,
    }
}
