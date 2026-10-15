use std::collections::BTreeMap;

use runx_contracts::{
    JsonObject, JsonValue, ResolutionRequest, ResumeDocument, RetryResend, StandingApproval,
};

use super::{SkillRunError, invalid};
use crate::execution::run_record::{Answer, PendingRequest};

/// What a host brings a paused run, by the address of the request each entry
/// answers: an agent's answers and a person's decisions, each in its own lane
/// and bound to the digest of the request it answers. Beside them, the
/// decisions the run's record holds that no step has used yet, which any
/// driver of the run uses. A close never reaches a driver: it seals the run
/// from its record.
#[derive(Clone, Debug, Default)]
pub(crate) struct ResolutionAnswers {
    entries: BTreeMap<String, Entry>,
    /// The person's new standing approvals, when this host brings them.
    standing: Option<Vec<StandingApproval>>,
}

#[derive(Clone, Debug)]
struct Entry {
    digest: String,
    value: JsonValue,
    human: bool,
    /// Brought by this host and not yet in the run's record.
    brought: bool,
}

impl ResolutionAnswers {
    pub(super) fn from_document(document: ResumeDocument) -> Result<Self, SkillRunError> {
        for approval in document.standing_approvals.iter().flatten() {
            approval.check().map_err(invalid)?;
        }
        let mut entries = document
            .answers
            .into_iter()
            .map(|(address, answer)| {
                let entry = Entry {
                    digest: answer.request_digest.into_string(),
                    value: answer.value,
                    human: false,
                    brought: true,
                };
                (address, entry)
            })
            .collect::<BTreeMap<_, _>>();
        for (address, approval) in document.approvals {
            let value = match (approval.approved, approval.value) {
                (Some(approved), None) => {
                    let mut decision =
                        JsonObject::from([("approved".to_owned(), JsonValue::Bool(approved))]);
                    if let Some(reason) = approval.reason {
                        decision
                            .insert("reason".to_owned(), JsonValue::String(reason.into_string()));
                    }
                    JsonValue::Object(decision)
                }
                (None, Some(value)) if approval.reason.is_none() => value,
                _ => {
                    return Err(invalid(format!(
                        "{address} takes approved and a reason for an approval, or a value alone for a retry"
                    )));
                }
            };
            let entry = Entry {
                digest: approval.request_digest.into_string(),
                value,
                human: true,
                brought: true,
            };
            if entries.insert(address.clone(), entry).is_some() {
                return Err(invalid(format!(
                    "request {address} is declared in both answers and approvals"
                )));
            }
        }
        Ok(Self {
            entries,
            standing: document.standing_approvals,
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

    /// Whether this host brings agent work, not only a person's decisions.
    pub(super) fn answers_agents(&self) -> bool {
        self.entries
            .values()
            .any(|entry| entry.brought && !entry.human)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.standing.is_none()
    }

    /// The person's new standing approvals this host brings.
    pub(super) fn standing(&self) -> Option<&[StandingApproval]> {
        self.standing.as_deref()
    }

    /// Approve the request at `address` for the person, as a standing
    /// approval they just granted covers it; a decision this host brings for
    /// it stands instead.
    pub(super) fn grant(&mut self, address: &str, digest: &str) {
        self.entries
            .entry(address.to_owned())
            .or_insert_with(|| Entry {
                digest: digest.to_owned(),
                value: JsonValue::Object(JsonObject::from([
                    ("approved".to_owned(), JsonValue::Bool(true)),
                    (
                        "reason".to_owned(),
                        JsonValue::String("a standing approval covers it".to_owned()),
                    ),
                ])),
                human: true,
                brought: true,
            });
    }

    /// Every entry answers a request the run waits on now, by its address
    /// and its digest, in its lane: a person's approvals and retries in
    /// theirs, an agent's inputs and acts in the agent's. A request that
    /// changed since its host read it refuses the whole resume, and no entry
    /// reaches a request first asked during it.
    pub(super) fn check_waiting(
        &self,
        run_id: &str,
        waiting: &[PendingRequest],
    ) -> Result<(), SkillRunError> {
        for (address, entry) in &self.entries {
            let request = match waiting.iter().find(|request| request.id == *address) {
                Some(request) if request.digest == entry.digest => request,
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
            };
            let request: ResolutionRequest = serde_json::to_value(&request.value)
                .and_then(serde_json::from_value)
                .map_err(|error| invalid(format!("the request at {address}: {error}")))?;
            check_lane(address, entry, &request)?;
        }
        Ok(())
    }

    /// Join the answers the run's record holds that no step has used yet.
    /// One this host brings again is the same answer and is not recorded
    /// twice; a different one replaces it, as a corrected answer to a
    /// request that refused the first.
    pub(super) fn join_recorded(&mut self, recorded: Vec<Answer>) {
        for answer in recorded {
            match self.entries.get_mut(&answer.request_id) {
                Some(entry)
                    if entry.digest == answer.request_digest
                        && entry.value == answer.value
                        && entry.human == answer.human =>
                {
                    entry.brought = false;
                }
                // This host's entry answers the request now.
                Some(_) => {}
                None => {
                    self.entries.insert(
                        answer.request_id,
                        Entry {
                            digest: answer.request_digest,
                            value: answer.value,
                            human: answer.human,
                            brought: false,
                        },
                    );
                }
            }
        }
    }

    /// The person's decisions this host brings that the run's record does
    /// not hold yet. Their lane checked each against its request, so no step
    /// refuses one; an agent's answer is checked by the step that uses it,
    /// and recorded only when used.
    pub(super) fn brought(&self) -> Vec<Answer> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.brought && entry.human)
            .map(|(address, entry)| Answer {
                request_id: address.clone(),
                request_digest: entry.digest.clone(),
                value: entry.value.clone(),
                human: entry.human,
            })
            .collect()
    }
}

/// An entry answers its request in that request's lane, in the shape it
/// takes.
fn check_lane(
    address: &str,
    entry: &Entry,
    request: &ResolutionRequest,
) -> Result<(), SkillRunError> {
    match request {
        ResolutionRequest::Approval { .. } | ResolutionRequest::Retry { .. } if !entry.human => {
            Err(invalid(format!(
                "{address} is a person's to answer: only an entry under approvals carries a host-attested human decision"
            )))
        }
        ResolutionRequest::Input { .. } | ResolutionRequest::AgentAct { .. } if entry.human => Err(
            invalid(format!("{address} is an agent's to answer, under answers")),
        ),
        ResolutionRequest::Approval { .. }
            if entry
                .value
                .as_object()
                .and_then(|decision| decision.get("approved"))
                .is_none() =>
        {
            Err(invalid(format!(
                "{address} is an approval: answer it with approved"
            )))
        }
        // A bounded retry is runx's own: it runs again after its delay.
        ResolutionRequest::Retry {
            retry_after_seconds,
            ..
        } if *retry_after_seconds > 0 => Err(invalid(format!(
            "runx runs {address} again itself {retry_after_seconds} seconds after it paused; resume with no entries then"
        ))),
        ResolutionRequest::Retry {
            resend: RetryResend::Unsafe,
            ..
        } => Err(invalid(format!(
            "{address} cannot be sent again safely; check what it did, then close the run"
        ))),
        ResolutionRequest::Retry { .. } if entry.value != retry_acknowledgement() => Err(invalid(
            format!("{address} is acknowledged only with value {{\"retry\": true}}"),
        )),
        _ => Ok(()),
    }
}

fn retry_acknowledgement() -> JsonValue {
    JsonValue::Object(JsonObject::from([(
        "retry".to_owned(),
        JsonValue::Bool(true),
    )]))
}

#[cfg(test)]
mod tests {
    use runx_contracts::{ApprovalGate, ResolutionRequest, ResumeDocument, RetryResend};

    use super::ResolutionAnswers;
    use crate::execution::run_record::{Answer, PendingRequest};

    fn answers(value: serde_json::Value) -> Result<ResolutionAnswers, String> {
        let document: ResumeDocument =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        ResolutionAnswers::from_document(document).map_err(|error| error.to_string())
    }

    fn waiting(address: &str, digest: &str, request: ResolutionRequest) -> PendingRequest {
        PendingRequest {
            id: address.to_owned(),
            value: runx_contracts::JsonValue::from_serialize(&request)
                .unwrap_or(runx_contracts::JsonValue::Null),
            digest: digest.to_owned(),
        }
    }

    fn input(address: &str) -> ResolutionRequest {
        ResolutionRequest::Input {
            id: address.into(),
            questions: Vec::new(),
        }
    }

    fn approval(address: &str) -> ResolutionRequest {
        ResolutionRequest::Approval {
            id: address.into(),
            gate: ApprovalGate {
                id: address.into(),
                reason: "Send it?".into(),
                gate_type: None,
                summary: None,
                payload: None,
                payload_ref: None,
            },
        }
    }

    fn retry(address: &str, retry_after_seconds: u64, resend: RetryResend) -> ResolutionRequest {
        ResolutionRequest::Retry {
            id: address.into(),
            reason: "it has not settled".into(),
            retry_after_seconds,
            resend,
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
                .check_waiting("run", &[waiting("draft", "sha256:a", input("draft"))])
                .is_ok()
        );
        let changed = bound
            .check_waiting("run", &[waiting("draft", "sha256:b", input("draft"))])
            .err()
            .ok_or("a changed request is refused")?;
        assert!(changed.to_string().contains("changed since it was read"));
        assert!(
            bound
                .check_waiting("run", &[waiting("other", "sha256:a", input("other"))])
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn each_lane_answers_only_its_own_requests() -> Result<(), String> {
        let refused = |document: serde_json::Value, request: ResolutionRequest| {
            answers(document).and_then(|bound| {
                bound
                    .check_waiting("run", &[waiting("ask", "sha256:a", request)])
                    .map_err(|error| error.to_string())
            })
        };
        let agent = serde_json::json!({"answers": {"ask": {"request_digest": "sha256:a", "value": {"retry": true}}}});
        let person = serde_json::json!({"approvals": {"ask": {"request_digest": "sha256:a", "value": {"retry": true}}}});
        assert!(refused(agent.clone(), approval("ask")).is_err());
        assert!(refused(agent, retry("ask", 0, RetryResend::Safe)).is_err());
        assert!(refused(person.clone(), input("ask")).is_err());
        assert!(refused(person.clone(), approval("ask")).is_err());
        assert!(refused(person.clone(), retry("ask", 0, RetryResend::Safe)).is_ok());
        let unsafe_resend = refused(person.clone(), retry("ask", 0, RetryResend::Unsafe))
            .err()
            .ok_or("an unsafe resend is refused")?;
        assert!(
            unsafe_resend.contains("cannot be sent again safely"),
            "{unsafe_resend}"
        );
        let bounded = refused(person, retry("ask", 300, RetryResend::Safe))
            .err()
            .ok_or("a bounded retry is runx's own")?;
        assert!(bounded.contains("again itself"), "{bounded}");
        assert!(
            refused(
                serde_json::json!({"approvals": {"ask": {"request_digest": "sha256:a", "value": {"retry": false}}}}),
                retry("ask", 0, RetryResend::Safe)
            )
            .is_err()
        );
        assert!(
            answers(serde_json::json!({"approvals": {"ask": {"request_digest": "sha256:a"}}}))
                .is_err(),
            "an entry decides something"
        );
        Ok(())
    }

    #[test]
    fn a_recorded_answer_is_used_and_a_redelivery_is_not_recorded_twice() -> Result<(), String> {
        let recorded = || Answer {
            request_id: "send.approval".to_owned(),
            request_digest: "sha256:b".to_owned(),
            value: runx_contracts::JsonValue::Object(
                [("approved".to_owned(), runx_contracts::JsonValue::Bool(true))].into(),
            ),
            human: true,
        };
        let mut empty = ResolutionAnswers::default();
        empty.join_recorded(vec![recorded()]);
        assert!(!empty.is_empty(), "a resume with no entries continues");
        assert!(empty.get("send.approval", "sha256:b").is_some());
        assert!(empty.brought().is_empty());

        let mut again = answers(serde_json::json!({
            "approvals": {"send.approval": {"request_digest": "sha256:b", "approved": true}}
        }))?;
        again.join_recorded(vec![recorded()]);
        assert!(
            again.brought().is_empty(),
            "the same answer is recorded once"
        );

        let mut corrected = answers(serde_json::json!({
            "approvals": {"send.approval": {"request_digest": "sha256:b", "approved": false}}
        }))?;
        corrected.join_recorded(vec![recorded()]);
        assert_eq!(
            corrected.brought().len(),
            1,
            "a different answer replaces it"
        );
        assert_eq!(
            corrected.get("send.approval", "sha256:b"),
            Some(&runx_contracts::JsonValue::Object(
                [(
                    "approved".to_owned(),
                    runx_contracts::JsonValue::Bool(false)
                )]
                .into(),
            ))
        );
        Ok(())
    }
}
