use runx_parser::{
    AgentTask, AskGate, AskReason, InputMode, Source, StepKind, ToolTarget, ValidateSkillMode,
    ValidateSkillOptions, parse_method_manifest_yaml, parse_skill_markdown,
    validate_method_manifest, validate_skill, validate_skill_with_options,
};

/// One method named `main` with the given steps (each line indented for a
/// step list item).
fn method(steps: &str) -> Result<runx_parser::SkillMethodDefinition, String> {
    method_with("", steps)
}

fn method_with(extra: &str, steps: &str) -> Result<runx_parser::SkillMethodDefinition, String> {
    let yaml = format!("minimum_runx: 0.12.0\nmethods:\n  main:\n{extra}    steps:\n{steps}");
    let raw = parse_method_manifest_yaml(&yaml).map_err(|error| error.to_string())?;
    let mut manifest = validate_method_manifest("demo", raw).map_err(|error| error.to_string())?;
    manifest
        .methods
        .remove("main")
        .ok_or_else(|| "method main is missing".to_owned())
}

fn only_step(method: &runx_parser::SkillMethodDefinition) -> &runx_parser::WorkflowStep {
    &method.workflow.steps[0]
}

#[test]
fn a_process_tool_parses_its_command_and_input_mode() -> Result<(), String> {
    let method = method(
        "      - id: run\n        tool: {command: node, args: [run.mjs], inputs_via: stdin}\n",
    )?;
    let StepKind::Tool(ToolTarget::Process(process)) = &only_step(&method).kind else {
        return Err("expected a process tool".to_owned());
    };
    assert_eq!(process.command, "node");
    assert_eq!(process.args, ["run.mjs"]);
    assert_eq!(process.inputs_via, Some(InputMode::Stdin));
    Ok(())
}

#[test]
fn a_worker_tool_parses_a_portable_module() -> Result<(), String> {
    let method = method(
        "      - id: run\n        tool: {module: domain/workflow.mjs, export: execute, cpu_seconds: 30}\n",
    )?;
    let StepKind::Tool(ToolTarget::Worker(worker)) = &only_step(&method).kind else {
        return Err("expected a worker tool".to_owned());
    };
    assert_eq!(worker.module, "domain/workflow.mjs");
    assert_eq!(worker.export.as_deref(), Some("execute"));
    assert_eq!(worker.cpu_seconds, Some(30));
    Ok(())
}

#[test]
fn a_worker_declares_exact_required_and_optional_environment() -> Result<(), String> {
    let method = method(
        "      - id: run\n        tool:\n          module: domain.mjs\n          environment:\n            required: [NITROSEND_ACCOUNT_ID, REGION]\n            optional: [TRACE_LABEL]\n",
    )?;
    let environment = method.environment();
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
            method(&steps).is_err(),
            "invalid environment passed: {environment}"
        );
    }
}

#[test]
fn a_worker_parses_runtime_owned_pages() -> Result<(), String> {
    let method = method_with(
        "    inputs:\n      archive_file: {type: string}\n      archive_base: {type: string}\n",
        "      - id: run\n        tool:\n          module: domain.mjs\n          paged_input:\n            path: archive_file\n            path_scope: archive_base\n            media_type: application/javascript\n            framing: json_array\n            page_bytes: 524288\n",
    )?;
    let StepKind::Tool(ToolTarget::Worker(worker)) = &only_step(&method).kind else {
        return Err("expected a worker tool".to_owned());
    };
    let pages = worker.paged_input.as_ref().ok_or("pages were not parsed")?;
    assert_eq!(
        pages.source,
        runx_parser::ArtifactPageInput::Path {
            path: "archive_file".to_owned(),
            path_scope: Some("archive_base".to_owned()),
            media_type: "application/javascript".to_owned(),
        }
    );
    assert_eq!(pages.page_bytes, 524_288);
    Ok(())
}

#[test]
fn a_worker_parses_an_already_admitted_artifact_source() -> Result<(), String> {
    let method = method_with(
        "    inputs:\n      snapshot: {type: string}\n",
        "      - id: run\n        tool:\n          module: domain.mjs\n          paged_input:\n            artifact: snapshot\n            framing: json_array\n            page_bytes: 65536\n",
    )?;
    let StepKind::Tool(ToolTarget::Worker(worker)) = &only_step(&method).kind else {
        return Err("expected a worker tool".to_owned());
    };
    let pages = worker.paged_input.as_ref().ok_or("pages were not parsed")?;
    assert_eq!(
        pages.source,
        runx_parser::ArtifactPageInput::Artifact {
            artifact: "snapshot".to_owned()
        }
    );
    let encoded = serde_json::to_value(pages).map_err(|error| error.to_string())?;
    let decoded: runx_parser::ArtifactPageSource =
        serde_json::from_value(encoded).map_err(|error| error.to_string())?;
    assert_eq!(&decoded, pages);
    Ok(())
}

#[test]
fn a_worker_rejects_ambiguous_or_reserved_page_inputs() {
    for pages in [
        "path: runx_page, media_type: application/json, framing: json_array",
        "path: archive, path_scope: archive, media_type: application/json, framing: json_array",
        "path: archive, media_type: application/json, framing: json_array, page_bytes: 4194305",
        "artifact: snapshot, path: archive, media_type: application/json, framing: json_array",
        "artifact: snapshot, path_scope: scope, framing: json_array",
        "artifact: snapshot, media_type: application/json, framing: json_array",
        "artifact: runx_page, framing: json_array",
        "artifact: nested.snapshot, framing: json_array",
        "artifact: null, framing: json_array",
        "artifact: 3, framing: json_array",
        "artifact: '', framing: json_array",
        "framing: json_array",
    ] {
        let steps = format!(
            "      - id: run\n        tool: {{module: domain.mjs, paged_input: {{{pages}}}}}\n"
        );
        assert!(method(&steps).is_err(), "invalid pages passed: {pages}");
    }
}

#[test]
fn a_worker_rejects_process_fields_escapes_and_bad_bounds() {
    for tool in [
        "{module: ../escape.mjs}",
        "{module: domain.mjs, command: node}",
        "{module: domain.mjs, args: [extra]}",
        "{module: domain.mjs, cpu_seconds: 31}",
        "{module: domain.mjs, cpu_seconds: 0}",
        // A worker's limit is its CPU time; a wall-clock timeout is a process's.
        "{module: domain.mjs, timeout_seconds: 5}",
        "{module: domain.mjs, export: not-valid}",
        "{command: node, module: domain.mjs}",
        "{args: [run.mjs]}",
    ] {
        let steps = format!("      - id: run\n        tool: {tool}\n");
        assert!(method(&steps).is_err(), "invalid tool passed: {tool}");
    }
}

#[test]
fn an_agent_step_names_its_task() -> Result<(), String> {
    let method = method(
        "      - id: decide\n        agent: decide-release\n        outputs: {decision: {type: string}}\n",
    )?;
    assert_eq!(
        only_step(&method).kind,
        StepKind::Agent(AgentTask {
            task: "decide-release".to_owned(),
            allowance: Default::default(),
            connectors: Default::default(),
            requires: Default::default(),
            context_kinds: Default::default(),
            context_entries: Default::default(),
        })
    );
    assert!(only_step(&method).outputs.is_some());
    Ok(())
}

#[test]
fn an_agent_step_requires_only_what_a_turn_can_ask_for() -> Result<(), String> {
    let parsed = method(
        "      - id: judge\n        agent: judge\n        requires: [fresh_context, separate_reviewer]\n",
    )?;
    let StepKind::Agent(task) = &only_step(&parsed).kind else {
        return Err("expected an agent step".to_owned());
    };
    assert_eq!(
        task.requires,
        [
            runx_contracts::TurnRequirement::FreshContext,
            runx_contracts::TurnRequirement::SeparateReviewer
        ]
    );
    for (requires, refusal) in [
        (
            "[telepathy]",
            "telepathy, which no agent harness is asked for",
        ),
        ("[fresh_context, fresh_context]", "repeats fresh_context"),
        ("[]", "lists nothing"),
    ] {
        let refused = method(&format!(
            "      - id: judge\n        agent: judge\n        requires: {requires}\n"
        ))
        .err()
        .unwrap_or_default();
        assert!(refused.contains(refusal), "{requires}: {refused}");
    }
    let on_a_tool = method(
        "      - id: run\n        tool: {command: node, args: [run.mjs]}\n        requires: [fresh_context]\n",
    );
    assert!(
        on_a_tool.is_err(),
        "only an agent's turn has an agent harness"
    );
    Ok(())
}

#[test]
fn an_agent_step_names_the_kinds_of_context_its_host_supplies() -> Result<(), String> {
    let parsed = method(
        "      - id: brief\n        agent: brief\n        context: {kinds: [people, open_threads]}\n",
    )?;
    let StepKind::Agent(task) = &only_step(&parsed).kind else {
        return Err("expected an agent step".to_owned());
    };
    assert_eq!(task.context_kinds, ["people", "open_threads"]);
    for (kinds, refusal) in [
        ("[People]", "a kind is lowercase words"),
        ("[people, people]", "repeats people"),
        ("[]", "lists nothing"),
    ] {
        let refused = method(&format!(
            "      - id: brief\n        agent: brief\n        context: {{kinds: {kinds}}}\n"
        ))
        .err()
        .unwrap_or_default();
        assert!(refused.contains(refusal), "{kinds}: {refused}");
    }
    Ok(())
}

#[test]
fn an_agent_step_is_given_context_entries_only_from_an_earlier_steps_output() -> Result<(), String>
{
    let recall = "      - id: recall\n        tool: {module: recall.mjs, export: recall}\n        outputs: {entries: {type: array}}\n";
    let parsed = method(&format!(
        "{recall}      - id: answer\n        agent: answer\n        context: {{entries: [$steps.recall.entries]}}\n"
    ))?;
    let answer = &parsed.workflow.steps[1];
    let StepKind::Agent(task) = &answer.kind else {
        return Err("expected an agent step".to_owned());
    };
    assert_eq!(
        task.context_entries,
        [Source::Step {
            step: "recall".to_owned(),
            path: vec!["entries".to_owned()],
        }]
    );
    // The step it reads from is one the workflow waits on.
    assert_eq!(answer.reads(), ["recall"]);
    for (entries, refusal) in [
        (
            "[$input.entries]",
            "reads an earlier step's output of context entries",
        ),
        (
            "[$steps.recall.entries, $steps.recall.entries]",
            "names $steps.recall.entries again",
        ),
        ("[$steps.later.entries]", "later"),
        (
            "$steps.recall.entries",
            "is a list of $steps.<id>.<field> references",
        ),
    ] {
        let refused = method(&format!(
            "{recall}      - id: answer\n        agent: answer\n        context: {{entries: {entries}}}\n"
        ))
        .err()
        .unwrap_or_default();
        assert!(refused.contains(refusal), "{entries}: {refused}");
    }
    Ok(())
}

#[test]
fn an_ask_step_yields_the_approval_decision_packet() -> Result<(), String> {
    let method = method(
        "      - id: approve\n        ask: {gate: release.publish, reason: Approve this exact release., type: release}\n        inputs: {version: 1.2.3}\n",
    )?;
    let step = only_step(&method);
    assert_eq!(
        step.kind,
        StepKind::Ask(AskGate {
            gate: "release.publish".to_owned(),
            reason: AskReason::Text("Approve this exact release.".to_owned()),
            gate_type: Some("release".to_owned()),
        })
    );
    let artifacts = step.artifacts.as_ref().ok_or("ask declares no packet")?;
    assert_eq!(artifacts.result_as.as_deref(), Some("approval_decision"));
    assert_eq!(
        artifacts
            .result_packet()
            .map(runx_contracts::PacketId::as_str),
        Some("runx.approval.decision.v1")
    );
    assert!(step.inputs.contains_key("version"));
    Ok(())
}

#[test]
fn an_operation_step_carries_its_call_on_the_step() -> Result<(), String> {
    let yaml = "minimum_runx: 0.12.0\nmethods:\n  main:\n    steps:\n      - id: post\n        operation: slack/channel.post\n        idempotency_key: $input.key\n        inputs: {channel_locator: $input.channel}\n        result:\n          required: [message_locator]\n";
    let raw = parse_method_manifest_yaml(yaml).map_err(|error| error.to_string())?;
    let manifest = validate_method_manifest("demo", raw).map_err(|error| error.to_string())?;
    let step = &manifest.methods["main"].workflow.steps[0];
    assert!(step.inputs.contains_key("inputs"));
    assert!(step.inputs.contains_key("result"));
    assert_eq!(
        step.inputs
            .get("idempotency_key")
            .and_then(|value| value.as_str()),
        Some("$input.key")
    );
    assert!(
        step.bindings
            .iter()
            .any(|binding| binding.at == ["idempotency_key"])
    );

    // The call is spelled one way: the old `input` is refused.
    let respelled = yaml.replace(
        "        inputs: {channel_locator: $input.channel}\n",
        "        input: {channel_locator: $input.channel}\n",
    );
    let raw = parse_method_manifest_yaml(&respelled).map_err(|error| error.to_string())?;
    let error = validate_method_manifest("demo", raw)
        .err()
        .ok_or("an operation step's old input key passed")?
        .to_string();
    assert!(
        error.contains("input does not apply to this operation step"),
        "{error}"
    );
    Ok(())
}

#[test]
fn a_method_result_defaults_to_its_last_step() -> Result<(), String> {
    let method = method(
        "      - id: first\n        tool: {module: one.mjs}\n      - id: second\n        tool: {module: two.mjs}\n",
    )?;
    assert_eq!(method.workflow.result_from, ["second"]);
    // The workflow carries its skill's identity, not the method's.
    assert_eq!(method.workflow.name, "demo");
    Ok(())
}

#[test]
fn a_method_takes_only_method_fields() {
    for extra in [
        "    type: graph\n",
        "    graph: {name: main}\n",
        "    command: node\n",
        "    scopes: [repo:read]\n",
        "    runx: {scopes: [repo:read]}\n",
    ] {
        assert!(
            method_with(extra, "      - id: run\n        tool: {module: one.mjs}\n").is_err(),
            "method field passed: {extra}"
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
        assert!(method(step).is_err(), "step passed: {step}");
    }
}

#[test]
fn skill_markdown_carries_only_manual_fields() -> Result<(), String> {
    let markdown =
        "---\nname: manual\ndescription: A manual.\nsource:\n  type: agent\n---\n# Manual\n";
    let raw = parse_skill_markdown(markdown).map_err(|error| error.to_string())?;
    assert!(validate_skill(raw.clone()).is_err());
    let lenient = validate_skill_with_options(
        raw,
        ValidateSkillOptions {
            mode: ValidateSkillMode::Lenient,
        },
    )
    .map_err(|error| error.to_string())?;
    assert_eq!(lenient.name, "manual");
    Ok(())
}

/// A method whose steps may call `tracker/item.update`.
fn acting(steps: &str) -> Result<runx_parser::SkillMethodDefinition, String> {
    let yaml = format!(
        "minimum_runx: 0.12.0\nmethods:\n  main:\n    inputs:\n      item: {{type: string}}\n    steps:\n{steps}"
    );
    let raw = parse_method_manifest_yaml(&yaml).map_err(|error| error.to_string())?;
    let mut manifest = validate_method_manifest("demo", raw).map_err(|error| error.to_string())?;
    manifest
        .methods
        .remove("main")
        .ok_or_else(|| "method main is missing".to_owned())
}

#[test]
fn an_operation_act_binds_the_invokers_act_whole_or_by_field() -> Result<(), String> {
    use runx_parser::Source;

    let whole = acting(
        "      - id: close\n        operation: tracker/item.update\n        inputs: {id: $input.item}\n        act: $run.act\n",
    )?;
    let act = only_step(&whole).act.clone().ok_or("the step has an act")?;
    assert!(act.values.is_empty());
    assert_eq!(act.bindings.len(), 1);
    assert!(act.bindings[0].at.is_empty());
    assert_eq!(act.bindings[0].source, Source::Act { path: Vec::new() });
    assert!(
        !only_step(&whole).inputs.contains_key("act"),
        "the act is not one of the call's literal fields"
    );

    let fields = acting(
        "      - id: close\n        operation: tracker/item.update\n        inputs: {id: $input.item}\n        act: {purpose: $run.act.purpose, target: $input.item, form: close}\n",
    )?;
    let act = only_step(&fields)
        .act
        .clone()
        .ok_or("the step has an act")?;
    assert_eq!(
        act.values.get("form"),
        Some(&runx_contracts::JsonValue::String("close".to_owned()))
    );
    let sources = act
        .bindings
        .iter()
        .map(|binding| (binding.at.join("."), binding.source.reference()))
        .collect::<Vec<_>>();
    assert!(sources.contains(&("purpose".to_owned(), "$run.act.purpose".to_owned())));
    assert!(sources.contains(&("target".to_owned(), "$input.item".to_owned())));
    Ok(())
}

#[test]
fn only_an_act_reads_the_invokers_act() -> Result<(), String> {
    for (steps, field) in [
        (
            "      - id: close\n        operation: tracker/item.update\n        inputs: {why: $run.act.purpose}\n",
            "inputs",
        ),
        (
            "      - id: close\n        operation: tracker/item.update\n        inputs: {id: $input.item}\n        when: {value: $run.act.purpose, equals: close}\n",
            "when",
        ),
        (
            "      - id: note\n        tool: {module: note.mjs}\n        inputs: {why: $run.act}\n",
            "inputs",
        ),
    ] {
        let Err(error) = acting(steps) else {
            return Err(format!("{field}: $run.act outside act: is refused"));
        };
        assert!(
            error.contains("only an act: may read") && error.contains(field),
            "{field}: {error}"
        );
    }
    Ok(())
}

#[test]
fn a_skill_step_gives_its_child_an_act_and_other_steps_take_none() -> Result<(), String> {
    let child = method(
        "      - id: child\n        skill: ./child\n        act: {purpose: $run.act.purpose}\n",
    )?;
    assert!(only_step(&child).act.is_some());
    for steps in [
        "      - id: note\n        tool: {module: note.mjs}\n        act: {purpose: Note}\n",
        "      - id: think\n        agent: draft\n        act: {purpose: Think}\n",
    ] {
        let Err(error) = method(steps) else {
            return Err("only governed calls and child runs take an act".to_owned());
        };
        assert!(error.contains("act does not apply"), "{error}");
    }
    Ok(())
}

#[test]
fn an_act_states_its_purpose() -> Result<(), String> {
    let Err(error) = acting(
        "      - id: close\n        operation: tracker/item.update\n        inputs: {id: $input.item}\n        act: {form: reply, reason: $run.act.reason}\n",
    ) else {
        return Err("an act without a purpose is refused".to_owned());
    };
    assert!(error.contains("needs a purpose"), "{error}");
    let Err(error) = acting(
        "      - id: close\n        operation: tracker/item.update\n        inputs: {id: $input.item}\n        act: {purpose: Close it, legitimacy: $run.act.reason}\n",
    ) else {
        return Err("an act states only an act's fields".to_owned());
    };
    assert!(
        error.contains("act.legitimacy is not a field of an act"),
        "{error}"
    );
    Ok(())
}

#[test]
fn an_input_cannot_take_the_name_of_the_invokers_act() -> Result<(), String> {
    let Err(error) = method_with(
        "    inputs:\n      act: {type: string, optional: true}\n",
        "      - id: note\n        tool: {module: note.mjs}\n",
    ) else {
        return Err("act is reserved".to_owned());
    };
    assert!(error.contains("never one of them"), "{error}");
    Ok(())
}
