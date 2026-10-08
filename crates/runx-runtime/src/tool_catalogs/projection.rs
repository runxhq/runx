use std::collections::BTreeMap;

use runx_contracts::tools::RuntimeCommand;
use runx_parser::ProcessTool;

pub(crate) fn runtime_command(tool: &ProcessTool) -> RuntimeCommand {
    RuntimeCommand {
        command: tool.command.clone(),
        args: tool.args.clone(),
        cwd: tool.cwd.clone(),
        env: BTreeMap::new(),
    }
}
