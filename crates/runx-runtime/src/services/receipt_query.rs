use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::JsonObject;

use self::input::QueryRequest;
pub(crate) use self::input::ReceiptQueryInput;
use self::projection::render_query;
use crate::RuntimeError;
use crate::paths::RunxPaths;
use crate::receipts::RuntimeReceiptSignaturePolicy;
use crate::receipts::archive::{ReceiptListing, list_receipts};
use crate::receipts::store::LocalReceiptStore;
use crate::services::ReceiptReadContext;
use crate::services::receipt_proof::prove_receipts_with_context;

mod input;
mod projection;

const TOOL: &str = "receipt.query";
const MAX_PROOF_RECEIPTS: usize = 100;

pub(crate) fn query_receipts(
    inputs: &ReceiptQueryInput,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<JsonObject, RuntimeError> {
    let request = QueryRequest::parse(inputs)?;
    let context = ReceiptReadContext::resolve(env, cwd)?;
    let resolved = context.resolved();
    let history = load_history(
        &request,
        context.store(),
        context.paths(),
        context.signature_policy(),
    )?;
    let history_ids = history
        .as_ref()
        .map(|history| {
            history
                .receipts
                .iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let proof_ids = request.exact_ids.as_ref().unwrap_or(&history_ids);
    let proof_count = proof_ids.len();
    let proof_limit_exceeded = request.verify_chain && proof_count > MAX_PROOF_RECEIPTS;
    let proof = load_proof(
        proof_ids,
        request.verify_chain || request.exact_ids.is_some(),
        proof_limit_exceeded,
        &context,
    )?;
    render_query(
        request,
        history,
        proof,
        context.signature_mode(),
        resolved.label.as_str(),
        proof_limit_exceeded,
        proof_count,
    )
}

fn load_history(
    request: &QueryRequest,
    store: &LocalReceiptStore,
    paths: &RunxPaths,
    policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Option<ReceiptListing>, RuntimeError> {
    if request.exact_ids.is_some() {
        return Ok(None);
    }
    list_receipts(store, paths, &request.filter, policy)
        .map(Some)
        .map_err(|error| invalid(format!("native receipt history failed: {error}")))
}

fn load_proof(
    receipt_ids: &[String],
    requested: bool,
    limit_exceeded: bool,
    context: &ReceiptReadContext,
) -> Result<Option<JsonObject>, RuntimeError> {
    if !requested || receipt_ids.is_empty() || limit_exceeded {
        return Ok(None);
    }
    prove_receipts_with_context(receipt_ids, context).map(Some)
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: TOOL.to_owned(),
        message: message.into(),
    }
}
