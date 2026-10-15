//! What a host brings a paused run (`runx resume <run-id> -`): an agent's
//! answers and a person's decisions, each bound by digest to the request it
//! answers, or a close. A decision therefore never reaches a request that
//! changed after the person saw it.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::JsonValue;
use crate::decision::ClosureDisposition;
use crate::schema::{NonEmptyString, RunxSchema};

/// Each map is keyed by the address of the request it answers. `answers` is
/// the agent's lane; `approvals` and `standing_approvals` are the person's,
/// and every entry in them resolves as theirs. A close comes alone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.resume.v1")]
pub struct ResumeDocument {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub answers: BTreeMap<String, BoundAnswer>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub approvals: BTreeMap<String, BoundApproval>,
    /// The person's new standing approvals, which replace the run's set for
    /// every later step and answer any open approval of a local effect they
    /// cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing_approvals: Option<Vec<StandingApproval>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<RunClose>,
}

/// A person's bounded confirmation of the local effects a run worked by an
/// agent would otherwise hold for them: exactly one of `edits`, `commands`
/// and `local`. It never covers an operation's own declared approval.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StandingApproval {
    /// File changes under these paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<StandingEdits>,
    /// A command or process whose argv begins with these tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<StandingCommands>,
    /// Every local effect.
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub local: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StandingEdits {
    /// Workspace-relative path prefixes, by whole components; `""` is the
    /// whole workspace.
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StandingCommands {
    /// The exact leading argv tokens. Argv has no shell, so a prefix admits
    /// nothing beyond the tokens it names.
    pub argv_prefix: Vec<NonEmptyString>,
}

impl StandingApproval {
    /// One kind of confirmation, each in its bounds: relative paths that
    /// stay inside the workspace, and a prefix of at least one token.
    pub fn check(&self) -> Result<(), String> {
        let kinds = usize::from(self.edits.is_some())
            + usize::from(self.commands.is_some())
            + usize::from(self.local);
        if kinds != 1 {
            return Err(
                "a standing approval is exactly one of edits, commands and local".to_owned(),
            );
        }
        if let Some(edits) = &self.edits {
            if edits.paths.is_empty() {
                return Err("standing edits name at least one path".to_owned());
            }
            for path in &edits.paths {
                let relative = std::path::Path::new(path);
                if relative.is_absolute()
                    || relative
                        .components()
                        .any(|component| !matches!(component, std::path::Component::Normal(_)))
                {
                    return Err(format!(
                        "standing edits path {path:?} must be workspace-relative, without . or .."
                    ));
                }
            }
        }
        if self
            .commands
            .as_ref()
            .is_some_and(|commands| commands.argv_prefix.is_empty())
        {
            return Err("standing commands name at least one argv token".to_owned());
        }
        Ok(())
    }
}

/// An agent's answer to an input request or an agent act.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundAnswer {
    /// The digest of the request answered, as the run reported it.
    pub request_digest: NonEmptyString,
    pub value: JsonValue,
}

/// A person's decision, validated against the request it answers: an
/// approval takes `approved` and `reason`, and a purchase's (N10) also the
/// `choice` of method it pays with; a claimed call's retry takes
/// `value: {"retry": true}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundApproval {
    pub request_digest: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<NonEmptyString>,
    /// The instance of the purchase request's method the person pays with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<JsonValue>,
}

/// Seal the run without continuing it: a person stopped it, or a new attempt
/// replaced it. An effect the run left unconfirmed stays unconfirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RunClose {
    pub disposition: CloseDisposition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloseDisposition {
    Declined,
    Superseded,
}

impl From<CloseDisposition> for ClosureDisposition {
    fn from(disposition: CloseDisposition) -> Self {
        match disposition {
            CloseDisposition::Declined => Self::Declined,
            CloseDisposition::Superseded => Self::Superseded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ResumeDocument;

    #[test]
    fn a_document_binds_each_entry_and_refuses_an_unbound_one() -> Result<(), serde_json::Error> {
        let value = serde_json::json!({
            "answers": {"draft": {"request_digest": "sha256:a", "value": {"text": "hello"}}},
            "approvals": {
                "send.approval": {"request_digest": "sha256:b", "approved": true, "reason": "checked"},
                "send/retry": {"request_digest": "sha256:c", "value": {"retry": true}}
            }
        });
        let document: ResumeDocument = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(document)?, value);
        for unbound in [
            serde_json::json!({"answers": {"draft": {"text": "hello"}}}),
            serde_json::json!({"approvals": {"send.approval": true}}),
            serde_json::json!({"draft": {"text": "hello"}}),
            serde_json::json!({"close": {"disposition": "closed"}}),
        ] {
            assert!(serde_json::from_value::<ResumeDocument>(unbound).is_err());
        }
        Ok(())
    }
}
