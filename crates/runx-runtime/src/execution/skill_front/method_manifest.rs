use super::{SkillRunError, invalid};

use crate::execution::orchestrator::SkillRunRequest;
use crate::services::{ReceiptServices, WorkspaceEnv};
use runx_parser::{SkillMethodDefinition, SkillMethodManifest};

pub(crate) fn selected_method<'a>(
    manifest: &'a SkillMethodManifest,
    requested: Option<&str>,
) -> Result<&'a SkillMethodDefinition, SkillRunError> {
    if let Some(name) = requested {
        return manifest
            .methods
            .get(name)
            .ok_or_else(|| invalid(format!("runner {name} is not declared in the manifest")));
    }
    manifest
        .default_method()
        .ok_or_else(|| invalid("the package has several methods and no default_method"))
}

pub(super) fn write_skill_receipt(
    request: &SkillRunRequest,
    workspace: &WorkspaceEnv,
    receipts: &ReceiptServices,
    receipt: &runx_contracts::Receipt,
) -> Result<(), SkillRunError> {
    let receipt_path = workspace.receipt_path(request.receipt_dir.as_deref());
    receipts
        .write_local_receipt(receipt, &receipt_path)
        .map_err(Into::into)
}
