// Module rationale: skill CLI parsing keeps shared state and
// option finalization in one module until the native parser surface stabilizes.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use runx_contracts::JsonValue;
use runx_runtime::WorkspaceEnv;

use super::inputs::{CliInputs, parse_input_arg, parse_input_document_arg};
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
        return Err("runx skill --inputs cannot be combined with -i inputs".to_owned());
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
    // Inspect projects readiness under the authority a host would start the
    // run with; context and a money limit bind only a run.
    if action == SkillAction::Inspect && (state.context.is_some() || state.max_cost.is_some()) {
        return Err(
            "runx skill --context and --max-cost bind a run and are not accepted by inspect"
                .to_owned(),
        );
    }
    let run_authority = limit_run(state.run_authority, state.max_cost)?;
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
        registry: state.registry,
        expected_digest: state.expected_digest,
        expected_package_digest: state.expected_package_digest,
        expected_execution_closure_digest: state.expected_execution_closure_digest,
        json: state.json,
        diagnostics: state.diagnostics,
        inputs: state.inputs,
        input_document: state.input_document,
        act: state.act,
        run_authority,
        context: state.context,
        managed_agent,
    })
}

/// `--max-cost <usd>`: the most the run, its child runs included, may spend.
/// It sets the run authority's money limit, so a direct run gets an authority
/// that only bounds it.
fn limit_run(
    authority: Option<Box<runx_runtime::RunAuthority>>,
    max_cost_micro: Option<u64>,
) -> Result<Option<Box<runx_runtime::RunAuthority>>, String> {
    let Some(limit) = max_cost_micro else {
        return Ok(authority);
    };
    let mut authority = authority.unwrap_or_else(|| {
        Box::new(runx_runtime::RunAuthority {
            grants: std::collections::BTreeMap::new(),
            actor_ref: None,
            mandate_ref: None,
            delegations: Vec::new(),
            max_fanout: None,
            max_child_depth: None,
            agent_bounds: false,
            standing_approvals: Vec::new(),
            max_cost_micro: None,
        })
    });
    if authority
        .max_cost_micro
        .is_some_and(|stated| stated != limit)
    {
        return Err(
            "runx skill --max-cost and the --run-authority money limit differ; state the limit once"
                .to_owned(),
        );
    }
    authority.max_cost_micro = Some(limit);
    Ok(Some(authority))
}

/// Dollars as integer micro-USD: `2`, `0.5`, `0.005`. A limit is never a
/// float, so more than six decimals, a sign or an exponent is refused.
fn parse_max_cost(value: &str) -> Result<u64, String> {
    let refuse = || {
        format!(
            "runx skill --max-cost takes dollars with at most six decimals, such as 0.50: {value:?}"
        )
    };
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || fraction.len() > 6
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || (value.contains('.') && fraction.is_empty())
    {
        return Err(refuse());
    }
    let whole = whole.parse::<u64>().map_err(|_| refuse())?;
    let fraction = format!("{fraction:0<6}")
        .parse::<u64>()
        .map_err(|_| refuse())?;
    whole
        .checked_mul(1_000_000)
        .and_then(|micro| micro.checked_add(fraction))
        .ok_or_else(refuse)
}

/// `--context <json>`: the caller's context for the run, as the
/// `current_context` and `historical_context` an agent context envelope
/// carries. Context informs the run and its children; it never grants
/// authority.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallerContext {
    #[serde(default)]
    pub current_context: Vec<runx_contracts::ContextEntry>,
    #[serde(default)]
    pub historical_context: Vec<runx_contracts::ContextEntry>,
}

fn parse_context(value: &str) -> Result<Box<CallerContext>, String> {
    serde_json::from_str(value)
        .map_err(|error| format!("runx skill --context must be JSON caller context: {error}"))
}

/// `--run-authority <json>`: a trusted host's authority for the run.
fn parse_run_authority(value: &str) -> Result<Box<runx_runtime::RunAuthority>, String> {
    serde_json::from_str(value).map_err(|error| {
        format!("runx skill --run-authority must be a JSON run authority: {error}")
    })
}

/// `--act <json>`: the act the invoker states for the run.
fn parse_act(value: &str) -> Result<runx_contracts::CallAct, String> {
    let act = serde_json::from_str::<JsonValue>(value)
        .map_err(|error| format!("runx skill --act must be a JSON act: {error}"))?;
    runx_contracts::CallAct::parse(&act).map_err(|error| format!("runx skill --act: {error}"))
}

#[derive(Default)]
struct SkillParseState {
    skill_path: Option<PathBuf>,
    runner: Option<String>,
    receipt_dir: Option<PathBuf>,
    run_id: Option<String>,
    registry: Option<String>,
    expected_digest: Option<String>,
    expected_package_digest: Option<String>,
    expected_execution_closure_digest: Option<String>,
    json: bool,
    diagnostics: bool,
    inspect: bool,
    inputs: CliInputs,
    input_document: Option<crate::document_input::DocumentInputSource>,
    act: Option<runx_contracts::CallAct>,
    run_authority: Option<Box<runx_runtime::RunAuthority>>,
    max_cost: Option<u64>,
    context: Option<Box<CallerContext>>,
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
        "--inputs" => {
            index = parse_input_document_arg(args, index, None, &mut state.input_document)?;
        }
        "-i" => index = parse_input_arg(args, index, None, &mut state.inputs)?,
        value if value.starts_with("--act=") => {
            state.act = Some(parse_act(value.trim_start_matches("--act="))?);
        }
        "--act" => {
            index += 1;
            state.act = Some(parse_act(&string_arg(args, index)?)?);
        }
        value if value.starts_with("--run-authority=") => {
            state.run_authority = Some(parse_run_authority(
                value.trim_start_matches("--run-authority="),
            )?);
        }
        "--run-authority" => {
            index += 1;
            state.run_authority = Some(parse_run_authority(&string_arg(args, index)?)?);
        }
        value if value.starts_with("--context=") => {
            state.context = Some(parse_context(value.trim_start_matches("--context="))?);
        }
        "--context" => {
            index += 1;
            state.context = Some(parse_context(&string_arg(args, index)?)?);
        }
        "--run-id" => {
            index += 1;
            state.run_id = Some(string_arg(args, index)?);
        }
        value if value.starts_with("--max-cost=") => {
            state.max_cost = Some(parse_max_cost(value.trim_start_matches("--max-cost="))?);
        }
        "--max-cost" => {
            index += 1;
            state.max_cost = Some(parse_max_cost(&string_arg(args, index)?)?);
        }
        "--json" | "-j" => state.json = true,
        "--diagnostics" => state.diagnostics = true,
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
        value if value.starts_with('-') => {
            return Err(format!(
                "unknown runx skill flag {value}; give inputs as -i name=value"
            ));
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

    fn plan(args: &[&str]) -> Result<super::SkillPlan, String> {
        super::parse_skill_plan(
            &args
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn max_cost_bounds_a_direct_run_in_micro_usd() -> Result<(), String> {
        let limited = plan(&["skill", "skills/research", "--max-cost", "0.25"])?;
        let authority = limited.run_authority.ok_or("no run authority")?;
        assert_eq!(authority.max_cost_micro, Some(250_000));
        assert_eq!(authority.actor_ref, None, "a limit names no actor");
        assert_eq!(
            plan(&["skill", "skills/research", "--max-cost=2"])?
                .run_authority
                .and_then(|authority| authority.max_cost_micro),
            Some(2_000_000)
        );
        assert_eq!(
            plan(&["skill", "skills/research", "--max-cost=0.000001"])?
                .run_authority
                .and_then(|authority| authority.max_cost_micro),
            Some(1)
        );
        for refused in ["-1", "0.0000001", "1e3", "", ".5", "5.", "$2", "0.5x"] {
            let Err(error) = plan(&["skill", "skills/research", "--max-cost", refused]) else {
                return Err(format!("--max-cost {refused:?} parsed"));
            };
            assert!(error.contains("--max-cost takes dollars"), "{error}");
        }
        Ok(())
    }

    #[test]
    fn max_cost_joins_a_host_authority_and_never_contradicts_it() -> Result<(), String> {
        let host =
            r#"{"actor_ref":{"type":"principal","uri":"runx:principal:assistant"},"max_fanout":2}"#;
        let joined = plan(&[
            "skill",
            "skills/research",
            "--run-authority",
            host,
            "--max-cost",
            "1",
        ])?
        .run_authority
        .ok_or("no run authority")?;
        assert_eq!(
            (joined.max_fanout, joined.max_cost_micro),
            (Some(2), Some(1_000_000))
        );
        assert!(joined.actor_ref.is_some());

        let stated = r#"{"actor_ref":{"type":"principal","uri":"runx:principal:assistant"},"max_cost_micro":500000}"#;
        let Err(error) = plan(&[
            "skill",
            "skills/research",
            "--run-authority",
            stated,
            "--max-cost",
            "1",
        ]) else {
            return Err("two different limits parsed".to_owned());
        };
        assert!(error.contains("state the limit once"), "{error}");
        let Err(error) = plan(&["skill", "inspect", "skills/research", "--max-cost", "1"]) else {
            return Err("inspect took a money limit".to_owned());
        };
        assert!(error.contains("bind a run"), "{error}");
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
            state.inputs.texts.get("claim").map(String::as_str),
            Some("abc")
        );
        assert_eq!(
            state.receipt_dir.as_deref(),
            Some(std::path::Path::new("receipts"))
        );
        assert!(state.json);
        Ok(())
    }

    #[test]
    fn skill_without_inputs_executes_default_runner() -> Result<(), String> {
        let args = ["skill", "skills/nws-weather-forecast"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Run);
        assert_eq!(
            plan.skill_path,
            std::path::PathBuf::from("skills/nws-weather-forecast")
        );
        assert_eq!(plan.runner, None);
        Ok(())
    }

    #[test]
    fn positional_runner_without_inputs_executes_runner() -> Result<(), String> {
        let args = ["skill", "skills/nws-weather-forecast", "normalize"]
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
        let args = [
            "skill",
            "inspect",
            "skills/nws-weather-forecast",
            "normalize",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Inspect);
        assert_eq!(
            plan.skill_path,
            std::path::PathBuf::from("skills/nws-weather-forecast")
        );
        assert_eq!(plan.runner.as_deref(), Some("normalize"));
        Ok(())
    }

    #[test]
    fn positional_runner_with_inputs_executes_runner() -> Result<(), String> {
        let args = [
            "skill",
            "skills/nws-weather-forecast",
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
            plan.inputs.texts.get("location").map(String::as_str),
            Some("hello")
        );
        Ok(())
    }

    #[test]
    fn bound_execution_requires_package_and_closure_digests() -> Result<(), String> {
        let package_only = [
            "skill",
            "skills/nws-weather-forecast",
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
            "skills/nws-weather-forecast",
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

    /// A host's caller context binds the run, never its inputs, and is
    /// refused when it names anything but the two context lists.
    #[test]
    fn caller_context_is_a_host_binding_not_a_skill_input() -> Result<(), String> {
        let args = |context: &str| {
            ["skill", "skills/ops-desk", "--context", context]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        };
        let plan = super::parse_skill_plan(&args(r#"{"current_context":[]}"#))?;
        assert_eq!(
            plan.context.as_deref(),
            Some(&super::CallerContext::default())
        );
        assert!(plan.inputs.is_empty());
        let refused = super::parse_skill_plan(&args(r#"{"memory":[]}"#))
            .err()
            .ok_or("an unknown context field was accepted")?;
        assert!(refused.contains("--context"), "{refused}");
        Ok(())
    }

    #[test]
    fn execution_bindings_are_runtime_metadata_not_skill_inputs() -> Result<(), String> {
        let args = [
            "skill",
            "skills/issue-to-pr",
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

    /// A flag runx does not own is refused, never read as an input, so an
    /// input is never mistaken for a flag nor a flag for an input.
    #[test]
    fn an_unknown_flag_is_refused_not_read_as_an_input() -> Result<(), String> {
        for flag in ["--runner", "--input-json", "--location", "--run"] {
            let args = ["skill", "skills/nws-weather-forecast", flag, "value"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>();
            let refused = super::parse_skill_plan(&args)
                .err()
                .ok_or(format!("{flag} was accepted"))?;
            assert!(
                refused.contains("give inputs as -i name=value"),
                "{refused}"
            );
        }
        Ok(())
    }
}
