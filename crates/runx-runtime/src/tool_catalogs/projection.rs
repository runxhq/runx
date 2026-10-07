use std::collections::BTreeMap;

use runx_contracts::tools::RuntimeCommand;
use runx_parser::{SkillSource, SourceKind};

pub(crate) fn runtime_command(source: &SkillSource) -> Option<RuntimeCommand> {
    match source.source_type {
        SourceKind::CliTool => source.command.as_ref().map(|command| RuntimeCommand {
            command: command.clone(),
            args: source.args.clone(),
            cwd: source.cwd.clone(),
            env: BTreeMap::new(),
        }),
        _ => None,
    }
}
