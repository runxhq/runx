use std::collections::BTreeMap;

use runx_contracts::{JsonObject, JsonValue, ResumeDocument, RunClose};

use super::{SkillRunError, invalid};
use crate::execution::run_record::PendingRequest;

/// What a host brings a paused run, by the address of the request each entry
/// answers: agent answers and a person's approvals, each in its own lane and
/// bound to the digest of the request it answers; or a close, which comes
/// alone.
#[derive(Clone, Debug, Default)]
pub(crate) struct ResolutionAnswers {
    entries: BTreeMap<String, Entry>,
    close: Option<RunClose>,
}

#[derive(Clone, Debug)]
struct Entry {
    digest: String,
    value: JsonValue,
    human: bool,
}

impl ResolutionAnswers {
    pub(super) fn from_document(document: ResumeDocument) -> Result<Self, SkillRunError> {
        if document.close.is_some()
            && (!document.answers.is_empty() || !document.approvals.is_empty())
        {
            return Err(invalid("a close comes alone, without answers or approvals"));
        }
        let mut entries = document
            .answers
            .into_iter()
            .map(|(address, answer)| {
                let entry = Entry {
                    digest: answer.request_digest.into_string(),
                    value: answer.value,
                    human: false,
                };
                (address, entry)
            })
            .collect::<BTreeMap<_, _>>();
        for (address, approval) in document.approvals {
            let mut decision =
                JsonObject::from([("approved".to_owned(), JsonValue::Bool(approval.approved))]);
            if let Some(reason) = approval.reason {
                decision.insert("reason".to_owned(), JsonValue::String(reason.into_string()));
            }
            let entry = Entry {
                digest: approval.request_digest.into_string(),
                value: JsonValue::Object(decision),
                human: true,
            };
            if entries.insert(address.clone(), entry).is_some() {
                return Err(invalid(format!(
                    "request {address} is declared in both answers and approvals"
                )));
            }
        }
        Ok(Self {
            entries,
            close: document.close,
        })
    }

    /// The answer to the request at `address` whose digest is `digest`; a
    /// different request at that address is not answered.
    pub(super) fn get(&self, address: &str, digest: &str) -> Option<&JsonValue> {
        self.entries
            .get(address)
            .filter(|entry| entry.digest == digest)
            .map(|entry| &entry.value)
    }

    pub(super) fn is_human_approval(&self, address: &str) -> bool {
        self.entries.get(address).is_some_and(|entry| entry.human)
    }

    /// Whether these answers carry agent work, not only a person's approvals.
    pub(super) fn answers_agents(&self) -> bool {
        self.entries.values().any(|entry| !entry.human)
    }

    pub(super) fn close(&self) -> Option<RunClose> {
        self.close
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.close.is_none()
    }

    /// Every entry answers a request the run waits on now, by its address and
    /// its digest. A request that changed since its host read it refuses the
    /// whole resume, and no entry reaches a request first asked during it.
    pub(super) fn check_waiting(
        &self,
        run_id: &str,
        waiting: &[PendingRequest],
    ) -> Result<(), SkillRunError> {
        for (address, entry) in &self.entries {
            match waiting.iter().find(|request| request.id == *address) {
                Some(request) if request.digest == entry.digest => {}
                Some(request) => {
                    return Err(invalid(format!(
                        "the request at {address} changed since it was read; run {run_id} waits on {}",
                        request.digest
                    )));
                }
                None => {
                    return Err(invalid(format!(
                        "{address} names no request run {run_id} waits on; it waits on [{}]",
                        waiting
                            .iter()
                            .map(|request| request.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use runx_contracts::ResumeDocument;

    use super::ResolutionAnswers;
    use crate::execution::run_record::PendingRequest;

    fn answers(value: serde_json::Value) -> Result<ResolutionAnswers, String> {
        let document: ResumeDocument =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        ResolutionAnswers::from_document(document).map_err(|error| error.to_string())
    }

    fn waiting(address: &str, digest: &str) -> PendingRequest {
        PendingRequest {
            id: address.to_owned(),
            value: runx_contracts::JsonValue::Null,
            digest: digest.to_owned(),
        }
    }

    #[test]
    fn an_entry_answers_only_the_request_it_is_bound_to() -> Result<(), String> {
        let bound = answers(serde_json::json!({
            "answers": {"draft": {"request_digest": "sha256:a", "value": "done"}},
            "approvals": {"send.approval": {"request_digest": "sha256:b", "approved": true}}
        }))?;
        assert!(bound.get("draft", "sha256:a").is_some());
        assert!(bound.get("draft", "sha256:changed").is_none());
        assert!(bound.is_human_approval("send.approval"));
        assert!(!bound.is_human_approval("draft"));
        assert!(bound.answers_agents());
        Ok(())
    }

    #[test]
    fn a_resume_refuses_a_changed_or_unasked_request() -> Result<(), String> {
        let bound = answers(serde_json::json!({
            "answers": {"draft": {"request_digest": "sha256:a", "value": "done"}}
        }))?;
        assert!(
            bound
                .check_waiting("run", &[waiting("draft", "sha256:a")])
                .is_ok()
        );
        let changed = bound
            .check_waiting("run", &[waiting("draft", "sha256:b")])
            .err()
            .ok_or("a changed request is refused")?;
        assert!(changed.to_string().contains("changed since it was read"));
        assert!(
            bound
                .check_waiting("run", &[waiting("other", "sha256:a")])
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn a_close_comes_alone() {
        assert!(answers(serde_json::json!({"close": {"disposition": "declined"}})).is_ok());
        assert!(
            answers(serde_json::json!({
                "close": {"disposition": "superseded"},
                "approvals": {"send.approval": {"request_digest": "sha256:b", "approved": true}}
            }))
            .is_err()
        );
    }
}
