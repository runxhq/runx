// Module rationale: skill CLI parsing keeps shared state and
// option finalization in one module until the native parser surface stabilizes.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use runx_contracts::JsonValue;
use runx_runtime::WorkspaceEnv;

use super::inputs::{
    parse_direct_input_arg, parse_input_arg, parse_input_document_arg, parse_json_input_arg,
};
use super::{SkillAction, SkillPlan};
use crate::managed_agent::{managed_agent_policy, parse_boolean_flag, parse_managed_agent_rounds};

pub fn parse_skill_plan(args: &[OsString]) -> Result<SkillPlan, String> {
    parse_skill_plan_inner(args)
}

pub fn parse_skill_plan_with_workspace(
    args: &[OsString],
    _workspace: &WorkspaceEnv,
) -> Result<SkillPlan, String> {
    parse_skill_plan_inner(args)
}

fn parse_skill_plan_inner(args: &[OsString]) -> Result<SkillPlan, String> {
    let mut state = SkillParseState::default();
    let mut index = 1;

    while index < args.len() {
        index = parse_skill_arg(args, index, &mut state)?;
        index += 1;
    }

    if state.input_document.is_some() && !state.inputs.is_empty() {
        return Err(
            "runx skill --inputs cannot be combined with -i, --input, --input-json, or direct --name inputs"
                .to_owned(),
        );
    }

    let Some(skill_path) = state.skill_path.as_ref() else {
        return Err("runx skill requires a skill package path".to_owned());
    };
    reject_resolver_flags_for_skill_management_action(skill_path, &state)?;
    let skill_path = skill_path.clone();
    let action = if state.inspect {
        SkillAction::Inspect
    } else {
        SkillAction::Run
    };
    if action == SkillAction::Inspect
        && (state.expected_package_digest.is_some()
            || state.expected_execution_closure_digest.is_some())
    {
        return Err(
            "runx skill --package-digest and --execution-closure-digest bind execution and are not accepted by inspect"
                .to_owned(),
        );
    }
    if action == SkillAction::Run
        && (state.expected_package_digest.is_some()
            != state.expected_execution_closure_digest.is_some())
    {
        return Err(
            "runx skill bound execution requires both --package-digest and --execution-closure-digest"
                .to_owned(),
        );
    }
    let managed_agent =
        managed_agent_policy("skill", state.managed_agent, state.managed_agent_rounds)?;

    Ok(SkillPlan {
        action,
        skill_path,
        runner: state.runner,
        receipt_dir: state.receipt_dir,
        run_id: state.run_id,
        answers: state.answers,
        registry: state.registry,
        expected_digest: state.expected_digest,
        expected_package_digest: state.expected_package_digest,
        expected_execution_closure_digest: state.expected_execution_closure_digest,
        json: state.json,
        diagnostics: state.diagnostics,
        trusted_command_execution: false,
        full_operator_context: state.full_operator_context,
        inputs: state.inputs,
        input_document: state.input_document,
        managed_agent,
    })
}

#[derive(Default)]
struct SkillParseState {
    skill_path: Option<PathBuf>,
    runner: Option<String>,
    receipt_dir: Option<PathBuf>,
    run_id: Option<String>,
    answers: Option<PathBuf>,
    registry: Option<String>,
    expected_digest: Option<String>,
    expected_package_digest: Option<String>,
    expected_execution_closure_digest: Option<String>,
    json: bool,
    diagnostics: bool,
    full_operator_context: bool,
    inspect: bool,
    force_run: bool,
    inputs: BTreeMap<String, JsonValue>,
    input_document: Option<crate::document_input::DocumentInputSource>,
    managed_agent: bool,
    managed_agent_rounds: Option<u32>,
}

fn reject_resolver_flags_for_skill_management_action(
    skill_path: &Path,
    state: &SkillParseState,
) -> Result<(), String> {
    if state.registry.is_none()
        && state.expected_digest.is_none()
        && state.expected_package_digest.is_none()
        && state.expected_execution_closure_digest.is_none()
    {
        return Ok(());
    }
    if !is_skill_management_action(skill_path) {
        return Ok(());
    }
    Err(
        "runx skill --registry, --digest, --package-digest, and --execution-closure-digest are only supported when running a skill ref"
            .to_owned(),
    )
}

fn is_skill_management_action(skill_path: &Path) -> bool {
    if skill_path.components().count() != 1 {
        return false;
    }
    matches!(
        skill_path.to_str(),
        Some("add" | "inspect" | "publish" | "search" | "validate")
    )
}

// Function rationale: this is the single skill-flag dispatch
// match (--receipt-dir/--json and positionals); splitting the
// arms would scatter the CLI parse contract.
fn parse_skill_arg(
    args: &[OsString],
    mut index: usize,
    state: &mut SkillParseState,
) -> Result<usize, String> {
    if args
        .get(index)
        .is_some_and(|value| value.to_str().is_none())
    {
        if state.skill_path.is_none() {
            state.skill_path = Some(PathBuf::from(args[index].clone()));
            return Ok(index);
        }
        return Err("runx skill runner names and option values must be UTF-8".to_owned());
    }
    let token = string_arg(args, index)?;
    match token.as_str() {
        value if value.starts_with("--receipt-dir=") => {
            state.receipt_dir = Some(PathBuf::from(value.trim_start_matches("--receipt-dir=")));
        }
        "--receipt-dir" | "-R" => {
            index += 1;
            state.receipt_dir = Some(PathBuf::from(path_arg(args, index, &token)?));
        }
        value if value.starts_with("--registry=") => {
            state.registry = Some(non_empty_flag_value(
                "--registry",
                value.trim_start_matches("--registry="),
            )?);
        }
        "--registry" => {
            index += 1;
            state.registry = Some(non_empty_flag_value(
                "--registry",
                &string_arg(args, index)?,
            )?);
        }
        value if value.starts_with("--digest=") => {
            state.expected_digest = Some(non_empty_flag_value(
                "--digest",
                value.trim_start_matches("--digest="),
            )?);
        }
        "--digest" => {
            index += 1;
            state.expected_digest =
                Some(non_empty_flag_value("--digest", &string_arg(args, index)?)?);
        }
        value if value.starts_with("--package-digest=") => {
            state.expected_package_digest = Some(non_empty_flag_value(
                "--package-digest",
                value.trim_start_matches("--package-digest="),
            )?);
        }
        "--package-digest" => {
            index += 1;
            state.expected_package_digest = Some(non_empty_flag_value(
                "--package-digest",
                &string_arg(args, index)?,
            )?);
        }
        value if value.starts_with("--execution-closure-digest=") => {
            state.expected_execution_closure_digest = Some(non_empty_flag_value(
                "--execution-closure-digest",
                value.trim_start_matches("--execution-closure-digest="),
            )?);
        }
        "--execution-closure-digest" => {
            index += 1;
            state.expected_execution_closure_digest = Some(non_empty_flag_value(
                "--execution-closure-digest",
                &string_arg(args, index)?,
            )?);
        }
        value if value.starts_with("--input=") => {
            index = parse_input_arg(
                args,
                index,
                Some(value.trim_start_matches("--input=")),
                &mut state.inputs,
            )?;
        }
        value if value.starts_with("--input-json=") => {
            index = parse_json_input_arg(
                args,
                index,
                Some(value.trim_start_matches("--input-json=")),
                &mut state.inputs,
            )?;
        }
        value if value.starts_with("--inputs=") => {
            index = parse_input_document_arg(
                args,
                index,
                Some(value.trim_start_matches("--inputs=")),
                &mut state.input_document,
            )?;
        }
        value if value.starts_with("-i=") => {
            index = parse_input_arg(
                args,
                index,
                Some(value.trim_start_matches("-i=")),
                &mut state.inputs,
            )?;
        }
        "--input" => index = parse_input_arg(args, index, None, &mut state.inputs)?,
        "--input-json" => index = parse_json_input_arg(args, index, None, &mut state.inputs)?,
        "--inputs" => {
            index = parse_input_document_arg(args, index, None, &mut state.input_document)?;
        }
        "-i" => index = parse_input_arg(args, index, None, &mut state.inputs)?,
        "--run" => state.force_run = true,
        "--json" | "-j" => state.json = true,
        "--diagnostics" => state.diagnostics = true,
        value if value.starts_with("--full-operator-context=") => {
            state.full_operator_context = parse_boolean_flag(
                "skill",
                "--full-operator-context",
                value.trim_start_matches("--full-operator-context="),
            )?;
        }
        "--full-operator-context" => state.full_operator_context = true,
        value if value.starts_with("--managed-agent=") => {
            state.managed_agent = parse_boolean_flag(
                "skill",
                "--managed-agent",
                value.trim_start_matches("--managed-agent="),
            )?;
        }
        "--managed-agent" => state.managed_agent = true,
        value if value.starts_with("--managed-agent-rounds=") => {
            state.managed_agent_rounds = Some(parse_managed_agent_rounds(
                "skill",
                value.trim_start_matches("--managed-agent-rounds="),
            )?);
        }
        "--managed-agent-rounds" => {
            index += 1;
            state.managed_agent_rounds = Some(parse_managed_agent_rounds(
                "skill",
                &string_arg(args, index)?,
            )?);
        }
        value if value.starts_with("--") => {
            index = parse_direct_input_arg(args, index, value, &mut state.inputs)?;
        }
        value => {
            if state.skill_path.is_none() && value == "inspect" {
                state.inspect = true;
            } else if state.skill_path.is_none() {
                state.skill_path = Some(PathBuf::from(value));
            } else if state.runner.is_none() {
                state.runner = Some(value.to_owned());
            } else {
                return Err(format!("unexpected runx skill argument {value}"));
            }
        }
    }
    Ok(index)
}

fn non_empty_flag_value(flag: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("runx skill {flag} requires a non-empty value"));
    }
    Ok(value.to_owned())
}

fn string_arg(args: &[OsString], index: usize) -> Result<String, String> {
    let value = args
        .get(index)
        .ok_or_else(|| "missing value for runx skill argument".to_owned())?;
    value
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| "runx skill arguments must be UTF-8".to_owned())
}

fn path_arg(args: &[OsString], index: usize, flag: &str) -> Result<OsString, String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("runx skill {flag} requires a path"))
}

#[cfg(test)]
mod tests {
    use super::{SkillAction, SkillParseState};

    #[test]
    fn managed_agent_requires_explicit_consent_and_binds_round_budget() -> Result<(), String> {
        let plan = super::parse_skill_plan(
            &[
                "skill",
                "skills/release",
                "--managed-agent",
                "--managed-agent-rounds=3",
            ]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>(),
        )?;
        assert_eq!(
            plan.managed_agent,
            runx_runtime::ManagedAgentPolicy::Inline { max_rounds: 3 }
        );

        let Err(error) = super::parse_skill_plan(
            &["skill", "skills/release", "--managed-agent-rounds=3"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>(),
        ) else {
            return Err("round budget without consent unexpectedly parsed".to_owned());
        };
        assert!(error.contains("requires --managed-agent"));
        Ok(())
    }

    #[test]
    fn short_human_flags_parse_for_skill_runs() -> Result<(), String> {
        let args = ["skill", "-i", "claim=abc", "-R", "receipts", "-j"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let mut state = SkillParseState::default();
        let mut index = 1;
        while index < args.len() {
            index = super::parse_skill_arg(&args, index, &mut state)?;
            index += 1;
        }
        assert_eq!(
            state.inputs.get("claim"),
            Some(&runx_contracts::JsonValue::String("abc".to_owned()))
        );
        assert_eq!(
            state.receipt_dir.as_deref(),
            Some(std::path::Path::new("receipts"))
        );
        assert!(state.json);
        Ok(())
    }

    #[test]
    fn input_json_parses_strict_json_values() -> Result<(), String> {
        let args = [
            "skill",
            "skills/data-store",
            "--input-json",
            "event",
            r#"{"type":"posting.claimed","payload":{"actor":"agent-9"}}"#,
            "--input-json=limits={\"rows\":10}",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;
        assert_eq!(plan.action, SkillAction::Run);

        assert_eq!(
            plan.inputs
                .get("event")
                .and_then(runx_contracts::JsonValue::as_object)
                .and_then(|event| event.get("type"))
                .and_then(runx_contracts::JsonValue::as_str),
            Some("posting.claimed")
        );
        let rows = plan
            .inputs
            .get("limits")
            .and_then(runx_contracts::JsonValue::as_object)
            .and_then(|limits| limits.get("rows"))
            .and_then(|rows| match rows {
                runx_contracts::JsonValue::Number(number) => number.as_f64(),
                _ => None,
            });
        assert_eq!(rows, Some(10.0));
        Ok(())
    }

    #[test]
    fn skill_without_inputs_executes_default_runner() -> Result<(), String> {
        let args = ["skill", "skills/weather-forecast"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Run);
        assert_eq!(
            plan.skill_path,
            std::path::PathBuf::from("skills/weather-forecast")
        );
        assert_eq!(plan.runner, None);
        Ok(())
    }

    #[test]
    fn positional_runner_without_inputs_executes_runner() -> Result<(), String> {
        let args = ["skill", "skills/weather-forecast", "normalize"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Run);
        assert_eq!(plan.runner.as_deref(), Some("normalize"));
        Ok(())
    }

    #[test]
    fn explicit_inspect_returns_skill_card() -> Result<(), String> {
        let args = ["skill", "inspect", "skills/weather-forecast", "normalize"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Inspect);
        assert_eq!(
            plan.skill_path,
            std::path::PathBuf::from("skills/weather-forecast")
        );
        assert_eq!(plan.runner.as_deref(), Some("normalize"));
        Ok(())
    }

    #[test]
    fn positional_runner_with_inputs_executes_runner() -> Result<(), String> {
        let args = [
            "skill",
            "skills/weather-forecast",
            "normalize",
            "-i",
            "location=hello",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Run);
        assert_eq!(plan.runner.as_deref(), Some("normalize"));
        assert_eq!(
            plan.inputs.get("location"),
            Some(&runx_contracts::JsonValue::String("hello".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn bound_execution_requires_package_and_closure_digests() -> Result<(), String> {
        let package_only = [
            "skill",
            "skills/weather-forecast",
            "--package-digest",
            "sha256:package",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
        let error = match super::parse_skill_plan(&package_only) {
            Err(error) => error,
            Ok(_) => return Err("a package digest formed a partial execution binding".to_owned()),
        };
        assert!(error.contains("requires both --package-digest"));

        let closure_only = [
            "skill",
            "skills/weather-forecast",
            "--execution-closure-digest",
            "sha256:closure",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
        let error = match super::parse_skill_plan(&closure_only) {
            Err(error) => error,
            Ok(_) => return Err("a closure digest formed a partial execution binding".to_owned()),
        };
        assert!(error.contains("requires both --package-digest"));
        Ok(())
    }

    #[test]
    fn full_operator_context_flag_is_not_a_skill_input() -> Result<(), String> {
        let args = [
            "skill",
            "skills/weather-forecast",
            "--full-operator-context",
            "--input",
            "location=hello",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert!(plan.full_operator_context);
        assert_eq!(plan.inputs.len(), 1);
        assert!(plan.inputs.contains_key("location"));
        Ok(())
    }

    #[test]
    fn inline_operator_context_flags_are_not_skill_inputs() -> Result<(), String> {
        let args = [
            "skill",
            "skills/weather-forecast",
            "--full-operator-context=true",
            "--input",
            "location=hello",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert!(plan.full_operator_context);
        assert_eq!(plan.inputs.len(), 1);
        assert!(plan.inputs.contains_key("location"));
        Ok(())
    }

    #[test]
    fn run_flag_executes_zero_input_runner() -> Result<(), String> {
        let args = ["skill", "skills/ops-desk", "refresh", "--run"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Run);
        assert_eq!(plan.runner.as_deref(), Some("refresh"));
        Ok(())
    }

    #[test]
    fn execution_bindings_are_runtime_metadata_not_skill_inputs() -> Result<(), String> {
        let args = [
            "skill",
            "skills/nitrosend",
            "status",
            "--package-digest",
            "sha256:package",
            "--execution-closure-digest",
            "sha256:closure",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(
            plan.expected_package_digest.as_deref(),
            Some("sha256:package"),
        );
        assert_eq!(
            plan.expected_execution_closure_digest.as_deref(),
            Some("sha256:closure"),
        );
        assert!(plan.inputs.is_empty());
        Ok(())
    }

    #[test]
    fn input_json_rejects_non_json_values() -> Result<(), String> {
        let args = [
            "skill",
            "skills/data-store",
            "--input-json",
            "event",
            "plain text",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
        let Err(error) = super::parse_skill_plan(&args) else {
            return Err("invalid json input should fail".to_owned());
        };

        assert!(error.contains("--input-json event is invalid JSON"));
        Ok(())
    }
}
