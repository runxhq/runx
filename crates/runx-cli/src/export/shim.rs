use std::path::Path;

use runx_runtime::export::{RunxExportMode, RunxExportRunner, RunxExportSkill};

use super::{GeneratedFile, Target, display_path};

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
        return render_native_instructions(target, skill, runx_bin);
    }
    let runx_bin = display_path(runx_bin);
    let mut output = String::new();
    output.push_str("---\n");
    output.push_str(&format!("name: {}\n", yaml_plain_or_quoted(&skill.name)));
    output.push_str("description: |-\n");
    output.push_str(&indent_block(&skill.description));
    if target == Target::Claude {
        output.push_str(&format!(
            "allowed-tools: Bash({} skill *) Bash({} resume *)\n",
            shell_quote(&runx_bin),
            shell_quote(&runx_bin),
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
    output.push_str(&render_source_manual(skill));

    if let Some(default) = skill.runners.iter().find(|runner| runner.default) {
        output.push_str(&render_default_runner(
            command_target,
            default,
            &skill.package_digest,
            &runx_bin,
        ));
    }
    let alternate_runners = skill
        .runners
        .iter()
        .filter(|runner| !runner.default)
        .collect::<Vec<_>>();
    if !alternate_runners.is_empty() {
        output.push_str(&render_runner_index(
            command_target,
            &alternate_runners,
            &runx_bin,
        ));
    }
    output.push_str(&format!(
        "<!-- {} source={} package-digest={} - generated, do not edit -->\n",
        target.marker(),
        display_path(&skill.abs_dir),
        skill.package_digest,
    ));
    output
}

fn render_native_instructions(target: Target, skill: &RunxExportSkill, runx_bin: &Path) -> String {
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
        display_path(&skill.abs_dir),
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

fn render_default_runner(
    command_target: &str,
    runner: &RunxExportRunner,
    package_digest: &str,
    runx_bin: &str,
) -> String {
    let mut output = String::new();
    let title = runner.name.as_deref().unwrap_or("default");
    output.push_str(&format!("## Default runner: `{title}`\n\n"));
    output.push_str("Inspect the exact input and effect contract on demand:\n\n```bash\n");
    output.push_str(&render_inspect_command(command_target, runner, runx_bin));
    output.push_str("\n```\n\n");
    if runner.examples.is_empty() {
        output.push_str("Invocation template (replace placeholders before running):\n\n");
    } else {
        output.push_str("Validated invocation example:\n\n");
    }
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

fn render_runner_index(
    command_target: &str,
    runners: &[&RunxExportRunner],
    runx_bin: &str,
) -> String {
    let mut output = String::from(
        "## Other runners\n\nSelect a non-default runner only when the source manual calls for it. Inspect it on demand for its inputs and exact execution-closure digest:\n\n",
    );
    for runner in runners {
        let name = runner.name.as_deref().unwrap_or("default");
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
    runner: &RunxExportRunner,
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
    runner: &RunxExportRunner,
    package_digest: &str,
    runx_bin: &str,
) -> String {
    let mut command = format!(
        "{} skill {}",
        shell_quote(runx_bin),
        shell_quote(command_target)
    );
    if let Some(name) = &runner.name {
        command.push(' ');
        command.push_str(&shell_quote(name));
    }
    let mut lines = vec![command];
    if let Some(example) = runner.examples.first() {
        for (name, value) in example {
            let encoded = serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned());
            lines.push(format!(
                "  --input-json {} {}",
                shell_quote(name),
                shell_quote(&encoded)
            ));
        }
    } else {
        for (name, input) in &runner.inputs {
            if input.required && input.default.is_none() {
                lines.push(format!("  --{name} \"<{name}>\""));
            }
        }
    }
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
- If runx returns `needs_agent` or `needs_approval`, read only the selected digest-bound request artifact and obey its exact output contract and `allowed_tools`.
- Answer each request by the id the run lists; relay approval requests and never fabricate human approval.

Pipe the structured continuation object to the same digest-bound run; do not create a manual answer file:

```bash
printf '%s' \"$RUNX_ANSWERS_JSON\" | {} resume \"<run_id>\" -{} --json
```

Repeat until sealed or an exact operator approval/input is required. Never place signing seeds, provider tokens, or raw credentials in the continuation object or response.

",
        shell_quote(runx_bin),
        binding,
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

fn shell_quote(value: &str) -> String {
    if value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '_' | '-' | ':')
    }) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
