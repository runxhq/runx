use std::ffi::OsString;

use crate::cli_args::{os_arg, split_flag};

use super::{ExportPlan, Target};

// Function rationale: this parser mirrors the flat native
// router grammar and keeps all export flags in one auditable pass.
pub fn parse_export_plan(args: &[OsString]) -> Result<ExportPlan, String> {
    let target = match os_arg(args, 1, "export")? {
        "claude" => Target::Claude,
        "codex" => Target::Codex,
        value => {
            return Err(format!(
                "runx export target must be claude or codex, got {value}"
            ));
        }
    };
    let mut refs = Vec::new();
    let mut project = false;
    let mut json = false;
    let mut index = 2;

    while index < args.len() {
        let token = os_arg(args, index, "export")?;
        if !crate::cli_args::is_flag(token) {
            refs.push(token.to_owned());
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        match flag {
            "--json" | "-j" => {
                if inline_value.is_some() {
                    return Err("--json does not take a value".to_owned());
                }
                json = true;
            }
            "--project" => {
                if inline_value.is_some() {
                    return Err("--project does not take a value".to_owned());
                }
                project = true;
            }
            _ => return Err(format!("unknown export flag {flag}")),
        }
        index += 1;
    }

    Ok(ExportPlan {
        target,
        refs,
        project,
        json,
    })
}
