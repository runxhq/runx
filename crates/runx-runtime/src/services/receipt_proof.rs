use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue};

use crate::receipts::RuntimeReceiptSignaturePolicy;
use crate::receipts::inspection::project_receipt_inspection_with_policy;
use crate::receipts::store::LocalReceiptStore;
use crate::services::ReceiptReadContext;
use crate::{ReceiptTreeConfig, RuntimeError, verify_runtime_receipt_tree_with_policy};

mod projection;
mod tree;

use projection::{matched_receipt, proof_packet, to_json_value, tree_findings, tree_projection};
use tree::{load_children, store_finding};

const TOOL: &str = "receipt.prove";
const MAX_RECEIPTS: usize = 100;
const MAX_TREE_RECEIPTS: usize = 10_000;

struct ProofProjection {
    matched: Vec<JsonValue>,
    details: Vec<JsonValue>,
    trees: Vec<JsonValue>,
    findings: Vec<JsonValue>,
}

impl ProofProjection {
    fn new() -> Self {
        Self {
            matched: Vec::new(),
            details: Vec::new(),
            trees: Vec::new(),
            findings: Vec::new(),
        }
    }

    fn inspect(
        &mut self,
        receipt_id: &str,
        store: &LocalReceiptStore,
        store_label: &crate::ReceiptStoreLabel,
        policy: RuntimeReceiptSignaturePolicy<'_>,
    ) -> Result<(), RuntimeError> {
        let root = match store.read_exact_with_policy(receipt_id, policy) {
            Ok(receipt) => receipt,
            Err(error) => {
                store_finding(&mut self.findings, receipt_id, &error, store_label);
                return Ok(());
            }
        };
        let (children, load_findings) = load_children(store, &root, policy);
        self.findings.extend(load_findings);
        let verification = verify_runtime_receipt_tree_with_policy(
            &root,
            children.clone(),
            ReceiptTreeConfig::default(),
            policy,
        );
        let tree_findings = tree_findings(&verification.findings);
        self.findings.extend(tree_findings.clone());
        let inspection = project_receipt_inspection_with_policy(&root, policy);
        self.matched.push(matched_receipt(&inspection));
        self.details.push(to_json_value(&inspection)?);
        self.trees.push(tree_projection(
            receipt_id,
            children.len() + 1,
            verification.valid,
            tree_findings,
        ));
        Ok(())
    }

    fn all_trees_valid(&self) -> bool {
        self.trees.iter().all(|tree| {
            tree.as_object()
                .and_then(|tree| tree.get("valid"))
                .and_then(JsonValue::as_bool)
                == Some(true)
        })
    }
}

pub(crate) fn prove_receipts(
    receipt_ids: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<JsonObject, RuntimeError> {
    let context = ReceiptReadContext::resolve(env, cwd)?;
    prove_receipts_with_context(receipt_ids, &context)
}

pub(crate) fn prove_receipts_with_context(
    receipt_ids: &[String],
    context: &ReceiptReadContext,
) -> Result<JsonObject, RuntimeError> {
    let receipt_ids = validate_receipt_ids(receipt_ids)?;
    let resolved = context.resolved();
    let mut projection = ProofProjection::new();
    for receipt_id in &receipt_ids {
        projection.inspect(
            receipt_id,
            context.store(),
            &resolved.label,
            context.signature_policy(),
        )?;
    }

    let complete = projection.matched.len() == receipt_ids.len();
    let trees_valid = projection.all_trees_valid();
    Ok(proof_packet(
        receipt_ids,
        projection,
        context.signature_mode(),
        resolved.label.as_str(),
        complete,
        trees_valid,
    ))
}

fn validate_receipt_ids(values: &[String]) -> Result<Vec<String>, RuntimeError> {
    if values.is_empty() || values.len() > MAX_RECEIPTS {
        return Err(invalid("receipt_ids must contain from 1 to 100 exact ids"));
    }
    let mut ids = BTreeSet::new();
    for value in values {
        let id = Some(value.trim())
            .filter(|id| valid_receipt_id(id))
            .ok_or_else(|| invalid("receipt_ids must contain lowercase sha256 ids"))?;
        if !ids.insert(id.to_owned()) {
            return Err(invalid("receipt_ids must be unique"));
        }
    }
    Ok(ids.into_iter().collect())
}

fn valid_receipt_id(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: TOOL.to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
