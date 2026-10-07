use runx_contracts::{
    JsonNumber, JsonObject, JsonValue, SkillArchitectureDecision, SkillExecutionLane,
};
use runx_parser::{SourceKind, ValidatedSkillPackage};

use super::super::path::invalid_skill_change;
use crate::RuntimeError;

#[derive(Default)]
pub(super) struct CandidateResourceUsage {
    pub(super) max_fanout: u64,
    pub(super) process_spawns: u64,
    pub(super) network: bool,
    pub(super) domain_modules: bool,
}

impl CandidateResourceUsage {
    pub(super) fn as_json(&self) -> JsonValue {
        JsonValue::Object(JsonObject::from([
            (
                "max_fanout".to_owned(),
                JsonValue::Number(JsonNumber::U64(self.max_fanout)),
            ),
            (
                "process_spawns".to_owned(),
                JsonValue::Number(JsonNumber::U64(self.process_spawns)),
            ),
            ("network".to_owned(), JsonValue::Bool(self.network)),
            (
                "domain_modules".to_owned(),
                JsonValue::Bool(self.domain_modules),
            ),
        ]))
    }
}

pub(super) fn candidate_resource_usage(package: &ValidatedSkillPackage) -> CandidateResourceUsage {
    let mut usage = CandidateResourceUsage {
        max_fanout: 1,
        domain_modules: !package.javascript_modules.is_empty(),
        ..CandidateResourceUsage::default()
    };
    for profile in package.profiles.values() {
        for runner in profile.runners.values() {
            classify_runner(runner, &mut usage);
        }
    }
    if usage.domain_modules {
        usage.process_spawns = usage.process_spawns.saturating_add(1);
    }
    usage
}

fn classify_runner(
    runner: &runx_parser::SkillRunnerDefinition,
    usage: &mut CandidateResourceUsage,
) {
    classify_source(runner.source.source_type, usage);
    let Some(graph) = &runner.source.graph else {
        return;
    };
    for group in graph.fanout_groups.keys() {
        let branches = graph
            .steps
            .iter()
            .filter(|step| step.fanout_group.as_deref() == Some(group.as_str()))
            .count() as u64;
        usage.max_fanout = usage.max_fanout.max(branches);
    }
    for step in &graph.steps {
        classify_graph_step(step, usage);
    }
}

fn classify_graph_step(step: &runx_parser::GraphStep, usage: &mut CandidateResourceUsage) {
    // Every operation is external I/O.
    if step.operation.is_some() {
        usage.network = true;
    }
    if let Some(tool) = &step.tool {
        classify_native_tool(tool, usage);
    }
    let Some(run) = &step.run else {
        return;
    };
    let Some(source) = run.source() else {
        return;
    };
    classify_source(source.source_type, usage);
}

fn classify_source(kind: SourceKind, usage: &mut CandidateResourceUsage) {
    classify_source_name(kind.as_str(), usage);
}

fn classify_source_name(kind: &str, usage: &mut CandidateResourceUsage) {
    match kind {
        "javascript" => usage.domain_modules = true,
        "cli-tool" => {
            usage.process_spawns = usage.process_spawns.saturating_add(1);
            usage.network = true;
        }
        _ => {}
    }
}

fn classify_native_tool(tool: &str, usage: &mut CandidateResourceUsage) {
    if tool == "command.execute" {
        usage.process_spawns = usage.process_spawns.saturating_add(1);
        usage.network = true;
    }
    if matches!(tool, "artifact.allocate" | "artifact.handoff") {
        usage.network = true;
    }
}

pub(super) fn validate_architecture_resources(
    architecture: &SkillArchitectureDecision,
    usage: &CandidateResourceUsage,
) -> Result<(), RuntimeError> {
    let budget = &architecture.resource_budget;
    if usage.max_fanout > budget.max_fanout {
        return Err(invalid_skill_change(format!(
            "candidate fan-out {} exceeds architecture budget {}",
            usage.max_fanout, budget.max_fanout
        )));
    }
    if usage.process_spawns > budget.max_process_spawns {
        return Err(invalid_skill_change(format!(
            "candidate process count {} exceeds architecture budget {}",
            usage.process_spawns, budget.max_process_spawns
        )));
    }
    if usage.network && !budget.network_allowed {
        return Err(invalid_skill_change(
            "candidate includes a network-capable lane but its architecture forbids network access",
        ));
    }
    let planned_domain_module = architecture
        .required_behaviors
        .iter()
        .any(|behavior| behavior.lane == SkillExecutionLane::DomainModule);
    if usage.domain_modules != planned_domain_module {
        return Err(invalid_skill_change(
            "candidate domain modules must exactly match the architecture decision",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CandidateResourceUsage, classify_native_tool, classify_source_name};

    #[test]
    fn supplied_evidence_indexing_does_not_claim_network_access() {
        let mut usage = CandidateResourceUsage::default();

        classify_native_tool("evidence.index_sources", &mut usage);

        assert!(!usage.network);
    }

    #[test]
    fn actual_network_tools_claim_network_access() {
        for tool in ["artifact.allocate", "artifact.handoff"] {
            let mut usage = CandidateResourceUsage::default();
            classify_native_tool(tool, &mut usage);
            assert!(usage.network, "{tool} must consume the network budget");
        }
    }

    #[test]
    fn trusted_host_processes_are_network_capable() {
        let mut usage = CandidateResourceUsage::default();
        classify_source_name("cli-tool", &mut usage);
        assert!(usage.network, "cli-tool must consume the network budget");

        let mut command = CandidateResourceUsage::default();
        classify_native_tool("command.execute", &mut command);
        assert!(command.network);
    }
}
