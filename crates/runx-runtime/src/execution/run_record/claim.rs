//! Exact retained claims, addressed by the receipt that seals them. Reading a
//! claim never replays a run or resolves its current skill package.

use runx_contracts::{JsonObject, Receipt};
use serde::Deserialize;

use super::{Blob, CallOutcomeEvent, KernelStore, RunEvent, RunRecordError};

#[derive(Deserialize)]
struct RetainedClaim {
    contract: JsonObject,
}

pub(crate) fn retained_claim(
    store: &KernelStore,
    receipt: &Receipt,
) -> Result<Option<JsonObject>, RunRecordError> {
    let Some((run_id, _)) = receipt
        .subject
        .reference
        .uri
        .strip_prefix("runx:run:")
        .and_then(|subject| subject.split_once('#'))
    else {
        return Ok(None);
    };
    let receipt_blob = Blob::json(receipt)?;
    let mut after = 0;
    loop {
        // This is a page size, not a limit on the run or its claims. Only
        // this exact indexed stream's metadata is read, never the archive.
        let page = store.read_after::<RunEvent>(run_id, after, 128)?;
        if page.is_empty() {
            return Ok(None);
        }
        for recorded in page {
            after = recorded.version;
            let record = match recorded.event {
                RunEvent::Step(step) if step.receipt == receipt_blob.digest() => Some(step.record),
                RunEvent::Call(call) => match call.outcome {
                    CallOutcomeEvent::Completed {
                        record, receipt, ..
                    } if receipt == receipt_blob.digest() => Some(record),
                    _ => None,
                },
                _ => None,
            };
            let Some(record) = record else { continue };
            let claim: RetainedClaim = store.blob_json(&record)?;
            crate::receipts::seal::validate_step_receipt_claim(receipt, &claim.contract).map_err(
                |error| RunRecordError::Invalid {
                    run_id: run_id.to_owned(),
                    reason: error.to_string(),
                },
            )?;
            return Ok(Some(claim.contract));
        }
    }
}
