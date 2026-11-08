use std::path::Path;

use runx_runtime::export::{RunxExportMethod, RunxExportMode, RunxExportSkill};

use super::{GeneratedFile, MCP_SERVER_NAME, MCP_SERVER_REF_MARKER, Target, display_path};

/// What a skill server offers for working the turns its own calls opened.
const TURN_TOOLS: [&str; 5] = [
    "turn_list",
    "turn_status",
    "turn_read",
    "turn_call",
    "turn_complete",
];

pub(super) fn plan_files(
    target: Target,
    project: bool,
    root: &Path,
    skills: &[RunxExportSkill],
    skill_dir: &Path,
    runx_bin: &Path,
) -> Vec<GeneratedFile> {
    skills
        .iter()
        .map(|skill| {
            let command_target = if project {
                skill
                    .abs_dir
                    .strip_prefix(root)
                    .map(display_path)
                    .unwrap_or_else(|_| display_path(&skill.abs_dir))
            } else {
                display_path(&skill.abs_dir)
            };
            let contents = render_shim(target, skill, &command_target, runx_bin);
            GeneratedFile {
                path: skill_dir.join(&skill.name).join("SKILL.md"),
                contents,
            }
        })
        .collect()
}

fn render_shim(
    target: Target,
    skill: &RunxExportSkill,
    command_target: &str,
    runx_bin: &Path,
) -> String {
    if skill.mode == RunxExportMode::NativeInstructions {
        return render_native_instructions(target, skill, command_target, runx_bin);
    }
    let runx_bin = display_path(runx_bin);
    let mut output = String::new();
    output.push_str("---\n");
    output.push_str(&format!("name: {}\n", yaml_plain_or_quoted(&skill.name)));
    output.push_str("description: |-\n");
    output.push_str(&indent_block(&skill.description));
    if target == Target::Claude {
        output.push_str(&format!(
            "allowed-tools: Bash({} skill *) Bash({} resume *){}\n",
            shell_quote(&runx_bin),
            shell_quote(&runx_bin),
            claude_server_tools(skill),
        ));
    }
    output.push_str("---\n");
    output.push_str(&format!("# {} - governed by runx\n\n", skill.name));
    output.push_str(
        "Run the declared runner through runx; do not bypass work that the runner owns. Runx governs policy, approvals, provider effects, and the signed receipt. Report an external mutation only when the result contains provider evidence.\n\n",
    );
    output.push_str(
        "Runx signs receipts with this home's own local key when no signer is configured. If any `RUNX_RECEIPT_SIGN_*` variable is present, the complete signer tuple must be present or runx fails closed. Never invent, copy, or print signing keys.\n\n",
    );
    output.push_str(&format!(
        "The bound package is `{command_target}`. Resolve the manual's package-relative resources there. Use the generated invocation below for this binding; manual examples explain usage and do not supply the user's task.\n\n"
    ));
    output.push_str(&render_source_manual(skill));

    if let Some(default) = skill.runners.iter().find(|runner| runner.default) {
        if default.makes_calls {
            output.push_str(&render_server_method(target, skill, default));
        } else {
            output.push_str(&render_default_method(
                command_target,
                default,
                &skill.package_digest,
                &runx_bin,
            ));
        }
    }
    let alternate_methods = skill
        .runners
        .iter()
        .filter(|runner| !runner.default)
        .collect::<Vec<_>>();
    if !alternate_methods.is_empty() {
        output.push_str(&render_method_index(
            &skill.name,
            command_target,
            &alternate_methods,
            &runx_bin,
        ));
    }
    if skill.runners.iter().any(|runner| runner.makes_calls) {
        output.push_str(MCP_SERVER_REF_MARKER);
        output.push('\n');
    }
    output.push_str(&format!(
        "<!-- {} source={} package-digest={} - generated, do not edit -->\n",
        target.marker(),
        command_target,
        skill.package_digest,
    ));
    output
}

/// The server tools a Claude shim may use, leading space included, or nothing
/// for a skill that runs entirely from the shell.
fn claude_server_tools(skill: &RunxExportSkill) -> String {
    let runners = skill
        .runners
        .iter()
        .filter(|runner| runner.makes_calls)
        .collect::<Vec<_>>();
    if runners.is_empty() {
        return String::new();
    }
    let mut tools = runners
        .iter()
        .map(|runner| server_tool_name(skill, runner))
        .collect::<Vec<_>>();
    tools.extend(TURN_TOOLS.iter().map(|tool| (*tool).to_owned()));
    tools
        .iter()
        .map(|tool| format!(" mcp__{MCP_SERVER_NAME}__{tool}"))
        .collect()
}

fn server_tool_name(skill: &RunxExportSkill, runner: &RunxExportMethod) -> String {
    let alternate = runner.name.as_deref().filter(|_| !runner.default);
    runx_runtime::export::skill_tool_name(&skill.name, alternate)
}

fn render_server_method(
    target: Target,
    skill: &RunxExportSkill,
    runner: &RunxExportMethod,
) -> String {
    let tool = server_tool_name(skill, runner);
    let title = runner.name.as_deref().unwrap_or("default");
    format!(
        "\
## Default runner: `{title}`

This skill's agent makes governed calls, so it runs on the `{MCP_SERVER_NAME}` MCP server rather than the shell. Call the `{tool}` tool with one object built from the user's actual task and admitted evidence, matching the tool's input schema, and state the requested purpose in `act`. Do not copy example values unless the user's job supplies them.

If it reports that the run needs agent work, work the run's open turns on the same connection until it seals:

- `turn_list` names the open turns. A run can open several in sequence, including nested ones.
- `turn_status` gives a turn's instructions, inputs, exact callable tools and output schema.
- `turn_read` reads the locators it returns.
- `turn_call` makes an offered call. Give it a stable `call_id`, so an exact retry returns the recorded call.
- `turn_complete` ends the turn with its output, or with `failure` if it cannot be done.

Relay human questions and approval requests to the person; never answer them yourself. Do not substitute a summary for calls that never happened. When the run seals, surface the outcome, receipt id, result and artifact refs; a sealed blocked or failed run is not success.

If this host has no `{tool}` tool from `{MCP_SERVER_NAME}`, stop and tell the person to run `runx export {host}` and start a new session. This skill cannot finish any other way.

",
        host = target.as_str(),
    )
}

fn render_native_instructions(
    target: Target,
    skill: &RunxExportSkill,
    command_target: &str,
    runx_bin: &Path,
) -> String {
    let mut output = String::new();
    output.push_str("---\n");
    output.push_str(&format!("name: {}\n", yaml_plain_or_quoted(&skill.name)));
    output.push_str("description: |-\n");
    output.push_str(&indent_block(&skill.description));
    if target == Target::Claude {
        output.push_str(&format!(
            "allowed-tools: Bash({} *)\n",
            shell_quote(&display_path(runx_bin))
        ));
    }
    output.push_str("---\n");
    output.push_str(&render_source_manual(skill));
    output.push_str(&format!(
        "<!-- {} source={} package-digest={} - generated, do not edit -->\n",
        target.marker(),
        command_target,
        skill.package_digest,
    ));
    output
}

fn render_source_manual(skill: &RunxExportSkill) -> String {
    format!(
        "<!-- runx-source-manual-begin digest={} package-digest={} bytes={} -->\n{}<!-- runx-source-manual-end -->\n\n",
        skill.manual_digest,
        skill.package_digest,
        skill.manual_markdown.len(),
        skill.manual_markdown
    )
}

fn render_default_method(
    command_target: &str,
    runner: &RunxExportMethod,
    package_digest: &str,
    runx_bin: &str,
) -> String {
    let mut output = String::new();
    let title = runner.name.as_deref().unwrap_or("default");
    output.push_str(&format!("## Default runner: `{title}`\n\n"));
    output.push_str("Inspect the exact input and effect contract on demand:\n\n```bash\n");
    output.push_str(&render_inspect_command(command_target, runner, runx_bin));
    output.push_str("\n```\n\n");
    output.push_str("Set `RUNX_ACT_JSON` to a JSON act with a short `purpose` describing this user's requested work, for example `{\"purpose\":\"Review the current project build configuration\"}`. This states the task; it grants no additional authority.\n\n");
    if !runner.inputs.is_empty() {
        output.push_str("Build `RUNX_INPUTS_JSON` as one JSON object from the user's actual task and admitted evidence, matching the inspected `input_schema`. Preserve nested JSON types and include required fields. Examples illustrate shapes; harness fixtures are test data, not user inputs. Never put credentials in inputs.\n\n");
    }
    output.push_str("Invoke from the user's working project, keeping the same working directory and Runx home throughout this run:\n\n");
    output.push_str("```bash\n");
    output.push_str(&render_command(
        command_target,
        runner,
        package_digest,
        runx_bin,
    ));
    output.push_str("\n```\n\n");
    output.push_str(&render_continuation(
        package_digest,
        runner.execution_closure_digest.as_deref(),
        runx_bin,
    ));
    output
}

fn render_method_index(
    skill: &str,
    command_target: &str,
    runners: &[&RunxExportMethod],
    runx_bin: &str,
) -> String {
    let mut output = String::from(
        "## Other runners\n\nSelect a non-default runner only when the source manual calls for it. Inspect it for its input_schema and execution_closure.closure_digest. Replace the runner name and closure digest in both invocation and continuation commands; keep the package digest. Never resume another runner with the default runner's closure digest.\n\n",
    );
    for runner in runners {
        let name = runner.name.as_deref().unwrap_or("default");
        if runner.makes_calls {
            let tool = runx_runtime::export::skill_tool_name(skill, runner.name.as_deref());
            output.push_str(&format!(
                "- `{name}`: the `{tool}` tool on the `{MCP_SERVER_NAME}` MCP server\n",
            ));
            continue;
        }
        output.push_str(&format!(
            "- `{name}`: `{}`\n",
            render_inspect_command(command_target, runner, runx_bin)
        ));
    }
    output.push('\n');
    output
}

fn render_inspect_command(
    command_target: &str,
    runner: &RunxExportMethod,
    runx_bin: &str,
) -> String {
    let mut command = format!(
        "{} skill inspect {}",
        shell_quote(runx_bin),
        shell_quote(command_target),
    );
    if let Some(name) = &runner.name {
        command.push(' ');
        command.push_str(&shell_quote(name));
    }
    command.push_str(" --json");
    command
}

fn render_command(
    command_target: &str,
    runner: &RunxExportMethod,
    package_digest: &str,
    runx_bin: &str,
) -> String {
    let prefix = if runner.inputs.is_empty() {
        ""
    } else {
        "printf '%s' \"$RUNX_INPUTS_JSON\" | "
    };
    let mut command = format!(
        "{prefix}{} skill {}",
        shell_quote(runx_bin),
        shell_quote(command_target)
    );
    if let Some(name) = &runner.name {
        command.push(' ');
        command.push_str(&shell_quote(name));
    }
    let mut lines = vec![command];
    if !runner.inputs.is_empty() {
        lines.push("  --inputs -".to_owned());
    }
    lines.push("  --act \"$RUNX_ACT_JSON\"".to_owned());
    lines.push(format!(
        "  --package-digest {}",
        shell_quote(package_digest)
    ));
    if let Some(closure_digest) = runner.execution_closure_digest.as_deref() {
        lines.push(format!(
            "  --execution-closure-digest {}",
            shell_quote(closure_digest)
        ));
    }
    lines.push("  --json".to_owned());
    lines.join(" \\\n")
}

fn render_continuation(
    package_digest: &str,
    execution_closure_digest: Option<&str>,
    runx_bin: &str,
) -> String {
    let mut binding = format!(" --package-digest {}", shell_quote(package_digest));
    if let Some(closure_digest) = execution_closure_digest {
        binding.push_str(&format!(
            " --execution-closure-digest {}",
            shell_quote(closure_digest)
        ));
    }
    format!(
        "\
Interpret the runx JSON result exactly:
- `status` is lifecycle state; `outcome` says whether the promised operation completed. A sealed blocked or failed run is not success.
- If `status` is `sealed`, surface the outcome, receipt id, result, and artifact refs.
- If runx returns `needs_agent` or `needs_approval`, read only the selected digest-bound request artifact and obey its exact output contract and authority. Do not substitute a summary for work that never happened.
- Read the artifact at its returned path relative to the original workspace and verify its digest. For agent acts, invocation.envelope carries the task, inputs and output_schema.
- Relay human questions and approval requests. Never fabricate human approval.

For an agent answer, construct RUNX_ANSWERS_JSON with this exact envelope:

```json
{answer_example}
```

{answer_guidance}

Pipe the structured continuation object to the same digest-bound run; do not create a manual answer file:

```bash
printf '%s' \"$RUNX_ANSWERS_JSON\" | {} resume \"<run_id>\" -{} --json
```

Repeat until sealed or an exact operator approval/input is required. Never place signing seeds, provider tokens, or raw credentials in the continuation object or response.

",
        shell_quote(runx_bin),
        binding,
        answer_example = crate::resume::ANSWER_DOCUMENT_EXAMPLE,
        answer_guidance = crate::resume::ANSWER_DOCUMENT_GUIDANCE,
    )
}

fn indent_block(value: &str) -> String {
    let mut output = String::new();
    for line in value.lines() {
        output.push_str("  ");
        output.push_str(line);
        output.push('\n');
    }
    if value.is_empty() {
        output.push_str("  \n");
    }
    output
}

fn yaml_plain_or_quoted(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        value.to_owned()
    } else {
        serde_json::to_string(value).unwrap_or_else(|_| "\"runx-skill\"".to_owned())
    }
}

pub(super) fn shell_quote(value: &str) -> String {
    if value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '_' | '-' | ':')
    }) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
