//! Exact retained claims, addressed by the receipt that seals them. Reading a
//! claim never replays a run or resolves its current skill package.

use std::collections::BTreeSet;

use runx_contracts::{JsonObject, Receipt};
use serde::Deserialize;

use super::{Blob, CallOutcomeEvent, KernelStore, RunEvent, RunRecordError, calls_key};

#[derive(Deserialize)]
struct RetainedClaim {
    contract: JsonObject,
    #[serde(default)]
    artifact_outputs: BTreeSet<String>,
}

pub(crate) fn retained_claim(
    store: &KernelStore,
    receipt: &Receipt,
) -> Result<Option<JsonObject>, RunRecordError> {
    let Some((run_id, fragment)) = receipt
        .subject
        .reference
        .uri
        .strip_prefix("runx:run:")
        .and_then(|subject| subject.split_once('#'))
    else {
        return Ok(None);
    };
    let receipt_blob = Blob::json(receipt)?;
    // A call's receipt names it `<step>/<index>`, and its events are on its
    // step's calls stream, one for each attempt the step made; a step's are
    // on its run's own stream.
    let step = fragment.split_once('@').map_or(fragment, |(step, _)| step);
    let streams = match step.rsplit_once('/') {
        Some((step_id, _)) => {
            let mut streams = Vec::new();
            for attempt in 1.. {
                let key = calls_key(run_id, step_id, attempt);
                if store.version::<RunEvent>(&key)? == 0 {
                    break;
                }
                streams.push(key);
            }
            streams
        }
        None => vec![run_id.to_owned()],
    };
    for stream in streams {
        if let Some(claim) = claim_on(store, receipt, &receipt_blob, run_id, &stream)? {
            return Ok(Some(claim));
        }
    }
    Ok(None)
}

/// The claim the step or call that sealed `receipt` retained, on `stream`.
fn claim_on(
    store: &KernelStore,
    receipt: &Receipt,
    receipt_blob: &Blob,
    run_id: &str,
    stream: &str,
) -> Result<Option<JsonObject>, RunRecordError> {
    let mut after = 0;
    loop {
        // This is a page size, not a limit on the run or its claims. Only
        // this exact indexed stream's metadata is read, never the archive.
        let page = store.read_after::<RunEvent>(stream, after, 128)?;
        if page.is_empty() {
            return Ok(None);
        }
        for recorded in page {
            after = recorded.version;
            let (record, held) = match recorded.event {
                RunEvent::Step(step) if step.receipt == receipt_blob.digest() => {
                    (Some(step.record), Vec::new())
                }
                RunEvent::Call(call) => match call.outcome {
                    CallOutcomeEvent::Completed {
                        record,
                        receipt,
                        outputs,
                        ..
                    } if receipt == receipt_blob.digest() => (Some(record), outputs),
                    _ => (None, Vec::new()),
                },
                _ => (None, Vec::new()),
            };
            let Some(record) = record else { continue };
            let mut claim: RetainedClaim = store.blob_json(&record)?;
            // A call's record names the artifacts that hold its fields.
            super::read_held_into(store, &mut claim.contract, &claim.artifact_outputs, &held)
                .map_err(|reason| RunRecordError::Invalid {
                    run_id: run_id.to_owned(),
                    reason,
                })?;
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
