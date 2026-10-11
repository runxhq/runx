use super::{
    PackageHarnessReport, SkillRunError, SkillRunOverrides, execute_skill_run_with_overrides,
};

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{JsonValue, Receipt};
use runx_parser::harness_fixture::HarnessExpectedStatus;
use runx_parser::{HarnessCaller, RunnerHarnessCase, SkillRunnerManifest};

use crate::RuntimeError;
use crate::effects::RuntimeEffectRegistry;
use crate::execution::harness::runner::HarnessReplayError;
use crate::execution::harness::{assert_json_expectation, assert_receipt_expectation, status_name};
use crate::execution::orchestrator::SkillRunRequest;
use crate::services::{ReceiptServices, WorkspaceEnv};

use super::resolution_answers::ResolutionAnswers;
use super::runner_manifest::selected_runner;

mod package;

pub(crate) use package::run_package_harness_with_effects;

/// Run a validated skill's declared inline cases through the same execution
/// path as `runx skill`. Package admission and harness workspace preparation
/// happen once in the owning package entry point.
fn run_loaded_inline_harness_with_effects(
    loaded: &crate::LoadedSkillPackage,
    cases_root: &Path,
    output_receipt_dir: &Path,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<PackageHarnessReport, SkillRunError> {
    let manifest = loaded.manifest().cloned().ok_or_else(|| {
        SkillRunError::Invalid(format!(
            "skill package {} does not declare run.yaml runners",
            loaded.directory.display()
        ))
    })?;
    let Some(harness) = manifest.harness.as_ref() else {
        return Ok(PackageHarnessReport::not_declared());
    };
    if harness.cases.is_empty() {
        return Ok(PackageHarnessReport::not_declared());
    }

    let workspace = WorkspaceEnv::from_admitted(env.clone()).map_err(RuntimeError::from)?;
    let context = InlineHarnessContext {
        skill_dir: &loaded.directory,
        cases_root,
        output_receipt_dir,
        env: workspace.env(),
        effects,
        manifest: &manifest,
        cwd: workspace.cwd(),
    };
    Ok(run_inline_harness_cases(context, &harness.cases))
}

#[derive(Clone, Copy)]
struct InlineHarnessContext<'a> {
    skill_dir: &'a Path,
    cases_root: &'a Path,
    output_receipt_dir: &'a Path,
    env: &'a BTreeMap<String, String>,
    effects: &'a RuntimeEffectRegistry,
    manifest: &'a SkillRunnerManifest,
    cwd: &'a Path,
}

fn run_inline_harness_cases(
    context: InlineHarnessContext<'_>,
    cases: &[RunnerHarnessCase],
) -> PackageHarnessReport {
    let mut assertion_errors = Vec::new();
    let mut case_names = Vec::with_capacity(cases.len());
    let mut receipt_ids = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        case_names.push(case.name.clone());
        let case_root = context.cases_root.join(index.to_string());
        let outcome = run_inline_harness_case(context, &case_root, case);
        if let Some(receipt_id) = outcome.receipt_id {
            receipt_ids.push(receipt_id);
        }
        if let Some(error) = outcome.assertion_error {
            assertion_errors.push(error);
        }
    }

    let status = if assertion_errors.is_empty() {
        "passed"
    } else {
        "failed"
    };
    PackageHarnessReport {
        assertion_error_count: assertion_errors.len(),
        status,
        case_count: cases.len(),
        assertion_errors,
        case_names,
        receipt_ids,
        // Every runner is a graph, so an inline case proves nothing about
        // composition; only `kind: graph` package fixtures count.
        graph_case_count: 0,
    }
}

struct InlineHarnessCaseOutcome {
    receipt_id: Option<String>,
    assertion_error: Option<String>,
}

fn run_inline_harness_case(
    context: InlineHarnessContext<'_>,
    case_root: &Path,
    case: &RunnerHarnessCase,
) -> InlineHarnessCaseOutcome {
    let receipt_dir = case_root.join("receipts");
    let runner = match selected_runner(context.manifest, case.runner.as_deref()) {
        Ok(runner) => runner,
        Err(error) => return inline_harness_case_error(&case.name, error),
    };
    // The harness executes below the normal preparation front, so admit the
    // fixture through the same complete contract before running any step.
    // Required fields, defaults, nested schemas, and packet bindings therefore
    // have one owner in both production and harness execution.
    let Ok(inputs) =
        crate::input_contract::materialize_complete_runner_inputs(&runner.inputs, &case.inputs)
    else {
        return InlineHarnessCaseOutcome {
            receipt_id: None,
            assertion_error: inline_harness_status_error(case, HarnessExpectedStatus::Failure),
        };
    };

    let mut request = inline_harness_case_request(
        context.skill_dir,
        &receipt_dir,
        context.env,
        case,
        context.cwd,
    );
    // Each case runs in a home and a project directory of its own: its runs,
    // effect custody and instances never reach another case or the operator.
    crate::paths::isolate(&mut request.env, case_root);
    request.inputs = inputs;
    match crate::execution::harness::provider_env(&case.caller) {
        Ok(env) => request.env.extend(env),
        Err(error) => return inline_harness_case_error(&case.name, error),
    }
    let seeded_answers = match seeded_answers_from_caller(&case.caller) {
        Ok(answers) => answers,
        Err(error) => return inline_harness_case_error(&case.name, error),
    };
    let overrides = SkillRunOverrides {
        runner: case.runner.clone(),
        seeded_answers,
        turn_scripts: case.caller.turns.clone(),
        ..Default::default()
    };
    execute_inline_harness_case(context, &request, &receipt_dir, case, &overrides)
}

fn execute_inline_harness_case(
    context: InlineHarnessContext<'_>,
    request: &SkillRunRequest,
    receipt_dir: &Path,
    case: &RunnerHarnessCase,
    overrides: &SkillRunOverrides,
) -> InlineHarnessCaseOutcome {
    let effects =
        match crate::execution::harness::effects_for_harness(context.effects, &case.caller) {
            Ok(effects) => effects,
            Err(error) => {
                return InlineHarnessCaseOutcome {
                    receipt_id: None,
                    assertion_error: Some(format!("{}: {error}", case.name)),
                };
            }
        };
    match execute_skill_run_with_overrides(request, overrides, &effects) {
        Ok(result) => {
            let output = result.output;
            let receipt_id = receipt_id_from_output(&output);
            if receipt_id.is_some()
                && let Err(error) =
                    persist_inline_case_receipts(request, receipt_dir, context.output_receipt_dir)
            {
                return InlineHarnessCaseOutcome {
                    receipt_id: None,
                    assertion_error: Some(format!(
                        "{}: failed to persist harness receipts: {error}",
                        case.name
                    )),
                };
            }
            InlineHarnessCaseOutcome {
                receipt_id,
                assertion_error: inline_harness_expectation_error(request, case, &output),
            }
        }
        Err(error) => InlineHarnessCaseOutcome {
            receipt_id: None,
            assertion_error: inline_harness_execution_error(case, &error),
        },
    }
}

fn persist_inline_case_receipts(
    request: &SkillRunRequest,
    case_receipt_dir: &Path,
    output_receipt_dir: &Path,
) -> Result<(), String> {
    let workspace = WorkspaceEnv::new(request.env.clone(), request.cwd.clone())
        .map_err(|error| error.to_string())?;
    let receipts = ReceiptServices::resolve(&workspace).map_err(|error| error.to_string())?;
    let produced = receipts
        .list_local_receipts(case_receipt_dir)
        .map_err(|error| error.to_string())?;
    receipts
        .write_local_receipts(&produced, output_receipt_dir)
        .map_err(|error| error.to_string())
}

fn inline_harness_case_request(
    skill_dir: &Path,
    receipt_dir: &Path,
    env: &BTreeMap<String, String>,
    case: &RunnerHarnessCase,
    cwd: &Path,
) -> SkillRunRequest {
    let mut env = env.clone();
    env.extend(case.env.clone());
    SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: case.caller.act.clone(),
        skill_path: skill_dir.to_path_buf(),
        receipt_dir: Some(receipt_dir.to_path_buf()),
        run_id: None,
        answers_path: None,
        inputs: case.inputs.clone(),
        env,
        cwd: cwd.to_path_buf(),
        managed_agent: crate::execution::orchestrator::ManagedAgentPolicy::HostDriven,
    }
}

fn inline_harness_case_error(
    case_name: &str,
    error: impl std::fmt::Display,
) -> InlineHarnessCaseOutcome {
    InlineHarnessCaseOutcome {
        receipt_id: None,
        assertion_error: Some(format!("{case_name}: {error}")),
    }
}

fn receipt_id_from_output(output: &JsonValue) -> Option<String> {
    output
        .as_object()
        .and_then(|object| object.get("receipt_id"))
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
}

fn inline_harness_expectation_error(
    request: &SkillRunRequest,
    case: &RunnerHarnessCase,
    output: &JsonValue,
) -> Option<String> {
    assert_inline_harness_expectations(request, case, output)
        .err()
        .map(|error| format!("{}: {error}", case.name))
}

fn assert_inline_harness_expectations(
    request: &SkillRunRequest,
    case: &RunnerHarnessCase,
    output: &JsonValue,
) -> Result<(), HarnessReplayError> {
    let actual_status = inline_harness_actual_status(output);
    if let Some(expected_status) = &case.expect.status
        && *expected_status != actual_status
    {
        // A failed run says why, so the case names what broke.
        let reason = output
            .as_object()
            .and_then(|object| object.get("error"))
            .and_then(JsonValue::as_object)
            .and_then(|error| error.get("message"))
            .and_then(JsonValue::as_str);
        let actual = match reason {
            Some(reason) => format!("{} ({reason})", status_name(&actual_status)),
            None => status_name(&actual_status).to_owned(),
        };
        return Err(HarnessReplayError::Mismatch {
            field: "expect.status".to_owned(),
            expected: status_name(expected_status).to_owned(),
            actual,
        });
    }

    if let Some(expected_receipt) = &case.expect.receipt {
        let (receipts, workspace) = harness_receipt_services(request)?;
        let actual_receipt = read_output_receipt(request, output, &receipts, &workspace)?;
        assert_receipt_expectation(
            expected_receipt,
            &actual_receipt,
            receipts.signature_config().signature_policy(),
        )?;
    }

    let result = output
        .as_object()
        .and_then(|object| object.get("result"))
        .unwrap_or(&JsonValue::Null);
    if let Some(expected_steps) = &case.expect.steps {
        let actual_steps = output
            .as_object()
            .and_then(|object| object.get("trace"))
            .and_then(JsonValue::as_object)
            .and_then(|trace| trace.get("steps"))
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
            .filter_map(|step| {
                step.as_object()
                    .and_then(|object| object.get("step_id"))
                    .and_then(JsonValue::as_str)
            })
            .collect::<Vec<_>>();
        if expected_steps
            .iter()
            .map(String::as_str)
            .ne(actual_steps.iter().copied())
        {
            return Err(HarnessReplayError::Mismatch {
                field: "expect.steps".to_owned(),
                expected: format!("{expected_steps:?}"),
                actual: format!("{actual_steps:?}"),
            });
        }
    }
    if let Some(expectation) = &case.expect.output {
        assert_json_expectation(expectation, result, "expect.output")?;
    }
    let record = if case.expect.step_outputs.is_empty() {
        None
    } else {
        let run_id = output
            .as_object()
            .and_then(|object| object.get("run_id"))
            .and_then(JsonValue::as_str)
            .ok_or_else(|| HarnessReplayError::InvalidReplayMetadata {
                field: "run_id".to_owned(),
                message: "skill run output omitted its run id".to_owned(),
            })?;
        let workspace = WorkspaceEnv::new(request.env.clone(), request.cwd.clone())
            .map_err(RuntimeError::from)?;
        let store = crate::kernel::store::KernelStore::for_project(workspace.paths())
            .map_err(crate::execution::run_record::RunRecordError::from)
            .map_err(RuntimeError::from)?;
        crate::execution::run_record::RunLog::open(store, run_id)
            .and_then(|log| log.load())
            .map_err(|error| HarnessReplayError::InvalidReplayMetadata {
                field: "run_record".to_owned(),
                message: error.to_string(),
            })?
    };
    for (step_id, expectation) in &case.expect.step_outputs {
        let actual = record
            .as_ref()
            .and_then(|record| {
                record.facts.iter().rev().find_map(|fact| match fact {
                    crate::execution::run_record::Replayed::Step(step)
                        if step.step_id == *step_id =>
                    {
                        Some(JsonValue::Object(step.contract.clone()))
                    }
                    _ => None,
                })
            })
            .unwrap_or(JsonValue::Null);
        assert_json_expectation(
            expectation,
            &actual,
            &format!("expect.step_outputs.{step_id}"),
        )?;
    }
    Ok(())
}

fn harness_receipt_services(
    request: &SkillRunRequest,
) -> Result<(ReceiptServices, WorkspaceEnv), HarnessReplayError> {
    let workspace =
        WorkspaceEnv::new(request.env.clone(), request.cwd.clone()).map_err(RuntimeError::from)?;
    let receipts = ReceiptServices::resolve(&workspace).map_err(|error| {
        HarnessReplayError::Runtime(RuntimeError::ReceiptInvalid {
            message: error.to_string(),
        })
    })?;
    Ok((receipts, workspace))
}

fn read_output_receipt(
    request: &SkillRunRequest,
    output: &JsonValue,
    receipts: &ReceiptServices,
    workspace: &WorkspaceEnv,
) -> Result<Receipt, HarnessReplayError> {
    let receipt_id = output
        .as_object()
        .and_then(|object| object.get("receipt_id"))
        .and_then(JsonValue::as_str)
        .ok_or_else(|| HarnessReplayError::InvalidReplayMetadata {
            field: "receipt_id".to_owned(),
            message: "skill run output omitted its receipt id".to_owned(),
        })?;
    let path = workspace.receipt_path(request.receipt_dir.as_deref());
    receipts
        .read_local_receipt(receipt_id, &path.path)
        .map_err(|error| HarnessReplayError::InvalidReplayMetadata {
            field: "receipt".to_owned(),
            message: error.to_string(),
        })
}

fn inline_harness_status_error(
    case: &RunnerHarnessCase,
    actual: HarnessExpectedStatus,
) -> Option<String> {
    let expected = case.expect.status.as_ref()?;
    (actual != *expected).then(|| {
        format!(
            "{}: expected status {}, got {}",
            case.name,
            status_name(expected),
            status_name(&actual)
        )
    })
}

fn inline_harness_execution_error(
    case: &RunnerHarnessCase,
    error: &impl std::fmt::Display,
) -> Option<String> {
    match case.expect.status.as_ref() {
        Some(HarnessExpectedStatus::Failure) => None,
        Some(expected) => Some(format!(
            "{}: expected status {}, execution failed: {error}",
            case.name,
            status_name(expected)
        )),
        None => Some(format!("{}: {error}", case.name)),
    }
}

// Preserve the harness caller's answer lanes, each keyed by the request's id
// in its own run: a case scripts its answers by what is asked, at any depth.
// Inline execution must exercise the same approval provenance as a live
// resume file.
fn seeded_answers_from_caller(
    caller: &HarnessCaller,
) -> Result<Option<ResolutionAnswers>, SkillRunError> {
    if caller.answers.is_empty() && caller.approvals.is_empty() {
        return Ok(None);
    }
    let approvals = caller
        .approvals
        .iter()
        .map(|(gate, approved)| (gate.clone(), JsonValue::Bool(*approved)));
    ResolutionAnswers::from_lanes(caller.answers.clone(), approvals)
        .map(|answers| Some(answers.scripted()))
}

// Map an `execute_skill_run` output onto the harness status vocabulary
// (sealed/failure/needs_agent/policy_denied). A pending run is needs_agent; a
// terminal run is derived from its closure disposition so the mapping matches
// the standalone harness `status_from_disposition`.
fn inline_harness_actual_status(output: &JsonValue) -> HarnessExpectedStatus {
    let Some(object) = output.as_object() else {
        return HarnessExpectedStatus::Sealed;
    };
    if object.get("status").and_then(JsonValue::as_str) == Some("needs_agent") {
        return HarnessExpectedStatus::NeedsAgent;
    }
    let disposition = object
        .get("closure")
        .and_then(JsonValue::as_object)
        .and_then(|closure| closure.get("disposition"))
        .and_then(JsonValue::as_str);
    match disposition {
        Some("deferred") => HarnessExpectedStatus::NeedsAgent,
        Some("blocked") => HarnessExpectedStatus::PolicyDenied,
        Some("declined" | "failed" | "killed" | "timed_out" | "superseded") => {
            HarnessExpectedStatus::Failure
        }
        _ => HarnessExpectedStatus::Sealed,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use runx_contracts::{JsonObject, JsonValue};
    use runx_parser::HarnessCaller;

    use super::seeded_answers_from_caller;

    #[test]
    fn inline_harness_keeps_approval_provenance_separate_from_agent_answers() -> Result<(), String>
    {
        let approval_id = "provider.write.approval";
        let caller = HarnessCaller {
            answers: JsonObject::from([(
                "agent.plan".to_owned(),
                JsonValue::String("ready".to_owned()),
            )]),
            approvals: BTreeMap::from([(approval_id.to_owned(), true)]),
            ..HarnessCaller::default()
        };
        let answers = seeded_answers_from_caller(&caller)
            .map_err(|error| error.to_string())?
            .ok_or("expected seeded answers")?;

        assert!(answers.is_human_approval(approval_id));
        assert!(!answers.is_human_approval("agent.plan"));
        assert_eq!(
            answers.get(approval_id, "sha256:approval"),
            Some(&JsonValue::Bool(true))
        );
        // A script answers an agent act by its id in its own run, at any
        // depth; an approval decides only the request it names.
        assert_eq!(
            answers.get("deliver#agent.plan", "sha256:plan"),
            Some(&JsonValue::String("ready".to_owned()))
        );
        assert_eq!(
            answers.get(&format!("deliver#{approval_id}"), "sha256:approval"),
            None
        );
        Ok(())
    }
}
