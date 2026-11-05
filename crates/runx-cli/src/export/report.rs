use std::io::Write;
use std::process::ExitCode;

use super::ExportReport;

pub(super) fn write_report(report: &ExportReport, json: bool) -> ExitCode {
    let output = if json {
        match serde_json::to_string_pretty(report) {
            Ok(value) => value,
            Err(error) => {
                let _ignored = writeln!(
                    std::io::stderr(),
                    "runx: failed to serialize export report: {error}"
                );
                return ExitCode::from(1);
            }
        }
    } else {
        human_report(report)
    };
    let mut stdout = std::io::stdout().lock();
    if writeln!(stdout, "{output}").is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn human_report(report: &ExportReport) -> String {
    let mut lines = vec![format!("runx export {} {}", report.target, report.scope)];
    for warning in &report.warnings {
        lines.push(format!("warning: {warning}"));
    }
    lines.push(format!("exported {} skill(s)", report.exported.len()));
    for exported in &report.exported {
        lines.push(format!("- {} -> {}", exported.skill, exported.path));
    }
    if !report.pruned.is_empty() {
        lines.push(format!("pruned {} stale file(s)", report.pruned.len()));
        for pruned in &report.pruned {
            lines.push(format!("- {pruned}"));
        }
    }
    if let Some(path) = &report.rules_file {
        lines.push(format!("updated rules: {path}"));
    }
    if let Some(server) = &report.mcp_server {
        let skills = server.refs.len();
        lines.push(if server.registered {
            format!(
                "registered MCP server {} for {skills} skill(s) whose agent makes calls; start a new host session to load it",
                server.name
            )
        } else {
            format!(
                "{skills} skill(s) whose agent makes calls need the MCP server {}; register it with: {}",
                server.name, server.command
            )
        });
    }
    lines.join("\n")
}
