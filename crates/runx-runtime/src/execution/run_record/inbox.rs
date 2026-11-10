//! A run's answer inbox (vocabulary-and-loops.md, "Answers reach a busy run
//! through an inbox"). A person's answer needs the run's driver, which a
//! session working another lead's turn holds, so a resume brought while a
//! driver lives writes its answers here, and the driver applies them between
//! its actions. Each delivery is checked against what the run waits on, its
//! digests and lanes, as it's written and again as it's applied.

use runx_contracts::ResumeDocument;
use serde::{Deserialize, Serialize};

use crate::kernel::store::{Append, Expected, KernelStore, KernelStoreError, StreamEvent};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum InboxEvent {
    /// A resume's answers, as its host bound them to what the run waited on.
    Delivered { answers: Box<ResumeDocument> },
}

impl StreamEvent for InboxEvent {
    const KIND: &'static str = "inbox";

    fn event_type(&self) -> &'static str {
        "delivered"
    }
}

/// Deliver `answers` to run `run_id`'s inbox, after every delivery before
/// it.
pub(crate) fn deliver(
    store: &KernelStore,
    run_id: &str,
    answers: &ResumeDocument,
) -> Result<(), KernelStoreError> {
    let expected = match store.version::<InboxEvent>(run_id)? {
        0 => Expected::Absent,
        version => Expected::Version(version),
    };
    store.append(vec![Append::kernel(
        run_id,
        expected,
        &InboxEvent::Delivered {
            answers: Box::new(answers.clone()),
        },
    )?])?;
    Ok(())
}

/// What was delivered to run `run_id` after version `after`, in the order
/// it was delivered, and the version read through.
pub(crate) fn delivered_after(
    store: &KernelStore,
    run_id: &str,
    after: u64,
) -> Result<(u64, Vec<ResumeDocument>), KernelStoreError> {
    let delivered = store.read_all_after::<InboxEvent>(run_id, after)?;
    let through = delivered.last().map_or(after, |recorded| recorded.version);
    Ok((
        through,
        delivered
            .into_iter()
            .map(|recorded| match recorded.event {
                InboxEvent::Delivered { answers } => *answers,
            })
            .collect(),
    ))
}
