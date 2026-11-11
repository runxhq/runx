//! A step's `input_digest`: what it was given, as reuse compares it (D32).

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::input_digest::{StepGiven, step_input_digest};
use runx_contracts::{CallAct, ContextEntry, JsonObject};
use runx_parser::{StepKind, ToolTarget, WorkflowStep};

/// What a step was given, beside its inputs.
pub(crate) struct Given<'a> {
    pub inputs: &'a JsonObject,
    pub act: Option<&'a CallAct>,
    pub context: &'a [ContextEntry],
    pub ask_reason: Option<&'a str>,
}

/// What `step` was given, as one digest. The environment its worker or
/// process declares is given too, as a fingerprint of the values it would
/// read, so a run given `REGION=eu` and one given `REGION=us` differ. None
/// when those values can't be resolved or fingerprinted, since a digest
/// that left them out would take two such runs for the same.
pub(crate) fn of(
    step: &WorkflowStep,
    given: &Given<'_>,
    env: &BTreeMap<String, String>,
    workflow_dir: &Path,
) -> Option<String> {
    let environment = match declared_environment(step) {
        None => None,
        Some(requirements) => {
            let values =
                crate::execution_environment::resolve_environment(requirements, env, workflow_dir)
                    .ok()?;
            let message = runx_contracts::canonical_json::to_canonical_bytes(&values).ok()?;
            Some(crate::secrets::fingerprint(
                &crate::paths::RunxPaths::resolve(env, workflow_dir),
                "runx.input_environment.v1",
                &message,
            )?)
        }
    };
    step_input_digest(&StepGiven {
        inputs: given.inputs,
        act: given.act,
        context: given.context,
        ask_reason: given.ask_reason,
        environment: environment.as_deref(),
    })
    .ok()
}

/// The environment `step`'s worker or process reads, when it declares one.
fn declared_environment(step: &WorkflowStep) -> Option<&runx_contracts::EnvironmentRequirements> {
    match &step.kind {
        StepKind::Tool(ToolTarget::Worker(worker)) => Some(&worker.environment),
        StepKind::Tool(ToolTarget::Process(process)) => Some(&process.environment),
        _ => None,
    }
    .filter(|requirements| !requirements.is_empty())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Arc;

    use runx_contracts::JsonValue;

    use crate::execution::run_record::{Replayed, RunLog};
    use crate::kernel::store::KernelStore;
    use crate::{LocalOrchestrator, ManagedAgentPolicy, SkillRunRequest};

    /// A package whose one worker step declares `REGION`.
    fn write_region(root: &Path) -> std::path::PathBuf {
        let skill = root.join("region");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: region\ndescription: Reads its region.\n---\n# Region\n\nRead it.\n",
        )
        .unwrap();
        std::fs::write(
            skill.join("run.yaml"),
            r#"minimum_runx: 0.12.0
methods:
  read:
    inputs:
      value: {type: string}
    steps:
      - id: read
        tool: {module: read.mjs, export: read, environment: {optional: [REGION]}}
        inputs: {value: $input.value}
        outputs:
          value: {type: string}
"#,
        )
        .unwrap();
        std::fs::write(
            skill.join("read.mjs"),
            "export function read({ value }) { return { value }; }\n",
        )
        .unwrap();
        skill
    }

    /// The `read` step's input digest in a run of `skill` under `env`.
    fn digest(root: &Path, skill: &Path, env: BTreeMap<String, String>) -> Option<String> {
        let result = LocalOrchestrator::default()
            .run_skill_with_binding(
                &SkillRunRequest {
                    current_context: Vec::new(),
                    historical_context: Vec::new(),
                    run_authority: None,
                    act: None,
                    skill_path: skill.to_path_buf(),
                    receipt_dir: Some(root.join("receipts")),
                    run_id: None,
                    answers: None,
                    inputs: BTreeMap::from([(
                        "value".to_owned(),
                        JsonValue::String("same".to_owned()),
                    )]),
                    env: env.clone(),
                    cwd: root.to_path_buf(),
                    managed_agent: ManagedAgentPolicy::HostDriven,
                },
                None,
                None,
                None,
            )
            .unwrap();
        let run_id = result.output.as_object().unwrap()["run_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let store =
            KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, root)).unwrap();
        let record = RunLog::open(Arc::clone(&store), run_id)
            .unwrap()
            .load()
            .unwrap()
            .unwrap();
        record
            .facts
            .into_iter()
            .find_map(|fact| match fact {
                Replayed::Step(step) if step.step_id == "read" => Some(step.input_digest),
                _ => None,
            })
            .expect("the read step is recorded")
    }

    /// What a worker reads from the environment it declares is part of what
    /// its step was given: two runs given the same value match, and a run
    /// given another value doesn't (§9, revise: `REGION=eu` then `REGION=us`).
    #[test]
    fn a_step_given_another_environment_value_is_given_something_else() {
        let root = tempfile::tempdir().unwrap();
        let skill = write_region(root.path());
        let env = |region: &str| {
            BTreeMap::from([
                (
                    crate::paths::RUNX_CWD_ENV.to_owned(),
                    root.path().display().to_string(),
                ),
                (
                    crate::paths::RUNX_HOME_ENV.to_owned(),
                    root.path().join("home").display().to_string(),
                ),
                ("REGION".to_owned(), region.to_owned()),
            ])
        };
        let eu = digest(root.path(), &skill, env("eu")).expect("a digest under a runx home");
        assert_eq!(digest(root.path(), &skill, env("eu")), Some(eu.clone()));
        assert_ne!(digest(root.path(), &skill, env("us")), Some(eu));
    }
}
