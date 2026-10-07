use super::{
    PackageHarnessReport, SkillRunError, SkillRunOverrides, execute_skill_run_with_overrides,
};

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{JsonValue, Receipt};
use runx_parser::harness_fixture::HarnessExpectedStatus;
use runx_parser::{HarnessCallerFixture, RunnerHarnessCase, SkillRunnerManifest};

use crate::RuntimeError;
use crate::effects::RuntimeEffectRegistry;
use crate::execution::harness::runner::HarnessReplayError;
use crate::execution::harness::{assert_json_expectation, assert_receipt_expectation, status_name};
use crate::execution::orchestrator::SkillRunRequest;
use crate::services::{ReceiptServices, WorkspaceEnv};

use super::resolution_answers::ResolutionAnswers;
use super::runner_manifest::selected_runner;
use super::state_store::read_graph_state;

mod package;

pub(crate) use package::run_package_harness_with_effects;

/// Run a validated skill's declared inline cases through the same execution
/// path as `runx skill`. Package admission and harness workspace preparation
/// happen once in the owning package entry point.
fn run_loaded_inline_harness_with_effects(
    loaded: &crate::LoadedSkillPackage,
    case_receipt_root: Option<&Path>,
    output_receipt_dir: Option<&Path>,
    env: &BTreeMap<String, String>,
    effects: &RuntimeEffectRegistry,
) -> Result<PackageHarnessReport, SkillRunError> {
    let manifest = loaded.manifest().cloned().ok_or_else(|| {
        SkillRunError::Invalid(format!(
            "skill package {} does not declare X.yaml runners",
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
        loaded,
        skill_dir: &loaded.directory,
        case_receipt_root,
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
    loaded: &'a crate::LoadedSkillPackage,
    skill_dir: &'a Path,
    case_receipt_root: Option<&'a Path>,
    output_receipt_dir: Option<&'a Path>,
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
    let mut graph_case_count = 0;
    for (index, case) in cases.iter().enumerate() {
        case_names.push(case.name.clone());
        let case_receipt_dir = context
            .case_receipt_root
            .map(|root| root.join(index.to_string()));
        let outcome = run_inline_harness_case(context, case_receipt_dir.as_deref(), case);
        if outcome.is_graph {
            graph_case_count += 1;
        }
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
        graph_case_count,
    }
}

struct InlineHarnessCaseOutcome {
    is_graph: bool,
    receipt_id: Option<String>,
    assertion_error: Option<String>,
}

fn run_inline_harness_case(
    context: InlineHarnessContext<'_>,
    receipt_dir: Option<&Path>,
    case: &RunnerHarnessCase,
) -> InlineHarnessCaseOutcome {
    let runner = match selected_runner(context.manifest, case.runner.as_deref()) {
        Ok(runner) => runner,
        Err(error) => return inline_harness_case_error(&case.name, error),
    };
    let is_graph = runner.source.source_type == runx_parser::SourceKind::Graph;

    // The harness executes below the normal preparation front, so admit the
    // fixture through the same complete contract before running any step.
    // Required fields, defaults, nested schemas, and packet bindings therefore
    // have one owner in both production and harness execution.
    let Ok(inputs) =
        crate::input_contract::materialize_complete_runner_inputs(&runner.inputs, &case.inputs)
    else {
        return InlineHarnessCaseOutcome {
            is_graph,
            receipt_id: None,
            assertion_error: inline_harness_status_error(case, HarnessExpectedStatus::Failure),
        };
    };

    let mut request = inline_harness_case_request(
        context.skill_dir,
        receipt_dir,
        context.env,
        case,
        context.cwd,
    );
    request.inputs = inputs;
    let seeded_answers = match seeded_answers_from_caller(&case.caller) {
        Ok(answers) => answers,
        Err(error) => return inline_harness_case_error(&case.name, error),
    };
    let overrides = SkillRunOverrides {
        runner: case.runner.clone(),
        seeded_answers,
        ..Default::default()
    };
    execute_inline_harness_case(context, &request, receipt_dir, case, runner, &overrides)
}

fn execute_inline_harness_case(
    context: InlineHarnessContext<'_>,
    request: &SkillRunRequest,
    receipt_dir: Option<&Path>,
    case: &RunnerHarnessCase,
    runner: &runx_parser::SkillRunnerDefinition,
    overrides: &SkillRunOverrides,
) -> InlineHarnessCaseOutcome {
    let is_graph = runner.source.source_type == runx_parser::SourceKind::Graph;
    let effects = match crate::execution::harness::effects_for_harness(
        context.effects,
        &case.caller.http_responses,
        &case.caller.http_exchanges,
        None,
        &case.caller.mcp_calls,
    ) {
        Ok(effects) => effects,
        Err(error) => {
            return InlineHarnessCaseOutcome {
                is_graph,
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
                && let (Some(receipt_dir), Some(output_receipt_dir)) =
                    (receipt_dir, context.output_receipt_dir)
                && let Err(error) =
                    persist_inline_case_receipts(request, receipt_dir, output_receipt_dir)
            {
                return InlineHarnessCaseOutcome {
                    is_graph,
                    receipt_id: None,
                    assertion_error: Some(format!(
                        "{}: failed to persist harness receipts: {error}",
                        case.name
                    )),
                };
            }
            InlineHarnessCaseOutcome {
                is_graph,
                receipt_id,
                assertion_error: inline_harness_expectation_error(
                    context.loaded,
                    request,
                    case,
                    is_graph,
                    &runner.name,
                    &output,
                ),
            }
        }
        Err(error) => InlineHarnessCaseOutcome {
            is_graph,
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
    let receipts = crate::services::ReceiptServices::from_env_or_local_development(&request.env)
        .map_err(|error| error.to_string())?;
    let produced = receipts
        .list_local_receipts(case_receipt_dir)
        .map_err(|error| error.to_string())?;
    receipts
        .write_local_receipts(&produced, output_receipt_dir)
        .map_err(|error| error.to_string())
}

fn inline_harness_case_request(
    skill_dir: &Path,
    receipt_dir: Option<&Path>,
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
        skill_path: skill_dir.to_path_buf(),
        receipt_dir: receipt_dir.map(Path::to_path_buf),
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
        is_graph: false,
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
    loaded: &crate::LoadedSkillPackage,
    request: &SkillRunRequest,
    case: &RunnerHarnessCase,
    is_graph: bool,
    runner_name: &str,
    output: &JsonValue,
) -> Option<String> {
    assert_inline_harness_expectations(loaded, request, case, is_graph, runner_name, output)
        .err()
        .map(|error| format!("{}: {error}", case.name))
}

fn assert_inline_harness_expectations(
    loaded: &crate::LoadedSkillPackage,
    request: &SkillRunRequest,
    case: &RunnerHarnessCase,
    is_graph: bool,
    runner_name: &str,
    output: &JsonValue,
) -> Result<(), HarnessReplayError> {
    let actual_status = inline_harness_actual_status(output);
    if let Some(expected_status) = &case.expect.status
        && *expected_status != actual_status
    {
        return Err(HarnessReplayError::Mismatch {
            field: "expect.status".to_owned(),
            expected: status_name(expected_status).to_owned(),
            actual: status_name(&actual_status).to_owned(),
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
    if !case.expect.steps.is_empty() {
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
        if case
            .expect
            .steps
            .iter()
            .map(String::as_str)
            .ne(actual_steps.iter().copied())
        {
            return Err(HarnessReplayError::Mismatch {
                field: "expect.steps".to_owned(),
                expected: case.expect.steps.join(","),
                actual: actual_steps.join(","),
            });
        }
    }
    if let Some(expectation) = &case.expect.output {
        assert_json_expectation(expectation, result, "expect.output")?;
    }
    let graph_state = if case.expect.step_outputs.is_empty() {
        None
    } else {
        if !is_graph {
            return Err(HarnessReplayError::InvalidReplayMetadata {
                field: "expect.step_outputs".to_owned(),
                message: "step output expectations require a graph runner".to_owned(),
            });
        }
        let run_id = output
            .as_object()
            .and_then(|object| object.get("run_id"))
            .and_then(JsonValue::as_str)
            .ok_or_else(|| HarnessReplayError::InvalidReplayMetadata {
                field: "run_id".to_owned(),
                message: "skill run output omitted its run id".to_owned(),
            })?;
        let (receipts, workspace) = harness_receipt_services(request)?;
        let closure = crate::skill_package::inspect_loaded_execution_closure_binding(
            loaded.clone().into(),
            runner_name,
            &request.env,
        )
        .map_err(|error| HarnessReplayError::InvalidReplayMetadata {
            field: "execution_closure_digest".to_owned(),
            message: error.to_string(),
        })?;
        Some(
            read_graph_state(
                request,
                &workspace,
                &receipts,
                run_id,
                runner_name,
                &loaded.package.package_digest,
                &closure.digest,
            )
            .map_err(|error| HarnessReplayError::InvalidReplayMetadata {
                field: "graph_state".to_owned(),
                message: error.to_string(),
            })?,
        )
    };
    for (step_id, expectation) in &case.expect.step_outputs {
        let actual = graph_state
            .as_ref()
            .and_then(|state| {
                state
                    .checkpoint
                    .steps
                    .iter()
                    .rev()
                    .find(|step| step.step_id == *step_id)
            })
            .map_or(JsonValue::Null, |step| {
                JsonValue::Object(step.contract.clone())
            });
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
    let receipts =
        ReceiptServices::from_env_or_local_development(&request.env).map_err(|error| {
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
    let path = receipts.resolve_path(workspace, request.receipt_dir.as_deref(), None);
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

// Preserve the harness caller's answer lanes while keying both by resolution
// request id. Inline execution must exercise the same approval provenance as a
// live resume file.
fn seeded_answers_from_caller(
    caller: &HarnessCallerFixture,
) -> Result<Option<ResolutionAnswers>, SkillRunError> {
    let answers = caller.answers.clone().unwrap_or_default();
    let approvals = caller
        .approvals
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|(gate, approved)| (gate, JsonValue::Bool(approved)));
    if answers.is_empty()
        && caller
            .approvals
            .as_ref()
            .is_none_or(|approvals| approvals.is_empty())
    {
        Ok(None)
    } else {
        ResolutionAnswers::from_lanes(answers, approvals).map(Some)
    }
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
    use runx_parser::HarnessCallerFixture;

    use super::seeded_answers_from_caller;

    #[test]
    fn inline_harness_keeps_approval_provenance_separate_from_agent_answers() -> Result<(), String>
    {
        let approval_id = "provider.write.approval";
        let caller = HarnessCallerFixture {
            answers: Some(JsonObject::from([(
                "agent.plan".to_owned(),
                JsonValue::String("ready".to_owned()),
            )])),
            approvals: Some(BTreeMap::from([(approval_id.to_owned(), true)])),
            http_responses: BTreeMap::new(),
            http_exchanges: Vec::new(),
            mcp_calls: Vec::new(),
        };
        let answers = seeded_answers_from_caller(&caller)
            .map_err(|error| error.to_string())?
            .ok_or("expected seeded answers")?;

        assert!(answers.is_human_approval(approval_id));
        assert!(!answers.is_human_approval("agent.plan"));
        assert_eq!(answers.get(approval_id), Some(&JsonValue::Bool(true)));
        Ok(())
    }
}
