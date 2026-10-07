//! The execution profile's `connectors:` block: which connector operations a
//! skill needs.
//!
//! A key that names a connector is a fixed requirement. A key with
//! `connector_from` is chosen at run time from a closed `allowed_connectors`
//! list. Operation steps name a key and an operation on it; scopes, verbs,
//! approval floors and credentials all come from the connector.

use std::collections::BTreeMap;

use runx_contracts::JsonValue;
use runx_contracts::connector::{ConnectorId, OperationName, is_connector_id};
use serde::{Deserialize, Serialize};

use crate::ValidationError;
use crate::graph::ExecutionGraph;

use super::SkillRunnerDefinition;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorRequirement {
    pub operations: Vec<OperationName>,
    /// `$input.<path>`: where a run-time choice of connector is read from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_from: Option<String>,
    /// The connectors a run-time choice may name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_connectors: Vec<ConnectorId>,
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

/// Parse and validate the block.
pub(crate) fn validate_connector_requirements(
    value: Option<&JsonValue>,
) -> Result<BTreeMap<String, ConnectorRequirement>, ValidationError> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let requirements: BTreeMap<String, ConnectorRequirement> = value
        .clone()
        .deserialize_into()
        .map_err(|error| invalid("connectors", format!("connectors is invalid: {error}")))?;
    for (key, requirement) in &requirements {
        let field = format!("connectors.{key}");
        if !is_connector_id(key) {
            return Err(invalid(
                &field,
                format!("{field} key must be a connector id or alias."),
            ));
        }
        if requirement.operations.is_empty() {
            return Err(invalid(
                &field,
                format!("{field}.operations must list at least one operation."),
            ));
        }
        for (index, operation) in requirement.operations.iter().enumerate() {
            if requirement.operations[..index].contains(operation) {
                return Err(invalid(
                    &field,
                    format!("{field}.operations lists {operation} twice."),
                ));
            }
        }
        match &requirement.connector_from {
            None => {
                if !requirement.allowed_connectors.is_empty() {
                    return Err(invalid(
                        &field,
                        format!(
                            "{field}.allowed_connectors needs connector_from naming the run-time choice."
                        ),
                    ));
                }
            }
            Some(from) => {
                let path_valid = from.strip_prefix("$input.").is_some_and(|path| {
                    !path.is_empty() && path.split('.').all(|segment| !segment.is_empty())
                });
                if !path_valid {
                    return Err(invalid(
                        &field,
                        format!("{field}.connector_from must be $input.<path>."),
                    ));
                }
                if requirement.allowed_connectors.is_empty() {
                    return Err(invalid(
                        &field,
                        format!(
                            "{field} needs allowed_connectors listing the connectors it may choose."
                        ),
                    ));
                }
            }
        }
    }
    Ok(requirements)
}

/// Every operation step names a declared requirement and, when it names an
/// operation, one that requirement lists. Each step is bound to its
/// requirement, so the kernel admits it without the manifest at hand.
pub(crate) fn bind_operation_steps(
    runners: &mut BTreeMap<String, SkillRunnerDefinition>,
    requirements: &BTreeMap<String, ConnectorRequirement>,
) -> Result<(), ValidationError> {
    for (name, runner) in runners {
        let Some(graph) = runner.source.graph.as_mut() else {
            continue;
        };
        bind_graph_operations(name, graph, requirements)?;
    }
    Ok(())
}

fn bind_graph_operations(
    runner: &str,
    graph: &mut ExecutionGraph,
    requirements: &BTreeMap<String, ConnectorRequirement>,
) -> Result<(), ValidationError> {
    for step in &mut graph.steps {
        let Some(target) = step.operation.as_mut() else {
            continue;
        };
        let field = format!("runners.{runner}.graph.steps.{}.operation", step.id);
        let requirement = requirements.get(&target.requirement).ok_or_else(|| {
            invalid(
                &field,
                format!(
                    "{field} names '{}', which the connectors block does not declare.",
                    target.requirement
                ),
            )
        })?;
        if let Some(operation) = &target.operation
            && !requirement.lists(operation.as_str())
        {
            return Err(invalid(
                &field,
                format!(
                    "{field} '{operation}' is not listed in connectors.{}.operations.",
                    target.requirement
                ),
            ));
        }
        if let Some(from) = &requirement.connector_from {
            if step.inputs.contains_key("connector") || step.context.contains_key("connector") {
                return Err(invalid(
                    &field,
                    format!(
                        "{field} binds a connector; connectors.{}.connector_from chooses it.",
                        target.requirement
                    ),
                ));
            }
            step.inputs
                .insert("connector".to_owned(), JsonValue::String(from.clone()));
        }
        target.declared = Some(requirement.clone());
    }
    Ok(())
}

fn invalid(field: &str, message: String) -> ValidationError {
    ValidationError::InvalidField {
        field: field.to_owned(),
        message,
    }
}
