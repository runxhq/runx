use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue};

use super::{SkillRunError, invalid};
use crate::RuntimeError;

/// Resolution values retain their authority lane instead of flattening every
/// caller-supplied value into an agent answer. Each is keyed by the address
/// of the one open request it answers, once.
#[derive(Clone, Debug, Default)]
pub(crate) struct ResolutionAnswers {
    values: JsonObject,
    human_approvals: BTreeSet<String>,
    /// The digest of the request each addressed answer was spent on.
    spent: BTreeMap<String, String>,
}

impl ResolutionAnswers {
    pub(super) fn agent(values: JsonObject) -> Self {
        Self {
            values,
            ..Self::default()
        }
    }

    pub(super) fn from_lanes(
        answers: JsonObject,
        approvals: impl IntoIterator<Item = (String, JsonValue)>,
    ) -> Result<Self, SkillRunError> {
        let mut resolved = Self::agent(answers);
        for (gate_id, decision) in approvals {
            if !is_human_approval_payload(&decision) {
                return Err(invalid(format!(
                    "approvals.{gate_id} must be a boolean or {{approved: boolean, reason?: string}}"
                )));
            }
            if resolved.values.insert(gate_id.clone(), decision).is_some() {
                return Err(invalid(format!(
                    "request {gate_id} is declared in both answers and approvals"
                )));
            }
            resolved.human_approvals.insert(gate_id);
        }
        Ok(resolved)
    }

    /// Spend the answer to `request_id` on the request with `digest`. An
    /// addressed answer answers that request wherever it is asked again,
    /// and no other: a different request at its address waits.
    pub(super) fn spend(&mut self, request_id: &str, digest: &str) {
        self.spent.insert(request_id.to_owned(), digest.to_owned());
    }

    /// The answer to `request_id`, the request with `digest`.
    pub(super) fn get(&self, request_id: &str, digest: &str) -> Option<&JsonValue> {
        if self
            .spent
            .get(request_id)
            .is_some_and(|spent| spent != digest)
        {
            return None;
        }
        self.values.get(request_id)
    }

    pub(super) fn is_human_approval(&self, request_id: &str) -> bool {
        self.human_approvals.contains(request_id)
    }

    /// A person approves only what they were shown: every approval a resume
    /// brings names one of `waiting`, the addresses the run is paused at, so
    /// none reaches a request first asked during the resume.
    pub(super) fn check_waiting(
        &self,
        run_id: &str,
        waiting: &[&str],
    ) -> Result<(), SkillRunError> {
        match self
            .human_approvals
            .iter()
            .find(|key| !waiting.contains(&key.as_str()))
        {
            Some(key) => Err(invalid(format!(
                "approval {key} names no request run {run_id} waits on; it waits on [{}]",
                waiting.join(", ")
            ))),
            None => Ok(()),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

pub(super) fn read_answers(path: &Path) -> Result<ResolutionAnswers, SkillRunError> {
    let raw = fs::read_to_string(path)
        .map_err(|source| RuntimeError::io(format!("reading {}", path.display()), source))?;
    let value = serde_json::from_str::<JsonValue>(&raw).map_err(|source| {
        RuntimeError::json(format!("parsing answers file {}", path.display()), source)
    })?;
    match value {
        JsonValue::Object(object) => normalize_answers(object),
        _ => Err(invalid("answers file must be a JSON object")),
    }
}

fn normalize_answers(mut object: JsonObject) -> Result<ResolutionAnswers, SkillRunError> {
    let nested_shape = object.contains_key("answers") || object.contains_key("approvals");
    if !nested_shape {
        return Ok(ResolutionAnswers::agent(object));
    }
    let extra = object
        .keys()
        .filter(|key| !matches!(key.as_str(), "answers" | "approvals"))
        .cloned()
        .collect::<Vec<_>>();
    if !extra.is_empty() {
        return Err(invalid(format!(
            "answers file mixes top-level keys [{}] with the nested answers/approvals shape",
            extra.join(", ")
        )));
    }
    let answers = match object.remove("answers") {
        Some(JsonValue::Object(nested)) => nested,
        Some(_) => return Err(invalid("answers field must be a JSON object")),
        None => JsonObject::new(),
    };
    let approvals = match object.remove("approvals") {
        Some(JsonValue::Object(approvals)) => approvals,
        Some(_) => return Err(invalid("approvals field must be a JSON object")),
        None => JsonObject::new(),
    };
    ResolutionAnswers::from_lanes(answers, approvals)
}

fn is_human_approval_payload(value: &JsonValue) -> bool {
    match value {
        JsonValue::Bool(_) => true,
        JsonValue::Object(object) => {
            matches!(object.get("approved"), Some(JsonValue::Bool(_)))
                && object
                    .keys()
                    .all(|key| matches!(key.as_str(), "approved" | "reason"))
                && match object.get("reason") {
                    None => true,
                    Some(JsonValue::String(reason)) => !reason.trim().is_empty(),
                    Some(_) => false,
                }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_answers;
    use runx_contracts::{JsonObject, JsonValue};

    #[test]
    fn nested_approvals_retain_host_attested_human_provenance() -> Result<(), String> {
        let approval_id = "send.approval";
        let answers = normalize_answers(JsonObject::from([
            (
                "answers".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "agent.task".to_owned(),
                    JsonValue::String("done".to_owned()),
                )])),
            ),
            (
                "approvals".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    approval_id.to_owned(),
                    JsonValue::Object(JsonObject::from([
                        ("approved".to_owned(), JsonValue::Bool(true)),
                        (
                            "reason".to_owned(),
                            JsonValue::String("operator authorized the exact send".to_owned()),
                        ),
                    ])),
                )])),
            ),
        ]))
        .map_err(|error| error.to_string())?;

        assert!(answers.is_human_approval(approval_id));
        assert!(!answers.is_human_approval("agent.task"));
        assert!(matches!(
            answers.get(approval_id, "sha256:approval"),
            Some(JsonValue::Object(_))
        ));
        Ok(())
    }

    #[test]
    fn an_addressed_answer_answers_its_request_and_no_other() -> Result<(), String> {
        let mut answers = normalize_answers(JsonObject::from([(
            "agent.answer.output".to_owned(),
            JsonValue::String("done".to_owned()),
        )]))
        .map_err(|error| error.to_string())?;
        answers.spend("agent.answer.output", "sha256:first");
        assert!(
            answers.get("agent.answer.output", "sha256:first").is_some(),
            "the same request asked again is answered again"
        );
        assert!(
            answers
                .get("agent.answer.output", "sha256:second")
                .is_none(),
            "a different request at the same address waits"
        );
        assert!(
            answers
                .get("answer/0#agent.answer.output", "sha256:first")
                .is_none(),
            "an addressed answer answers only its own address"
        );
        Ok(())
    }

    #[test]
    fn flat_answers_do_not_gain_human_approval_authority() -> Result<(), String> {
        let answers = normalize_answers(JsonObject::from([(
            "send.approval".to_owned(),
            JsonValue::Bool(true),
        )]))
        .map_err(|error| error.to_string())?;

        assert!(!answers.is_human_approval("send.approval"));
        Ok(())
    }
}
