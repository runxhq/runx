// Module rationale: skill CLI parsing keeps shared state and
// option finalization in one module until the native parser surface stabilizes.
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use runx_contracts::JsonValue;
use runx_runtime::WorkspaceEnv;

use super::inputs::{CliInputs, parse_input_arg, parse_input_document_arg};
use super::method_ref::MethodRef;
use super::{NamedAuthority, SkillAction, SkillPlan};
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

    let Some(target) = state.target.as_ref() else {
        return Err("runx skill requires a skill package path".to_owned());
    };
    reject_resolver_flags_for_skill_management_action(&target.skill_path, &state)?;
    let MethodRef { skill_path, method } = target.clone();
    let action = if state.inspect {
        SkillAction::Inspect
    } else {
        SkillAction::Run
    };
    // Inspect projects readiness under the authority a host would start the
    // run with; context and a money limit bind only a run.
    if action == SkillAction::Inspect && (state.context.is_some() || state.max_cost.is_some()) {
        return Err(
            "runx skill --context and --max-spend bind a run and are not accepted by inspect"
                .to_owned(),
        );
    }
    // Authority is given one way, by a saved authority the run names; with
    // none, `--max-spend` gives the run an authority that only bounds it.
    let (run_authority, authority) = match state.authority {
        Some(name) => (
            None,
            Some(NamedAuthority {
                name,
                max_spend_micro_usd: state.max_cost,
            }),
        ),
        None => (limit_run(None, state.max_cost)?, None),
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
        method,
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
        authority,
        context: state.context,
        managed_agent,
    })
}

/// `--max-spend <usd>`: the most the run, its child runs included, may spend.
/// It sets the money limit of the authority the run names, or gives a run
/// that names none an authority that only bounds it.
pub(super) fn limit_run(
    authority: Option<Box<runx_runtime::RunAuthority>>,
    max_spend_micro_usd: Option<u64>,
) -> Result<Option<Box<runx_runtime::RunAuthority>>, String> {
    let Some(limit) = max_spend_micro_usd else {
        return Ok(authority);
    };
    let mut authority = authority.unwrap_or_default();
    if authority
        .max_spend_micro_usd
        .is_some_and(|stated| stated != limit)
    {
        return Err(
            "runx skill --max-spend and the saved authority's money limit differ; state the limit once"
                .to_owned(),
        );
    }
    authority.max_spend_micro_usd = Some(limit);
    Ok(Some(authority))
}

/// Dollars as integer micro-USD: `2`, `0.5`, `0.005`. A limit is never a
/// float, so more than six decimals, a sign or an exponent is refused.
fn parse_max_cost(value: &str) -> Result<u64, String> {
    let refuse = || {
        format!(
            "runx skill --max-spend takes dollars with at most six decimals, such as 0.50: {value:?}"
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

/// `--act <json>`: the act the invoker states for the run.
fn parse_act(value: &str) -> Result<runx_contracts::CallAct, String> {
    let act = serde_json::from_str::<JsonValue>(value)
        .map_err(|error| format!("runx skill --act must be a JSON act: {error}"))?;
    runx_contracts::CallAct::parse(&act).map_err(|error| format!("runx skill --act: {error}"))
}

#[derive(Default)]
struct SkillParseState {
    target: Option<MethodRef>,
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
    authority: Option<String>,
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
        if state.target.is_none() {
            state.target = Some(MethodRef::parse(&args[index], "skill")?);
            return Ok(index);
        }
        return Err("runx skill option values must be UTF-8".to_owned());
    }
    let token = string_arg(args, index)?;
    match token.as_str() {
        value if value.starts_with("--receipt-dir=") => {
            state.receipt_dir = Some(PathBuf::from(value.trim_start_matches("--receipt-dir=")));
        }
        "--receipt-dir" => {
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
        value if value.starts_with("--authority=") => {
            state.authority = Some(value.trim_start_matches("--authority=").to_owned());
        }
        "--authority" => {
            index += 1;
            state.authority = Some(string_arg(args, index)?);
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
        value if value.starts_with("--max-spend=") => {
            state.max_cost = Some(parse_max_cost(value.trim_start_matches("--max-spend="))?);
        }
        "--max-spend" => {
            index += 1;
            state.max_cost = Some(parse_max_cost(&string_arg(args, index)?)?);
        }
        "--json" | "-j" => state.json = true,
        "--diagnostics" => state.diagnostics = true,
        value if value.starts_with("--drive=") => {
            state.managed_agent =
                parse_boolean_flag("skill", "--drive", value.trim_start_matches("--drive="))?;
        }
        "--drive" => state.managed_agent = true,
        value if value.starts_with("--drive-rounds=") => {
            state.managed_agent_rounds = Some(parse_managed_agent_rounds(
                "skill",
                value.trim_start_matches("--drive-rounds="),
            )?);
        }
        "--drive-rounds" => {
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
            if state.target.is_none() && value == "inspect" {
                state.inspect = true;
            } else if state.target.is_none() {
                state.target = Some(MethodRef::parse(OsStr::new(value), "skill")?);
            } else {
                return Err(format!(
                    "unexpected runx skill argument {value}; name a method as <ref>#<method>"
                ));
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
            &["skill", "skills/release", "--drive", "--drive-rounds=3"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>(),
        )?;
        assert_eq!(
            plan.managed_agent,
            runx_runtime::ManagedAgentPolicy::Inline { max_rounds: 3 }
        );

        let Err(error) = super::parse_skill_plan(
            &["skill", "skills/release", "--drive-rounds=3"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>(),
        ) else {
            return Err("round budget without consent unexpectedly parsed".to_owned());
        };
        assert!(error.contains("requires --drive"));
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
        let limited = plan(&["skill", "skills/research", "--max-spend", "0.25"])?;
        let authority = limited.run_authority.ok_or("no run authority")?;
        assert_eq!(authority.max_spend_micro_usd, Some(250_000));
        assert_eq!(authority.actor_ref, None, "a limit names no actor");
        assert_eq!(
            plan(&["skill", "skills/research", "--max-spend=2"])?
                .run_authority
                .and_then(|authority| authority.max_spend_micro_usd),
            Some(2_000_000)
        );
        assert_eq!(
            plan(&["skill", "skills/research", "--max-spend=0.000001"])?
                .run_authority
                .and_then(|authority| authority.max_spend_micro_usd),
            Some(1)
        );
        for refused in ["-1", "0.0000001", "1e3", "", ".5", "5.", "$2", "0.5x"] {
            let Err(error) = plan(&["skill", "skills/research", "--max-spend", refused]) else {
                return Err(format!("--max-spend {refused:?} parsed"));
            };
            assert!(error.contains("--max-spend takes dollars"), "{error}");
        }
        Ok(())
    }

    #[test]
    fn max_cost_joins_a_saved_authority_and_never_contradicts_it() -> Result<(), String> {
        let named = plan(&[
            "skill",
            "skills/research",
            "--authority",
            "ops",
            "--max-spend",
            "1",
        ])?;
        assert_eq!(
            named.authority,
            Some(super::NamedAuthority {
                name: "ops".to_owned(),
                max_spend_micro_usd: Some(1_000_000),
            })
        );
        assert_eq!(
            named.run_authority, None,
            "it applies once the authority is read"
        );
        let saved = |limit: Option<u64>| {
            Some(Box::new(runx_runtime::RunAuthority {
                max_fanout: Some(2),
                max_spend_micro_usd: limit,
                ..runx_runtime::RunAuthority::default()
            }))
        };
        let joined = super::limit_run(saved(None), Some(1_000_000))?.ok_or("an authority")?;
        assert_eq!(
            (joined.max_fanout, joined.max_spend_micro_usd),
            (Some(2), Some(1_000_000))
        );
        let Err(error) = super::limit_run(saved(Some(500_000)), Some(1_000_000)) else {
            return Err("two different limits joined".to_owned());
        };
        assert!(error.contains("state the limit once"), "{error}");
        let Err(error) = plan(&["skill", "inspect", "skills/research", "--max-spend", "1"]) else {
            return Err("inspect took a money limit".to_owned());
        };
        assert!(error.contains("bind a run"), "{error}");
        // A host's stated authority takes the limit the same way.
        let host =
            r#"{"actor_ref":{"type":"principal","uri":"runx:principal:assistant"},"max_fanout":2}"#;
        let joined = plan(&[
            "skill",
            "skills/research",
            "--run-authority",
            host,
            "--max-spend",
            "1",
        ])?
        .run_authority
        .ok_or("no run authority")?;
        assert_eq!(
            (joined.max_fanout, joined.max_spend_micro_usd),
            (Some(2), Some(1_000_000))
        );
        let stated = r#"{"max_spend_micro_usd":500000}"#;
        let Err(error) = plan(&[
            "skill",
            "skills/research",
            "--run-authority",
            stated,
            "--max-spend",
            "1",
        ]) else {
            return Err("two different limits parsed".to_owned());
        };
        assert!(error.contains("state the limit once"), "{error}");
        Ok(())
    }

    #[test]
    fn the_receipt_dir_has_no_short_flag() -> Result<(), String> {
        // 0.12 drops -R: a receipt directory is named by --receipt-dir alone.
        let args = ["skill", "-R", "receipts"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let mut state = SkillParseState::default();
        let error = super::parse_skill_arg(&args, 1, &mut state)
            .err()
            .ok_or("-R was read as a flag")?;
        assert!(error.contains("unknown runx skill flag -R"), "{error}");
        assert!(state.receipt_dir.is_none());
        Ok(())
    }

    #[test]
    fn short_human_flags_parse_for_skill_runs() -> Result<(), String> {
        let args = [
            "skill",
            "-i",
            "claim=abc",
            "--receipt-dir",
            "receipts",
            "-j",
        ]
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
    fn skill_without_inputs_executes_default_method() -> Result<(), String> {
        let args = ["skill", "skills/nws-weather"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Run);
        assert_eq!(
            plan.skill_path,
            std::path::PathBuf::from("skills/nws-weather")
        );
        assert_eq!(plan.method, None);
        Ok(())
    }

    #[test]
    fn a_reference_names_its_method_after_hash() -> Result<(), String> {
        let args = ["skill", "skills/nws-weather#normalize"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Run);
        assert_eq!(
            plan.skill_path,
            std::path::PathBuf::from("skills/nws-weather")
        );
        assert_eq!(plan.method.as_deref(), Some("normalize"));
        Ok(())
    }

    #[test]
    fn explicit_inspect_returns_skill_card() -> Result<(), String> {
        let args = ["skill", "inspect", "skills/nws-weather#normalize"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Inspect);
        assert_eq!(
            plan.skill_path,
            std::path::PathBuf::from("skills/nws-weather")
        );
        assert_eq!(plan.method.as_deref(), Some("normalize"));
        Ok(())
    }

    #[test]
    fn a_method_reference_with_inputs_executes_method() -> Result<(), String> {
        let args = [
            "skill",
            "skills/nws-weather#normalize",
            "-i",
            "location=hello",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
        let plan = super::parse_skill_plan(&args)?;

        assert_eq!(plan.action, SkillAction::Run);
        assert_eq!(plan.method.as_deref(), Some("normalize"));
        assert_eq!(
            plan.inputs.texts.get("location").map(String::as_str),
            Some("hello")
        );
        Ok(())
    }

    /// A method is named one way, `<ref>#<method>`: a second positional is
    /// refused, never read as a method, for a run and an inspection alike.
    #[test]
    fn a_positional_method_is_refused() -> Result<(), String> {
        for args in [
            &["skill", "skills/nws-weather", "normalize"][..],
            &["skill", "inspect", "skills/nws-weather", "normalize"][..],
        ] {
            let refused = plan(args)
                .err()
                .ok_or(format!("{args:?} parsed a positional method"))?;
            assert!(refused.contains("<ref>#<method>"), "{refused}");
        }
        for refused in ["skills/nws-weather#", "#normalize"] {
            let error = plan(&["skill", refused])
                .err()
                .ok_or(format!("{refused:?} parsed"))?;
            assert!(error.contains("<ref>#<method>"), "{error}");
        }
        Ok(())
    }

    #[test]
    fn bound_execution_requires_package_and_closure_digests() -> Result<(), String> {
        let package_only = [
            "skill",
            "skills/nws-weather",
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
            "skills/nws-weather",
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
            ["skill", "skills/operations-review", "--context", context]
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
            "skills/github-issue-to-pr#status",
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
        for flag in ["--method", "--input-json", "--location", "--run"] {
            let args = ["skill", "skills/nws-weather", flag, "value"]
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
