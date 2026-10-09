use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue};

use super::{SkillRunError, invalid};
use crate::RuntimeError;

/// Resolution values retain their authority lane instead of flattening every
/// caller-supplied value into an agent answer. Approval provenance is needed
/// both by live resume files and by inline harness cases.
#[derive(Clone, Debug, Default)]
pub(crate) struct ResolutionAnswers {
    values: JsonObject,
    human_approvals: BTreeSet<String>,
    request_digests: BTreeMap<String, String>,
    digest_bound_requests: BTreeSet<String>,
    keys: AnswerKeys,
    /// The digest of the request each addressed answer was spent on.
    spent: BTreeMap<String, String>,
}

/// How an answer names the request it answers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AnswerKeys {
    /// By address: a resume answers one open request, once.
    #[default]
    Address,
    /// By the request's id in its own run: a harness script answers every
    /// ask of that id, at any depth.
    #[cfg(feature = "process-tool")]
    Script,
}

impl ResolutionAnswers {
    pub(super) fn agent(values: JsonObject) -> Self {
        Self {
            values,
            ..Self::default()
        }
    }

    /// These answers as a script: each answers every request with its id,
    /// in whichever run asks it.
    #[cfg(feature = "process-tool")]
    pub(super) fn scripted(self) -> Self {
        Self {
            keys: AnswerKeys::Script,
            ..self
        }
    }

    /// The key `request_id`, an address, is answered under.
    fn key<'a>(&self, request_id: &'a str) -> &'a str {
        match self.keys {
            AnswerKeys::Address => request_id,
            #[cfg(feature = "process-tool")]
            AnswerKeys::Script => crate::execution::run_record::local_request_id(request_id),
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
    /// and no other: a different request at its address waits. A script
    /// answers each.
    pub(super) fn spend(&mut self, request_id: &str, digest: &str) {
        if self.keys == AnswerKeys::Address {
            self.spent.insert(request_id.to_owned(), digest.to_owned());
        }
    }

    /// The answer to `request_id`, the request with `digest`.
    pub(super) fn get(&self, request_id: &str, digest: &str) -> Option<&JsonValue> {
        let key = self.key(request_id);
        if self.spent.get(key).is_some_and(|spent| spent != digest) {
            return None;
        }
        self.values.get(key)
    }

    pub(super) fn is_human_approval(&self, request_id: &str) -> bool {
        self.human_approvals.contains(self.key(request_id))
    }

    pub(super) fn request_digest(&self, request_id: &str) -> Option<&str> {
        self.request_digests
            .get(self.key(request_id))
            .map(String::as_str)
    }

    pub(super) fn requires_request_digest(&self, request_id: &str) -> bool {
        self.digest_bound_requests.contains(self.key(request_id))
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
    let nested_shape = object.contains_key("answers")
        || object.contains_key("approvals")
        || object.contains_key("request_digests");
    if !nested_shape {
        return Ok(ResolutionAnswers::agent(object));
    }
    let extra = object
        .keys()
        .filter(|key| !matches!(key.as_str(), "answers" | "approvals" | "request_digests"))
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
    let request_digests = match object.remove("request_digests") {
        Some(JsonValue::Object(digests)) => digests
            .into_iter()
            .map(|(id, digest)| match digest {
                JsonValue::String(digest) if !digest.trim().is_empty() => Ok((id, digest)),
                _ => Err(invalid(
                    "request_digests values must be non-empty digest strings",
                )),
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?,
        Some(_) => return Err(invalid("request_digests field must be a JSON object")),
        None => BTreeMap::new(),
    };
    let mut resolved = ResolutionAnswers::from_lanes(answers, approvals)?;
    if !request_digests.is_empty() {
        resolved
            .digest_bound_requests
            .extend(resolved.values.keys().cloned());
    }
    resolved.request_digests = request_digests;
    Ok(resolved)
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
