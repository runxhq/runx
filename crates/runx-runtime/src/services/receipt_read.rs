//! Sensitive declared-output reads. Receipt metadata remains available through
//! receipt.query/prove; this boundary requires receipt.claim.read authority.

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{JsonValue, ReceiptRead, ReceiptReadSchema, ReceiptReadStatus, SubjectKind};

use super::ReceiptReadContext;
use super::receipt_proof::load_verified_tree;
use crate::RuntimeError;
use crate::execution::run_record::retained_claim;
use crate::kernel::store::{KERNEL_STORE_FILE, KernelStore, MAX_BLOB_BYTES};

pub(crate) fn read_receipt_claim(
    receipt_id: &str,
    root_receipt_id: Option<&str>,
    json_pointer: &str,
    max_bytes: u64,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ReceiptRead, RuntimeError> {
    if max_bytes == 0 || max_bytes > MAX_BLOB_BYTES as u64 {
        return Err(invalid(
            "max_bytes must be positive and at most the native kernel blob ceiling",
        ));
    }
    let context = ReceiptReadContext::resolve(env, cwd)?;
    let tree = load_verified_tree(
        context.store(),
        root_receipt_id.unwrap_or(receipt_id),
        context.signature_policy(),
    )
    .map_err(|error| invalid(error.public_message(&context.resolved().label)))?;
    if !tree.verification.valid || !tree.findings.is_empty() {
        return Err(invalid(
            "the requested receipt tree is incomplete or invalid",
        ));
    }
    let original_root = root_receipt_id.map(|_| tree.root.id.to_string().into());
    // The verified load contains only signed descendants. Do not look up an
    // unrelated selected id, even if another valid receipt exists in the store.
    let receipt = if tree.root.id.as_str() == receipt_id {
        tree.root
    } else {
        tree.children
            .into_iter()
            .find(|receipt| receipt.id.as_str() == receipt_id)
            .ok_or_else(|| {
                invalid("the selected receipt does not belong to the original receipt tree")
            })?
    };
    let mut output = ReceiptRead {
        schema: ReceiptReadSchema::V1,
        status: ReceiptReadStatus::NotAStep,
        receipt_id: receipt.id.to_string().into(),
        root_receipt_id: original_root,
        subject_kind: receipt.subject.kind,
        subject_uri: receipt.subject.reference.uri.to_string().into(),
        issuer_type: receipt.issuer.issuer_type.clone(),
        outcome: receipt.seal.outcome,
        outcome_digest: receipt.subject.outcome_digest.clone(),
        children: receipt.lineage.children.clone(),
        json_pointer: json_pointer.to_owned(),
        value: None,
        bytes: 0,
    };
    if receipt.subject.kind != SubjectKind::Step {
        return Ok(output);
    }
    output.status = ReceiptReadStatus::NotRetained;
    let database = context.paths().project().join(KERNEL_STORE_FILE);
    match database.try_exists() {
        Ok(true) => {}
        Ok(false) => return Ok(output),
        Err(_) => return Err(invalid("the selected kernel store is unreadable")),
    }
    let store = KernelStore::for_project(context.paths())
        .map_err(|error| RuntimeError::effect_state("reading retained receipt claims", error))?;
    let Some(claim) = retained_claim(&store, &receipt)? else {
        return Ok(output);
    };
    // serde_json owns RFC 6901 selection, including escaped and empty keys.
    // The complete native claim has already passed its signed digest check.
    let claim = serde_json::to_value(claim)
        .map_err(|error| RuntimeError::json("selecting retained claim", error))?;
    let selected = claim
        .pointer(json_pointer)
        .ok_or_else(|| invalid("json_pointer does not resolve in the retained claim"))?;
    let bytes = serde_json::to_vec(selected)
        .map_err(|error| RuntimeError::json("measuring retained claim selection", error))?;
    if bytes.len() as u64 > max_bytes {
        return Err(invalid(
            "the selected claim exceeds max_bytes; select a narrower json_pointer or increase the budget",
        ));
    }
    output.value = Some(
        serde_json::from_slice::<JsonValue>(&bytes)
            .map_err(|error| RuntimeError::json("projecting retained claim", error))?,
    );
    output.bytes = bytes.len() as u64;
    output.status = ReceiptReadStatus::Available;
    Ok(output)
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: "receipt.read".to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
