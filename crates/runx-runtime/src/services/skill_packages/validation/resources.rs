use runx_contracts::{
    JsonNumber, JsonObject, JsonValue, SkillArchitectureDecision, SkillExecutionLane,
};
use runx_parser::{StepKind, ToolTarget, ValidatedSkillPackage};

use super::super::path::invalid_skill_change;
use crate::RuntimeError;

#[derive(Default)]
pub(super) struct CandidateResourceUsage {
    pub(super) process_spawns: u64,
    pub(super) network: bool,
    pub(super) domain_modules: bool,
}

impl CandidateResourceUsage {
    pub(super) fn as_json(&self) -> JsonValue {
        JsonValue::Object(JsonObject::from([
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
    for step in &runner.graph.steps {
        match &step.kind {
            // Every operation is external I/O.
            StepKind::Operation(_) => usage.network = true,
            StepKind::Tool(ToolTarget::Catalog(tool)) => classify_native_tool(tool, usage),
            StepKind::Tool(ToolTarget::Worker(_)) => usage.domain_modules = true,
            StepKind::Tool(ToolTarget::Process(_)) => classify_process(usage),
            StepKind::Agent(_) | StepKind::Ask(_) | StepKind::Skill(_) => {}
        }
    }
}

/// A trusted host process is unconfined, so it consumes the network budget.
fn classify_process(usage: &mut CandidateResourceUsage) {
    usage.process_spawns = usage.process_spawns.saturating_add(1);
    usage.network = true;
}

fn classify_native_tool(tool: &str, usage: &mut CandidateResourceUsage) {
    if tool == "command.execute" {
        classify_process(usage);
    }
}

pub(super) fn validate_architecture_resources(
    architecture: &SkillArchitectureDecision,
    usage: &CandidateResourceUsage,
) -> Result<(), RuntimeError> {
    let budget = &architecture.resource_budget;
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
    use super::{CandidateResourceUsage, classify_native_tool, classify_process};

    #[test]
    fn supplied_evidence_indexing_does_not_claim_network_access() {
        let mut usage = CandidateResourceUsage::default();

        classify_native_tool("evidence.index_sources", &mut usage);

        assert!(!usage.network);
    }

    #[test]
    fn trusted_host_processes_are_network_capable() {
        let mut usage = CandidateResourceUsage::default();
        classify_process(&mut usage);
        assert!(
            usage.network,
            "a trusted host process must consume the network budget"
        );

        let mut command = CandidateResourceUsage::default();
        classify_native_tool("command.execute", &mut command);
        assert!(command.network);
    }
}
