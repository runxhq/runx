use runx_parser::{
    AgentTask, AskGate, InputMode, StepKind, ToolTarget, ValidateSkillOptions,
    parse_runner_manifest_yaml, parse_skill_markdown, validate_runner_manifest, validate_skill,
    validate_skill_with_options,
};

/// One runner named `main` with the given steps (each line indented for a
/// step list item).
fn runner(steps: &str) -> Result<runx_parser::SkillRunnerDefinition, String> {
    runner_with("", steps)
}

fn runner_with(extra: &str, steps: &str) -> Result<runx_parser::SkillRunnerDefinition, String> {
    let yaml = format!("skill: demo\nrunners:\n  main:\n{extra}    steps:\n{steps}");
    let raw = parse_runner_manifest_yaml(&yaml).map_err(|error| error.to_string())?;
    let mut manifest = validate_runner_manifest(raw).map_err(|error| error.to_string())?;
    manifest
        .runners
        .remove("main")
        .ok_or_else(|| "runner main is missing".to_owned())
}

fn only_step(runner: &runx_parser::SkillRunnerDefinition) -> &runx_parser::GraphStep {
    &runner.graph.steps[0]
}

#[test]
fn a_process_tool_parses_its_command_and_input_mode() -> Result<(), String> {
    let runner = runner(
        "      - id: run\n        tool: {command: node, args: [run.mjs], input_mode: stdin}\n",
    )?;
    let StepKind::Tool(ToolTarget::Process(process)) = &only_step(&runner).kind else {
        return Err("expected a process tool".to_owned());
    };
    assert_eq!(process.command, "node");
    assert_eq!(process.args, ["run.mjs"]);
    assert_eq!(process.input_mode, Some(InputMode::Stdin));
    Ok(())
}

#[test]
fn a_worker_tool_parses_a_portable_module() -> Result<(), String> {
    let runner = runner(
        "      - id: run\n        tool: {module: domain/workflow.mjs, export: execute, timeout_seconds: 30}\n",
    )?;
    let StepKind::Tool(ToolTarget::Worker(worker)) = &only_step(&runner).kind else {
        return Err("expected a worker tool".to_owned());
    };
    assert_eq!(worker.module, "domain/workflow.mjs");
    assert_eq!(worker.export.as_deref(), Some("execute"));
    assert_eq!(worker.timeout_seconds, Some(30));
    Ok(())
}

#[test]
fn a_worker_declares_exact_required_and_optional_environment() -> Result<(), String> {
    let runner = runner(
        "      - id: run\n        tool:\n          module: domain.mjs\n          environment:\n            required: [NITROSEND_ACCOUNT_ID, REGION]\n            optional: [TRACE_LABEL]\n",
    )?;
    let environment = runner.environment();
    assert_eq!(environment.required, ["NITROSEND_ACCOUNT_ID", "REGION"]);
    assert_eq!(environment.optional, ["TRACE_LABEL"]);
    Ok(())
}

#[test]
fn environment_rejects_duplicates_invalid_names_and_runtime_owned_names() {
    for environment in [
        "required: [REGION]\n            optional: [REGION]",
        "required: [not-portable!]",
        "required: [RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64]",
    ] {
        let steps = format!(
            "      - id: run\n        tool:\n          module: domain.mjs\n          environment:\n            {environment}\n"
        );
        assert!(
            runner(&steps).is_err(),
            "invalid environment passed: {environment}"
        );
    }
}

#[test]
fn a_worker_parses_runtime_owned_pages() -> Result<(), String> {
    let runner = runner_with(
        "    inputs:\n      archive_file: {type: string, required: true}\n      archive_base: {type: string, required: true}\n",
        "      - id: run\n        tool:\n          module: domain.mjs\n          pages:\n            path_from: archive_file\n            path_scope_from: archive_base\n            media_type: application/javascript\n            framing: json_array\n            page_bytes: 524288\n",
    )?;
    let StepKind::Tool(ToolTarget::Worker(worker)) = &only_step(&runner).kind else {
        return Err("expected a worker tool".to_owned());
    };
    let pages = worker.pages.as_ref().ok_or("pages were not parsed")?;
    assert_eq!(pages.path_from, "archive_file");
    assert_eq!(pages.path_scope_from.as_deref(), Some("archive_base"));
    assert_eq!(pages.page_bytes, 524_288);
    Ok(())
}

#[test]
fn a_worker_rejects_ambiguous_or_reserved_page_inputs() {
    for pages in [
        "path_from: runx_page, media_type: application/json, framing: json_array",
        "path_from: archive, path_scope_from: archive, media_type: application/json, framing: json_array",
        "path_from: archive, media_type: application/json, framing: json_array, page_bytes: 4194305",
    ] {
        let steps =
            format!("      - id: run\n        tool: {{module: domain.mjs, pages: {{{pages}}}}}\n");
        assert!(runner(&steps).is_err(), "invalid pages passed: {pages}");
    }
}

#[test]
fn a_worker_rejects_process_fields_escapes_and_bad_bounds() {
    for tool in [
        "{module: ../escape.mjs}",
        "{module: domain.mjs, command: node}",
        "{module: domain.mjs, args: [extra]}",
        "{module: domain.mjs, timeout_seconds: 31}",
        "{module: domain.mjs, export: not-valid}",
        "{command: node, module: domain.mjs}",
        "{args: [run.mjs]}",
    ] {
        let steps = format!("      - id: run\n        tool: {tool}\n");
        assert!(runner(&steps).is_err(), "invalid tool passed: {tool}");
    }
}

#[test]
fn an_agent_step_names_its_role_and_task() -> Result<(), String> {
    let runner = runner(
        "      - id: decide\n        agent: {role: reviewer, task: decide-release}\n        outputs: {decision: string}\n",
    )?;
    assert_eq!(
        only_step(&runner).kind,
        StepKind::Agent(AgentTask {
            role: "reviewer".to_owned(),
            task: "decide-release".to_owned(),
            allowance: Default::default(),
        })
    );
    assert!(only_step(&runner).outputs.is_some());
    Ok(())
}

#[test]
fn an_ask_step_yields_the_approval_decision_packet() -> Result<(), String> {
    let runner = runner(
        "      - id: approve\n        ask: {gate: release.publish, reason: Approve this exact release., type: release}\n        inputs: {version: 1.2.3}\n",
    )?;
    let step = only_step(&runner);
    assert_eq!(
        step.kind,
        StepKind::Ask(AskGate {
            gate: "release.publish".to_owned(),
            reason: "Approve this exact release.".to_owned(),
            gate_type: Some("release".to_owned()),
        })
    );
    let artifacts = step.artifacts.as_ref().ok_or("ask declares no packet")?;
    assert_eq!(artifacts.wrap_as.as_deref(), Some("approval_decision"));
    assert_eq!(
        artifacts.packet.as_deref(),
        Some("runx.approval.decision.v1")
    );
    assert!(step.inputs.contains_key("version"));
    Ok(())
}

#[test]
fn an_operation_step_carries_its_call_on_the_step() -> Result<(), String> {
    let yaml = "skill: demo\nconnectors:\n  slack:\n    operations: [channel.post]\nrunners:\n  main:\n    steps:\n      - id: post\n        operation: slack/channel.post\n        idempotency_key: $input.key\n        input: {channel_locator: $input.channel}\n        result_fields: [message_locator]\n";
    let raw = parse_runner_manifest_yaml(yaml).map_err(|error| error.to_string())?;
    let manifest = validate_runner_manifest(raw).map_err(|error| error.to_string())?;
    let step = &manifest.runners["main"].graph.steps[0];
    assert!(step.inputs.contains_key("input"));
    assert!(step.inputs.contains_key("result_fields"));
    assert_eq!(
        step.inputs
            .get("idempotency_key")
            .and_then(|value| value.as_str()),
        Some("$input.key")
    );
    assert_eq!(step.idempotency_key, None);

    let wrapped = yaml.replace(
        "        input: {channel_locator: $input.channel}\n",
        "        inputs: {input: {channel_locator: $input.channel}}\n",
    );
    let raw = parse_runner_manifest_yaml(&wrapped).map_err(|error| error.to_string())?;
    assert!(validate_runner_manifest(raw).is_err());
    Ok(())
}

#[test]
fn a_runner_result_defaults_to_its_last_step() -> Result<(), String> {
    let runner = runner(
        "      - id: first\n        tool: {module: one.mjs}\n      - id: second\n        tool: {module: two.mjs}\n",
    )?;
    assert_eq!(runner.graph.result_from, ["second"]);
    // The graph carries its skill's identity, not the runner's.
    assert_eq!(runner.graph.name, "demo");
    Ok(())
}

#[test]
fn a_runner_takes_only_runner_fields() {
    for extra in [
        "    type: graph\n",
        "    graph: {name: main}\n",
        "    command: node\n",
        "    scopes: [repo:read]\n",
        "    runx: {scopes: [repo:read]}\n",
    ] {
        assert!(
            runner_with(extra, "      - id: run\n        tool: {module: one.mjs}\n").is_err(),
            "runner field passed: {extra}"
        );
    }
}

#[test]
fn a_step_declares_exactly_one_kind() {
    for step in [
        "      - id: run\n        inputs: {}\n",
        "      - id: run\n        tool: {module: one.mjs}\n        skill: ../other\n",
        "      - id: run\n        run: {type: javascript, module: one.mjs}\n",
    ] {
        assert!(runner(step).is_err(), "step passed: {step}");
    }
}

#[test]
fn skill_markdown_carries_only_manual_fields() -> Result<(), String> {
    let markdown =
        "---\nname: manual\ndescription: A manual.\nsource:\n  type: agent\n---\n# Manual\n";
    let raw = parse_skill_markdown(markdown).map_err(|error| error.to_string())?;
    assert!(validate_skill(raw.clone()).is_err());
    let lenient = validate_skill_with_options(raw, ValidateSkillOptions::lenient())
        .map_err(|error| error.to_string())?;
    assert_eq!(lenient.name, "manual");
    Ok(())
}
