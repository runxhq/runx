//! What a host brings a paused run (`runx resume <run-id> -`): an agent's
//! answers and a person's decisions, each bound by digest to the request it
//! answers, or a close. A decision therefore never reaches a request that
//! changed after the person saw it.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::JsonValue;
use crate::receipt::RunOutcome;
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
    /// every later step and answer any open approval of the floor they
    /// cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing_approvals: Option<Vec<StandingApproval>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<RunClose>,
}

/// A person's bounded confirmation of the writes a run under `effects: ask`
/// would otherwise hold for them: exactly one of `edits`, `commands`,
/// `operations` and `local`. It never covers a write's own approval, an
/// operation's declared one or a step's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StandingApproval {
    /// File changes under these paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<StandingEdits>,
    /// A command or process whose argv begins with these tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<StandingCommands>,
    /// A connector operation's write that these names cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<StandingOperations>,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StandingOperations {
    /// Connector operations as `<connector>/<operation>`, or every operation
    /// of exactly one connector as `<connector>/*`.
    pub names: Vec<NonEmptyString>,
}

impl StandingOperations {
    /// Whether these names cover `operation` on `connector`.
    #[must_use]
    pub fn cover(&self, connector: &str, operation: &str) -> bool {
        self.names
            .iter()
            .any(|name| match name.as_str().strip_suffix("/*") {
                Some(every) => every == connector,
                None => {
                    name.as_str()
                        .strip_prefix(connector)
                        .and_then(|rest| rest.strip_prefix('/'))
                        == Some(operation)
                }
            })
    }
}

impl StandingApproval {
    /// One kind of confirmation, each in its bounds: relative paths that
    /// stay inside the workspace, a prefix of at least one token, and
    /// operations each named on its connector.
    pub fn check(&self) -> Result<(), String> {
        let kinds = usize::from(self.edits.is_some())
            + usize::from(self.commands.is_some())
            + usize::from(self.operations.is_some())
            + usize::from(self.local);
        if kinds != 1 {
            return Err(
                "a standing approval is exactly one of edits, commands, operations and local"
                    .to_owned(),
            );
        }
        if let Some(operations) = &self.operations {
            if operations.names.is_empty() {
                return Err("standing operations name at least one operation".to_owned());
            }
            for name in &operations.names {
                let named = match name.as_str().strip_suffix("/*") {
                    Some(connector) => crate::connector::is_connector_name(connector),
                    None => name
                        .as_str()
                        .rsplit_once('/')
                        .is_some_and(|(connector, operation)| {
                            crate::connector::is_connector_name(connector)
                                && crate::connector::is_operation_name(operation)
                        }),
                };
                if !named {
                    return Err(format!(
                        "standing operations name {name:?} is not <connector>/<operation> or <connector>/*"
                    ));
                }
            }
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
/// approval takes `approved` and `reason`, and a purchase's also the
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
    pub outcome: CloseOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloseOutcome {
    Declined,
    Superseded,
}

impl From<CloseOutcome> for RunOutcome {
    fn from(outcome: CloseOutcome) -> Self {
        match outcome {
            CloseOutcome::Declined => Self::Declined,
            CloseOutcome::Superseded => Self::Superseded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ResumeDocument, StandingApproval, StandingOperations};

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
            serde_json::json!({"close": {"outcome": "completed"}}),
        ] {
            assert!(serde_json::from_value::<ResumeDocument>(unbound).is_err());
        }
        Ok(())
    }

    #[test]
    fn standing_operations_name_each_operation_on_its_connector()
    -> Result<(), Box<dyn std::error::Error>> {
        let standing = |names: serde_json::Value| -> Result<StandingApproval, serde_json::Error> {
            serde_json::from_value(serde_json::json!({"operations": {"names": names}}))
        };
        for names in [
            serde_json::json!(["slack/thread.reply"]),
            serde_json::json!(["github/*", "google/search-console/sitemaps.submit"]),
        ] {
            assert_eq!(standing(names.clone())?.check(), Ok(()), "{names}");
        }
        for names in [
            serde_json::json!([]),
            serde_json::json!(["thread.reply"]),
            serde_json::json!(["slack/"]),
            serde_json::json!(["slack/Thread"]),
            serde_json::json!(["*"]),
            serde_json::json!(["/*"]),
        ] {
            assert!(standing(names.clone())?.check().is_err(), "{names}");
        }
        let both: StandingApproval = serde_json::from_value(serde_json::json!({
            "operations": {"names": ["slack/*"]}, "local": true
        }))?;
        assert!(both.check().is_err(), "a standing approval is one kind");

        let operations = StandingOperations {
            names: vec!["slack/thread.reply".into(), "github/*".into()],
        };
        assert!(operations.cover("slack", "thread.reply"));
        assert!(!operations.cover("slack", "channel.post"));
        assert!(operations.cover("github", "branch.push"));
        assert!(!operations.cover("github/enterprise", "branch.push"));
        assert!(!operations.cover("slackx", "thread.reply"));
        Ok(())
    }
}
