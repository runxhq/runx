use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_runtime::{
    DevFixtureLane, DevLoopOptions, DevReport, DevReportStatus, WorkspaceEnv, render_dev_result,
    run_dev_once,
};

use crate::router::DevPlan;

pub fn run_native_dev(plan: DevPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let root = resolve_root(workspace.env(), workspace.cwd());

    let mut options = DevLoopOptions::new(&root);
    options.env = workspace.env().clone();
    options.unit_path = plan
        .root
        .as_ref()
        .map(|path| resolve_unit_path(&root, path));
    if let Some(lane) = &plan.lane {
        options.lane = if lane == "all" {
            None
        } else {
            match lane.parse::<DevFixtureLane>() {
                Ok(lane) => Some(lane),
                Err(error) => {
                    let _ignored =
                        crate::cli_io::write_stderr(&format!("runx: invalid dev lane: {error}\n"));
                    return ExitCode::from(2);
                }
            }
        };
    }
    let report = match run_dev_once(&options) {
        Ok(report) => report,
        Err(error) => {
            let _ignored = crate::cli_io::write_stderr(&format!("runx: dev failed: {error:?}\n"));
            return ExitCode::from(1);
        }
    };

    let exit_code = match report.status {
        DevReportStatus::Success => 0,
        DevReportStatus::Skipped => 0,
        DevReportStatus::NeedsApproval => 0,
        DevReportStatus::Failure => 1,
    };

    let stdout = match render_dev_stdout(&report, plan.json) {
        Ok(stdout) => stdout,
        Err(error) => {
            let _ignored = crate::cli_io::write_stderr(&format!(
                "runx: failed to serialize dev report: {error}\n"
            ));
            return ExitCode::from(1);
        }
    };

    let _ignored = crate::cli_io::write_stdout(&stdout);
    ExitCode::from(exit_code)
}

fn resolve_root(env: &BTreeMap<String, String>, current_dir: &Path) -> PathBuf {
    runx_runtime::RunxPaths::resolve(env, current_dir)
        .workspace()
        .to_path_buf()
}

fn resolve_unit_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn render_dev_stdout(report: &DevReport, json: bool) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string_pretty(report).map(|text| format!("{text}\n"))
    } else {
        Ok(render_dev_result(report))
    }
}

#[cfg(test)]
mod tests {
    use runx_contracts::{
        DevReportSchema, DoctorReport, DoctorReportSchema, DoctorStatus, DoctorSummary,
    };
    use runx_runtime::{DevReport, DevReportStatus};

    use super::render_dev_stdout;

    #[test]
    fn dev_json_stdout_is_pretty_printed_like_ts_cli() -> Result<(), serde_json::Error> {
        let report = DevReport {
            schema: DevReportSchema::V1,
            status: DevReportStatus::Skipped,
            doctor: DoctorReport {
                schema: DoctorReportSchema::V1,
                status: DoctorStatus::Success,
                summary: DoctorSummary {
                    errors: 0,
                    warnings: 0,
                    infos: 0,
                },
                diagnostics: Vec::new(),
            },
            fixtures: Vec::new(),
            receipt_id: None,
        };

        let stdout = render_dev_stdout(&report, true)?;

        assert!(stdout.starts_with("{\n  \"schema\": \"runx.dev.v1\""));
        assert!(stdout.contains("\n  \"fixtures\": []\n"));
        assert!(stdout.ends_with('\n'));
        Ok(())
    }
}
