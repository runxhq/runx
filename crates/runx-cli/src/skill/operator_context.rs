use std::io::Write;

use runx_runtime::skill_front::{
    PreparedSkillRunReport, SkillOperatorContextContextSkill, SkillOperatorContextDocument,
    SkillOperatorContextNode, SkillOperatorContextStep, SkillOperatorContextTool,
};

// Function rationale: compact and full modes share one ordered
// operator-facing rendering contract so decision fields cannot drift.
pub(super) fn write_operator_context(
    report: &PreparedSkillRunReport,
    full: bool,
) -> Result<(), String> {
    let mut out = String::new();
    out.push_str("Prepared run\n");
    out.push_str(&format!(
        "  Skill:  {}\n  Runner: {}\n  Source: {}\n",
        report.request.skill_path.display(),
        report.request.runner,
        source_label(report),
    ));
    if let Some(run_id) = &report.request.run_id {
        out.push_str(&format!("  Run:    {run_id}\n"));
    }
    if let Some(receipt_dir) = &report.request.receipt_dir {
        out.push_str(&format!("  Receipts: {}\n", receipt_dir.display()));
    }
    let total_steps = report.governance.declared_steps + report.governance.conditional_steps;
    let execution_boundaries = report
        .governance
        .execution_boundaries
        .iter()
        .map(|kind| kind.as_str().to_owned())
        .collect::<Vec<_>>();
    out.push_str(&format!(
        "  Steps:  {total_steps} total, {} conditional\n  Tools:  {}\n  Boundaries: {}\n",
        report.governance.conditional_steps,
        list_or_none(&report.governance.tool_refs),
        list_or_none(&execution_boundaries),
    ));
    out.push_str(&format!(
        "  Managed agent: {} act(s), {}\n",
        report.governance.managed_agent_acts,
        match report.governance.managed_agent_max_rounds {
            Some(rounds) => format!("enabled, max {rounds} round(s) per act"),
            None => "off; caller resolves needs_agent".to_owned(),
        },
    ));
    out.push_str("  Inputs: ");
    if report.request.inputs.is_empty() {
        out.push_str("none\n");
    } else {
        out.push_str(
            &report
                .request
                .inputs
                .iter()
                .map(|input| format!("{} ({})", input.name, input.value_type))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push('\n');
    }
    out.push_str(&format!("  Digest: {}\n", report.digest));
    if let Some(reason) = &report.blocked_reason {
        out.push_str(&format!("\nPreparation blocked: {reason}\n"));
        if let Some(receipt_id) = &report.refusal_receipt_id {
            out.push_str(&format!("Refusal receipt: {receipt_id}\n"));
        }
        append_resolution_trace(&mut out, report);
    }
    if !full {
        out.push_str("  Full context: add --full-operator-context\n");
        let _ignored = writeln!(std::io::stderr(), "{out}");
        return Ok(());
    }
    let Some(chain) = report.chain.as_ref() else {
        let _ignored = writeln!(std::io::stderr(), "{out}");
        return Ok(());
    };
    out.push_str("\nFull operator context\n");
    out.push_str(&format!("cwd: {}\n", report.request.cwd.display()));
    out.push_str(&format!("entry_kind: {}\n", report.request.entry.kind));
    out.push_str("request_bindings:\n");
    for input in &report.request.inputs {
        out.push_str(&format!(
            "  - {}: type={}, bytes={}, sha256={}\n",
            input.name, input.value_type, input.canonical_bytes, input.sha256
        ));
    }
    append_resolution_trace(&mut out, report);
    out.push_str(&format!(
        "skill_path: {}\n",
        chain.entry.package.directory.display()
    ));
    out.push_str(&format!(
        "chain_nodes: {} / {}\nchain_content_bytes: {} / {}\nchain_max_depth: {}\n",
        chain.node_count,
        chain.max_nodes,
        chain.content_bytes,
        chain.max_content_bytes,
        chain.max_depth,
    ));
    append_node(&mut out, &chain.entry, true)?;
    let _ignored = writeln!(std::io::stderr(), "{out}");
    Ok(())
}

fn source_label(report: &PreparedSkillRunReport) -> String {
    let entry = &report.request.entry;
    match (&entry.skill_id, &entry.version, &entry.trust_tier) {
        (Some(skill), Some(version), Some(trust)) => {
            format!("{skill}@{version} ({trust})")
        }
        (Some(skill), Some(version), None) => format!("{skill}@{version}"),
        _ => format!("{} ({})", entry.source_label, entry.kind),
    }
}

fn append_resolution_trace(out: &mut String, report: &PreparedSkillRunReport) {
    out.push_str("resolution_trace:\n");
    for entry in &report.trace {
        out.push_str(&format!(
            "  - {} {} {}: {}\n",
            entry.node_path, entry.stage, entry.outcome, entry.detail
        ));
    }
}

fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(", ")
    }
}

// Function rationale: a recursive skill node is rendered as one
// ordered packet so child contracts, tools, context, and steps stay adjacent.
fn append_node(
    out: &mut String,
    node: &SkillOperatorContextNode,
    entry: bool,
) -> Result<(), String> {
    out.push_str(&format!("\n--- skill node: {} ---\n", node.node_path));
    out.push_str(&format!(
        "package_dir: {}\npackage_source: {}\npackage_source_label: {}\n",
        node.package.directory.display(),
        node.package.source,
        node.package.source_label,
    ));
    if let Some(reference) = &node.package.reference {
        out.push_str(&format!("package_ref: {reference}\n"));
    }
    if let Some(registry) = &node.package.registry {
        out.push_str(&format!(
            "registry_ref: {}\nregistry_skill_id: {}\nregistry_version: {}\nregistry_digest: {}\nregistry_package_digest: {}\nregistry_trust_tier: {}\n",
            registry.reference,
            registry.skill_id,
            registry.version,
            registry.digest,
            registry.package_digest.as_deref().unwrap_or("none"),
            registry.trust_tier,
        ));
    }
    out.push_str(&format!(
        "runner_name: {}\nrunner_selection: {}\nrunner_scopes: {}\n",
        node.runner.name,
        node.runner.selection,
        list_or_none(&node.runner.scopes),
    ));
    if let Some(requested) = &node.runner.requested_name {
        out.push_str(&format!("runner_requested_name: {requested}\n"));
    }
    append_document(
        out,
        if entry {
            "root skill".to_owned()
        } else {
            format!("skill contract: {}", node.node_path)
        },
        &node.skill_markdown,
        "markdown",
    );
    let runner_label = if entry {
        format!("selected runner: {}", node.runner.name)
    } else {
        format!(
            "selected runner: {} at {}",
            node.runner.name, node.node_path
        )
    };
    append_json_block(out, &runner_label, &node.runner.raw)?;
    append_tools(out, &node.node_path, &node.tools);
    for step in &node.steps {
        append_step(out, step)?;
    }
    Ok(())
}

fn append_step(out: &mut String, step: &SkillOperatorContextStep) -> Result<(), String> {
    let definition = &step.definition;
    out.push_str(&format!("\n--- graph step: {} ---\n", step.node_path));
    out.push_str(&format!(
        "step_id: {}\ntarget: {}\nexecution_boundary: {}\n",
        definition.id,
        step.target_label(),
        step.execution_boundary
            .map(|observation| observation.kind.as_str())
            .unwrap_or("none"),
    ));
    append_string_list(out, "tool_refs", &step.tool_refs);
    append_json_block(
        out,
        &format!("graph step contract: {}", step.node_path),
        definition,
    )?;
    for context in &step.context_skills {
        append_context_skill(out, &step.node_path, context)?;
    }
    for child in &step.children {
        append_node(out, child, false)?;
    }
    Ok(())
}

fn append_context_skill(
    out: &mut String,
    step_path: &str,
    context: &SkillOperatorContextContextSkill,
) -> Result<(), String> {
    out.push_str(&format!(
        "\ncontext_attachment: {step_path}\ncontext_ref: {}\ncontext_artifact_sha256: {}\ncontext_artifact_bytes: {}\n",
        context.reference, context.artifact_sha256, context.artifact_bytes,
    ));
    append_json_block(
        out,
        &format!("context skill: {} at {step_path}", context.reference),
        &context.artifact,
    )
}

fn append_tools(out: &mut String, node_path: &str, tools: &[SkillOperatorContextTool]) {
    out.push_str(&format!("\n--- tool manifests: {node_path} ---\n"));
    if tools.is_empty() {
        out.push_str("none declared by selected runner or graph steps\n");
        return;
    }
    for tool in tools {
        match (&tool.path, &tool.sha256, &tool.content) {
            (Some(path), Some(sha256), Some(content)) => {
                out.push_str(&format!(
                    "\n--- tool manifest: {} at {node_path} ---\npath: {}\nsha256: {sha256}\nsource: {}\nexecution_boundary: {}\n\n```json\n{content}\n```\n",
                    tool.name,
                    path.display(),
                    tool.source,
                    tool.execution_boundary.kind.as_str(),
                ));
            }
            _ => out.push_str(&format!(
                "tool: {}\nsource: {}\nexecution_boundary: {}\n",
                tool.name,
                tool.source,
                tool.execution_boundary.kind.as_str(),
            )),
        }
    }
}

fn append_document(
    out: &mut String,
    label: String,
    document: &SkillOperatorContextDocument,
    language: &str,
) {
    out.push_str(&format!("\n--- {label} ---\n"));
    match &document.path {
        Some(path) => out.push_str(&format!("path: {}\n", path.display())),
        None => out.push_str(&format!("source: {}\n", document.source_label)),
    }
    out.push_str(&format!(
        "sha256: {}\n\n```{language}\n{}\n```\n",
        document.sha256, document.content
    ));
}

fn append_json_block(
    out: &mut String,
    label: &str,
    value: &impl serde::Serialize,
) -> Result<(), String> {
    let contents = serde_json::to_string_pretty(value)
        .map_err(|error| format!("could not render {label} as JSON: {error}"))?;
    out.push_str(&format!("\n--- {label} ---\n\n```json\n{contents}\n```\n"));
    Ok(())
}

fn append_string_list(out: &mut String, label: &str, values: &[String]) {
    if values.is_empty() {
        out.push_str(&format!("{label}: none\n"));
    } else {
        out.push_str(&format!("{label}: {}\n", values.join(", ")));
    }
}
